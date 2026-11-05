//! Merged Masters: relay of fleet requests through another Master's own identity, up to `MAX_HOPS` Masters deep.
//! The merged Master keeps its keys; the merging Master only learns public Target metadata. Each relayed request
//! carries its Relay Path (the Masters before the one that receives it) and the time left; every Master on the way
//! checks its own grant, refuses loops and paths that are too long, and forwards hosts it reaches through its own
//! Merges as `peer/child` IDs.
use super::{
    attach, database, handle, list_targets, printable, role, slug, str_arg, target, transport,
    Target, MAX_TARGETS,
};
use crate::{config::Config, protocol::Request, ssh_banner};
use anyhow::{bail, ensure, Context, Result};
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex, PoisonError, RwLock,
    },
    thread,
    time::{Duration, Instant},
};
use uuid::Uuid;

const PROBE_TIMEOUT: Duration = Duration::from_secs(20);
/// Upper bound on concurrent probes, so a 50-Target fleet never bursts 50 SSH clients.
const PROBE_WORKERS: usize = 8;
const HEALTH_TTL: chrono::TimeDelta = chrono::TimeDelta::seconds(30);
/// Concurrent health callers (window, CLI, merging Masters) wait for one probe and share it.
static LOCAL_PROBE: Mutex<()> = Mutex::new(());
static MERGED_PROBE: Mutex<()> = Mutex::new(());
/// Pairing back covers the merged Master's doctor of this machine and its own join.
const PAIR_TIMEOUT: Duration = Duration::from_secs(150);
const MASTERS_TENANT: &str = "masters";

const RELAY_METHODS: &[&str] = &[
    "targets.doctor",
    "jobs.start",
    "jobs.list",
    "jobs.status",
    "jobs.logs",
    "jobs.cancel",
    "files.list",
    "files.read",
    "files.search",
    "files.grep",
    "files.write",
    "files.upload",
    "files.mkdir",
    "files.remove",
    "terminal.open",
    "terminal.read",
    "terminal.write",
    "terminal.status",
    "terminal.resize",
    "terminal.close",
    "containers.list",
    "containers.logs",
    "containers.action",
    "monitor.stats",
];

/// Relayable methods that only read. Every other method in `RELAY_METHODS` needs Full control; a View-only Merge is
/// refused it. A new relayable method is placed in one class or the other here.
pub(crate) const READ_METHODS: &[&str] = &[
    "targets.doctor",
    "jobs.list",
    "jobs.status",
    "jobs.logs",
    "files.list",
    "files.read",
    "files.search",
    "files.grep",
    "containers.list",
    "containers.logs",
    "monitor.stats",
];
/// The relay protocol a Master speaks; 2 adds Merge Roles, Relay Paths and multi-hop listing. A join reply without
/// it comes from an older Master, which lists only its direct hosts.
pub const RELAY_VERSION: u32 = 2;
/// The most Masters a Relay Path may hold: a host up to this many Merges away is reachable.
pub const MAX_HOPS: usize = 4;
/// The originating Master's limit for a relayed request, end to end.
const RELAY_BUDGET: Duration = Duration::from_secs(60);
/// What each hop keeps back from the time it passes on, for its own SSH call.
const HOP_MARGIN: Duration = Duration::from_secs(5);

/// The step that sends one request to the next Master on a path (`relay.request`, `relay.join`, `relay.health`).
/// Production uses the SSH transport; tests wire in-process Masters with `set_hop`.
pub type Hop = dyn Fn(&Path, &Config, &Target, &str, Value, Duration) -> Result<Value> + Send + Sync;
static HOP: RwLock<Option<Arc<Hop>>> = RwLock::new(None);
#[doc(hidden)]
pub fn set_hop(hop: Option<Arc<Hop>>) {
    *HOP.write().unwrap_or_else(PoisonError::into_inner) = hop;
}
/// Sends to the next Master and waits at most `timeout`. Running out (or the SSH call timing out) is reported as
/// `E_RELAY_TIMEOUT` naming that Master; a timeout further down keeps the name of the Master it came from.
fn hop(state: &Path, config: &Config, peer: &Target, method: &str, params: Value, timeout: Duration) -> Result<Value> {
    let custom = HOP.read().unwrap_or_else(PoisonError::into_inner).clone();
    let result = match custom {
        None => transport::request_within(state, config, peer, method, params, timeout),
        Some(step) => {
            let (tx, rx) = std::sync::mpsc::channel();
            let (state, config, peer_copy, method) = (state.to_path_buf(), config.clone(), peer.clone(), method.to_owned());
            thread::spawn(move || {
                let _ = tx.send(step(&state, &config, &peer_copy, &method, params, timeout));
            });
            rx.recv_timeout(timeout).unwrap_or_else(|_| Err(anyhow::anyhow!("relay timed out")))
        }
    };
    result.map_err(|error| {
        let message = format!("{error:#}");
        if message.contains("timed out") && !message.contains("E_RELAY_TIMEOUT") {
            anyhow::anyhow!("E_RELAY_TIMEOUT: {} didn't answer in time", peer.label)
        } else {
            error
        }
    })
}

/// Where a relayed request comes from (the Masters before this one, origin first) and when it must be answered.
/// It rides in the request params as `__relay` from `relay_request` to `send`, and as `path` / `timeoutMs`
/// between Masters.
pub(super) struct Relay {
    path: Vec<Value>,
    deadline: Instant,
}
impl Relay {
    /// A request this Master starts itself.
    fn origin() -> Self {
        Self { path: Vec::new(), deadline: Instant::now() + RELAY_BUDGET }
    }
    /// Takes the context `relay_request` put in the params (none: this Master is the origin).
    pub(super) fn take(params: &mut serde_json::Map<String, Value>) -> Self {
        match params.remove("__relay") {
            Some(context) => Self {
                path: context["path"].as_array().cloned().unwrap_or_default(),
                deadline: Instant::now() + Duration::from_millis(context["timeoutMs"].as_u64().unwrap_or(0)),
            },
            None => Self::origin(),
        }
    }
    fn left(&self) -> Duration {
        self.deadline.saturating_duration_since(Instant::now())
    }
}

fn now() -> String {
    chrono::Utc::now().to_rfc3339()
}

/// What this Master grants one Master that merged it: Full control or View only, optionally only the hosts with one
/// of `tags` (this Master's own Tags), and whether it may pass on the hosts this Master reaches through its Merges.
pub(crate) struct Grant {
    pub role: String,
    pub tags: Option<Vec<String>>,
    pub pass_through: bool,
}
impl Grant {
    fn json(&self) -> Value {
        json!({"role":self.role,"tags":self.tags,"passThrough":self.pass_through})
    }
    /// Whether the Tag limit lets the merging Master see and use `host` (an ID this Master uses).
    fn reaches(&self, state: &Path, host: &str) -> Result<bool> {
        let Some(wanted) = &self.tags else { return Ok(true) };
        let tags = crate::tags::list(state)?;
        Ok(tags["hosts"][host].as_array().into_iter().flatten().filter_map(Value::as_str)
            .any(|tag| wanted.iter().any(|w| w.to_lowercase() == tag.to_lowercase())))
    }
    /// The role and Tag checks for one relayed call; `E_MERGE_ROLE` names this Master and the limit.
    fn allow(&self, state: &Path, config: &Config, method: &str, host: &str) -> Result<()> {
        if self.role != "full" && !READ_METHODS.contains(&method) {
            bail!("E_MERGE_ROLE: {} gives this Master View only", config.label);
        }
        if !self.reaches(state, host)? {
            let tags = self.tags.as_deref().unwrap_or_default().join(", ");
            bail!("E_MERGE_ROLE: {} gives this Master only hosts with Tag {tags}", config.label);
        }
        Ok(())
    }
}

/// Joining creates the grant; relaying requires one. A disabled grant stays disabled
/// until this Master allows it again, so the merging Master cannot simply rejoin.
fn grant(db: &Connection, master_id: &str, join: bool) -> Result<Grant> {
    let revoked: Option<bool> = db
        .query_row(
            "SELECT revoked FROM relays WHERE master_id=?1",
            [master_id],
            |row| row.get(0),
        )
        .optional()?;
    match revoked {
        Some(true) => bail!("E_PERMISSION: merge was disabled on this Master"),
        None if !join => bail!("E_PERMISSION: merge this Master before relaying"),
        _ => {}
    }
    let label = db
        .query_row("SELECT document FROM masters WHERE id=?1", [master_id], |row| {
            row.get::<_, String>(0)
        })
        .optional()?
        .and_then(|document| serde_json::from_str::<Value>(&document).ok())
        .and_then(|master| master["label"].as_str().map(str::to_owned))
        .unwrap_or_else(|| master_id.to_owned());
    db.execute(
        "INSERT INTO relays(master_id,label,joined_at,last_used_at,revoked) VALUES(?1,?2,?3,?3,0)
         ON CONFLICT(master_id) DO UPDATE SET label=excluded.label,last_used_at=excluded.last_used_at",
        params![master_id, label, now()],
    )?;
    let (role, tags, pass_through) = db.query_row(
        "SELECT role,tags,pass_through FROM relays WHERE master_id=?1",
        [master_id],
        |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?, row.get::<_, bool>(2)?)),
    )?;
    let tags = tags.map(|text| serde_json::from_str(&text)).transpose().context("invalid stored Merge Tags")?;
    Ok(Grant { role, tags, pass_through })
}

pub fn relay_join(state: &Path, config: &Config, master_id: &str) -> Result<Value> {
    role(config, "master")?;
    let db = database(state)?;
    let grant = grant(&db, master_id, true)?;
    let mut targets = Vec::new();
    for t in list_targets(&db)? {
        if t.runner_installed && t.node_id != master_id && grant.reaches(state, &t.id)? {
            targets.push(json!({"id":t.id,"label":t.label,"os":t.os,"tenant":t.tenant,"nodeId":t.node_id,"runnerInstalled":true,"hops":1,"masters":[]}));
        }
    }
    let (passed, beyond) = if grant.pass_through { passed_on(&db, config, master_id, &grant, state)? } else { (Vec::new(), 0) };
    targets.extend(passed);
    Ok(json!({"nodeId":config.node_id,"label":config.label,"targets":targets,"grant":grant.json(),"relayVersion":RELAY_VERSION,"beyond":beyond}))
}

/// The hosts this Master reaches through its own Merges, as it passes them on to `master_id`: from its stored merge
/// records (no live calls), one hop further, with the Masters on the way and the lowest grant so far. Rows that would
/// pass through `master_id` or this Master (loops), or lie more than `MAX_HOPS` away, are left out; the latter are
/// counted in `beyond`.
fn passed_on(db: &Connection, config: &Config, master_id: &str, grant: &Grant, state: &Path) -> Result<(Vec<Value>, u64)> {
    let mut rows = Vec::new();
    let mut beyond = 0;
    for merge in merges(db)? {
        let peer_id = merge["targetId"].as_str().unwrap_or_default();
        let Ok(peer) = target(db, peer_id) else { continue };
        if peer.node_id == master_id || merge["status"] != "merged" {
            continue;
        }
        beyond += merge["beyond"].as_u64().unwrap_or(0);
        let peer_hop = json!({"id":peer.node_id,"label":merge["label"].as_str().unwrap_or(&peer.label)});
        let peer_view = merge["grant"]["role"] == "view";
        for child in merge["children"].as_array().into_iter().flatten() {
            let id = format!("{peer_id}/{}", child["id"].as_str().unwrap_or_default());
            let hops = child["hops"].as_u64().unwrap_or(1) + 1;
            let mut masters = vec![peer_hop.clone()];
            masters.extend(child["masters"].as_array().cloned().unwrap_or_default());
            if masters.iter().any(|m| m["id"] == master_id || m["id"] == config.node_id.as_str()) || child["nodeId"] == master_id {
                continue;
            }
            if hops as usize > MAX_HOPS {
                beyond += 1;
                continue;
            }
            if !grant.reaches(state, &id)? {
                continue;
            }
            let (access, from) = if peer_view { ("view", peer_hop["label"].clone()) }
                else if child["access"] == "view" { ("view", child["accessFrom"].clone()) } else { ("full", Value::Null) };
            rows.push(json!({"id":id,"label":child["label"],"os":child["os"],"tenant":child["tenant"],"nodeId":child["nodeId"],
                "runnerInstalled":true,"hops":hops,"masters":masters,"access":access,"accessFrom":from}));
        }
    }
    Ok((rows, beyond))
}

/// A relayed host ID: `child`, or `peer/…/child` for a host this Master reaches through its Merges.
fn relayed_id(id: &str) -> Result<()> {
    let parts: Vec<&str> = id.split('/').collect();
    ensure!(parts.len() <= MAX_HOPS, "E_RELAY_TOO_FAR: {id} is more than {MAX_HOPS} Masters away");
    parts.iter().try_for_each(|part| slug(part, "relayed targetId"))
}
/// The Relay Path of a request from `master_id`: the one it sent, or just that Master for an older one. It must end
/// with the Master that sent it, must not hold this Master (a loop), and may hold at most `MAX_HOPS` Masters.
fn relay_path(db: &Connection, config: &Config, master_id: &str, p: &Value) -> Result<Vec<Value>> {
    let path = match p.get("path").and_then(Value::as_array) {
        Some(path) => path.clone(),
        None => {
            let label: Option<String> = db.query_row("SELECT label FROM relays WHERE master_id=?1", [master_id], |row| row.get(0)).optional()?;
            vec![json!({"id":master_id,"label":label.unwrap_or_else(|| master_id.to_owned())})]
        }
    };
    ensure!(path.last().is_some_and(|last| last["id"] == master_id), "E_PERMISSION: the Relay Path doesn't end with the Master that sent it");
    for hop in &path {
        Uuid::parse_str(hop["id"].as_str().unwrap_or_default()).context("E_ARGUMENT: invalid Relay Path")?;
        printable(hop["label"].as_str().unwrap_or_default(), "Relay Path label", 256)?;
    }
    ensure!(!path.iter().any(|hop| hop["id"] == config.node_id.as_str()), "E_RELAY_LOOP: this request has already passed through {}", config.label);
    ensure!(path.len() <= MAX_HOPS, "E_RELAY_TOO_FAR: the host is more than {MAX_HOPS} Masters away; merge a closer Master");
    Ok(path)
}
pub fn relay_request(state: &Path, config: &Config, master_id: &str, p: &Value) -> Result<Value> {
    role(config, "master")?;
    let child = str_arg(p, "targetId")?;
    relayed_id(child)?;
    let method = str_arg(p, "method")?;
    ensure!(
        RELAY_METHODS.contains(&method),
        "E_PERMISSION: method is not available over relay"
    );
    let db = database(state)?;
    let path = relay_path(&db, config, master_id, p)?;
    let grant = grant(&db, master_id, false)?;
    drop(db);
    grant.allow(state, config, method, child)?;
    ensure!(
        grant.pass_through || !child.contains('/'),
        "E_MERGE_ROLE: {} doesn't pass on the hosts it reaches through other Masters",
        config.label
    );
    let left = p["timeoutMs"].as_u64().map_or(RELAY_BUDGET, Duration::from_millis).min(RELAY_BUDGET);
    let mut forwarded = path;
    forwarded.push(json!({"id":config.node_id,"label":config.label}));
    let mut inner = p.get("params").cloned().unwrap_or_else(|| json!({}));
    let object = inner.as_object_mut().context("E_ARGUMENT: relay params must be an object")?;
    object.insert("targetId".into(), json!(child));
    object.insert("__relay".into(), json!({"path":forwarded,"timeoutMs":left.as_millis() as u64}));
    handle(
        state,
        config,
        &Request {
            method: method.into(),
            params: inner,
        },
    )?
    .context("relay method is not handled by the fleet")
}

pub(crate) fn relay_attach(
    state: &Path,
    config: &Config,
    master_id: &str,
    p: &Value,
) -> Result<transport::Stream> {
    role(config, "master")?;
    stream_allowed(state, config, master_id, p)?;
    attach(state, config, p)
}

/// A relayed Shell stream needs Full control on its host; checked when it opens and again on every frame, so a role
/// lowered meanwhile stops the input.
pub(crate) fn stream_allowed(state: &Path, config: &Config, master_id: &str, p: &Value) -> Result<()> {
    let child = str_arg(p, "targetId")?;
    relayed_id(child)?;
    let db = database(state)?;
    if p.get("path").is_some() {
        relay_path(&db, config, master_id, p)?;
    }
    let grant = grant(&db, master_id, false)?;
    drop(db);
    grant.allow(state, config, "terminal.open", child)?;
    ensure!(
        grant.pass_through || !child.contains('/'),
        "E_MERGE_ROLE: {} doesn't pass on the hosts it reaches through other Masters",
        config.label
    );
    Ok(())
}

fn relays(state: &Path) -> Result<Value> {
    let db = database(state)?;
    let mut stmt = db.prepare(
        "SELECT master_id,label,joined_at,last_used_at,revoked,role,tags,pass_through FROM relays ORDER BY label",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok(json!({
            "masterId": row.get::<_, String>(0)?,
            "label": row.get::<_, String>(1)?,
            "joinedAt": row.get::<_, String>(2)?,
            "lastUsedAt": row.get::<_, Option<String>>(3)?,
            "revoked": row.get::<_, bool>(4)?,
            "role": row.get::<_, String>(5)?,
            "tags": row.get::<_, Option<String>>(6)?.and_then(|text| serde_json::from_str::<Value>(&text).ok()),
            "passThrough": row.get::<_, bool>(7)?,
        }))
    })?;
    Ok(json!(rows.collect::<rusqlite::Result<Vec<_>>>()?))
}

fn set_relay(state: &Path, master_id: &str, revoked: bool) -> Result<Value> {
    Uuid::parse_str(master_id).context("invalid Master id")?;
    let changed = database(state)?.execute(
        "UPDATE relays SET revoked=?2 WHERE master_id=?1",
        params![master_id, revoked],
    )?;
    ensure!(changed > 0, "unknown merged Master");
    Ok(json!({"masterId":master_id,"revoked":revoked}))
}

/// `relays.set`: the role (`full` | `view`), the Tag limit (`tags`: a list, or null for all hosts) and the
/// pass-through switch for one Master that merged this one. Fields left out keep their value.
fn set_grant(state: &Path, p: &Value) -> Result<Value> {
    let master_id = str_arg(p, "masterId")?;
    Uuid::parse_str(master_id).context("invalid Master id")?;
    let db = database(state)?;
    ensure!(
        db.query_row("SELECT 1 FROM relays WHERE master_id=?1", [master_id], |_| Ok(())).optional()?.is_some(),
        "unknown merged Master"
    );
    if let Some(role) = p.get("role") {
        let role = role.as_str().filter(|r| matches!(*r, "full" | "view")).context("E_ARGUMENT: role is full or view")?;
        db.execute("UPDATE relays SET role=?2 WHERE master_id=?1", params![master_id, role])?;
    }
    if let Some(tags) = p.get("tags") {
        let stored = match tags {
            Value::Null => None,
            Value::Array(list) => {
                let mut kept: Vec<String> = Vec::new();
                for tag in list {
                    let tag = tag.as_str().context("E_ARGUMENT: tags must be text")?.trim();
                    ensure!((1..=crate::tags::MAX_LENGTH).contains(&tag.chars().count()), "E_TAG_LENGTH: a Tag has 1-{} characters", crate::tags::MAX_LENGTH);
                    if !kept.iter().any(|k| k.to_lowercase() == tag.to_lowercase()) {
                        kept.push(tag.to_owned());
                    }
                }
                ensure!(!kept.is_empty(), "E_ARGUMENT: choose at least one Tag, or all hosts");
                Some(serde_json::to_string(&kept)?)
            }
            _ => bail!("E_ARGUMENT: tags must be a list of text or null"),
        };
        db.execute("UPDATE relays SET tags=?2 WHERE master_id=?1", params![master_id, stored])?;
    }
    if let Some(on) = p.get("passThrough") {
        let on = on.as_bool().context("E_ARGUMENT: passThrough is true or false")?;
        db.execute("UPDATE relays SET pass_through=?2 WHERE master_id=?1", params![master_id, on])?;
    }
    drop(db);
    relays(state)?
        .as_array()
        .and_then(|rows| rows.iter().find(|row| row["masterId"] == master_id).cloned())
        .context("unknown merged Master")
}

pub(super) enum Via {
    Direct(Target),
    Relay { peer: Target, child: String },
}

/// Merged Targets are addressed as `<merged Master Target id>/<its Target id>`.
pub(super) fn relay_target(db: &Connection, id: &str) -> Result<(Target, String)> {
    let (peer_id, child) = id
        .split_once('/')
        .with_context(|| format!("unknown target: {id}"))?;
    relayed_id(child)?;
    let peer = target(db, peer_id)?;
    ensure!(
        peer.runner_installed,
        "RUNNER_NOT_INSTALLED: install and import a v2 registration for {peer_id}"
    );
    let merge = merge_document(db, peer_id)?.with_context(|| {
        format!("E_TARGET: {peer_id} is not merged; merge it from the allowed Masters page")
    })?;
    ensure!(
        merge["children"]
            .as_array()
            .is_some_and(|children| children.iter().any(|t| t["id"] == child)),
        "unknown target: {id}; sync the merge with {peer_id}"
    );
    Ok((peer, child.to_owned()))
}

pub(super) fn resolve(db: &Connection, id: &str) -> Result<Via> {
    if id.contains('/') {
        let (peer, child) = relay_target(db, id)?;
        return Ok(Via::Relay { peer, child });
    }
    let target = target(db, id)?;
    ensure!(
        target.runner_installed,
        "RUNNER_NOT_INSTALLED: install and import a v2 registration for {id}"
    );
    Ok(Via::Direct(target))
}

pub(super) fn send(
    state: &Path,
    config: &Config,
    via: &Via,
    fleet_method: &str,
    bridge_method: &str,
    params: Value,
    relay: &Relay,
) -> Result<Value> {
    match via {
        // The last hop: this Master's own SSH call, within what is left (a direct request keeps its usual limit).
        Via::Direct(target) if relay.path.is_empty() => transport::request(state, config, target, bridge_method, params),
        Via::Direct(target) => {
            transport::request_within(state, config, target, bridge_method, params, relay.left().max(Duration::from_secs(1)))
        }
        Via::Relay { peer, child } => {
            let wait = relay.left();
            let pass = wait.checked_sub(HOP_MARGIN).filter(|left| !left.is_zero())
                .with_context(|| format!("E_RELAY_TIMEOUT: {} didn't answer in time", peer.label))?;
            let mut path = relay.path.clone();
            if path.is_empty() {
                path.push(json!({"id":config.node_id,"label":config.label}));
            }
            hop(state, config, peer, "relay.request",
                json!({"targetId":child,"method":fleet_method,"params":params,"path":path,"timeoutMs":pass.as_millis() as u64}), wait)
        }
    }
}

pub(super) fn doctor(state: &Path, config: &Config, id: &str, p: &Value) -> Result<Value> {
    let (peer, child) = relay_target(&database(state)?, id)?;
    let mut params = p.clone();
    let object = params.as_object_mut().context("request params must be an object")?;
    object.remove("targetId");
    let relay = Relay::take(object);
    let mut result = send(
        state,
        config,
        &Via::Relay { peer, child },
        "targets.doctor",
        "",
        params,
        &relay,
    )?;
    result["targetId"] = json!(id);
    Ok(result)
}

fn merge_document(db: &Connection, target_id: &str) -> Result<Option<Value>> {
    db.query_row(
        "SELECT document FROM merges WHERE target_id=?1",
        [target_id],
        |row| row.get::<_, String>(0),
    )
    .optional()?
    .map(|document| serde_json::from_str(&document).context("invalid stored merge"))
    .transpose()
}

fn save_merge(db: &Connection, document: &Value) -> Result<()> {
    db.execute(
        "INSERT INTO merges(target_id,document) VALUES(?1,?2)
         ON CONFLICT(target_id) DO UPDATE SET document=excluded.document",
        params![
            str_arg(document, "targetId")?,
            serde_json::to_string(document)?
        ],
    )?;
    Ok(())
}

fn merges(db: &Connection) -> Result<Vec<Value>> {
    let mut stmt = db.prepare("SELECT document FROM merges ORDER BY target_id")?;
    let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
    rows.map(|row| serde_json::from_str(&row?).context("invalid stored merge"))
        .collect()
}

/// Validate a `relay.join` reply from the Master registered as `target_id` and store it.
pub fn record_merge(state: &Path, target_id: &str, join: &Value) -> Result<Value> {
    let db = database(state)?;
    let peer = target(&db, target_id)?;
    ensure!(
        join["nodeId"] == peer.node_id.as_str(),
        "E_IDENTITY: merged Master identity does not match {target_id}"
    );
    let label = join["label"].as_str().unwrap_or(&peer.label);
    printable(label, "Master label", 256)?;
    let list = join["targets"]
        .as_array()
        .context("invalid merge reply: targets missing")?;
    ensure!(
        list.len() <= MAX_TARGETS,
        "merged Master reports more than {MAX_TARGETS} Targets"
    );
    let mut seen = BTreeSet::new();
    let mut children = Vec::new();
    for child in list {
        let id = str_arg(child, "id")?;
        relayed_id(id)?;
        ensure!(seen.insert(id), "duplicate merged target id {id}");
        let label = str_arg(child, "label")?;
        printable(label, "label", 256)?;
        let tenant = str_arg(child, "tenant")?;
        slug(tenant, "tenant")?;
        let os = str_arg(child, "os")?;
        ensure!(
            matches!(os, "windows" | "linux"),
            "os must be windows or linux"
        );
        let node_id = str_arg(child, "nodeId")?;
        Uuid::parse_str(node_id).context("invalid merged Target nodeId")?;
        // Hosts the merged Master reaches through its own Merges: how far, through whom, and the lowest grant there.
        let hops = child["hops"].as_u64().unwrap_or(1);
        ensure!((1..=MAX_HOPS as u64).contains(&hops) && id.split('/').count() as u64 <= hops, "invalid merged target hops for {id}");
        let mut masters = Vec::new();
        for hop in child["masters"].as_array().into_iter().flatten() {
            Uuid::parse_str(hop["id"].as_str().unwrap_or_default()).context("invalid merged target path")?;
            let label = str_arg(hop, "label")?;
            printable(label, "Master label", 256)?;
            masters.push(json!({"id":hop["id"],"label":label}));
        }
        ensure!(masters.len() as u64 == hops - 1, "invalid merged target path for {id}");
        let access = if child["access"] == "view" { "view" } else { "full" };
        let from = child["accessFrom"].as_str().filter(|_| access == "view");
        if let Some(from) = from {
            printable(from, "Master label", 256)?;
        }
        children.push(
            json!({"id":id,"label":label,"tenant":tenant,"os":os,"nodeId":node_id,"hops":hops,"masters":masters,"access":access,"accessFrom":from}),
        );
    }
    let reverse = merge_document(&db, target_id)?.map_or(Value::Null, |d| d["reverse"].clone());
    // What the merged Master grants this one (none from an older Master: Full control, as before).
    let grant = match &join["grant"] {
        Value::Object(_) => json!({"role":if join["grant"]["role"] == "view" { "view" } else { "full" },
            "tags":join["grant"]["tags"].as_array().cloned(),"passThrough":join["grant"]["passThrough"] != false}),
        _ => Value::Null,
    };
    let document = json!({"targetId":target_id,"label":label,"status":"merged","error":null,
        "syncedAt":now(),"children":children,"reverse":reverse,"grant":grant,"relayVersion":join["relayVersion"],
        "beyond":join["beyond"].as_u64().unwrap_or(0)});
    save_merge(&db, &document)?;
    Ok(document)
}

/// Collapse `E_X: E_X: message` chains that build up across relay hops.
fn tidy(message: &str) -> String {
    let mut rest = message.trim();
    while let Some((code, tail)) = rest.split_once(": ") {
        let is_code = !code.is_empty() && code.bytes().all(|b| b.is_ascii_uppercase() || b == b'_');
        if !(is_code && tail.starts_with(&format!("{code}: "))) {
            break;
        }
        rest = tail;
    }
    rest.to_owned()
}

fn failure_status(message: &str) -> &'static str {
    if message.contains("method is not available over bridge") {
        "outdated"
    } else if message.contains("master role is not enabled") {
        "not-master"
    } else if message.contains("merge was disabled") {
        "disabled"
    } else {
        "error"
    }
}

/// A failed sync keeps the last known Targets so existing relays keep working.
fn record_failure(state: &Path, target_id: &str, label: &str, message: &str) -> Result<()> {
    let db = database(state)?;
    let previous = merge_document(&db, target_id)?;
    let document = json!({
        "targetId": target_id,
        "label": previous.as_ref().and_then(|d| d["label"].as_str()).unwrap_or(label),
        "status": failure_status(message),
        "error": message,
        "syncedAt": previous.as_ref().map(|d| d["syncedAt"].clone()).unwrap_or(Value::Null),
        "children": previous.as_ref().map(|d| d["children"].clone()).unwrap_or_else(|| json!([])),
        "reverse": previous.as_ref().map_or(Value::Null, |d| d["reverse"].clone()),
    });
    save_merge(&db, &document)
}

fn join(state: &Path, config: &Config, target_id: &str) -> Result<Value> {
    let peer = target(&database(state)?, target_id)?;
    ensure!(
        peer.runner_installed,
        "RUNNER_NOT_INSTALLED: install and import a v2 registration for {target_id}"
    );
    match hop(state, config, &peer, "relay.join", json!({}), transport::RELAY_TIMEOUT) {
        Ok(reply) => record_merge(state, target_id, &reply),
        Err(error) => {
            let message = tidy(&format!("{error:#}"));
            record_failure(state, target_id, &peer.label, &message)?;
            bail!("{message}")
        }
    }
}

/// The Target this Master registered for the Master `master_id`, if it paired back.
fn requester_target(db: &Connection, master_id: &str) -> Result<Option<Target>> {
    Ok(list_targets(db)?.into_iter().find(|t| t.node_id == master_id))
}

/// A merged Master asks to become one of this Master's Targets, so merges run both ways.
pub fn relay_pair(state: &Path, config: &Config, master_id: &str, p: &Value) -> Result<Value> {
    role(config, "master")?;
    let db = database(state)?;
    grant(&db, master_id, false)?;
    // "unknown" (two requests, one per installer) is for the local add flow only.
    let os = str_arg(p, "os")?;
    ensure!(
        matches!(os, "windows" | "linux"),
        "E_ARGUMENT: os must be windows or linux"
    );
    if let Some(existing) = requester_target(&db, master_id)? {
        return Ok(json!({"existingTargetId":existing.id}));
    }
    let label: String = db.query_row(
        "SELECT label FROM relays WHERE master_id=?1",
        [master_id],
        |row| row.get(0),
    )?;
    drop(db);
    crate::pairing::handle(
        state,
        config,
        &Request {
            method: "pairing.create".into(),
            params: json!({"label":label,"tenant":MASTERS_TENANT,"os":os,
                "routes":p.get("routes").context("E_ARGUMENT: routes required")?}),
        },
    )?
    .context("pairing is unavailable")
}

/// Accept the merged Master's own pairing code, then merge it back.
pub fn relay_pair_complete(
    state: &Path,
    config: &Config,
    master_id: &str,
    p: &Value,
) -> Result<Value> {
    role(config, "master")?;
    grant(&database(state)?, master_id, false)?;
    let mut pairing = Value::Null;
    if let Some(code) = p.get("code").and_then(Value::as_str) {
        let receipt = crate::pairing::decode_receipt(code)?;
        ensure!(
            receipt.registration.node_id == master_id,
            "E_IDENTITY: pairing code belongs to another machine"
        );
        pairing = crate::pairing::handle(
            state,
            config,
            &Request {
                method: "pairing.complete".into(),
                params: json!({"code":code}),
            },
        )?
        .context("pairing is unavailable")?;
    }
    let target = requester_target(&database(state)?, master_id)?
        .context("E_TARGET: this Master is not paired back yet")?;
    match join(state, config, &target.id) {
        Ok(merge) => Ok(json!({"targetId":target.id,"status":"two-way","error":null,
            "pairing":pairing["state"],"children":merge["children"].as_array().map_or(0, Vec::len)})),
        Err(error) => Ok(json!({"targetId":target.id,"status":"needs-attention",
            "error":tidy(&format!("{error:#}")),"pairing":pairing["state"]})),
    }
}

fn merged_peer(db: &Connection, target_id: &str) -> Result<Target> {
    let peer = target(db, target_id)?;
    ensure!(
        merge_document(db, target_id)?.is_some(),
        "E_TARGET: merge {target_id} before opening the other direction"
    );
    Ok(peer)
}

/// Addresses of this machine that the merged Master's routes leave from.
fn local_routes(peer: &Target) -> Vec<Value> {
    let mut seen = BTreeSet::new();
    peer.routes
        .iter()
        .filter(|r| r.proxy_jump.is_none())
        .filter_map(|r| {
            let socket = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
            socket.connect((r.address.as_str(), r.port)).ok()?;
            let ip = socket.local_addr().ok()?.ip();
            (!ip.is_unspecified() && !ip.is_loopback()).then(|| ip.to_string())
        })
        .filter(|ip| seen.insert(ip.clone()))
        .map(|ip| json!({"kind":crate::pairing::route_kind(&ip),"address":ip,"port":22}))
        .collect()
}

fn set_reverse(state: &Path, target_id: &str, reverse: Value) -> Result<()> {
    let db = database(state)?;
    if let Some(mut document) = merge_document(&db, target_id)? {
        document["reverse"] = reverse;
        save_merge(&db, &document)?;
    }
    Ok(())
}

fn pair_request(state: &Path, config: &Config, target_id: &str) -> Result<Value> {
    let peer = merged_peer(&database(state)?, target_id)?;
    let routes = local_routes(&peer);
    ensure!(
        !routes.is_empty(),
        "E_ROUTE: no local address reaches {target_id}"
    );
    let params = json!({"routes":routes,"os":std::env::consts::OS});
    match transport::request(state, config, &peer, "relay.pair", params) {
        Ok(reply) => {
            let peer_target = reply.get("existingTargetId").unwrap_or(&reply["targetId"]);
            set_reverse(state, target_id, json!({"status":"pending","peerTargetId":peer_target,
                "requestId":reply["requestId"],"error":null,"updatedAt":now()}))?;
            Ok(reply)
        }
        Err(error) => {
            let message = tidy(&format!("{error:#}"));
            set_reverse(state, target_id, json!({"status":"error","error":message,"updatedAt":now()}))?;
            bail!("{message}")
        }
    }
}

fn pair_complete(state: &Path, config: &Config, target_id: &str, code: Option<&str>) -> Result<Value> {
    let peer = merged_peer(&database(state)?, target_id)?;
    let params = code.map_or_else(|| json!({}), |code| json!({"code":code}));
    match transport::request_within(state, config, &peer, "relay.pair_complete", params, PAIR_TIMEOUT) {
        Ok(reply) => {
            set_reverse(state, target_id, json!({"status":reply["status"],"peerTargetId":reply["targetId"],
                "error":reply["error"],"updatedAt":now()}))?;
            Ok(reply)
        }
        Err(error) => {
            let message = tidy(&format!("{error:#}"));
            set_reverse(state, target_id, json!({"status":"error","error":message,"updatedAt":now()}))?;
            bail!("{message}")
        }
    }
}

fn installed_targets(db: &Connection) -> Result<Vec<Target>> {
    Ok(list_targets(db)?
        .into_iter()
        .filter(|t| t.runner_installed)
        .collect())
}

/// `work` for each item on at most `PROBE_WORKERS` threads, results in input order.
fn bounded<T: Sync, R: Send>(items: &[T], work: impl Fn(&T) -> R + Sync) -> Vec<R> {
    let next = AtomicUsize::new(0);
    let results: Vec<Mutex<Option<R>>> = items.iter().map(|_| Mutex::new(None)).collect();
    thread::scope(|scope| {
        for _ in 0..PROBE_WORKERS.min(items.len()) {
            scope.spawn(|| loop {
                let index = next.fetch_add(1, Ordering::Relaxed);
                let Some(item) = items.get(index) else {
                    break;
                };
                let result = work(item);
                *results[index]
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner) = Some(result);
            });
        }
    });
    results
        .into_iter()
        .map(|slot| {
            slot.into_inner()
                .unwrap_or_else(PoisonError::into_inner)
                .expect("every item is probed")
        })
        .collect()
}

fn status(state: &Path, config: &Config, target: &Target) -> Result<Value, String> {
    transport::request_within(
        state,
        config,
        target,
        "system.status",
        json!({}),
        PROBE_TIMEOUT,
    )
    .map_err(|error| format!("{error:#}"))
}

/// `system.status` of each Target, in input order.
fn probe(state: &Path, config: &Config, targets: &[Target]) -> Vec<Result<Value, String>> {
    bounded(targets, |t| status(state, config, t))
}

/// None when only a ProxyJump route could reach the Target, which needs a real SSH probe.
fn reachable(target: &Target) -> Option<Result<(), String>> {
    let mut failures = Vec::new();
    for route in target.routes.iter().filter(|r| r.proxy_jump.is_none()) {
        match ssh_banner::read(&route.address, route.port) {
            Ok(_) => return Some(Ok(())),
            Err(error) => failures.push(error),
        }
    }
    if target.routes.iter().any(|r| r.proxy_jump.is_some()) {
        return None;
    }
    Some(Err(if failures.is_empty() {
        "no direct route".into()
    } else {
        failures.join("; ")
    }))
}

fn is_master(status: &Value) -> bool {
    status["roles"]
        .as_array()
        .is_some_and(|roles| roles.iter().any(|r| r == "master" || r == "both"))
}

/// Reports which Targets are Masters. `only` probes a single Target (used right after pairing).
fn discover(state: &Path, config: &Config, only: Option<&str>) -> Result<Value> {
    let mut targets = installed_targets(&database(state)?)?;
    if let Some(id) = only {
        targets.retain(|t| t.id == id);
    }
    let rows: Vec<Value> = probe(state, config, &targets)
        .into_iter()
        .zip(&targets)
        .map(|(status, t)| match status {
            Ok(status) => json!({"targetId":t.id,"label":t.label,"isMaster":is_master(&status),
                "roles":status["roles"],"version":status["version"],"error":null}),
            Err(error) => {
                json!({"targetId":t.id,"label":t.label,"isMaster":false,"error":error})
            }
        })
        .collect();
    Ok(json!(rows))
}

fn cached_health(db: &Connection) -> Result<BTreeMap<String, Value>> {
    let mut stmt = db.prepare("SELECT target_id,document FROM health")?;
    let rows = stmt.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })?;
    let mut cached = BTreeMap::new();
    for row in rows {
        let (id, document) = row?;
        if let Ok(value) = serde_json::from_str(&document) {
            cached.insert(id, value);
        }
    }
    Ok(cached)
}

/// A Target that keeps failing is retried less often: 2x, 4x, then 8x `HEALTH_TTL`.
fn fresh(row: Option<&Value>) -> bool {
    let Some(row) = row else {
        return false;
    };
    let failures = row["failures"].as_u64().unwrap_or(0).min(3) as u32;
    row["checkedAt"]
        .as_str()
        .and_then(|at| chrono::DateTime::parse_from_rfc3339(at).ok())
        .is_some_and(|at| {
            chrono::Utc::now().signed_duration_since(at) < HEALTH_TTL * 2_i32.pow(failures)
        })
}

fn save_health(db: &Connection, rows: &[Value]) -> Result<()> {
    for row in rows {
        db.execute(
            "INSERT INTO health(target_id,document) VALUES(?1,?2)
             ON CONFLICT(target_id) DO UPDATE SET document=excluded.document",
            params![str_arg(row, "targetId")?, serde_json::to_string(row)?],
        )?;
    }
    Ok(())
}

/// Reachability of this Master's own Targets, probed at most once per `HEALTH_TTL`.
/// Routine checks only read the SSH banner; `refresh` asks each Target's service over SSH.
fn local_health(state: &Path, config: &Config, refresh: bool) -> Result<Vec<Value>> {
    let _probing = LOCAL_PROBE.lock().unwrap_or_else(PoisonError::into_inner);
    let db = database(state)?;
    let targets = installed_targets(&db)?;
    let mut cached = cached_health(&db)?;
    let stale: Vec<Target> = targets
        .iter()
        .filter(|t| refresh || !fresh(cached.get(&t.id)))
        .cloned()
        .collect();
    // A banner read is timed: its milliseconds are the Target's response time. A full SSH check keeps the last one.
    let results = bounded(&stale, |t| {
        let started = Instant::now();
        match (refresh, reachable(t)) {
            (false, Some(result)) => {
                result.map(|()| (None, Some(started.elapsed().as_millis() as u64)))
            }
            _ => status(state, config, t).map(|status| (Some(status), None)),
        }
    });
    let checked = now();
    let rows: Vec<Value> = results
        .into_iter()
        .zip(&stale)
        .map(|(result, t)| {
            let previous = cached.get(&t.id);
            let kept = |key: &str| previous.map_or(Value::Null, |p| p[key].clone());
            // Online since the first check of the current online run.
            let since = match previous {
                Some(p) if p["online"] == true && p["onlineSince"].is_string() => {
                    p["onlineSince"].clone()
                }
                _ => json!(checked),
            };
            match result {
                Ok((status, latency)) => json!({"targetId":t.id,"online":true,"checkedAt":checked,
                    "error":null,"failures":0,"onlineSince":since,
                    "latencyMs":latency.map_or_else(|| kept("latencyMs"), |ms| json!(ms)),
                    "isMaster":status.as_ref().map_or_else(|| kept("isMaster"), |s| json!(is_master(s))),
                    "version":status.as_ref().map_or_else(|| kept("version"), |s| s["version"].clone())}),
                Err(error) => json!({"targetId":t.id,"online":false,"checkedAt":checked,
                    "error":error,"isMaster":false,"version":null,"onlineSince":null,"latencyMs":null,
                    "failures":previous.and_then(|p| p["failures"].as_u64()).unwrap_or(0) + 1}),
            }
        })
        .collect();
    save_health(&db, &rows)?;
    for row in rows {
        cached.insert(str_arg(&row, "targetId")?.to_owned(), row);
    }
    Ok(targets
        .iter()
        .filter_map(|t| cached.remove(&t.id))
        .collect())
}

/// Health of this Master's children for a Master that merged it.
pub fn relay_health(state: &Path, config: &Config, master_id: &str) -> Result<Value> {
    role(config, "master")?;
    let db = database(state)?;
    let grant = grant(&db, master_id, false)?;
    let skipped: BTreeSet<String> = list_targets(&db)?
        .into_iter()
        .filter(|t| t.node_id == master_id)
        .map(|t| t.id)
        .collect();
    drop(db);
    let mut rows = Vec::new();
    for row in local_health(state, config, false)? {
        let id = row["targetId"].as_str().unwrap_or_default();
        if !skipped.contains(id) && grant.reaches(state, id)? {
            rows.push(row);
        }
    }
    // Hosts passed on from this Master's own Merges: what it last heard (its own health check keeps that fresh).
    if grant.pass_through {
        let db = database(state)?;
        let (passed, _) = passed_on(&db, config, master_id, &grant, state)?;
        let cached = cached_health(&db)?;
        rows.extend(passed.iter().filter_map(|row| cached.get(row["id"].as_str()?).cloned()));
    }
    Ok(json!(rows))
}

/// A child's row as its merged Master reported it, under this Master's composite id. `failures` stays behind:
/// backing off is for the Master that probes, not for this one.
fn reported_row(id: &str, r: &Value, via: &str) -> Value {
    json!({"targetId":id,"online":r["online"].as_bool(),"checkedAt":r["checkedAt"],"error":r["error"],
        "isMaster":r["isMaster"],"version":r["version"],"latencyMs":r["latencyMs"],"onlineSince":r["onlineSince"],"via":via})
}

/// Children of each merged Master, asked through that Master at most once per `HEALTH_TTL`.
/// An unreachable merged Master leaves its children unknown (`online: null`), not offline.
/// A merged Master already found offline is not asked, which would hold an SSH client open.
fn merged_health(
    state: &Path,
    config: &Config,
    refresh: bool,
    offline: &BTreeSet<String>,
) -> Result<Vec<Value>> {
    let _probing = MERGED_PROBE.lock().unwrap_or_else(PoisonError::into_inner);
    let db = database(state)?;
    let mut cached = cached_health(&db)?;
    let mut peers = Vec::new();
    for merge in merges(&db)? {
        let peer_id = str_arg(&merge, "targetId")?.to_owned();
        let Ok(peer) = target(&db, &peer_id) else {
            continue;
        };
        let ids: Vec<String> = merge["children"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|c| c["id"].as_str().map(|id| format!("{peer_id}/{id}")))
            .collect();
        let stale = refresh || ids.iter().any(|id| !fresh(cached.get(id)));
        peers.push((peer, ids, stale));
    }
    drop(db);
    let replies: Vec<Option<Result<Value, String>>> = bounded(&peers, |(peer, _, stale)| {
        if !stale {
            return None;
        }
        if offline.contains(&peer.id) {
            return Some(Err("Master is offline".into()));
        }
        Some(
            transport::request_within(
                state,
                config,
                peer,
                "relay.health",
                json!({}),
                transport::RELAY_TIMEOUT,
            )
            .map_err(|error| format!("{error:#}")),
        )
    });
    let checked = now();
    let mut rows = Vec::new();
    for ((peer, ids, _), reply) in peers.iter().zip(replies) {
        let Some(reply) = reply else {
            rows.extend(ids.iter().filter_map(|id| cached.remove(id)));
            continue;
        };
        let reported: BTreeMap<String, Value> = match &reply {
            Ok(list) => list
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|r| r["targetId"].as_str().map(|id| (id.to_owned(), r.clone())))
                .collect(),
            Err(_) => BTreeMap::new(),
        };
        let mut fresh_rows = Vec::new();
        for id in ids {
            let child = id.split_once('/').map(|(_, c)| c).unwrap_or_default();
            fresh_rows.push(match (reported.get(child), &reply) {
                (Some(r), _) => reported_row(id, r, &peer.id),
                (None, Err(error)) => json!({"targetId":id,"online":null,"checkedAt":checked,
                    "error":format!("via {}: {error}", peer.label),"isMaster":false,
                    "version":null,"via":peer.id}),
                (None, Ok(_)) => json!({"targetId":id,"online":null,"checkedAt":checked,
                    "error":"not reported by the merged Master","isMaster":false,
                    "version":null,"via":peer.id}),
            });
        }
        save_health(&database(state)?, &fresh_rows)?;
        rows.extend(fresh_rows);
    }
    Ok(rows)
}

fn fleet_health(state: &Path, config: &Config, refresh: bool) -> Result<Value> {
    let mut rows = local_health(state, config, refresh)?;
    let offline: BTreeSet<String> = rows
        .iter()
        .filter(|row| row["online"] == false)
        .filter_map(|row| row["targetId"].as_str().map(str::to_owned))
        .collect();
    rows.extend(merged_health(state, config, refresh, &offline)?);
    Ok(json!(rows))
}

/// Hosts reached through merged Masters, as read-only fleet rows. A host is listed once, keyed by its `nodeId`: a
/// machine this Master reaches directly is left to its direct row, and one reached through several Merges keeps the
/// route with the fewest hops, then the one whose last health check worked. Rows whose path passes through this
/// Master (loops) are dropped. Each row carries the Masters on the way (`path`) and the lowest grant along it.
pub(super) fn merged_targets(db: &Connection, direct: &[Target], node_id: &str) -> Result<Vec<Value>> {
    let known: BTreeSet<&str> = direct.iter().map(|t| t.node_id.as_str()).collect();
    let health = cached_health(db)?;
    let mut best: BTreeMap<String, (u64, bool, Value)> = BTreeMap::new();
    let mut order = Vec::new();
    for merge in merges(db)? {
        let peer = merge["targetId"].as_str().unwrap_or_default();
        let via_label = direct
            .iter()
            .find(|t| t.id == peer)
            .map_or_else(|| merge["label"].clone(), |t| json!(t.label));
        let master_label = merge["label"].clone();
        let peer_view = merge["grant"]["role"] == "view";
        for child in merge["children"].as_array().into_iter().flatten() {
            let child_node = child["nodeId"].as_str().unwrap_or_default();
            let masters = child["masters"].as_array().cloned().unwrap_or_default();
            if known.contains(child_node) || child_node == node_id || masters.iter().any(|m| m["id"] == node_id) {
                continue;
            }
            let id = format!("{peer}/{}", child["id"].as_str().unwrap_or_default());
            let hops = child["hops"].as_u64().unwrap_or(1);
            let online = health.get(&id).is_some_and(|row| row["online"] == true);
            let mut path = vec![master_label.clone()];
            path.extend(masters.iter().map(|m| m["label"].clone()));
            let (access, from) = if peer_view { ("view", master_label.clone()) }
                else if child["access"] == "view" { ("view", child["accessFrom"].clone()) } else { ("full", Value::Null) };
            let row = json!({
                "schemaVersion": 2,
                "id": id,
                "nodeId": child["nodeId"],
                "label": child["label"],
                "tenant": child["tenant"],
                "os": child["os"],
                "user": "",
                "hostKeys": [],
                "routes": [],
                "bridgePath": "",
                "runnerInstalled": true,
                "relayed": true,
                "via": peer,
                "viaLabel": via_label,
                "path": path,
                "hops": hops,
                "access": access,
                "accessFrom": from,
            });
            let better = best.get(child_node).is_none_or(|(h, o, _)| hops < *h || (hops == *h && online && !*o));
            if !best.contains_key(child_node) {
                order.push(child_node.to_owned());
            }
            if better {
                best.insert(child_node.to_owned(), (hops, online, row));
            }
        }
    }
    Ok(order.into_iter().filter_map(|node| best.remove(&node).map(|(_, _, row)| row)).collect())
}

#[test]
fn repeated_error_codes_collapse_but_distinct_context_stays() {
    assert_eq!(
        tidy("E_PERMISSION: E_PERMISSION: E_PERMISSION: merge was disabled on this Master"),
        "E_PERMISSION: merge was disabled on this Master"
    );
    assert_eq!(
        tidy("E_OPERATION: jobId=1: E_PERMISSION: denied"),
        "E_OPERATION: jobId=1: E_PERMISSION: denied"
    );
}

pub(super) fn handle_local(state: &Path, config: &Config, req: &Request) -> Result<Option<Value>> {
    let result = match req.method.as_str() {
        "merges.list" => {
            role(config, "master")?;
            json!(merges(&database(state)?)?)
        }
        "merges.discover" => {
            role(config, "master")?;
            discover(state, config, req.params["targetId"].as_str())?
        }
        "fleet.health" => {
            role(config, "master")?;
            let refresh = req.params["refresh"].as_bool().unwrap_or(false);
            fleet_health(state, config, refresh)?
        }
        "merges.add" | "merges.sync" => {
            role(config, "master")?;
            join(state, config, str_arg(&req.params, "targetId")?)?
        }
        "merges.pair_request" => {
            role(config, "master")?;
            pair_request(state, config, str_arg(&req.params, "targetId")?)?
        }
        "merges.pair_complete" => {
            role(config, "master")?;
            let code = req.params.get("code").and_then(Value::as_str);
            pair_complete(state, config, str_arg(&req.params, "targetId")?, code)?
        }
        "merges.remove" => {
            role(config, "master")?;
            let id = str_arg(&req.params, "targetId")?;
            let changed = database(state)?.execute("DELETE FROM merges WHERE target_id=?1", [id])?;
            ensure!(changed > 0, "unknown merge");
            json!({"targetId":id,"removed":true})
        }
        "relays.list" => relays(state)?,
        "relays.set" => set_grant(state, &req.params)?,
        "relays.revoke" => set_relay(state, str_arg(&req.params, "masterId")?, true)?,
        "relays.allow" => set_relay(state, str_arg(&req.params, "masterId")?, false)?,
        _ => return Ok(None),
    };
    Ok(Some(result))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reported_rows_keep_response_time_and_online_since() {
        let r = json!({"targetId":"child","online":true,"checkedAt":"t","error":null,"failures":2,"isMaster":false,
            "version":"0.5.1","latencyMs":38,"onlineSince":"s"});
        let row = reported_row("peer/child", &r, "peer");
        assert_eq!((row["targetId"].as_str(), row["via"].as_str()), (Some("peer/child"), Some("peer")));
        assert_eq!((row["latencyMs"].as_u64(), row["onlineSince"].as_str()), (Some(38), Some("s")));
        assert_eq!(row["failures"], Value::Null);
        // A Master older than 0.5.1 sends neither: they stay null.
        let old = reported_row("peer/child", &json!({"online":true}), "peer");
        assert_eq!((old["latencyMs"].clone(), old["onlineSince"].clone()), (Value::Null, Value::Null));
    }
}
