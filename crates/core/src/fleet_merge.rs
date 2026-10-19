//! Merged Masters: one-hop relay of fleet requests through another Master's own identity.
//! The merged Master keeps its keys; the merging Master only learns public Target metadata.
use super::{
    attach, database, handle, list_targets, printable, role, slug, str_arg, target, transport,
    Target, MAX_TARGETS,
};
use crate::{config::Config, protocol::Request};
use anyhow::{bail, ensure, Context, Result};
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, BTreeSet},
    io::Read,
    net::{TcpStream, ToSocketAddrs},
    path::Path,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Mutex, PoisonError,
    },
    thread,
    time::Duration,
};
use uuid::Uuid;

const PROBE_TIMEOUT: Duration = Duration::from_secs(20);
const BANNER_TIMEOUT: Duration = Duration::from_secs(3);
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
    "files.mkdir",
    "files.remove",
    "terminal.open",
    "terminal.read",
    "terminal.write",
    "terminal.status",
    "terminal.resize",
    "terminal.close",
];

fn now() -> String {
    chrono::Utc::now().to_rfc3339()
}

/// Joining creates the grant; relaying requires one. A disabled grant stays disabled
/// until this Master allows it again, so the merging Master cannot simply rejoin.
fn grant(db: &Connection, master_id: &str, join: bool) -> Result<()> {
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
    Ok(())
}

pub fn relay_join(state: &Path, config: &Config, master_id: &str) -> Result<Value> {
    role(config, "master")?;
    let db = database(state)?;
    grant(&db, master_id, true)?;
    let targets: Vec<Value> = list_targets(&db)?
        .into_iter()
        .filter(|t| t.runner_installed && t.node_id != master_id)
        .map(|t| {
            json!({"id":t.id,"label":t.label,"os":t.os,"tenant":t.tenant,"nodeId":t.node_id,"runnerInstalled":true})
        })
        .collect();
    Ok(json!({"nodeId":config.node_id,"label":config.label,"targets":targets}))
}

pub fn relay_request(state: &Path, config: &Config, master_id: &str, p: &Value) -> Result<Value> {
    role(config, "master")?;
    let child = str_arg(p, "targetId")?;
    slug(child, "relayed targetId")?;
    let method = str_arg(p, "method")?;
    ensure!(
        RELAY_METHODS.contains(&method),
        "E_PERMISSION: method is not available over relay"
    );
    grant(&database(state)?, master_id, false)?;
    let mut inner = p.get("params").cloned().unwrap_or_else(|| json!({}));
    inner
        .as_object_mut()
        .context("E_ARGUMENT: relay params must be an object")?
        .insert("targetId".into(), json!(child));
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
    slug(str_arg(p, "targetId")?, "relayed targetId")?;
    grant(&database(state)?, master_id, false)?;
    attach(state, config, p)
}

fn relays(state: &Path) -> Result<Value> {
    let db = database(state)?;
    let mut stmt = db.prepare(
        "SELECT master_id,label,joined_at,last_used_at,revoked FROM relays ORDER BY label",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok(json!({
            "masterId": row.get::<_, String>(0)?,
            "label": row.get::<_, String>(1)?,
            "joinedAt": row.get::<_, String>(2)?,
            "lastUsedAt": row.get::<_, Option<String>>(3)?,
            "revoked": row.get::<_, bool>(4)?,
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

pub(super) enum Via {
    Direct(Target),
    Relay { peer: Target, child: String },
}

/// Merged Targets are addressed as `<merged Master Target id>/<its Target id>`.
pub(super) fn relay_target(db: &Connection, id: &str) -> Result<(Target, String)> {
    let (peer_id, child) = id
        .split_once('/')
        .with_context(|| format!("unknown target: {id}"))?;
    slug(child, "merged target id")?;
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
) -> Result<Value> {
    match via {
        Via::Direct(target) => transport::request(state, config, target, bridge_method, params),
        Via::Relay { peer, child } => transport::request_within(
            state,
            config,
            peer,
            "relay.request",
            json!({"targetId":child,"method":fleet_method,"params":params}),
            transport::RELAY_TIMEOUT,
        ),
    }
}

pub(super) fn doctor(state: &Path, config: &Config, id: &str, p: &Value) -> Result<Value> {
    let (peer, child) = relay_target(&database(state)?, id)?;
    let mut params = p.clone();
    if let Some(object) = params.as_object_mut() {
        object.remove("targetId");
    }
    let mut result = send(
        state,
        config,
        &Via::Relay { peer, child },
        "targets.doctor",
        "",
        params,
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
        slug(id, "merged target id")?;
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
        children.push(
            json!({"id":id,"label":label,"tenant":tenant,"os":os,"nodeId":node_id}),
        );
    }
    let reverse = merge_document(&db, target_id)?.map_or(Value::Null, |d| d["reverse"].clone());
    let document = json!({"targetId":target_id,"label":label,"status":"merged","error":null,
        "syncedAt":now(),"children":children,"reverse":reverse});
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
    match transport::request(state, config, &peer, "relay.join", json!({})) {
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
            params: json!({"label":label,"tenant":MASTERS_TENANT,"os":str_arg(p, "os")?,
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

/// An OpenSSH banner without logging in, so no session, process or logon event on the Target.
fn banner(address: &str, port: u16) -> Result<(), String> {
    let fail = |error: &dyn std::fmt::Display| format!("{address}:{port}: {error}");
    let mut last = fail(&"address did not resolve");
    for addr in (address, port).to_socket_addrs().map_err(|e| fail(&e))? {
        let mut stream = match TcpStream::connect_timeout(&addr, BANNER_TIMEOUT) {
            Ok(stream) => stream,
            Err(error) => {
                last = fail(&error);
                continue;
            }
        };
        stream
            .set_read_timeout(Some(BANNER_TIMEOUT))
            .map_err(|e| fail(&e))?;
        let mut head = [0u8; 255];
        let read = stream.read(&mut head).map_err(|e| fail(&e))?;
        return if head[..read].windows(4).any(|w| w == b"SSH-") {
            Ok(())
        } else {
            Err(fail(&"port answered without an SSH banner"))
        };
    }
    Err(last)
}

/// None when only a ProxyJump route could reach the Target, which needs a real SSH probe.
fn reachable(target: &Target) -> Option<Result<(), String>> {
    let mut failures = Vec::new();
    for route in target.routes.iter().filter(|r| r.proxy_jump.is_none()) {
        match banner(&route.address, route.port) {
            Ok(()) => return Some(Ok(())),
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

fn discover(state: &Path, config: &Config) -> Result<Value> {
    let targets = installed_targets(&database(state)?)?;
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
    let results = bounded(&stale, |t| match (refresh, reachable(t)) {
        (false, Some(result)) => result.map(|()| None),
        _ => status(state, config, t).map(Some),
    });
    let checked = now();
    let rows: Vec<Value> = results
        .into_iter()
        .zip(&stale)
        .map(|(result, t)| {
            let previous = cached.get(&t.id);
            match result {
                Ok(Some(status)) => json!({"targetId":t.id,"online":true,"checkedAt":checked,
                    "error":null,"failures":0,"isMaster":is_master(&status),"version":status["version"]}),
                Ok(None) => json!({"targetId":t.id,"online":true,"checkedAt":checked,
                    "error":null,"failures":0,
                    "isMaster":previous.map_or(json!(false), |p| p["isMaster"].clone()),
                    "version":previous.map_or(Value::Null, |p| p["version"].clone())}),
                Err(error) => json!({"targetId":t.id,"online":false,"checkedAt":checked,
                    "error":error,"isMaster":false,"version":null,
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
    grant(&db, master_id, false)?;
    let skipped: BTreeSet<String> = list_targets(&db)?
        .into_iter()
        .filter(|t| t.node_id == master_id)
        .map(|t| t.id)
        .collect();
    drop(db);
    let rows: Vec<Value> = local_health(state, config, false)?
        .into_iter()
        .filter(|row| !skipped.contains(row["targetId"].as_str().unwrap_or_default()))
        .collect();
    Ok(json!(rows))
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
                (Some(r), _) => json!({"targetId":id,"online":r["online"].as_bool(),
                    "checkedAt":r["checkedAt"],"error":r["error"],"isMaster":r["isMaster"],
                    "version":r["version"],"via":peer.id}),
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

/// Children of merged Masters as read-only fleet rows. A machine this Master already
/// reaches directly is listed only once, as its direct Target.
pub(super) fn merged_targets(db: &Connection, direct: &[Target]) -> Result<Vec<Value>> {
    let known: BTreeSet<&str> = direct.iter().map(|t| t.node_id.as_str()).collect();
    let mut rows = Vec::new();
    for merge in merges(db)? {
        let peer = merge["targetId"].as_str().unwrap_or_default();
        let via_label = direct
            .iter()
            .find(|t| t.id == peer)
            .map_or_else(|| merge["label"].clone(), |t| json!(t.label));
        for child in merge["children"].as_array().into_iter().flatten() {
            if known.contains(child["nodeId"].as_str().unwrap_or_default()) {
                continue;
            }
            rows.push(json!({
                "schemaVersion": 2,
                "id": format!("{peer}/{}", child["id"].as_str().unwrap_or_default()),
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
            }));
        }
    }
    Ok(rows)
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
            discover(state, config)?
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
        "relays.revoke" => set_relay(state, str_arg(&req.params, "masterId")?, true)?,
        "relays.allow" => set_relay(state, str_arg(&req.params, "masterId")?, false)?,
        _ => return Ok(None),
    };
    Ok(Some(result))
}
