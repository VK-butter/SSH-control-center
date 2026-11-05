//! Group runs: one ordinary job on every host with a Tag or in a folder, with one result per host. Hosts are
//! chosen when the run starts; jobs go out through the normal `jobs.start` path (direct, or through a Merge), at
//! most `AT_ONCE` at a time. This Master keeps the `KEEP` most recent runs so they can be opened again.
use crate::{config::Config, fleet, protocol::Request};
use anyhow::{bail, ensure, Context, Result};
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};
use std::{
    path::Path,
    sync::{Mutex, PoisonError},
};
use uuid::Uuid;

pub const AT_ONCE: usize = 8;
pub const KEEP: usize = 20;
const SCRIPT_KEPT: usize = 4096;
/// Job states after which a host is done.
const FINISHED: &[&str] = &["succeeded", "failed", "cancelled", "interrupted"];
/// Every change to a run record is read-modify-write; one lock keeps parallel starts from losing each other's rows.
static RECORDS: Mutex<()> = Mutex::new(());

/// A host in a group, as the step closure sees it.
#[derive(Clone, Debug)]
pub struct Member {
    pub id: String,
    pub label: String,
    pub os: String,
}
/// What `act` is asked to do on one host: start its job, read its status, or cancel it.
pub enum Step<'a> {
    Start { job: &'a str, script: &'a str, cwd: &'a str, as_admin: bool, env: &'a Value },
    Status { job: &'a str },
    Cancel { job: &'a str },
}
pub type Act<'a> = dyn Fn(&Member, Step) -> Result<Value> + Sync + 'a;

fn database(state: &Path) -> Result<Connection> {
    let db = crate::tags::database(state)?;
    db.execute_batch("CREATE TABLE IF NOT EXISTS group_runs (id TEXT PRIMARY KEY, created_at TEXT NOT NULL, document TEXT NOT NULL);")?;
    Ok(db)
}
fn now() -> String {
    chrono::Utc::now().to_rfc3339()
}
fn default_cwd(os: &str) -> &'static str {
    if os == "windows" { "C:\\" } else { "/" }
}

/// The hosts a selector names, split into the ones that run and the ones skipped (no service to run a job).
fn resolve(state: &Path, config: &Config, selector: &Value) -> Result<(Vec<Value>, Vec<Value>)> {
    let hosts = fleet::handle(state, config, &Request { method: "targets.list".into(), params: json!({}) })?.context("targets unavailable")?;
    let wanted: Vec<String> = if let Some(tag) = selector.get("tag").and_then(Value::as_str) {
        let key = tag.trim().to_lowercase();
        let all = crate::tags::list(state)?;
        all["tags"].as_array().into_iter().flatten()
            .filter(|row| row["tag"].as_str().is_some_and(|t| t.to_lowercase() == key))
            .flat_map(|row| row["hosts"].as_array().cloned().unwrap_or_default())
            .filter_map(|id| id.as_str().map(str::to_owned)).collect()
    } else if let Some(folder) = selector.get("folder").and_then(Value::as_str) {
        let db = database(state)?;
        let mut stmt = db.prepare("SELECT target_id FROM folder_members WHERE folder_id=?1")?;
        let rows = stmt.query_map([folder], |row| row.get::<_, String>(0))?.collect::<rusqlite::Result<Vec<_>>>()?;
        rows
    } else {
        bail!("E_ARGUMENT: choose a Tag or a folder");
    };
    let (mut run, mut skipped) = (Vec::new(), Vec::new());
    for host in hosts.as_array().into_iter().flatten().filter(|h| h["id"].as_str().is_some_and(|id| wanted.iter().any(|w| w == id))) {
        let row = json!({"targetId":host["id"],"label":host["label"],"os":host["os"]});
        if host["runnerInstalled"] == true { run.push(row) } else {
            let mut row = row;
            row["reason"] = json!("E_RUNNER_NOT_INSTALLED: this host has no SSH Control Center service to run a job");
            skipped.push(row);
        }
    }
    run.sort_by(|a, b| a["targetId"].as_str().cmp(&b["targetId"].as_str()));
    skipped.sort_by(|a, b| a["targetId"].as_str().cmp(&b["targetId"].as_str()));
    Ok((run, skipped))
}

pub fn preview(state: &Path, config: &Config, selector: &Value) -> Result<Value> {
    let (run, skipped) = resolve(state, config, selector)?;
    Ok(json!({"hosts":run,"skipped":skipped}))
}

fn read(db: &Connection, id: &str) -> Result<Value> {
    let document: Option<String> = db.query_row("SELECT document FROM group_runs WHERE id=?1", [id], |row| row.get(0)).optional()?;
    Ok(serde_json::from_str(&document.context("E_NOT_FOUND: this group run is no longer kept")?)?)
}
fn write(db: &Connection, run: &Value) -> Result<()> {
    db.execute("UPDATE group_runs SET document=?2 WHERE id=?1", params![run["id"].as_str(), run.to_string()])?;
    Ok(())
}
/// Applies `change` to one host's row of a run, under the records lock.
fn update(state: &Path, run_id: &str, target: &str, change: impl FnOnce(&mut Value)) -> Result<Value> {
    let _guard = RECORDS.lock().unwrap_or_else(PoisonError::into_inner);
    let db = database(state)?;
    let mut run = read(&db, run_id)?;
    if let Some(host) = run["hosts"].as_array_mut().and_then(|hosts| hosts.iter_mut().find(|h| h["targetId"] == target)) {
        change(host);
    }
    write(&db, &run)?;
    Ok(run)
}

/// Records a new run (every host queued, skipped hosts marked) and returns it; `drive` then starts the jobs.
pub fn start(state: &Path, config: &Config, p: &Value) -> Result<Value> {
    let selector = p.get("selector").cloned().context("E_ARGUMENT: missing selector")?;
    let (run, skipped) = resolve(state, config, &selector)?;
    ensure!(!run.is_empty(), "E_GROUP_EMPTY: no host in this group can run a job");
    let scripts = p.get("scripts").cloned().unwrap_or_else(|| json!({}));
    for host in &run {
        let os = host["os"].as_str().unwrap_or("linux");
        ensure!(
            scripts[os].as_str().is_some_and(|s| !s.trim().is_empty()),
            "E_GROUP_SCRIPT: this group has {} hosts; write a script for them too",
            if os == "windows" { "Windows" } else { "Linux" }
        );
    }
    let kept = |os: &str| scripts[os].as_str().map(|s| s.chars().take(SCRIPT_KEPT).collect::<String>());
    let cwd = |os: &str| match p.get("cwd") {
        Some(Value::String(path)) => path.clone(),
        Some(Value::Object(map)) => map.get(os).and_then(Value::as_str).unwrap_or(default_cwd(os)).to_owned(),
        _ => default_cwd(os).to_owned(),
    };
    let hosts: Vec<Value> = run.iter().map(|h| {
        let os = h["os"].as_str().unwrap_or("linux");
        json!({"targetId":h["targetId"],"label":h["label"],"os":os,"cwd":cwd(os),"jobId":Value::Null,"state":"queued","error":Value::Null,"status":Value::Null,"exitCode":Value::Null})
    }).chain(skipped.iter().map(|h| json!({"targetId":h["targetId"],"label":h["label"],"os":h["os"],"state":"skipped","error":h["reason"]}))).collect();
    let id = Uuid::new_v4().to_string();
    let record = json!({"id":id,"createdAt":now(),"selector":selector,"scripts":{"linux":kept("linux"),"windows":kept("windows")},
        "asAdmin":p["asAdmin"] == true,"env":p.get("env").cloned().unwrap_or_else(|| json!({})),"hosts":hosts,"done":false,
        "channel":p["channel"].as_str().unwrap_or("unknown")});
    let _guard = RECORDS.lock().unwrap_or_else(PoisonError::into_inner);
    let db = database(state)?;
    db.execute("INSERT INTO group_runs(id,created_at,document) VALUES(?1,?2,?3)", params![id, record["createdAt"].as_str(), record.to_string()])?;
    db.execute("DELETE FROM group_runs WHERE id NOT IN (SELECT id FROM group_runs ORDER BY created_at DESC, rowid DESC LIMIT ?1)", [KEEP as i64])?;
    Ok(record)
}

fn member(host: &Value) -> Member {
    let text = |key: &str| host[key].as_str().unwrap_or_default().to_owned();
    Member { id: text("targetId"), label: text("label"), os: text("os") }
}

/// Starts the job of every queued host, at most `AT_ONCE` at a time. A host that can't be reached is recorded as
/// unreachable with the reason and is not tried again here.
pub fn drive(state: &Path, _config: &Config, run_id: &str, act: &Act) -> Result<()> {
    let run = read(&database(state)?, run_id)?;
    let queued: Vec<Value> = run["hosts"].as_array().into_iter().flatten().filter(|h| h["state"] == "queued").cloned().collect();
    let next = Mutex::new(queued.into_iter());
    std::thread::scope(|scope| {
        for _ in 0..AT_ONCE {
            scope.spawn(|| loop {
                let Some(host) = next.lock().unwrap_or_else(PoisonError::into_inner).next() else { break };
                let target = member(&host);
                let still_queued = database(state).and_then(|db| read(&db, run_id)).ok()
                    .and_then(|run| run["hosts"].as_array().cloned())
                    .is_some_and(|hosts| hosts.iter().any(|h| h["targetId"] == host["targetId"] && h["state"] == "queued"));
                if !still_queued {
                    continue;
                }
                let script = run["scripts"][target.os.as_str()].as_str().unwrap_or_default();
                let job = Uuid::new_v4().to_string();
                let as_admin = run["asAdmin"] == true && target.os != "windows";
                let cwd = host["cwd"].as_str().unwrap_or("/");
                let result = act(&target, Step::Start { job: &job, script, cwd, as_admin, env: &run["env"] });
                let reason = result.as_ref().err().map(|error| error.root_cause().to_string());
                crate::activity::record(state, chrono::Utc::now(), crate::activity::Sent {
                    job: &job, target: &target.id, label: Some(&target.label), group: Some((run_id, &run["selector"])), script, cwd,
                    shell: None, as_admin, env: &run["env"], channel: run["channel"].as_str().unwrap_or("unknown"),
                    status: if reason.is_some() { "unreachable" } else { "queued" }, error: reason.as_deref() });
                let _ = update(state, run_id, &target.id, |row| match result {
                    Ok(_) => { row["state"] = json!("started"); row["jobId"] = json!(job); row["status"] = json!("queued"); }
                    // The cause only: the job ID note added for one-host jobs means nothing here.
                    Err(error) => { row["state"] = json!("unreachable"); row["error"] = json!(error.root_cause().to_string()); }
                });
            });
        }
    });
    Ok(())
}

/// Reads each started, unfinished host's job status (read-only) and returns the run.
pub fn refresh(state: &Path, _config: &Config, run_id: &str, act: &Act) -> Result<Value> {
    let run = read(&database(state)?, run_id)?;
    for host in run["hosts"].as_array().into_iter().flatten() {
        let unfinished = host["status"].as_str().is_none_or(|s| !FINISHED.contains(&s));
        if host["state"] != "started" || !unfinished {
            continue;
        }
        let job = host["jobId"].as_str().unwrap_or_default().to_owned();
        let target = member(host);
        match act(&target, Step::Status { job: &job }) {
            Ok(status) => {
                let _ = crate::activity::update_status(state, &job, &status);
                update(state, run_id, &target.id, |row| {
                    row["status"] = status["status"].clone();
                    row["exitCode"] = status["exitCode"].clone();
                    row["error"] = Value::Null;
                })?;
            }
            // A host that stops answering keeps its last status; the reason is shown until it answers again.
            Err(error) => {
                update(state, run_id, &target.id, |row| row["error"] = json!(format!("{error:#}")))?;
            }
        }
    }
    let _guard = RECORDS.lock().unwrap_or_else(PoisonError::into_inner);
    let db = database(state)?;
    let mut run = read(&db, run_id)?;
    let done = run["hosts"].as_array().into_iter().flatten().all(|h| match h["state"].as_str() {
        Some("started") => h["status"].as_str().is_some_and(|s| FINISHED.contains(&s)),
        Some("queued") => false,
        _ => true,
    });
    run["done"] = json!(done);
    write(&db, &run)?;
    Ok(run)
}

/// Cancels the job of one host (`only`) or of every started, unfinished host. Hosts not started yet are dropped
/// from the run (marked cancelled) so a running `drive` doesn't start them.
pub fn cancel(state: &Path, _config: &Config, run_id: &str, only: Option<&str>, act: &Act) -> Result<Value> {
    let run = read(&database(state)?, run_id)?;
    for host in run["hosts"].as_array().into_iter().flatten().filter(|h| only.is_none_or(|id| h["targetId"] == id)) {
        let target = member(host);
        match host["state"].as_str() {
            Some("queued") => {
                update(state, run_id, &target.id, |row| { row["state"] = json!("started"); row["status"] = json!("cancelled"); })?;
            }
            Some("started") if host["status"].as_str().is_none_or(|s| !FINISHED.contains(&s)) => {
                let job = host["jobId"].as_str().unwrap_or_default().to_owned();
                match act(&target, Step::Cancel { job: &job }) {
                    Ok(reply) => { update(state, run_id, &target.id, |row| { row["status"] = reply.get("status").cloned().unwrap_or(json!("cancelled")); })?; }
                    Err(error) => { update(state, run_id, &target.id, |row| row["error"] = json!(format!("{error:#}")))?; }
                }
            }
            _ => {}
        }
    }
    read(&database(state)?, run_id)
}

/// Puts the hosts that couldn't be reached, failed, were interrupted or cancelled back in the queue; `drive` then
/// starts them with new job IDs. Hosts that succeeded are left alone.
pub fn retry(state: &Path, _config: &Config, run_id: &str) -> Result<Value> {
    let _guard = RECORDS.lock().unwrap_or_else(PoisonError::into_inner);
    let db = database(state)?;
    let mut run = read(&db, run_id)?;
    for row in run["hosts"].as_array_mut().into_iter().flatten() {
        let failed = row["state"] == "unreachable"
            || (row["state"] == "started" && row["status"].as_str().is_some_and(|s| ["failed", "interrupted", "cancelled"].contains(&s)));
        if failed {
            row["state"] = json!("queued");
            for field in ["jobId", "error", "status", "exitCode"] {
                row[field] = Value::Null;
            }
        }
    }
    run["done"] = json!(false);
    write(&db, &run)?;
    Ok(run)
}

/// One run (`runId`), or the recent runs newest first.
pub fn status(state: &Path, _config: &Config, p: &Value) -> Result<Value> {
    let db = database(state)?;
    if let Some(id) = p.get("runId").and_then(Value::as_str) {
        return read(&db, id);
    }
    let mut stmt = db.prepare("SELECT document FROM group_runs ORDER BY created_at DESC, rowid DESC")?;
    let runs = stmt.query_map([], |row| row.get::<_, String>(0))?
        .map(|doc| Ok(serde_json::from_str::<Value>(&doc?)?))
        .collect::<Result<Vec<_>>>()?;
    Ok(json!({"runs":runs}))
}

/// The real host steps: the Master's own job methods (direct, or through a Merge).
pub fn act(state: &Path, config: &Config, target: &Member, step: Step) -> Result<Value> {
    let call = |method: &str, params: Value| fleet::handle(state, config, &Request { method: method.into(), params })?.context("job runner unavailable");
    match step {
        Step::Start { job, script, cwd, as_admin, env } => {
            let mut params = json!({"targetId":target.id,"jobId":job,"script":script,"cwd":cwd,"env":env});
            if as_admin {
                params["asAdmin"] = json!(true);
            }
            call("jobs.start", params)
        }
        Step::Status { job } => call("jobs.status", json!({"targetId":target.id,"jobId":job})),
        Step::Cancel { job } => call("jobs.cancel", json!({"targetId":target.id,"jobId":job})),
    }
}

fn run_id(p: &Value) -> Result<&str> {
    p.get("runId").and_then(Value::as_str).context("E_ARGUMENT: missing runId")
}
pub fn handle(state: &Path, config: &Config, req: &Request) -> Result<Option<Value>> {
    if !req.method.starts_with("groups.") {
        return Ok(None);
    }
    ensure!(config.roles.iter().any(|r| r == "master"), "E_ROLE: master role is required");
    let p = &req.params;
    let real = |target: &Member, step: Step| act(state, config, target, step);
    Ok(Some(match req.method.as_str() {
        "groups.preview" => preview(state, config, p.get("selector").context("E_ARGUMENT: missing selector")?)?,
        "groups.run" => {
            let run = start(state, config, p)?;
            let (state, config, id) = (state.to_path_buf(), config.clone(), run["id"].as_str().unwrap_or_default().to_owned());
            std::thread::spawn(move || {
                if let Err(error) = drive(&state, &config, &id, &|target, step| act(&state, &config, target, step)) {
                    eprintln!("SSH Control Center: group run {id} stopped: {error:#}");
                }
            });
            run
        }
        "groups.cancel" => cancel(state, config, run_id(p)?, p.get("targetId").and_then(Value::as_str), &real)?,
        "groups.retry" => {
            let run = retry(state, config, run_id(p)?)?;
            let (state, config, id) = (state.to_path_buf(), config.clone(), run_id(p)?.to_owned());
            std::thread::spawn(move || {
                let _ = drive(&state, &config, &id, &|target, step| act(&state, &config, target, step));
            });
            run
        }
        "groups.status" => match p.get("runId").and_then(Value::as_str) {
            Some(id) => refresh(state, config, id, &real)?,
            None => status(state, config, p)?,
        },
        _ => return Ok(None),
    }))
}
