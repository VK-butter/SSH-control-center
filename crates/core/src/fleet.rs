//! Independent Master registry, public enrollment, and authenticated SSH dispatch.
//! Never reads or edits the operator's ambient SSH files.
use crate::{config::Config, protocol::Request};
use anyhow::{bail, ensure, Context, Result};
use base64::{engine::general_purpose::STANDARD, Engine};
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
    process::Command,
    time::Duration,
};
use uuid::Uuid;
#[path = "fleet_ssh.rs"]
mod transport;
#[path = "fleet_merge.rs"]
mod merge;
pub(crate) use merge::relay_attach;
pub use merge::{
    record_merge, relay_health, relay_join, relay_pair, relay_pair_complete, relay_request,
};
pub(crate) type RemoteStream = transport::Stream;
pub const MAX_TARGETS: usize = 50;
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProxyJump {
    pub address: String,
    #[serde(default = "ssh_port")]
    pub port: u16,
    pub user: String,
    pub host_keys: Vec<String>,
    pub identity_file: String,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Route {
    pub kind: String,
    pub address: String,
    #[serde(default = "ssh_port")]
    pub port: u16,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proxy_jump: Option<ProxyJump>,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Target {
    pub schema_version: u32,
    pub id: String,
    pub node_id: String,
    pub label: String,
    pub tenant: String,
    pub os: String,
    pub user: String,
    pub host_keys: Vec<String>,
    pub routes: Vec<Route>,
    pub bridge_path: String,
    pub runner_installed: bool,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct Master {
    pub(crate) id: String,
    pub(crate) label: String,
    pub(crate) public_key: String,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct Bundle {
    pub(crate) schema_version: u32,
    pub(crate) target_id: String,
    pub(crate) tenant: String,
    pub(crate) os: String,
    pub(crate) user: String,
    pub(crate) routes: Vec<Route>,
    pub(crate) master: Master,
}
fn ssh_port() -> u16 {
    22
}
pub(crate) fn database(state: &Path) -> Result<Connection> {
    ensure!(state.is_dir(), "service data directory is not initialized");
    let db = Connection::open(state.join("fleet.sqlite"))?;
    db.busy_timeout(Duration::from_secs(15))?;
    db.execute_batch(
        "PRAGMA journal_mode=WAL;
         CREATE TABLE IF NOT EXISTS targets (id TEXT PRIMARY KEY, document TEXT NOT NULL);
         CREATE TABLE IF NOT EXISTS masters (id TEXT PRIMARY KEY, document TEXT NOT NULL, revoked INTEGER NOT NULL DEFAULT 0);
         CREATE TABLE IF NOT EXISTS tenant_keys (tenant TEXT PRIMARY KEY, public_key TEXT NOT NULL);
         CREATE TABLE IF NOT EXISTS dispatches (id TEXT PRIMARY KEY, target_id TEXT NOT NULL, payload TEXT NOT NULL);
         CREATE TABLE IF NOT EXISTS relays (master_id TEXT PRIMARY KEY, label TEXT NOT NULL, joined_at TEXT NOT NULL, last_used_at TEXT, revoked INTEGER NOT NULL DEFAULT 0);
         CREATE TABLE IF NOT EXISTS merges (target_id TEXT PRIMARY KEY, document TEXT NOT NULL);
         CREATE TABLE IF NOT EXISTS health (target_id TEXT PRIMARY KEY, document TEXT NOT NULL);"
    )?;
    Ok(db)
}
fn slug(value: &str, field: &str) -> Result<()> {
    ensure!(
        !value.is_empty()
            && value.len() <= 64
            && value
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'),
        "{field} must contain 1-64 letters, digits, hyphens or underscores"
    );
    Ok(())
}
fn printable(value: &str, field: &str, max: usize) -> Result<()> {
    ensure!(
        !value.is_empty() && value.len() <= max && !value.chars().any(char::is_control),
        "{field} must be a nonempty printable string (maximum {max} bytes)"
    );
    Ok(())
}
fn address(value: &str) -> Result<()> {
    ensure!(
        !value.is_empty()
            && value.len() <= 253
            && !value.starts_with('-')
            && value
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b':' | b'_')),
        "invalid route address (use an IP address or DNS name without brackets)"
    );
    Ok(())
}
fn username(value: &str) -> Result<()> {
    ensure!(
        !value.is_empty()
            && value.len() <= 128
            && value.as_bytes()[0].is_ascii_alphanumeric()
            && value.bytes().all(
                |b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'@' | b'\\' | b'-')
            ),
        "unsupported SSH username"
    );
    Ok(())
}
fn wire_string<'a>(wire: &mut &'a [u8]) -> Result<&'a [u8]> {
    ensure!(wire.len() >= 4, "truncated SSH public key");
    let len = u32::from_be_bytes(wire[..4].try_into()?) as usize;
    *wire = &wire[4..];
    ensure!(len <= wire.len(), "truncated SSH public key");
    let result = &wire[..len];
    *wire = &wire[len..];
    Ok(result)
}
fn public_key(value: &str) -> Result<String> {
    let value = value.trim();
    printable(value, "public key", 16384)?;
    let parts: Vec<_> = value.split_whitespace().collect();
    ensure!(
        parts.len() >= 2,
        "public key must have algorithm and base64 data"
    );
    let decoded = STANDARD
        .decode(parts[1])
        .context("invalid public key base64")?;
    let mut wire = decoded.as_slice();
    ensure!(
        wire_string(&mut wire)? == parts[0].as_bytes(),
        "public key algorithm disagrees with encoded key"
    );
    match parts[0] {
        "ssh-ed25519" => ensure!(
            wire_string(&mut wire)?.len() == 32,
            "invalid Ed25519 key length"
        ),
        "ssh-rsa" => {
            let exponent = wire_string(&mut wire)?;
            ensure!(
                !exponent.is_empty() && exponent.len() <= 8,
                "invalid RSA exponent"
            );
            ensure!(
                wire_string(&mut wire)?.len() >= 256,
                "RSA host keys must be at least 2048 bits"
            );
        }
        "ecdsa-sha2-nistp256" | "ecdsa-sha2-nistp384" | "ecdsa-sha2-nistp521" => {
            let curve = parts[0].strip_prefix("ecdsa-sha2-").unwrap();
            ensure!(
                wire_string(&mut wire)? == curve.as_bytes(),
                "invalid ECDSA curve"
            );
            let point = wire_string(&mut wire)?;
            let length = match curve {
                "nistp256" => 65,
                "nistp384" => 97,
                _ => 133,
            };
            ensure!(
                point.len() == length && point[0] == 4,
                "invalid ECDSA point"
            );
        }
        _ => bail!("unsupported SSH public key algorithm"),
    }
    ensure!(wire.is_empty(), "trailing bytes in public key");
    Ok(format!("{} {}", parts[0], parts[1]))
}
fn host_keys(keys: &[String]) -> Result<Vec<String>> {
    ensure!(
        !keys.is_empty() && keys.len() <= 16,
        "1-16 pinned host keys required"
    );
    keys.iter()
        .map(|key| public_key(key))
        .collect::<Result<BTreeSet<_>>>()
        .map(|set| set.into_iter().collect())
}
fn routes(routes: &mut [Route]) -> Result<()> {
    ensure!(
        !routes.is_empty() && routes.len() <= 16,
        "1-16 routes required"
    );
    let mut unique = BTreeSet::new();
    for route in routes.iter_mut() {
        ensure!(
            matches!(route.kind.as_str(), "lan" | "tailscale" | "public"),
            "unsupported route kind"
        );
        address(&route.address)?;
        ensure!(route.port > 0, "invalid SSH port");
        ensure!(
            unique.insert((route.address.to_lowercase(), route.port)),
            "duplicate route address/port"
        );
        if let Some(jump) = &mut route.proxy_jump {
            address(&jump.address)?;
            username(&jump.user)?;
            ensure!(jump.port > 0, "invalid jump port");
            jump.host_keys = host_keys(&jump.host_keys)?;
            printable(&jump.identity_file, "jump identityFile", 4096)?;
            ensure!(
                {
                    let path = jump.identity_file.as_bytes();
                    // This public metadata belongs to the Master, whose OS may
                    // differ from the Target validating the enrollment bundle.
                    jump.identity_file.starts_with('/')
                        || (path.len() > 3
                            && path[0].is_ascii_alphabetic()
                            && path[1] == b':'
                            && matches!(path[2], b'/' | b'\\'))
                },
                "jump identityFile must be absolute"
            );
            ensure!(
                !jump.identity_file.contains(['"', '%', '$', '\u{0060}']),
                "invalid jump identityFile"
            );
        }
    }
    // Explicit array order is fallback priority; enrollment UI defaults to LAN, Tailscale, public.
    Ok(())
}
pub(crate) fn validate_target(target: &mut Target, allow_legacy: bool) -> Result<()> {
    ensure!(
        target.schema_version == 2,
        "unsupported registration schema"
    );
    slug(&target.id, "target id")?;
    slug(&target.tenant, "tenant")?;
    printable(&target.label, "label", 256)?;
    username(&target.user)?;
    ensure!(
        matches!(target.os.as_str(), "windows" | "linux"),
        "os must be windows or linux"
    );
    ensure!(
        target.runner_installed || allow_legacy,
        "registration must come from an installed Target runner"
    );
    if target.runner_installed {
        Uuid::parse_str(&target.node_id).context("invalid Target nodeId")?;
    } else {
        ensure!(
            target.node_id.is_empty(),
            "a Target imported from a v1 registry may not attest a node identity"
        );
    }
    target.host_keys = host_keys(&target.host_keys)?;
    routes(&mut target.routes)?;
    printable(&target.bridge_path, "bridgePath", 4096)?;
    ensure!(
        !target
            .bridge_path
            .contains(['"', '\'', '%', '$', '\u{0060}', ';', '&', '|', '<', '>']),
        "unsupported bridgePath characters"
    );
    if target.os == "windows" {
        let bytes = target.bridge_path.as_bytes();
        ensure!(
            bytes.len() > 3
                && bytes[0].is_ascii_alphabetic()
                && bytes[1] == b':'
                && matches!(bytes[2], b'/' | b'\\'),
            "Windows bridgePath must be an absolute drive path"
        );
    } else {
        ensure!(
            target.bridge_path.starts_with('/'),
            "Linux bridgePath must be absolute"
        );
    }
    Ok(())
}
fn validate_master(master: &mut Master) -> Result<()> {
    Uuid::parse_str(&master.id).context("invalid Master id")?;
    printable(&master.label, "Master label", 256)?;
    master.public_key = public_key(&master.public_key)?;
    ensure!(
        master.public_key.starts_with("ssh-ed25519 "),
        "Master key must be Ed25519"
    );
    Ok(())
}
pub(crate) fn validate_bundle(bundle: &mut Bundle) -> Result<()> {
    ensure!(
        bundle.schema_version == 2,
        "unsupported enrollment bundle schema"
    );
    slug(&bundle.target_id, "targetId")?;
    slug(&bundle.tenant, "tenant")?;
    ensure!(
        matches!(bundle.os.as_str(), "windows" | "linux"),
        "os must be windows or linux"
    );
    username(&bundle.user)?;
    routes(&mut bundle.routes)?;
    validate_master(&mut bundle.master)
}
fn str_arg<'a>(params: &'a Value, key: &str) -> Result<&'a str> {
    params
        .get(key)
        .and_then(Value::as_str)
        .with_context(|| format!("missing string parameter: {key}"))
}
fn role(config: &Config, expected: &str) -> Result<()> {
    ensure!(
        config.roles.iter().any(|r| r == expected || r == "both"),
        "{expected} role is not enabled"
    );
    Ok(())
}
pub(crate) fn list_targets(db: &Connection) -> Result<Vec<Target>> {
    let mut stmt = db.prepare("SELECT document FROM targets ORDER BY id")?;
    let documents = stmt.query_map([], |row| row.get::<_, String>(0))?;
    documents
        .map(|row| serde_json::from_str(&row?).map_err(Into::into))
        .collect()
}
pub(crate) fn target(db: &Connection, id: &str) -> Result<Target> {
    let text = db
        .query_row("SELECT document FROM targets WHERE id=?1", [id], |row| {
            row.get::<_, String>(0)
        })
        .optional()?
        .with_context(|| format!("unknown target: {id}"))?;
    serde_json::from_str(&text).context("invalid stored target")
}
pub(crate) fn save_target(db: &Connection, target: &Target) -> Result<()> {
    if let Some(old) = db
        .query_row(
            "SELECT document FROM targets WHERE id=?1",
            [&target.id],
            |row| row.get::<_, String>(0),
        )
        .optional()?
    {
        let old: Target = serde_json::from_str(&old)?;
        ensure!(old.host_keys == target.host_keys, "HOST_KEY_MISMATCH: remove the old registration after independently verifying new host keys");
        ensure!(
            old.tenant == target.tenant,
            "target tenant cannot change; remove and re-enroll explicitly"
        );
        ensure!(
            !old.runner_installed || old.node_id == target.node_id,
            "TARGET_IDENTITY_MISMATCH: target node identity changed"
        );
        ensure!(
            !old.runner_installed || target.runner_installed,
            "an installed Target cannot be downgraded by v1 registry import"
        );
    } else {
        let count: usize = db.query_row("SELECT COUNT(*) FROM targets", [], |row| row.get(0))?;
        ensure!(
            count < MAX_TARGETS,
            "Master supports at most {MAX_TARGETS} targets"
        );
    }
    for existing in list_targets(db)? {
        if existing.id == target.id {
            continue;
        }
        ensure!(
            target.node_id.is_empty() || existing.node_id != target.node_id,
            "Target nodeId already registered as {}",
            existing.id
        );
        for route in &target.routes {
            ensure!(!existing.routes.iter().any(|r| r.address.eq_ignore_ascii_case(&route.address) && r.port == route.port),
                "route address/port already registered as {}", existing.id);
        }
    }
    db.execute("INSERT INTO targets(id,document) VALUES(?1,?2) ON CONFLICT(id) DO UPDATE SET document=excluded.document",
        params![target.id, serde_json::to_string(target)?])?;
    Ok(())
}
pub fn authorized_master(state: &Path, master_id: &str) -> Result<bool> {
    let db = database(state)?;
    Ok(db
        .query_row(
            "SELECT revoked=0 FROM masters WHERE id=?1",
            [master_id],
            |row| row.get(0),
        )
        .optional()?
        .unwrap_or(false))
}
/// OpenSSH-compatible fingerprint of public key wire bytes, never private material.
pub fn public_key_fingerprint(public: &str) -> Result<String> {
    use base64::engine::general_purpose::STANDARD_NO_PAD;
    use sha2::{Digest, Sha256};
    let canonical = public_key(public)?;
    let encoded = canonical
        .split_whitespace()
        .nth(1)
        .context("missing public key")?;
    let bytes = STANDARD.decode(encoded)?;
    Ok(format!(
        "SHA256:{}",
        STANDARD_NO_PAD.encode(Sha256::digest(bytes))
    ))
}
/// Bind authorization to the actual key in the SSH forced command, so stale
/// authorized_keys entries stay invalid when the same Master rotates a key.
pub fn authorized_master_key(state: &Path, master_id: &str, fingerprint: &str) -> Result<bool> {
    let db = database(state)?;
    let document = db
        .query_row(
            "SELECT document FROM masters WHERE id=?1 AND revoked=0",
            [master_id],
            |row| row.get::<_, String>(0),
        )
        .optional()?;
    let Some(document) = document else {
        return Ok(false);
    };
    let master: Master = serde_json::from_str(&document)?;
    Ok(public_key_fingerprint(&master.public_key)? == fingerprint)
}
pub fn register_master(state: &Path, master: &Value) -> Result<Value> {
    let mut master: Master =
        serde_json::from_value(master.clone()).context("invalid Master enrollment")?;
    validate_master(&mut master)?;
    let mut db = database(state)?;
    let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
    if let Some((document, revoked)) = tx
        .query_row(
            "SELECT document,revoked FROM masters WHERE id=?1",
            [&master.id],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, bool>(1)?)),
        )
        .optional()?
    {
        let old: Master = serde_json::from_str(&document)?;
        ensure!(
            revoked || old.public_key == master.public_key,
            "Master key changed; revoke the existing enrollment before replacing it"
        );
    }
    tx.execute("INSERT INTO masters(id,document,revoked) VALUES(?1,?2,0) ON CONFLICT(id) DO UPDATE SET document=excluded.document,revoked=0",
        params![master.id, serde_json::to_string(&master)?])?;
    tx.commit()?;
    let mut enrolled = serde_json::to_value(&master)?;
    enrolled["fingerprint"] = json!(public_key_fingerprint(&master.public_key)?);
    Ok(enrolled)
}
pub fn revoke_master(state: &Path, master_id: &str) -> Result<Value> {
    Uuid::parse_str(master_id).context("invalid Master id")?;
    let changed =
        database(state)?.execute("UPDATE masters SET revoked=1 WHERE id=?1", [master_id])?;
    ensure!(changed > 0, "unknown Master");
    Ok(json!({"id":master_id,"revoked":true}))
}
fn masters(state: &Path) -> Result<Value> {
    let db = database(state)?;
    let mut stmt = db.prepare("SELECT document,revoked FROM masters ORDER BY id")?;
    let rows = stmt.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, bool>(1)?))
    })?;
    let mut result = Vec::new();
    for row in rows {
        let (document, revoked) = row?;
        let mut value: Value = serde_json::from_str(&document)?;
        value["revoked"] = json!(revoked);
        result.push(value);
    }
    Ok(json!(result))
}
fn key_path(state: &Path, tenant: &str) -> PathBuf {
    state.join("keys").join(format!("{tenant}.ed25519"))
}
fn quiet_command(name: &str) -> Command {
    let mut command = Command::new(name);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    command
}
fn master_key(state: &Path, tenant: &str) -> Result<String> {
    slug(tenant, "tenant")?;
    let mut db = database(state)?;
    let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let folder = state.join("keys");
    fs::create_dir_all(&folder)?;
    crate::security::protect(&folder, true)?;
    let path = key_path(state, tenant);
    let pub_path = path.with_extension("ed25519.pub");
    if !path.exists() {
        ensure!(!pub_path.exists(), "public key exists without its private key; restore the key pair before exporting enrollment");
        let output = quiet_command("ssh-keygen")
            .args(["-q", "-t", "ed25519", "-N", "", "-C", "sshcc-service"])
            .arg("-f")
            .arg(&path)
            .output()
            .context("ssh-keygen is required to generate a Master key")?;
        ensure!(
            output.status.success(),
            "ssh-keygen failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    crate::security::protect(&path, false)?;
    ensure!(
        pub_path.is_file(),
        "Master public key is missing; restore the key pair"
    );
    crate::security::protect(&pub_path, false)?;
    let key = public_key(&fs::read_to_string(&pub_path)?)?;
    ensure!(
        key.starts_with("ssh-ed25519 "),
        "Master key must be Ed25519"
    );
    let old: Option<String> = tx
        .query_row(
            "SELECT public_key FROM tenant_keys WHERE tenant=?1",
            [tenant],
            |row| row.get(0),
        )
        .optional()?;
    ensure!(
        old.as_ref().is_none_or(|old| old == &key),
        "Master key changed unexpectedly"
    );
    tx.execute(
        "INSERT OR IGNORE INTO tenant_keys(tenant,public_key) VALUES(?1,?2)",
        params![tenant, key],
    )?;
    tx.commit()?;
    Ok(key)
}
fn default_bridge(os: &str) -> String {
    if os == "windows" {
        "C:/Program Files/SSH Control Center/sshcc.exe"
    } else {
        "/usr/bin/sshcc"
    }
    .into()
}
pub(crate) fn enrollment_export(state: &Path, config: &Config, p: &Value) -> Result<Value> {
    role(config, "master")?;
    let mut bundle = Bundle {
        schema_version: 2,
        target_id: str_arg(p, "targetId")?.into(),
        tenant: str_arg(p, "tenant")?.into(),
        os: str_arg(p, "os")?.into(),
        user: str_arg(p, "user")?.into(),
        routes: serde_json::from_value(p.get("routes").context("missing routes")?.clone())?,
        master: Master {
            id: config.node_id.clone(),
            label: config.label.clone(),
            public_key: String::new(),
        },
    };
    slug(&bundle.target_id, "targetId")?;
    slug(&bundle.tenant, "tenant")?;
    username(&bundle.user)?;
    routes(&mut bundle.routes)?;
    ensure!(
        matches!(bundle.os.as_str(), "windows" | "linux"),
        "os must be windows or linux"
    );
    bundle.master.public_key = master_key(state, &bundle.tenant)?;
    validate_bundle(&mut bundle)?;
    Ok(serde_json::to_value(bundle)?)
}
fn enrollment_registration(config: &Config, p: &Value) -> Result<Value> {
    role(config, "target")?;
    let os = str_arg(p, "os")?;
    let mut target = Target {
        schema_version: 2,
        id: str_arg(p, "targetId")?.into(),
        node_id: config.node_id.clone(),
        label: p
            .get("label")
            .and_then(Value::as_str)
            .unwrap_or(&config.label)
            .into(),
        tenant: str_arg(p, "tenant")?.into(),
        os: os.into(),
        user: str_arg(p, "user")?.into(),
        routes: serde_json::from_value(p.get("routes").context("missing routes")?.clone())?,
        host_keys: serde_json::from_value(
            p.get("hostKeys")
                .context("missing pinned hostKeys")?
                .clone(),
        )?,
        bridge_path: p
            .get("bridgePath")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .unwrap_or_else(|| default_bridge(os)),
        runner_installed: true,
    };
    validate_target(&mut target, false)?;
    Ok(serde_json::to_value(target)?)
}
fn legacy_target(value: &Value, proxy_jump: Option<&Value>) -> Result<Target> {
    ensure!(
        value.get("schemaVersion").and_then(Value::as_u64) == Some(1),
        "unsupported registry schema (expected schemaVersion 1)"
    );
    let id = str_arg(value, "id")?;
    let host = str_arg(value, "address")?;
    let port = value
        .get("port")
        .and_then(Value::as_u64)
        .context("v1 record port missing")?;
    ensure!(port > 0 && port <= 65535, "invalid v1 record port");
    let mode = str_arg(value, "mode")?;
    ensure!(
        matches!(mode, "company" | "customer"),
        "invalid v1 record mode (company or customer)"
    );
    ensure!(
        mode != "customer" || value.get("shareRequired").and_then(Value::as_bool) != Some(true),
        "a customer-mode v1 record may not set shareRequired"
    );
    let mut keys: Vec<String> = serde_json::from_value(
        value
            .get("hostKeys")
            .context("v1 record hostKeys missing")?
            .clone(),
    )?;
    keys.retain(|key| key.split_whitespace().next() != Some("ssh-dss"));
    let kind = if host.ends_with(".ts.net")
        || host
            .parse::<std::net::Ipv4Addr>()
            .is_ok_and(|ip| ip.octets()[0] == 100 && (64..=127).contains(&ip.octets()[1]))
    {
        "tailscale"
    } else if host.parse::<std::net::IpAddr>().is_ok_and(|ip| match ip {
        std::net::IpAddr::V4(ip) => ip.is_private() || ip.is_loopback() || ip.is_link_local(),
        std::net::IpAddr::V6(ip) => {
            ip.is_loopback() || ip.is_unique_local() || ip.is_unicast_link_local()
        }
    }) || !host.contains('.')
    {
        "lan"
    } else {
        "public"
    };
    let os = str_arg(value, "os")?;
    let jump = if mode == "company" {
        proxy_jump
            .map(|v| serde_json::from_value(v.clone()))
            .transpose()?
    } else {
        None
    };
    let mut target = Target {
        schema_version: 2,
        id: id.into(),
        node_id: String::new(),
        label: value
            .get("hostname")
            .and_then(Value::as_str)
            .unwrap_or(id)
            .into(),
        tenant: str_arg(value, "tenant")?.into(),
        os: os.into(),
        user: str_arg(value, "user")?.into(),
        routes: vec![Route {
            kind: kind.into(),
            address: host.into(),
            port: port as u16,
            proxy_jump: jump,
        }],
        host_keys: keys,
        bridge_path: default_bridge(os),
        runner_installed: false,
    };
    validate_target(&mut target, true)?;
    Ok(target)
}
fn import_legacy(state: &Path, p: &Value) -> Result<Value> {
    let doc = p.get("document").context("missing v1 registry document")?;
    let doc = doc.get("hosts").unwrap_or(doc);
    let values: Vec<&Value> = if doc.get("schemaVersion").is_some() {
        vec![doc]
    } else if let Some(array) = doc.as_array() {
        array.iter().collect()
    } else if let Some(map) = doc.as_object() {
        map.values().collect()
    } else {
        bail!("v1 registry document must be a registration, array, or hosts map");
    };
    ensure!(values.len() <= MAX_TARGETS, "too many v1 registry records");
    let targets = values
        .into_iter()
        .map(|v| legacy_target(v, p.get("proxyJump")))
        .collect::<Result<Vec<_>>>()?;
    let mut db = database(state)?;
    let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
    for target in &targets {
        save_target(&tx, target)?;
    }
    tx.commit()?;
    Ok(json!({"imported":targets.len(),"targets":targets,"runnerInstalled":false}))
}
fn dispatch(state: &Path, config: &Config, request: &Request) -> Result<Value> {
    let p = &request.params;
    let id = str_arg(p, "targetId")?;
    let db = database(state)?;
    let via = merge::resolve(&db, id)?;
    let mut remote = p
        .as_object()
        .context("request params must be an object")?
        .clone();
    remote.remove("targetId");
    let method = match request.method.as_str() {
        "jobs.start" => "runner.start",
        "jobs.list" => "runner.list",
        "jobs.status" => "runner.status",
        "jobs.logs" => "runner.logs",
        "jobs.cancel" => "runner.cancel",
        "files.list" => "fs.list",
        "files.read" => "fs.read",
        "files.search" => "fs.search",
        "files.grep" => "fs.grep",
        "files.write" => "fs.write",
        "files.mkdir" => "fs.mkdir",
        "files.remove" => "fs.remove",
        "terminal.open" => "pty.open",
        "terminal.read" => "pty.read",
        "terminal.write" => "pty.write",
        "terminal.status" => "pty.status",
        "terminal.resize" => "pty.resize",
        "terminal.close" => "pty.close",
        _ => bail!("unsupported fleet dispatch"),
    };
    if method == "runner.start" {
        let job_id = match remote.get("jobId") {
            Some(value) => value
                .as_str()
                .context("jobId must be UUID string")?
                .to_owned(),
            None => Uuid::new_v4().to_string(),
        };
        Uuid::parse_str(&job_id).context("jobId must be UUID")?;
        remote.insert("jobId".into(), json!(job_id));
        let document = serde_json::to_string(&remote)?;
        db.execute(
            "INSERT OR IGNORE INTO dispatches(id,target_id,payload) VALUES(?1,?2,?3)",
            params![job_id, id, document],
        )?;
        let old: (String, String) = db.query_row(
            "SELECT target_id,payload FROM dispatches WHERE id=?1",
            [&job_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        ensure!(
            old.0 == id && old.1 == document,
            "JOB_ID_CONFLICT: jobId already used with a different target or payload"
        );
        return merge::send(
            state,
            config,
            &via,
            &request.method,
            method,
            Value::Object(remote),
        )
        .with_context(|| {
            format!("jobId={job_id}; use this same ID to reconcile an uncertain dispatch")
        });
    }
    merge::send(
        state,
        config,
        &via,
        &request.method,
        method,
        Value::Object(remote),
    )
}
pub(crate) fn attach(state: &Path, config: &Config, params: &Value) -> Result<transport::Stream> {
    role(config, "master")?;
    let supplied = str_arg(params, "targetId")?;
    let db = database(state)?;
    if supplied.contains('/') {
        let (peer, child) = merge::relay_target(&db, supplied)?;
        let mut relayed = params.clone();
        relayed["targetId"] = json!(child);
        return transport::attach(state, config, &peer, "relay.attach", &relayed);
    }
    let target = if let Ok(target) = target(&db, supplied) {
        target
    } else {
        let matching: Vec<_> = list_targets(&db)?
            .into_iter()
            .filter(|t| t.label == supplied)
            .collect();
        ensure!(
            matching.len() == 1,
            "E_TARGET: target name must match exactly one Target; use Target ID"
        );
        matching.into_iter().next().unwrap()
    };
    ensure!(
        target.runner_installed,
        "E_RUNNER_NOT_INSTALLED: target has no bridge"
    );
    transport::attach(state, config, &target, "pty.attach", params)
}
/// None means the request belongs to the local runtime rather than the fleet.
pub fn handle(state: &Path, config: &Config, req: &Request) -> Result<Option<Value>> {
    let result = match req.method.as_str() {
        "enrollment.export" => enrollment_export(state, config, &req.params)?,
        "enrollment.accept" => {
            role(config, "target")?;
            let mut bundle: Bundle = serde_json::from_value(
                req.params.get("bundle").context("missing bundle")?.clone(),
            )?;
            validate_bundle(&mut bundle)?;
            register_master(state, &serde_json::to_value(bundle.master)?)?
        }
        "enrollment.registration" => enrollment_registration(config, &req.params)?,
        "masters.list" => masters(state)?,
        "masters.revoke" => revoke_master(state, str_arg(&req.params, "masterId")?)?,
        "targets.list" => {
            role(config, "master")?;
            let db = database(state)?;
            let direct = list_targets(&db)?;
            let mut rows = direct
                .iter()
                .map(serde_json::to_value)
                .collect::<serde_json::Result<Vec<_>>>()?;
            rows.extend(merge::merged_targets(&db, &direct)?);
            json!(rows)
        }
        "targets.import" | "targets.update" => {
            role(config, "master")?;
            let field = if req.method == "targets.import" {
                "registration"
            } else {
                "target"
            };
            let mut value: Target = serde_json::from_value(
                req.params
                    .get(field)
                    .context("missing registration/target")?
                    .clone(),
            )?;
            validate_target(&mut value, req.method == "targets.update")?;
            if value.runner_installed {
                ensure!(key_path(state,&value.tenant).is_file(),"export this Master's enrollment bundle for the tenant before importing its Target");
            }
            let mut db = database(state)?;
            let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
            if req.method == "targets.update" {
                target(&tx, &value.id)?;
            }
            save_target(&tx, &value)?;
            tx.commit()?;
            serde_json::to_value(value)?
        }
        "targets.import_legacy" => {
            role(config, "master")?;
            import_legacy(state, &req.params)?
        }
        "targets.remove" => {
            role(config, "master")?;
            let id = str_arg(&req.params, "targetId")?;
            let changed = database(state)?.execute("DELETE FROM targets WHERE id=?1", [id])?;
            ensure!(changed > 0, "unknown target");
            json!({"id":id,"removed":true})
        }
        "targets.doctor" => {
            role(config, "master")?;
            let id = str_arg(&req.params, "targetId")?;
            if id.contains('/') {
                merge::doctor(state, config, id, &req.params)?
            } else {
                let target = target(&database(state)?, id)?;
                transport::doctor(state, config, &target, &req.params)?
            }
        }
        "jobs.start" | "jobs.list" | "jobs.status" | "jobs.logs" | "jobs.cancel" | "files.list"
        | "files.read" | "files.search" | "files.grep" | "files.write" | "files.mkdir"
        | "files.remove" | "terminal.open"
        | "terminal.read" | "terminal.write" | "terminal.status" | "terminal.resize"
        | "terminal.close" => {
            role(config, "master")?;
            dispatch(state, config, req)?
        }
        _ => return merge::handle_local(state, config, req),
    };
    Ok(Some(result))
}
