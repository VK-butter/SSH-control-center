//! Activity: what this Master did. **Recent Jobs** (`job_log`) holds every job this Master starts, one host or a
//! group run; the **audit log** (`audit.jsonl`) holds one JSON line per action that changes something. Both are
//! this Master's own record: shown and written in Thailand time, kept `keepDays` days, never in a backup. The audit
//! log is a record for the owner, not proof, and its channel is a label the client reports (ADR 0002).
use crate::{config::Config, fleet, protocol::Request, security};
use anyhow::{ensure, Context, Result};
use chrono::{DateTime, Duration, FixedOffset, NaiveDate, SecondsFormat, TimeZone, Utc};
use rusqlite::{params, Connection};
use serde_json::{json, Map, Value};
use std::{
    collections::BTreeSet,
    io::Write,
    path::{Path, PathBuf},
    sync::{Mutex, PoisonError},
};

pub const KEEP_DAYS: i64 = 14;
const MAX_DAYS: i64 = 3650;
const SCRIPT_KEPT: usize = 4096;
/// Job states after which a job is done; anything else is asked again while the page is open.
const FINISHED: &[&str] = &["succeeded", "failed", "cancelled", "interrupted", "unreachable"];
/// Rows refreshed per request, so a page of offline hosts can't hold a refresh for long.
const REFRESH_AT_ONCE: usize = 8;

/// Methods that change something, plus opening a Shell. A file read is audited only when it is a download, and an
/// upload once, on its last chunk. A new method that changes something is added here.
const AUDITED: &[&str] = &[
    "jobs.start", "jobs.cancel", "groups.run", "groups.cancel", "groups.retry",
    "targets.import", "targets.update", "targets.import_legacy", "targets.remove",
    "enrollment.export", "enrollment.accept", "masters.revoke",
    "merges.add", "merges.sync", "merges.pair_request", "merges.pair_complete", "merges.remove",
    "relays.revoke", "relays.allow", "relays.set", "pairing.create", "pairing.complete", "pairing.cancel",
    "tags.set", "folders.create", "folders.rename", "folders.delete", "folders.assign",
    "datacenters.create", "datacenters.rename", "datacenters.delete", "datacenters.assign",
    "files.write", "files.mkdir", "files.remove", "containers.action", "terminal.open",
    "backup.create", "backup.restore", "keys.rotate", "activity.settings",
    "alarms.rule_set", "alarms.rule_delete", "alarms.settings", "alarms.ack", "alarms.check_updates",
    "targets.upgrade", "targets.upgrade_all",
];
/// Params copied into an audit line's object, in this order. Nothing else is: no script, content, env or passphrase.
const OBJECT_KEYS: &[&str] = &["targetId", "master", "jobId", "runId", "path", "container", "action", "id", "name", "folderId", "datacenterId", "masterId", "keepDays"];

/// Every audit write and prune goes through one lock, so a prune never drops a line appended meanwhile.
static AUDIT: Mutex<()> = Mutex::new(());
/// State folders whose last write failed; the warning stays until a write works again.
static FAILED: Mutex<BTreeSet<PathBuf>> = Mutex::new(BTreeSet::new());

pub fn thai() -> FixedOffset {
    FixedOffset::east_opt(7 * 3600).unwrap()
}
/// ISO 8601 in Thailand time, e.g. `2026-10-25T14:03:00+07:00`.
pub fn stamp(at: DateTime<Utc>) -> String {
    at.with_timezone(&thai()).to_rfc3339_opts(SecondsFormat::Secs, false)
}
fn parse(at: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(at).ok().map(|at| at.with_timezone(&Utc))
}

pub fn audited(method: &str, params: &Value) -> bool {
    AUDITED.contains(&method)
        || (method == "files.read" && params["download"] == true)
        || (method == "files.upload" && params["done"] == true)
}
/// The main argument(s) of a request, as one line: host, then what on it.
pub fn object(method: &str, params: &Value) -> String {
    let mut parts: Vec<String> = OBJECT_KEYS.iter().filter_map(|key| match &params[*key] {
        Value::String(text) if !text.is_empty() => Some(text.clone()),
        Value::Number(number) => Some(number.to_string()),
        _ => None,
    }).collect();
    match method {
        "groups.run" => {
            let selector = &params["selector"];
            parts.extend(selector["tag"].as_str().map(|tag| format!("tag {tag}")));
            parts.extend(selector["folder"].as_str().map(|folder| format!("folder {folder}")));
        }
        "relays.set" => {
            parts.extend(params["role"].as_str().map(str::to_owned));
            match &params["tags"] {
                Value::Array(tags) => parts.push(format!("tags {}", tags.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(", "))),
                Value::Null if params.get("tags").is_some() => parts.push("all hosts".into()),
                _ => {}
            }
            parts.extend(params["passThrough"].as_bool().map(|on| format!("pass-through {}", if on { "on" } else { "off" })));
        }
        "tags.set" => parts.push(params["tags"].as_array().into_iter().flatten().filter_map(Value::as_str).collect::<Vec<_>>().join(", ")),
        "targets.import" | "targets.update" => {
            let host = params.get("registration").or_else(|| params.get("target"));
            parts.extend(host.and_then(|h| h["id"].as_str()).map(str::to_owned));
        }
        _ => {}
    }
    parts.join(" · ")
}
/// The error code of a failed request, as the client sees it.
pub fn code(error: &anyhow::Error) -> String {
    let message = format!("{error:#}");
    message.split(':').next().filter(|s| s.starts_with("E_") && s.len() < 60).unwrap_or("E_OPERATION").to_owned()
}

// ---- settings ----

fn settings_path(state: &Path) -> PathBuf {
    state.join("activity.json")
}
/// `keepDays`, `since` (when this Master started recording) and `lastPrune`. Written on first use.
fn settings(state: &Path, now: DateTime<Utc>) -> Result<Value> {
    let path = settings_path(state);
    if let Ok(bytes) = std::fs::read(&path) {
        if let Ok(value) = serde_json::from_slice::<Value>(&bytes) {
            return Ok(value);
        }
    }
    let value = json!({"keepDays":KEEP_DAYS,"since":stamp(now),"lastPrune":Value::Null});
    save_settings(state, &value)?;
    Ok(value)
}
fn save_settings(state: &Path, value: &Value) -> Result<()> {
    let path = settings_path(state);
    std::fs::write(&path, serde_json::to_vec_pretty(value)?)?;
    security::protect(&path, false)
}
fn keep_days(state: &Path, now: DateTime<Utc>) -> i64 {
    settings(state, now).ok().and_then(|s| s["keepDays"].as_i64()).unwrap_or(KEEP_DAYS)
}
pub fn set_keep_days(state: &Path, days: i64, now: DateTime<Utc>) -> Result<Value> {
    ensure!((1..=MAX_DAYS).contains(&days), "E_ACTIVITY_DAYS: keep 1-{MAX_DAYS} days");
    let mut value = settings(state, now)?;
    value["keepDays"] = json!(days);
    save_settings(state, &value)?;
    status(state, now)
}
/// `keepDays`, `since`, and `warning` when the last write failed.
pub fn status(state: &Path, now: DateTime<Utc>) -> Result<Value> {
    let value = settings(state, now)?;
    let warning = FAILED.lock().unwrap_or_else(PoisonError::into_inner).contains(state);
    Ok(json!({"keepDays":value["keepDays"],"since":value["since"],"warning":warning}))
}
fn wrote(state: &Path, result: &Result<()>) {
    let mut failed = FAILED.lock().unwrap_or_else(PoisonError::into_inner);
    match result {
        Ok(()) => failed.remove(state),
        Err(error) => {
            eprintln!("SSH Control Center: activity can't be recorded: {error:#}");
            failed.insert(state.to_path_buf())
        }
    };
}

// ---- audit log ----

fn audit_path(state: &Path) -> PathBuf {
    state.join("audit.jsonl")
}
/// Appends one line. A failure is remembered for the warning; it never fails the action.
pub fn audit(state: &Path, line: Value) {
    let result = (|| -> Result<()> {
        let _guard = AUDIT.lock().unwrap_or_else(PoisonError::into_inner);
        let path = audit_path(state);
        let new = !path.exists();
        let mut file = std::fs::OpenOptions::new().create(true).append(true).open(&path)?;
        if new {
            security::protect(&path, false)?;
        }
        let mut bytes = serde_json::to_vec(&line)?;
        bytes.push(b'\n');
        file.write_all(&bytes)?;
        Ok(())
    })();
    wrote(state, &result);
}
fn audit_lines(state: &Path) -> Vec<Value> {
    let _guard = AUDIT.lock().unwrap_or_else(PoisonError::into_inner);
    std::fs::read_to_string(audit_path(state)).unwrap_or_default().lines()
        .filter_map(|line| serde_json::from_str(line).ok()).collect()
}

/// What `Service::handle` calls after every request: audit it if it changes something, and record a job this Master
/// started. Requests relayed from a merged Master are audited with their inner method; other bridge traffic (a
/// Master using this machine as its Target) is not this Master's activity.
pub fn after(state: &Path, config: &Config, req: &Request, user: &str, channel: Option<&str>, result: &Result<Value>) {
    if !config.roles.iter().any(|r| r == "master") {
        return;
    }
    let now = Utc::now();
    if req.method == "bridge.request" {
        let inner = &req.params["request"];
        if inner["method"] != "relay.request" {
            return;
        }
        let relayed = &inner["params"];
        let method = relayed["method"].as_str().unwrap_or_default();
        let mut params = relayed["params"].clone();
        if params.is_object() {
            params["targetId"] = relayed["targetId"].clone();
        }
        if audited(method, &params) {
            // The full Relay Path, origin first; an older Master sends none, so just its own label.
            let master = req.params["masterId"].as_str().unwrap_or_default();
            let path: Vec<&str> = relayed["path"].as_array().into_iter().flatten().filter_map(|hop| hop["label"].as_str()).collect();
            let label = if path.is_empty() { relay_label(state, master).unwrap_or_else(|| master.to_owned()) } else { path.join(" → ") };
            audit(state, line(now, method, &params, result, &format!("merge: {label}"), user));
        }
        return;
    }
    let channel = channel.unwrap_or("unknown");
    if let (Ok(job), "jobs.start") = (result, req.method.as_str()) {
        record_job(state, now, &req.params, job, channel);
    }
    if result.is_ok() && req.method == "backup.restore" {
        restart_history(state, now);
    }
    if audited(&req.method, &req.params) {
        audit(state, line(now, &req.method, &req.params, result, channel, user));
    }
}
fn line(now: DateTime<Utc>, method: &str, params: &Value, result: &Result<Value>, channel: &str, user: &str) -> Value {
    let outcome = match result {
        Ok(_) => "ok".to_owned(),
        Err(error) => code(error),
    };
    json!({"time":stamp(now),"action":method,"object":object(method, params),"result":outcome,"channel":channel,"user":user})
}
fn relay_label(state: &Path, master: &str) -> Option<String> {
    fleet::database(state).ok()?
        .query_row("SELECT label FROM relays WHERE master_id=?1", [master], |row| row.get(0)).ok()
}
/// A restored Master starts a new history: the old machine's lines aren't in the backup, and this one's are gone.
fn restart_history(state: &Path, now: DateTime<Utc>) {
    let result = (|| -> Result<()> {
        {
            let _guard = AUDIT.lock().unwrap_or_else(PoisonError::into_inner);
            let _ = std::fs::remove_file(audit_path(state));
        }
        database(state)?.execute("DELETE FROM job_log", [])?;
        let mut value = settings(state, now)?;
        value["since"] = json!(stamp(now));
        save_settings(state, &value)
    })();
    wrote(state, &result);
}

// ---- Recent Jobs ----

fn database(state: &Path) -> Result<Connection> {
    let db = fleet::database(state)?;
    db.execute_batch(
        "CREATE TABLE IF NOT EXISTS job_log (job_id TEXT PRIMARY KEY, target_id TEXT NOT NULL, group_id TEXT, sent_at INTEGER NOT NULL, document TEXT NOT NULL);",
    )?;
    Ok(db)
}
/// One job this Master sent. `group` is the run's ID and selector for a group run's host.
pub struct Sent<'a> {
    pub job: &'a str,
    pub target: &'a str,
    pub label: Option<&'a str>,
    pub group: Option<(&'a str, &'a Value)>,
    pub script: &'a str,
    pub cwd: &'a str,
    pub shell: Option<&'a str>,
    pub as_admin: bool,
    pub env: &'a Value,
    pub channel: &'a str,
    pub status: &'a str,
    pub error: Option<&'a str>,
}
pub fn record(state: &Path, now: DateTime<Utc>, sent: Sent) {
    let env_names: Vec<&String> = sent.env.as_object().map(|env| env.keys().collect()).unwrap_or_default();
    let document = json!({"jobId":sent.job,"targetId":sent.target,"label":sent.label,"groupId":sent.group.map(|g| g.0),
        "selector":sent.group.map(|g| g.1.clone()),"script":sent.script.chars().take(SCRIPT_KEPT).collect::<String>(),
        "cwd":sent.cwd,"shell":sent.shell,"asAdmin":sent.as_admin,"envNames":env_names,"channel":sent.channel,
        "sentAt":stamp(now),"status":sent.status,"exitCode":Value::Null,"startedAt":Value::Null,"finishedAt":Value::Null,"error":sent.error});
    let result = database(state).and_then(|db| {
        db.execute("INSERT OR REPLACE INTO job_log(job_id,target_id,group_id,sent_at,document) VALUES(?1,?2,?3,?4,?5)",
            params![sent.job, sent.target, sent.group.map(|g| g.0), now.timestamp_millis(), document.to_string()])?;
        Ok(())
    });
    wrote(state, &result);
}
/// A one-host `jobs.start` from a client, after it was accepted.
fn record_job(state: &Path, now: DateTime<Utc>, p: &Value, job: &Value, channel: &str) {
    let text = |key: &str| p[key].as_str().unwrap_or_default();
    let id = job["id"].as_str().or(p["jobId"].as_str()).unwrap_or_default();
    record(state, now, Sent { job: id, target: text("targetId"), label: None, group: None, script: text("script"),
        cwd: text("cwd"), shell: p["shell"].as_str(), as_admin: p["asAdmin"] == true, env: &p["env"], channel,
        status: job["status"].as_str().unwrap_or("queued"), error: None });
}
/// Keeps a recorded job's last status read (from a refresh here or a group run's own refresh).
pub fn update_status(state: &Path, job: &str, status: &Value) -> Result<()> {
    let db = database(state)?;
    let Ok(document) = db.query_row("SELECT document FROM job_log WHERE job_id=?1", [job], |row| row.get::<_, String>(0)) else {
        return Ok(());
    };
    let mut document: Value = serde_json::from_str(&document)?;
    for field in ["status", "exitCode", "startedAt", "finishedAt"] {
        if let Some(value) = status.get(field) {
            document[field] = value.clone();
        }
    }
    document["error"] = status.get("error").cloned().unwrap_or(Value::Null);
    db.execute("UPDATE job_log SET document=?2 WHERE job_id=?1", params![job, document.to_string()])?;
    Ok(())
}

// ---- filters ----

/// What a view shows: a time range (`range`: `today`, `yesterday`, `Nd`, or `from`/`to` dates, Thai calendar days),
/// the hosts (`hosts`, none = all), free `text` (audit: action and object) and a job `status`.
pub struct Filter {
    pub from: DateTime<Utc>,
    pub to: DateTime<Utc>,
    pub hosts: Option<BTreeSet<String>>,
    pub text: String,
    pub status: String,
}
fn thai_midnight(day: NaiveDate) -> DateTime<Utc> {
    thai().from_local_datetime(&day.and_hms_opt(0, 0, 0).unwrap()).unwrap().with_timezone(&Utc)
}
pub fn range(p: &Value, now: DateTime<Utc>) -> Result<(DateTime<Utc>, DateTime<Utc>)> {
    let today = now.with_timezone(&thai()).date_naive();
    let day = |key: &str| -> Result<Option<NaiveDate>> {
        p[key].as_str().filter(|s| !s.is_empty())
            .map(|s| NaiveDate::parse_from_str(s, "%Y-%m-%d").context("E_ARGUMENT: dates are YYYY-MM-DD")).transpose()
    };
    let (first, last) = match p["range"].as_str().unwrap_or("") {
        "" | "all" if p["from"].is_null() && p["to"].is_null() => return Ok((DateTime::<Utc>::MIN_UTC, DateTime::<Utc>::MAX_UTC)),
        "today" => (today, today),
        "yesterday" => (today - Duration::days(1), today - Duration::days(1)),
        days if days.ends_with('d') => {
            let n: i64 = days.trim_end_matches('d').parse().context("E_ARGUMENT: range is today, yesterday or Nd")?;
            ensure!(n >= 1, "E_ARGUMENT: range is today, yesterday or Nd");
            (today - Duration::days(n - 1), today)
        }
        _ => (day("from")?.unwrap_or(NaiveDate::MIN), day("to")?.unwrap_or(today)),
    };
    let from = if first == NaiveDate::MIN { DateTime::<Utc>::MIN_UTC } else { thai_midnight(first) };
    Ok((from, thai_midnight(last + Duration::days(1))))
}
pub fn filter(p: &Value, now: DateTime<Utc>) -> Result<Filter> {
    let (from, to) = range(p, now)?;
    let hosts = p["hosts"].as_array().map(|hosts| hosts.iter().filter_map(Value::as_str).map(str::to_owned).collect());
    Ok(Filter { from, to, hosts, text: p["text"].as_str().unwrap_or("").trim().to_lowercase(), status: p["status"].as_str().unwrap_or("all").to_owned() })
}
fn status_matches(wanted: &str, status: &str) -> bool {
    match wanted {
        "succeeded" => status == "succeeded",
        "failed" => ["failed", "cancelled", "interrupted", "unreachable"].contains(&status),
        "running" => !FINISHED.contains(&status),
        _ => true,
    }
}

/// Audit lines in the range, newest first. With hosts and text, a line matches either: its host is one of the
/// hosts, or its action or object contains the text.
pub fn audit_list(state: &Path, filter: &Filter) -> Vec<Value> {
    let mut lines: Vec<Value> = audit_lines(state).into_iter().filter(|line| {
        let Some(at) = line["time"].as_str().and_then(parse) else { return false };
        if at < filter.from || at >= filter.to {
            return false;
        }
        let object = line["object"].as_str().unwrap_or_default();
        let host = object.split(" · ").next().unwrap_or_default();
        let by_host = filter.hosts.as_ref().is_some_and(|hosts| hosts.contains(host));
        let by_text = !filter.text.is_empty()
            && (line["action"].as_str().unwrap_or_default().to_lowercase().contains(&filter.text) || object.to_lowercase().contains(&filter.text));
        (filter.hosts.is_none() && filter.text.is_empty()) || by_host || by_text
    }).collect();
    lines.reverse();
    lines
}

/// Recorded jobs in the range, newest first, filtered by host and status. A group run's hosts come back as one
/// `{"kind":"group"}` entry when any of them matches.
pub fn job_list(state: &Path, filter: &Filter) -> Result<Vec<Value>> {
    let db = database(state)?;
    let mut stmt = db.prepare("SELECT document FROM job_log WHERE sent_at>=?1 AND sent_at<?2 ORDER BY sent_at DESC, rowid DESC")?;
    let rows: Vec<Value> = stmt.query_map(params![filter.from.timestamp_millis(), filter.to.timestamp_millis()], |row| row.get::<_, String>(0))?
        .filter_map(|doc| doc.ok().and_then(|doc| serde_json::from_str(&doc).ok())).collect();
    let matches = |row: &Value| {
        filter.hosts.as_ref().is_none_or(|hosts| hosts.contains(row["targetId"].as_str().unwrap_or_default()))
            && status_matches(&filter.status, row["status"].as_str().unwrap_or_default())
    };
    let mut out: Vec<Value> = Vec::new();
    let mut groups: Map<String, Value> = Map::new();
    for row in rows {
        let Some(group) = row["groupId"].as_str().map(str::to_owned) else {
            if matches(&row) {
                out.push(json!({"kind":"job","job":row}));
            }
            continue;
        };
        let hit = matches(&row);
        match groups.get_mut(&group) {
            Some(entry) => {
                entry["hosts"].as_array_mut().unwrap().push(row);
                if hit {
                    entry["hit"] = json!(true);
                }
            }
            None => {
                out.push(json!({"kind":"group","groupId":group}));
                groups.insert(group.clone(), json!({"groupId":group,"selector":row["selector"],"channel":row["channel"],"sentAt":row["sentAt"],"hit":hit,"hosts":[row]}));
            }
        }
    }
    Ok(out.into_iter().filter_map(|entry| {
        if entry["kind"] != "group" {
            return Some(entry);
        }
        let mut group = groups.remove(entry["groupId"].as_str()?)?;
        if group["hit"] != true {
            return None;
        }
        group.as_object_mut()?.remove("hit");
        // Oldest host first, as the run started them; the sent time is the run's first.
        group["hosts"].as_array_mut()?.reverse();
        group["sentAt"] = group["hosts"][0]["sentAt"].clone();
        group["kind"] = json!("group");
        Some(group)
    }).collect())
}
/// Asks the host again for every listed job that hasn't finished (read-only), then lists again.
pub fn refresh(state: &Path, filter: &Filter, ask: &(dyn Fn(&str, &str) -> Result<Value> + Sync)) -> Result<Vec<Value>> {
    let unfinished: Vec<(String, String)> = job_list(state, filter)?.iter().flat_map(|entry| match entry["kind"].as_str() {
        Some("group") => entry["hosts"].as_array().cloned().unwrap_or_default(),
        _ => vec![entry["job"].clone()],
    }).filter(|job| !FINISHED.contains(&job["status"].as_str().unwrap_or_default()))
        .map(|job| (job["targetId"].as_str().unwrap_or_default().to_owned(), job["jobId"].as_str().unwrap_or_default().to_owned()))
        .collect();
    for batch in unfinished.chunks(REFRESH_AT_ONCE) {
        std::thread::scope(|scope| {
            for (target, job) in batch {
                scope.spawn(move || {
                    let status = ask(target, job).unwrap_or_else(|error| json!({"error":format!("{error:#}")}));
                    let _ = update_status(state, job, &status);
                });
            }
        });
    }
    job_list(state, filter)
}

// ---- CSV ----

fn cell(text: &str) -> String {
    if text.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", text.replace('"', "\"\""))
    } else {
        text.to_owned()
    }
}
fn csv(header: &[&str], rows: impl Iterator<Item = Vec<String>>) -> String {
    let mut out = String::from("\u{feff}");
    out.push_str(&header.join(","));
    out.push_str("\r\n");
    for row in rows {
        out.push_str(&row.iter().map(|value| cell(value)).collect::<Vec<_>>().join(","));
        out.push_str("\r\n");
    }
    out
}
fn text(value: &Value) -> String {
    match value {
        Value::Null => String::new(),
        Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}
/// A job time in Thai time (Targets write UTC).
fn thai_text(value: &Value) -> String {
    value.as_str().and_then(parse).map(stamp).unwrap_or_else(|| text(value))
}
pub fn jobs_csv(entries: &[Value]) -> String {
    let jobs = entries.iter().flat_map(|entry| match entry["kind"].as_str() {
        Some("group") => entry["hosts"].as_array().cloned().unwrap_or_default(),
        _ => vec![entry["job"].clone()],
    });
    csv(&["time", "host", "group", "status", "exit_code", "started", "finished", "cwd", "as_admin", "channel", "job_id", "script"],
        jobs.map(|job| vec![thai_text(&job["sentAt"]), text(&job["targetId"]), text(&job["groupId"]), text(&job["status"]),
            text(&job["exitCode"]), thai_text(&job["startedAt"]), thai_text(&job["finishedAt"]), text(&job["cwd"]),
            text(&job["asAdmin"]), text(&job["channel"]), text(&job["jobId"]), text(&job["script"])]))
}
pub fn audit_csv(lines: &[Value]) -> String {
    csv(&["time", "action", "object", "result", "channel", "user"],
        lines.iter().map(|line| ["time", "action", "object", "result", "channel", "user"].iter().map(|key| text(&line[*key])).collect()))
}

// ---- keeping ----

/// Removes audit lines and recorded jobs older than `keepDays`.
pub fn prune(state: &Path, now: DateTime<Utc>) -> Result<()> {
    let cutoff = now - Duration::days(keep_days(state, now));
    database(state)?.execute("DELETE FROM job_log WHERE sent_at<?1", [cutoff.timestamp_millis()])?;
    let _guard = AUDIT.lock().unwrap_or_else(PoisonError::into_inner);
    let path = audit_path(state);
    let Ok(text) = std::fs::read_to_string(&path) else { return Ok(()) };
    let kept: String = text.lines()
        .filter(|line| serde_json::from_str::<Value>(line).ok().and_then(|l| l["time"].as_str().and_then(parse)).is_none_or(|at| at >= cutoff))
        .map(|line| format!("{line}\n")).collect();
    if kept.len() != text.len() {
        let temp = state.join("audit.jsonl.tmp");
        std::fs::write(&temp, kept)?;
        security::protect(&temp, false)?;
        security::replace(&temp, &path)?;
    }
    Ok(())
}
/// Prunes once per Thai day; called by the service's timer.
pub fn daily(state: &Path, now: DateTime<Utc>) -> Result<()> {
    let mut value = settings(state, now)?;
    let today = now.with_timezone(&thai()).date_naive().to_string();
    if value["lastPrune"] == today.as_str() {
        return Ok(());
    }
    prune(state, now)?;
    value["lastPrune"] = json!(today);
    save_settings(state, &value)
}

// ---- methods ----

/// `activity.*`, Owner only. The service's own fleet call reads job status for `refresh`.
pub fn handle(state: &Path, config: &Config, req: &Request, owner: bool) -> Result<Option<Value>> {
    if !req.method.starts_with("activity.") {
        return Ok(None);
    }
    ensure!(config.roles.iter().any(|r| r == "master"), "E_ROLE: master role is required");
    ensure!(owner, "E_PERMISSION: only this Master's Owner can read its activity");
    let now = Utc::now();
    let p = &req.params;
    let ask = |target: &str, job: &str| {
        fleet::handle(state, config, &Request { method: "jobs.status".into(), params: json!({"targetId":target,"jobId":job}) })?
            .context("job runner unavailable")
    };
    Ok(Some(match req.method.as_str() {
        "activity.status" => status(state, now)?,
        "activity.settings" => set_keep_days(state, p["keepDays"].as_i64().context("E_ARGUMENT: missing keepDays")?, now)?,
        "activity.audit" => json!({"lines":audit_list(state, &filter(p, now)?),"status":status(state, now)?}),
        "activity.jobs" => {
            let filter = filter(p, now)?;
            let jobs = if p["refresh"] == true { refresh(state, &filter, &ask)? } else { job_list(state, &filter)? };
            json!({"jobs":jobs,"status":status(state, now)?})
        }
        "activity.csv" => {
            let filter = filter(p, now)?;
            let text = match p["view"].as_str() {
                Some("audit") => audit_csv(&audit_list(state, &filter)),
                Some("jobs") => jobs_csv(&job_list(state, &filter)?),
                _ => anyhow::bail!("E_ARGUMENT: view is jobs or audit"),
            };
            json!({"csv":text})
        }
        _ => return Ok(None),
    }))
}
