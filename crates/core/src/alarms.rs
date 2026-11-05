//! Health alarms (roadmap entry 7): rules on every host, a folder, a Tag or chosen hosts, each with one condition
//! (disk %, memory %, unreachable, container stopped, updates pending) and one level (yellow / red). The service runs
//! a pass every 5, 15 or 30 minutes while it is on, asking only the hosts some rule covers and only what their rules
//! need; with no rules it asks nothing. The OS-update check is a read-only system job, run once a month or on demand.
//! The step that asks a host is a closure (`Ask`), and "now" is passed in, so passes are tested without SSH.
use crate::{config::Config, fleet, protocol::Request};
use anyhow::{bail, ensure, Context, Result};
use chrono::{DateTime, Duration, Utc};
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Map, Value};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
    sync::{
        atomic::{AtomicBool, Ordering},
        Mutex, PoisonError,
    },
};
use uuid::Uuid;

pub const INTERVALS: [i64; 3] = [5, 15, 30];
const CONDITIONS: &[&str] = &["disk", "memory", "unreachable", "container", "updates"];
/// A disk or memory alarm clears this many points under its line, so a value hovering at the line doesn't flap.
const MARGIN: f64 = 2.0;
/// Memory and unreachable fire only after this many checks in a row.
const IN_A_ROW: u64 = 2;
const UPDATES_EVERY: i64 = 30;
const AT_ONCE: usize = 8;
/// One pass at a time, whoever starts it (the timer or "Check now").
static PASSING: AtomicBool = AtomicBool::new(false);
/// When each state folder's next pass is due, kept in memory so the timer's frequent `due` costs nothing between
/// passes (no database, no thread). Cleared whenever rules or the interval change.
static NEXT: Mutex<BTreeMap<std::path::PathBuf, DateTime<Utc>>> = Mutex::new(BTreeMap::new());
fn forget_next(state: &Path) {
    NEXT.lock().unwrap_or_else(PoisonError::into_inner).remove(state);
}
/// Whether a pass is due: cheap between passes (memory only).
pub fn due(state: &Path, config: &Config, now: DateTime<Utc>) -> bool {
    if !config.roles.iter().any(|r| r == "master") || PASSING.load(Ordering::SeqCst) {
        return false;
    }
    if NEXT.lock().unwrap_or_else(PoisonError::into_inner).get(state).is_some_and(|next| now < *next) {
        return false;
    }
    let next = (|| -> Result<Option<DateTime<Utc>>> {
        let db = database(state)?;
        let count: i64 = db.query_row("SELECT COUNT(*) FROM alarm_rules", [], |r| r.get(0))?;
        let every = Duration::minutes(interval(&db)?);
        if count == 0 {
            // No rules: nothing to check; look again in one interval (or as soon as a rule is added).
            return Ok(Some(now + every));
        }
        let last = setting(&db, "lastPass")?.and_then(|t| DateTime::parse_from_rfc3339(&t).ok()).map(|t| t.with_timezone(&Utc));
        Ok(last.map(|t| t + every).filter(|next| now < *next))
    })().unwrap_or(None);
    match next {
        Some(next) => {
            NEXT.lock().unwrap_or_else(PoisonError::into_inner).insert(state.to_path_buf(), next);
            false
        }
        None => true,
    }
}
static STATE: Mutex<()> = Mutex::new(());

/// What a pass asks one host for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Ask {
    /// `monitor.stats`: disk and memory; a failure means the host didn't answer.
    Stats,
    /// `containers.list`.
    Containers,
    /// The read-only update check: `{count, packages}`.
    Updates,
}
pub type Asker<'a> = dyn Fn(&Value, Ask) -> Result<Value> + Sync + 'a;

pub(crate) fn database(state: &Path) -> Result<Connection> {
    let db = crate::tags::database(state)?;
    db.execute_batch(
        "CREATE TABLE IF NOT EXISTS alarm_rules (id TEXT PRIMARY KEY, document TEXT NOT NULL, created_at TEXT NOT NULL);
         CREATE TABLE IF NOT EXISTS alarm_settings (key TEXT PRIMARY KEY, value TEXT NOT NULL);
         CREATE TABLE IF NOT EXISTS alarm_state (key TEXT PRIMARY KEY, document TEXT NOT NULL);",
    )?;
    // The three starting rules, once: deleting them later sticks.
    let seeded: Option<String> = db.query_row("SELECT value FROM alarm_settings WHERE key='seeded'", [], |r| r.get(0)).optional()?;
    if seeded.is_none() {
        for rule in defaults() {
            db.execute("INSERT OR IGNORE INTO alarm_rules(id,document,created_at) VALUES(?1,?2,?3)",
                params![rule["id"].as_str(), rule.to_string(), Utc::now().to_rfc3339()])?;
        }
        db.execute("INSERT OR REPLACE INTO alarm_settings(key,value) VALUES('seeded','1')", [])?;
    }
    Ok(db)
}
/// Every host: unreachable, disk over 90%, memory over 90%, all red.
pub fn defaults() -> Vec<Value> {
    [("default-unreachable", "unreachable", Value::Null), ("default-disk", "disk", json!(90)), ("default-memory", "memory", json!(90))]
        .into_iter()
        .map(|(id, condition, threshold)| json!({"id":id,"scope":{"all":true},"condition":condition,"threshold":threshold,"containers":[],"level":"red"}))
        .collect()
}
fn setting(db: &Connection, key: &str) -> Result<Option<String>> {
    Ok(db.query_row("SELECT value FROM alarm_settings WHERE key=?1", [key], |r| r.get(0)).optional()?)
}
fn set_setting(db: &Connection, key: &str, value: &str) -> Result<()> {
    db.execute("INSERT OR REPLACE INTO alarm_settings(key,value) VALUES(?1,?2)", params![key, value])?;
    Ok(())
}
pub fn interval(db: &Connection) -> Result<i64> {
    Ok(setting(db, "interval")?.and_then(|v| v.parse().ok()).unwrap_or(5))
}

// ---- rules ----

pub fn rules(state: &Path) -> Result<Vec<Value>> {
    let db = database(state)?;
    let mut stmt = db.prepare("SELECT document FROM alarm_rules ORDER BY created_at, id")?;
    let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
    rows.map(|row| Ok(serde_json::from_str(&row?)?)).collect()
}
/// Checks and normalises a rule; `E_ALARM_RULE` names what is wrong.
fn clean_rule(p: &Value) -> Result<Value> {
    let scope = &p["scope"];
    let scope = if scope["all"] == true {
        json!({"all":true})
    } else if let Some(folder) = scope["folder"].as_str().filter(|f| !f.is_empty()) {
        json!({"folder":folder})
    } else if let Some(tag) = scope["tag"].as_str().map(str::trim).filter(|t| !t.is_empty()) {
        json!({"tag":tag})
    } else if let Some(hosts) = scope["hosts"].as_array().filter(|h| !h.is_empty()) {
        let hosts: Vec<&str> = hosts.iter().filter_map(Value::as_str).collect();
        ensure!(!hosts.is_empty(), "E_ALARM_RULE: choose at least one host");
        json!({"hosts":hosts})
    } else {
        bail!("E_ALARM_RULE: choose every host, a folder, a Tag or hosts");
    };
    let condition = p["condition"].as_str().filter(|c| CONDITIONS.contains(c)).context("E_ALARM_RULE: the condition is disk, memory, unreachable, container or updates")?;
    let threshold = match condition {
        "disk" | "memory" => {
            let n = p["threshold"].as_i64().context("E_ALARM_RULE: a percentage from 1 to 99")?;
            ensure!((1..=99).contains(&n), "E_ALARM_RULE: a percentage from 1 to 99");
            json!(n)
        }
        _ => Value::Null,
    };
    let containers: Vec<String> = if condition == "container" {
        p["containers"].as_array().into_iter().flatten().filter_map(Value::as_str).map(str::trim).filter(|n| !n.is_empty()).map(str::to_owned).collect()
    } else {
        Vec::new()
    };
    let level = p["level"].as_str().filter(|l| matches!(*l, "yellow" | "red")).context("E_ALARM_RULE: the level is yellow or red")?;
    // A disk rule may watch one partition (entry 8); without it the fullest one decides.
    let partition = if condition == "disk" { p["partition"].as_str().map(str::trim).filter(|v| !v.is_empty()).map(str::to_owned) } else { None };
    ensure!(partition.as_ref().is_none_or(|v| v.chars().count() <= 256 && !v.chars().any(char::is_control)), "E_ALARM_RULE: a partition is a path such as /var or D:\\");
    let id = p["id"].as_str().map(str::to_owned).unwrap_or_else(|| Uuid::new_v4().to_string());
    let mut rule = json!({"id":id,"scope":scope,"condition":condition,"threshold":threshold,"containers":containers,"level":level});
    if let Some(partition) = partition {
        rule["partition"] = json!(partition);
    }
    Ok(rule)
}
pub fn set_rule(state: &Path, p: &Value) -> Result<Value> {
    let rule = clean_rule(p)?;
    forget_next(state);
    let db = database(state)?;
    db.execute(
        "INSERT INTO alarm_rules(id,document,created_at) VALUES(?1,?2,?3) ON CONFLICT(id) DO UPDATE SET document=excluded.document",
        params![rule["id"].as_str(), rule.to_string(), Utc::now().to_rfc3339()],
    )?;
    // A changed rule starts over: its old alarms no longer say what it watches.
    db.execute("DELETE FROM alarm_state WHERE key LIKE ?1", [format!("alarm:{}:%", rule["id"].as_str().unwrap_or_default())])?;
    Ok(rule)
}
pub fn delete_rule(state: &Path, id: &str) -> Result<Value> {
    forget_next(state);
    let db = database(state)?;
    ensure!(db.execute("DELETE FROM alarm_rules WHERE id=?1", [id])? > 0, "E_ALARM_RULE: no such rule");
    db.execute("DELETE FROM alarm_state WHERE key LIKE ?1", [format!("alarm:{id}:%")])?;
    Ok(json!({"id":id,"deleted":true}))
}
pub fn set_interval(state: &Path, minutes: i64) -> Result<Value> {
    ensure!(INTERVALS.contains(&minutes), "E_ALARM_INTERVAL: check every 5, 15 or 30 minutes");
    forget_next(state);
    set_setting(&database(state)?, "interval", &minutes.to_string())?;
    Ok(json!({"interval":minutes}))
}

// ---- hosts covered ----

/// The hosts a scope covers (direct, merged and far), as `targets.list` rows.
fn covered(state: &Path, scope: &Value, hosts: &[Value]) -> Result<Vec<Value>> {
    let wanted: Option<BTreeSet<String>> = if scope["all"] == true {
        None
    } else if let Some(tag) = scope["tag"].as_str() {
        let key = tag.to_lowercase();
        let all = crate::tags::list(state)?;
        Some(all["hosts"].as_object().into_iter().flatten()
            .filter(|(_, tags)| tags.as_array().into_iter().flatten().any(|t| t.as_str().is_some_and(|t| t.to_lowercase() == key)))
            .map(|(id, _)| id.clone()).collect())
    } else if let Some(folder) = scope["folder"].as_str() {
        let db = database(state)?;
        let mut stmt = db.prepare("SELECT target_id FROM folder_members WHERE folder_id=?1")?;
        let rows = stmt.query_map([folder], |row| row.get::<_, String>(0))?.collect::<rusqlite::Result<_>>()?;
        Some(rows)
    } else {
        Some(scope["hosts"].as_array().into_iter().flatten().filter_map(Value::as_str).map(str::to_owned).collect())
    };
    Ok(hosts.iter().filter(|h| h["runnerInstalled"] != false)
        .filter(|h| wanted.as_ref().is_none_or(|w| w.contains(h["id"].as_str().unwrap_or_default()))).cloned().collect())
}
fn host_list(state: &Path, config: &Config) -> Result<Vec<Value>> {
    let list = fleet::handle(state, config, &Request { method: "targets.list".into(), params: json!({}) })?.context("targets unavailable")?;
    Ok(list.as_array().cloned().unwrap_or_default())
}

// ---- state ----

fn load(db: &Connection, key: &str) -> Result<Value> {
    Ok(db.query_row("SELECT document FROM alarm_state WHERE key=?1", [key], |r| r.get::<_, String>(0)).optional()?
        .map(|d| serde_json::from_str(&d)).transpose()?.unwrap_or_else(|| json!({})))
}
fn save(db: &Connection, key: &str, doc: &Value) -> Result<()> {
    db.execute("INSERT OR REPLACE INTO alarm_state(key,document) VALUES(?1,?2)", params![key, doc.to_string()])?;
    Ok(())
}
fn percent(used: f64, total: f64) -> Option<f64> {
    (total > 0.0).then(|| (used / total * 100.0 * 10.0).round() / 10.0)
}
/// The disk reading a rule looks at: the named partition (None if the host has no such path), or the fullest one.
/// Hosts from before entry 8 report only `disk` (`/` or `C:\`).
fn disk_reading(stats: &Value, partition: Option<&str>) -> Option<(f64, String)> {
    let mut disks: Vec<&Value> = stats["disks"].as_array().map(|d| d.iter().collect()).unwrap_or_default();
    if disks.is_empty() && stats["disk"].is_object() {
        disks.push(&stats["disk"]);
    }
    let same = |a: &str, b: &str| {
        let norm = |p: &str| { let p = p.trim().to_lowercase(); if p.len() > 1 { p.trim_end_matches(['/', '\\']).to_owned() } else { p } };
        norm(a) == norm(b)
    };
    let readings = disks.into_iter().filter_map(|d| {
        let path = d["path"].as_str().unwrap_or("/").to_owned();
        Some((percent(d["total"].as_f64()? - d["free"].as_f64()?, d["total"].as_f64()?)?, path))
    });
    match partition {
        Some(name) => readings.into_iter().find(|(_, path)| same(path, name)),
        None => readings.into_iter().max_by(|a, b| a.0.total_cmp(&b.0)),
    }
}
fn memory_percent(stats: &Value) -> Option<f64> {
    let (total, available) = (stats["memory"]["total"].as_f64()?, stats["memory"]["available"].as_f64()?);
    percent(total - available, total)
}

/// What one pass learned about one host. `None`: not asked this pass.
#[derive(Default)]
struct Seen {
    stats: Option<Result<Value, String>>,
    containers: Option<Result<Value, String>>,
}

/// One alarm (rule × host) moves on: `firing` None keeps it as it is (nothing learned this pass).
fn judge(alarm: &mut Value, rule: &Value, now: DateTime<Utc>, firing: Option<bool>, value: Value) {
    let Some(firing) = firing else { return }; // nothing learned: keep as is
    let was = alarm["active"] == true;
    if firing && !was {
        alarm["active"] = json!(true);
        alarm["since"] = json!(crate::activity::stamp(now));
        alarm["acknowledged"] = json!(false);
    } else if !firing && was {
        alarm["active"] = json!(false);
        alarm["acknowledged"] = json!(false);
        alarm["since"] = Value::Null;
    }
    alarm["value"] = value;
    alarm["level"] = rule["level"].clone();
    alarm["condition"] = rule["condition"].clone();
    alarm["threshold"] = rule["threshold"].clone();
}

/// One pass: asks each covered host what its rules need (at most `AT_ONCE` hosts at a time), then moves every alarm
/// on. `only` limits it to these hosts and asks (used by "Check now"). Returns what was asked, for tests.
pub fn pass(state: &Path, config: &Config, now: DateTime<Utc>, ask: &Asker, only: Option<(&BTreeSet<String>, &[Ask])>) -> Result<BTreeMap<String, Vec<Ask>>> {
    let rules = rules(state)?;
    let hosts = host_list(state, config)?;
    let mut needs: BTreeMap<String, (Value, BTreeSet<Ask>)> = BTreeMap::new();
    let mut covers: Vec<(Value, Vec<Value>)> = Vec::new();
    {
        let db = database(state)?;
        for rule in &rules {
            let list = covered(state, &rule["scope"], &hosts)?;
            for host in &list {
                let id = host["id"].as_str().unwrap_or_default().to_owned();
                let entry = needs.entry(id.clone()).or_insert_with(|| (host.clone(), BTreeSet::new()));
                match rule["condition"].as_str().unwrap_or_default() {
                    "disk" | "memory" | "unreachable" => { entry.1.insert(Ask::Stats); }
                    "container" => { entry.1.insert(Ask::Containers); }
                    "updates" => {
                        let info = load(&db, &format!("updates:{id}"))?;
                        let due = info["checkedAt"].as_str().and_then(|t| DateTime::parse_from_rfc3339(t).ok())
                            .is_none_or(|t| now - t.with_timezone(&Utc) >= Duration::days(UPDATES_EVERY));
                        if due || info["checkNow"] == true {
                            entry.1.insert(Ask::Updates);
                        }
                    }
                    _ => {}
                }
            }
            covers.push((rule.clone(), list));
        }
    }
    if let Some((ids, asks)) = only {
        needs.retain(|id, _| ids.contains(id));
        for (_, wanted) in needs.values_mut() {
            wanted.retain(|a| asks.contains(a));
        }
    }
    // Ask, at most AT_ONCE hosts at a time.
    let jobs: Vec<(String, Value, Vec<Ask>)> = needs.into_iter().filter(|(_, (_, a))| !a.is_empty())
        .map(|(id, (host, asks))| (id, host, asks.into_iter().collect())).collect();
    let results: Mutex<BTreeMap<String, Seen>> = Mutex::new(BTreeMap::new());
    let updates: Mutex<Vec<(String, Value)>> = Mutex::new(Vec::new());
    for batch in jobs.chunks(AT_ONCE) {
        std::thread::scope(|scope| {
            for (id, host, asks) in batch {
                let (results, updates) = (&results, &updates);
                scope.spawn(move || {
                    let mut seen = Seen::default();
                    for a in asks {
                        match a {
                            Ask::Stats => seen.stats = Some(ask(host, Ask::Stats).map_err(|e| format!("{e:#}"))),
                            Ask::Containers => seen.containers = Some(ask(host, Ask::Containers).map_err(|e| format!("{e:#}"))),
                            Ask::Updates => {
                                let result = if host["access"] == "view" {
                                    json!({"count":null,"packages":[],"error":"view-only"})
                                } else {
                                    match ask(host, Ask::Updates) {
                                        Ok(found) => json!({"count":found["count"],"packages":found["packages"],"error":null}),
                                        Err(error) => json!({"count":null,"packages":[],"error":format!("{error:#}")}),
                                    }
                                };
                                updates.lock().unwrap_or_else(PoisonError::into_inner).push((id.clone(), result));
                            }
                        }
                    }
                    results.lock().unwrap_or_else(PoisonError::into_inner).insert(id.clone(), seen);
                });
            }
        });
    }
    let seen = results.into_inner().unwrap_or_else(PoisonError::into_inner);
    let asked: BTreeMap<String, Vec<Ask>> = jobs.iter().map(|(id, _, a)| (id.clone(), a.clone())).collect();
    let _guard = STATE.lock().unwrap_or_else(PoisonError::into_inner);
    let db = database(state)?;
    for (id, mut result) in updates.into_inner().unwrap_or_else(PoisonError::into_inner) {
        result["checkedAt"] = json!(crate::activity::stamp(now));
        result["checkNow"] = json!(false);
        save(&db, &format!("updates:{id}"), &result)?;
    }
    for (rule, list) in &covers {
        let rule_id = rule["id"].as_str().unwrap_or_default();
        let threshold = rule["threshold"].as_f64().unwrap_or(0.0);
        for host in list {
            let id = host["id"].as_str().unwrap_or_default();
            let key = format!("alarm:{rule_id}:{id}");
            let mut alarm = load(&db, &key)?;
            alarm["rule"] = json!(rule_id);
            alarm["host"] = json!(id);
            alarm["label"] = host["label"].clone();
            let seen = seen.get(id);
            let stats = seen.and_then(|s| s.stats.as_ref());
            let (firing, value) = match rule["condition"].as_str().unwrap_or_default() {
                "disk" => match stats {
                    Some(Ok(stats)) => match disk_reading(stats, rule["partition"].as_str()) {
                        Some((pct, path)) => {
                            alarm["partition"] = json!(path);
                            (line(&alarm, pct, threshold), json!(pct))
                        }
                        // A named partition this host doesn't have: nothing to watch there.
                        None if rule["partition"].is_string() => (Some(false), Value::Null),
                        None => (None, alarm["value"].clone()),
                    },
                    _ => (None, alarm["value"].clone()),
                },
                "memory" => match stats {
                    Some(Ok(stats)) => match memory_percent(stats) {
                        Some(pct) => {
                            // Not active yet: only after IN_A_ROW readings over the line. Active: clears under the margin.
                            let count = if pct >= threshold { alarm["count"].as_u64().unwrap_or(0) + 1 } else { 0 };
                            alarm["count"] = json!(count);
                            let firing = if alarm["active"] == true { line(&alarm, pct, threshold) } else { Some(count >= IN_A_ROW) };
                            (firing, json!(pct))
                        }
                        None => (None, alarm["value"].clone()),
                    },
                    _ => (None, alarm["value"].clone()),
                },
                "unreachable" => match stats {
                    Some(Ok(_)) => {
                        alarm["count"] = json!(0);
                        (Some(false), Value::Null)
                    }
                    Some(Err(error)) => {
                        let count = alarm["count"].as_u64().unwrap_or(0) + 1;
                        alarm["count"] = json!(count);
                        (if count >= IN_A_ROW { Some(true) } else { None }, json!(error))
                    }
                    None => (None, alarm["value"].clone()),
                },
                "container" => match seen.and_then(|s| s.containers.as_ref()) {
                    Some(Ok(list)) => {
                        let names: Vec<String> = rule["containers"].as_array().into_iter().flatten().filter_map(Value::as_str).map(str::to_owned).collect();
                        let now_running: BTreeMap<String, bool> = list["containers"].as_array().into_iter().flatten().filter_map(|c| {
                            let name = c["name"].as_str().or(c["names"].as_str())?.to_owned();
                            Some((name, c["state"] == "running"))
                        }).filter(|(name, _)| names.is_empty() || names.contains(name)).collect();
                        let before: BTreeMap<String, bool> = serde_json::from_value(alarm["running"].clone()).unwrap_or_default();
                        let mut stopped: BTreeSet<String> = serde_json::from_value(alarm["stopped"].clone()).unwrap_or_default();
                        for (name, was) in &before {
                            if *was && now_running.get(name) == Some(&false) {
                                stopped.insert(name.clone());
                            }
                        }
                        stopped.retain(|name| now_running.get(name) == Some(&false));
                        alarm["running"] = json!(now_running);
                        alarm["stopped"] = json!(stopped);
                        (Some(!stopped.is_empty()), json!(stopped))
                    }
                    _ => (None, alarm["value"].clone()),
                },
                "updates" => {
                    let info = load(&db, &format!("updates:{id}"))?;
                    match info["count"].as_u64() {
                        Some(count) => (Some(count >= 1), json!(count)),
                        None => (None, alarm["value"].clone()),
                    }
                }
                _ => (None, Value::Null),
            };
            judge(&mut alarm, rule, now, firing, value);
            save(&db, &key, &alarm)?;
        }
    }
    if only.is_none() {
        set_setting(&db, "lastPass", &now.to_rfc3339())?;
        forget_next(state);
    }
    Ok(asked)
}
/// Over the line fires, under the line minus the margin clears, in between keeps the current state.
fn line(alarm: &Value, value: f64, threshold: f64) -> Option<bool> {
    if value >= threshold {
        Some(true)
    } else if value < threshold - MARGIN {
        Some(false)
    } else {
        Some(alarm["active"] == true)
    }
}

/// Runs a pass when the interval is due (the service's timer calls this every few seconds).
pub fn tick(state: &Path, config: &Config, now: DateTime<Utc>, ask: &Asker) -> Result<bool> {
    if !config.roles.iter().any(|r| r == "master") {
        return Ok(false);
    }
    let db = database(state)?;
    if rules(state)?.is_empty() {
        return Ok(false);
    }
    let last = setting(&db, "lastPass")?.and_then(|t| DateTime::parse_from_rfc3339(&t).ok()).map(|t| t.with_timezone(&Utc));
    let every = Duration::minutes(interval(&db)?);
    if last.is_some_and(|t| now - t < every) {
        return Ok(false);
    }
    drop(db);
    if PASSING.swap(true, Ordering::SeqCst) {
        return Ok(false);
    }
    let result = pass(state, config, now, ask, None);
    PASSING.store(false, Ordering::SeqCst);
    result.map(|_| true)
}

// ---- reading and acting ----

/// Active alarms, red first then newest, with counts of the unacknowledged ones per level.
pub fn list(state: &Path) -> Result<Value> {
    let db = database(state)?;
    let mut stmt = db.prepare("SELECT key,document FROM alarm_state WHERE key LIKE 'alarm:%'")?;
    let mut alarms: Vec<Value> = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
        .filter_map(|row| row.ok())
        .filter_map(|(key, doc)| serde_json::from_str::<Value>(&doc).ok().map(|mut d| { d["id"] = json!(key.trim_start_matches("alarm:")); d }))
        .filter(|d| d["active"] == true)
        .collect();
    alarms.sort_by(|a, b| (b["level"] == "red").cmp(&(a["level"] == "red")).then(b["since"].as_str().cmp(&a["since"].as_str())));
    let count = |level: &str| alarms.iter().filter(|a| a["level"] == level && a["acknowledged"] != true).count();
    Ok(json!({"alarms":alarms,"red":count("red"),"yellow":count("yellow")}))
}
pub fn acknowledge(state: &Path, id: &str) -> Result<Value> {
    let _guard = STATE.lock().unwrap_or_else(PoisonError::into_inner);
    let db = database(state)?;
    let key = format!("alarm:{id}");
    let mut alarm = load(&db, &key)?;
    ensure!(alarm["active"] == true, "E_ALARM_RULE: no such alarm");
    alarm["acknowledged"] = json!(true);
    save(&db, &key, &alarm)?;
    alarm["id"] = json!(id);
    Ok(alarm)
}
/// One host's alarms and its last update check.
pub fn host(state: &Path, id: &str) -> Result<Value> {
    let all = list(state)?;
    let alarms: Vec<Value> = all["alarms"].as_array().into_iter().flatten().filter(|a| a["host"] == id).cloned().collect();
    let updates = load(&database(state)?, &format!("updates:{id}"))?;
    Ok(json!({"alarms":alarms,"updates":if updates.as_object().is_some_and(Map::is_empty) { Value::Null } else { updates }}))
}
/// Every host's last update check, for tables.
pub fn updates(state: &Path) -> Result<Value> {
    let db = database(state)?;
    let mut stmt = db.prepare("SELECT key,document FROM alarm_state WHERE key LIKE 'updates:%'")?;
    let rows: Map<String, Value> = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
        .filter_map(|row| row.ok())
        .filter_map(|(key, doc)| Some((key.trim_start_matches("updates:").to_owned(), serde_json::from_str(&doc).ok()?)))
        .collect();
    Ok(Value::Object(rows))
}

// ---- the real host step ----

/// The read-only update check scripts. Nothing is installed, and Ubuntu's package lists are not refreshed.
pub const APT_CHECK: &str = "apt list --upgradable 2>/dev/null";
pub const WINDOWS_CHECK: &str = "$s = New-Object -ComObject Microsoft.Update.Session; $r = $s.CreateUpdateSearcher().Search('IsInstalled=0 and IsHidden=0'); ConvertTo-Json -Compress -InputObject @($r.Updates | ForEach-Object { $_.Title })";
/// `apt list --upgradable`: `name/suite version arch [upgradable from: …]` lines after "Listing...".
pub fn parse_apt(text: &str) -> Value {
    let packages: Vec<String> = text.lines()
        .filter(|l| l.contains("[upgradable from"))
        .filter_map(|l| l.split('/').next().map(str::trim).filter(|n| !n.is_empty()).map(str::to_owned))
        .collect();
    json!({"count":packages.len(),"packages":packages})
}
/// The Windows script prints a JSON array of update titles (or a single string, or nothing).
pub fn parse_windows(text: &str) -> Result<Value> {
    let text = text.trim();
    let packages: Vec<String> = if text.is_empty() || text == "null" {
        Vec::new()
    } else {
        match serde_json::from_str::<Value>(text).context("unexpected Windows Update output")? {
            Value::Array(list) => list.into_iter().filter_map(|v| v.as_str().map(str::to_owned)).collect(),
            Value::String(one) => vec![one],
            _ => bail!("unexpected Windows Update output"),
        }
    };
    Ok(json!({"count":packages.len(),"packages":packages}))
}
/// Production `Ask`: the Master's own fleet methods (direct, or through a Merge). The update check is a system job:
/// started through the fleet directly, so it is not in Recent Jobs.
pub fn real_ask(state: &Path, config: &Config, host: &Value, what: Ask) -> Result<Value> {
    let id = host["id"].as_str().unwrap_or_default();
    let call = |method: &str, params: Value| fleet::handle(state, config, &Request { method: method.into(), params })?.context("fleet unavailable");
    match what {
        Ask::Stats => call("monitor.stats", json!({"targetId":id})),
        Ask::Containers => call("containers.list", json!({"targetId":id})),
        Ask::Updates => {
            let windows = host["os"] == "windows";
            let job = Uuid::new_v4().to_string();
            call("jobs.start", json!({"targetId":id,"jobId":job,"script":if windows { WINDOWS_CHECK } else { APT_CHECK },
                "cwd":if windows { "C:\\" } else { "/" }}))?;
            for _ in 0..120 {
                let status = call("jobs.status", json!({"targetId":id,"jobId":job}))?;
                match status["status"].as_str().unwrap_or_default() {
                    "succeeded" => {
                        let out = call("jobs.logs", json!({"targetId":id,"jobId":job,"stream":"stdout","offset":0,"limit":1048576}))?;
                        let text = out["content"].as_str().unwrap_or_default();
                        return if windows { parse_windows(text) } else { Ok(parse_apt(text)) };
                    }
                    "failed" | "cancelled" | "interrupted" => bail!("the update check ended {}", status["status"]),
                    _ => std::thread::sleep(std::time::Duration::from_secs(1)),
                }
            }
            bail!("the update check took too long")
        }
    }
}

// ---- methods ----

pub fn handle(state: &Path, config: &Config, req: &Request) -> Result<Option<Value>> {
    if !req.method.starts_with("alarms.") {
        return Ok(None);
    }
    ensure!(config.roles.iter().any(|r| r == "master"), "E_ROLE: master role is required");
    let p = &req.params;
    let text = |key: &str| p[key].as_str().with_context(|| format!("E_ARGUMENT: missing {key}"));
    Ok(Some(match req.method.as_str() {
        "alarms.list" => list(state)?,
        "alarms.rules" => json!({"rules":rules(state)?,"interval":interval(&database(state)?)?}),
        "alarms.rule_set" => set_rule(state, p)?,
        "alarms.rule_delete" => delete_rule(state, text("id")?)?,
        "alarms.settings" => set_interval(state, p["interval"].as_i64().context("E_ALARM_INTERVAL: check every 5, 15 or 30 minutes")?)?,
        "alarms.ack" => acknowledge(state, text("id")?)?,
        "alarms.host" => host(state, text("targetId")?)?,
        "alarms.updates" => updates(state)?,
        "alarms.check_updates" => {
            let target = text("targetId")?.to_owned();
            let hosts = host_list(state, config)?;
            let ids: BTreeSet<String> = hosts.iter().filter_map(|h| h["id"].as_str()).filter(|id| target == "all" || *id == target).map(str::to_owned).collect();
            ensure!(!ids.is_empty(), "E_TARGET: unknown host {target}");
            let db = database(state)?;
            for id in &ids {
                let mut info = load(&db, &format!("updates:{id}"))?;
                info["checkNow"] = json!(true);
                save(&db, &format!("updates:{id}"), &info)?;
            }
            drop(db);
            // At once, in the background; the window reads the result with alarms.host.
            let (state, config) = (state.to_path_buf(), config.clone());
            std::thread::spawn(move || {
                let asks = [Ask::Updates];
                let step = |h: &Value, a: Ask| real_ask(&state, &config, h, a);
                if let Err(error) = check_now(&state, &config, &ids, &asks, &step) {
                    eprintln!("SSH Control Center: update check failed: {error:#}");
                }
            });
            json!({"queued":target})
        }
        _ => return Ok(None),
    }))
}
/// "Check now": the update check on these hosts, even if no rule covers them.
pub fn check_now(state: &Path, config: &Config, ids: &BTreeSet<String>, asks: &[Ask], ask: &Asker) -> Result<()> {
    let now = Utc::now();
    let hosts = host_list(state, config)?;
    let _guard = STATE.lock().unwrap_or_else(PoisonError::into_inner);
    let db = database(state)?;
    for host in hosts.iter().filter(|h| ids.contains(h["id"].as_str().unwrap_or_default())) {
        if !asks.contains(&Ask::Updates) {
            continue;
        }
        let id = host["id"].as_str().unwrap_or_default();
        let mut result = if host["access"] == "view" {
            json!({"count":null,"packages":[],"error":"view-only"})
        } else {
            match ask(host, Ask::Updates) {
                Ok(found) => json!({"count":found["count"],"packages":found["packages"],"error":null}),
                Err(error) => json!({"count":null,"packages":[],"error":format!("{error:#}")}),
            }
        };
        result["checkedAt"] = json!(crate::activity::stamp(now));
        result["checkNow"] = json!(false);
        save(&db, &format!("updates:{id}"), &result)?;
    }
    drop(db);
    drop(_guard);
    // Update alarms follow the new counts.
    pass(state, config, now, ask, Some((ids, &[])))?;
    Ok(())
}
