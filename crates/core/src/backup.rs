//! Master backup and restore: one passphrase-encrypted file that moves a Master to a fresh install.
//! The restored machine becomes the same Master (node ID, label, roles, tenant keys and fleet), so Targets
//! accept it unchanged. The Owner and execution mode stay as this machine's installer set them.
//! Owner-only and local-only: the service never routes these methods through a bridge or relay.
use crate::{config::{self, Config}, protocol::Request, security};
use anyhow::{bail, ensure, Context, Result};
use argon2::{Algorithm, Argon2, Params, Version};
use base64::{engine::general_purpose::STANDARD, Engine};
use chacha20poly1305::{
    aead::{rand_core::RngCore, Aead, KeyInit, OsRng, Payload as Sealed},
    XChaCha20Poly1305, XNonce,
};
use rusqlite::{types::Value as Sql, Connection, TransactionBehavior};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

/// The payload format this Master writes. Older files are migrated on restore; newer ones are refused.
/// 1: first format. 2: adds Tags (roadmap entry 4).
pub const FORMAT_VERSION: u16 = 4;
pub const MIN_PASSPHRASE: usize = 12;
const MAGIC: &[u8; 8] = b"SSHCCBAK";
const HEADER: usize = 8 + 2 + 12 + 16 + 24;
const MAX_FILE: usize = 4 * 1024 * 1024;
/// Argon2id cost for new files; files carry their own, bounded on read.
const KDF: (u32, u32, u32) = (64 * 1024, 3, 1);
/// Fleet tables that travel, with their columns. Jobs, terminals, health, dispatches and pairing
/// requests stay behind.
const TABLES: &[(&str, &[&str])] = &[
    ("targets", &["id", "document"]),
    ("masters", &["id", "document", "revoked"]),
    ("tenant_keys", &["tenant", "public_key"]),
    ("relays", &["master_id", "label", "joined_at", "last_used_at", "revoked", "role", "tags", "pass_through"]),
    ("merges", &["target_id", "document"]),
    ("folders", &["id", "master", "name", "created_at"]),
    ("folder_members", &["target_id", "folder_id"]),
    ("datacenters", &["id", "name", "created_at"]),
    ("datacenter_members", &["master", "datacenter_id"]),
    ("tags", &["target_id", "key", "tag"]),
    ("alarm_rules", &["id", "document", "created_at"]),
    ("alarm_settings", &["key", "value"]),
];

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Contents {
    created_at: String,
    node_id: String,
    label: String,
    roles: Vec<String>,
    os: String,
    data_dir: String,
    /// File name inside the keys directory → base64 bytes.
    keys: BTreeMap<String, String>,
    tables: BTreeMap<String, Vec<Map<String, Value>>>,
}

/// Last backup and restore times, kept beside the fleet and never part of a backup.
#[derive(Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Record {
    last_backup_at: Option<String>,
    restored_at: Option<String>,
    #[serde(default)]
    keys_changed_at: Option<String>,
}

fn now() -> String {
    chrono::Utc::now().to_rfc3339()
}
fn record_path(state: &Path) -> PathBuf {
    state.join("backup.json")
}
fn read_record(state: &Path) -> Result<Record> {
    match fs::read(record_path(state)) {
        Ok(bytes) => Ok(serde_json::from_slice(&bytes)?),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Record::default()),
        Err(error) => Err(error.into()),
    }
}
fn write_record(state: &Path, record: &Record) -> Result<()> {
    let path = record_path(state);
    let tmp = state.join(format!(".backup-{}.tmp", uuid::Uuid::new_v4()));
    fs::write(&tmp, serde_json::to_vec_pretty(record)?)?;
    security::protect(&tmp, false)?;
    security::replace(&tmp, &path)?;
    Ok(())
}

fn passphrase(p: &Value) -> Result<&str> {
    let value = p.get("passphrase").and_then(Value::as_str).context("E_ARGUMENT: missing passphrase")?;
    ensure!(
        value.chars().count() >= MIN_PASSPHRASE,
        "E_BACKUP_PASSPHRASE_SHORT: the passphrase must be at least {MIN_PASSPHRASE} characters"
    );
    Ok(value)
}
fn file(p: &Value) -> Result<Vec<u8>> {
    let text = p.get("file").and_then(Value::as_str).context("E_ARGUMENT: missing file")?;
    ensure!(text.len() <= MAX_FILE * 4 / 3 + 4, "E_BACKUP_NOT_A_BACKUP: the file is too large to be a backup");
    STANDARD.decode(text).context("E_ARGUMENT: file must be base64")
}
fn derive(passphrase: &str, salt: &[u8], (memory, passes, lanes): (u32, u32, u32)) -> Result<[u8; 32]> {
    let params = Params::new(memory, passes, lanes, Some(32)).map_err(|e| anyhow::anyhow!("E_BACKUP_NOT_A_BACKUP: {e}"))?;
    let mut key = [0u8; 32];
    Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
        .hash_password_into(passphrase.as_bytes(), salt, &mut key)
        .map_err(|e| anyhow::anyhow!("E_BACKUP_NOT_A_BACKUP: {e}"))?;
    Ok(key)
}

fn seal(contents: &Contents, passphrase: &str, version: u16) -> Result<Vec<u8>> {
    let mut salt = [0u8; 16];
    let mut nonce = [0u8; 24];
    OsRng.fill_bytes(&mut salt);
    OsRng.fill_bytes(&mut nonce);
    let mut header = Vec::with_capacity(HEADER);
    header.extend_from_slice(MAGIC);
    header.extend_from_slice(&version.to_be_bytes());
    for value in [KDF.0, KDF.1, KDF.2] {
        header.extend_from_slice(&value.to_be_bytes());
    }
    header.extend_from_slice(&salt);
    header.extend_from_slice(&nonce);
    let key = derive(passphrase, &salt, KDF)?;
    let body = serde_json::to_vec(contents)?;
    let sealed = XChaCha20Poly1305::new(&key.into())
        .encrypt(XNonce::from_slice(&nonce), Sealed { msg: &body, aad: &header })
        .map_err(|_| anyhow::anyhow!("E_BACKUP: encryption failed"))?;
    header.extend_from_slice(&sealed);
    Ok(header)
}

/// The header is read before the passphrase is tried, so "not a backup" and "newer" are told apart from
/// "wrong passphrase or damaged" (the AEAD cannot tell those two apart, by design).
fn open(bytes: &[u8], passphrase: &str) -> Result<(u16, Contents)> {
    ensure!(
        bytes.len() > HEADER && bytes.len() <= MAX_FILE && &bytes[..8] == MAGIC,
        "E_BACKUP_NOT_A_BACKUP: this file is not an SSH Control Center backup"
    );
    let version = u16::from_be_bytes([bytes[8], bytes[9]]);
    ensure!(version >= 1, "E_BACKUP_NOT_A_BACKUP: this file is not an SSH Control Center backup");
    ensure!(
        version <= FORMAT_VERSION,
        "E_BACKUP_NEWER: this backup comes from a newer Master (format {version}); update this Master first"
    );
    let number = |at: usize| u32::from_be_bytes(bytes[at..at + 4].try_into().unwrap());
    let kdf = (number(10), number(14), number(18));
    ensure!(
        (8 * 1024..=1024 * 1024).contains(&kdf.0) && (1..=16).contains(&kdf.1) && (1..=16).contains(&kdf.2),
        "E_BACKUP_NOT_A_BACKUP: this file is not an SSH Control Center backup"
    );
    let (salt, nonce) = (&bytes[22..38], &bytes[38..HEADER]);
    let key = derive(passphrase, salt, kdf)?;
    let body = XChaCha20Poly1305::new(&key.into())
        .decrypt(XNonce::from_slice(nonce), Sealed { msg: &bytes[HEADER..], aad: &bytes[..HEADER] })
        .map_err(|_| anyhow::anyhow!("E_BACKUP_BAD_PASSPHRASE_OR_DAMAGED: the passphrase is wrong or the file is damaged"))?;
    let contents = migrate(version, serde_json::from_slice(&body).context("E_BACKUP_BAD_PASSPHRASE_OR_DAMAGED: the backup contents are damaged")?)?;
    Ok((version, contents))
}

/// Brings an older payload up to the current format. Format 1 has no Tags table, which restores as no Tags. Before
/// format 3 a merge had no Merge Role: it restores as Full control, all hosts, pass-through on, as on an update. Before
/// format 4 there were no alarm rules: the tables restore empty, and the next use seeds the three default rules.
fn migrate(version: u16, mut payload: Value) -> Result<Contents> {
    if version < 3 {
        let relays = payload.get_mut("tables").and_then(|t| t.get_mut("relays")).and_then(Value::as_array_mut);
        for row in relays.into_iter().flatten() {
            row["role"] = json!("full");
            row["pass_through"] = json!(1);
        }
    }
    serde_json::from_value(payload).context("E_BACKUP_BAD_PASSPHRASE_OR_DAMAGED: the backup contents are damaged")
}

fn sql_to_json(value: Sql) -> Result<Value> {
    Ok(match value {
        Sql::Null => Value::Null,
        Sql::Integer(n) => json!(n),
        Sql::Text(text) => json!(text),
        Sql::Real(_) | Sql::Blob(_) => bail!("E_BACKUP: unexpected column type"),
    })
}
fn json_to_sql(value: &Value) -> Result<Sql> {
    Ok(match value {
        Value::Null => Sql::Null,
        Value::Number(n) => Sql::Integer(n.as_i64().context("E_BACKUP_BAD_PASSPHRASE_OR_DAMAGED: invalid number")?),
        Value::String(text) => Sql::Text(text.clone()),
        Value::Bool(flag) => Sql::Integer(i64::from(*flag)),
        _ => bail!("E_BACKUP_BAD_PASSPHRASE_OR_DAMAGED: invalid column value"),
    })
}
fn dump(db: &Connection) -> Result<BTreeMap<String, Vec<Map<String, Value>>>> {
    let mut tables = BTreeMap::new();
    for (table, columns) in TABLES {
        let mut stmt = db.prepare(&format!("SELECT {} FROM {table} ORDER BY rowid", columns.join(",")))?;
        let mut rows = stmt.query([])?;
        let mut out = Vec::new();
        while let Some(row) = rows.next()? {
            let mut object = Map::new();
            for (i, column) in columns.iter().enumerate() {
                object.insert((*column).into(), sql_to_json(row.get(i)?)?);
            }
            out.push(object);
        }
        tables.insert((*table).into(), out);
    }
    Ok(tables)
}
fn count(db: &Connection, table: &str) -> Result<i64> {
    Ok(db.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| row.get(0))?)
}
/// Empty: no hosts, Merges or folders. Allowed Masters, relays and keys from a fresh install are replaced.
fn is_empty(db: &Connection) -> Result<bool> {
    Ok(count(db, "targets")? + count(db, "merges")? + count(db, "folders")? == 0)
}
fn key_name(name: &str) -> Result<()> {
    ensure!(
        !name.is_empty() && name.len() <= 128 && !name.starts_with('.')
            && name.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.')),
        "E_BACKUP_BAD_PASSPHRASE_OR_DAMAGED: invalid key file name"
    );
    Ok(())
}

/// The key file name when `path` lies in the old Master's keys directory, so the same file can be pointed at under
/// this machine's data directory. Windows paths compare without case and with either slash.
fn old_key_name(path: &str, old_dir: &str, old_os: &str) -> Option<String> {
    let windows = old_os == "windows";
    let plain = |value: &str| {
        let value = value.strip_prefix(r"\\?\").unwrap_or(value).replace('\\', "/");
        if windows { value.to_ascii_lowercase() } else { value }
    };
    let prefix = format!("{}/keys/", plain(old_dir).trim_end_matches('/'));
    let candidate = plain(path);
    let rest = candidate.strip_prefix(&prefix)?;
    // The name keeps its own case; only the comparison ignores it.
    let original = path.replace('\\', "/");
    let name = &original[original.len() - rest.len()..];
    key_name(name).ok().map(|()| name.to_owned())
}
/// Rewrites route identity files for this machine and lists the ones outside the old keys directory.
fn adapt_targets(rows: &mut [Map<String, Value>], contents: &Contents, keys: &Path) -> Result<Vec<Value>> {
    let mut check = Vec::new();
    for row in rows {
        let Some(Value::String(text)) = row.get("document") else { continue };
        let mut target: Value = serde_json::from_str(text).context("E_BACKUP_BAD_PASSPHRASE_OR_DAMAGED: invalid host record")?;
        let mut changed = false;
        let (id, label) = (target["id"].clone(), target["label"].clone());
        for route in target["routes"].as_array_mut().into_iter().flatten() {
            let Some(path) = route["proxyJump"]["identityFile"].as_str().map(str::to_owned) else { continue };
            match old_key_name(&path, &contents.data_dir, &contents.os) {
                Some(name) => {
                    route["proxyJump"]["identityFile"] = json!(keys.join(name).to_string_lossy());
                    changed = true;
                }
                None => check.push(json!({"targetId":id,"label":label,"path":path})),
            }
        }
        if changed {
            row.insert("document".into(), json!(target.to_string()));
        }
    }
    Ok(check)
}
/// Two-way Merges need OpenSSH Server and the sshcc-bridge account, which a fresh machine does not have, so they
/// come back one-way with Enable two-way offered again.
fn adapt_merges(rows: &mut [Map<String, Value>]) -> Result<Vec<Value>> {
    let mut again = Vec::new();
    for row in rows {
        let Some(Value::String(text)) = row.get("document") else { continue };
        let mut merge: Value = serde_json::from_str(text).context("E_BACKUP_BAD_PASSPHRASE_OR_DAMAGED: invalid Merge record")?;
        if merge["reverse"].is_null() {
            continue;
        }
        if merge["reverse"]["status"] == "two-way" {
            again.push(json!({"targetId":merge["targetId"],"label":merge["label"]}));
        }
        merge["reverse"] = Value::Null;
        row.insert("document".into(), json!(merge.to_string()));
    }
    Ok(again)
}
/// Other Masters that reach this one and still use its old address: allowed directly, or merging it.
fn reached_by(tables: &BTreeMap<String, Vec<Map<String, Value>>>) -> Vec<Value> {
    let mut found: BTreeMap<String, (String, Vec<&str>)> = BTreeMap::new();
    let live = |row: &Map<String, Value>| !matches!(row.get("revoked"), Some(Value::Number(n)) if n.as_i64() != Some(0));
    for row in tables.get("masters").into_iter().flatten().filter(|row| live(row)) {
        let master: Value = row.get("document").and_then(Value::as_str).and_then(|d| serde_json::from_str(d).ok()).unwrap_or_default();
        let (Some(id), label) = (master["id"].as_str(), master["label"].as_str().unwrap_or_default()) else { continue };
        found.entry(id.into()).or_insert_with(|| (label.into(), Vec::new())).1.push("direct");
    }
    for row in tables.get("relays").into_iter().flatten().filter(|row| live(row)) {
        let (Some(id), label) = (row.get("master_id").and_then(Value::as_str), row.get("label").and_then(Value::as_str).unwrap_or_default()) else { continue };
        found.entry(id.into()).or_insert_with(|| (label.into(), Vec::new())).1.push("merge");
    }
    found.into_iter().map(|(id, (label, ways))| json!({"masterId":id,"label":label,"ways":ways})).collect()
}

fn role(config: &Config) -> Result<()> {
    ensure!(config.roles.iter().any(|r| r == "master"), "E_ROLE: master role is required");
    Ok(())
}

pub fn create(state: &Path, config: &Config, passphrase: &str) -> Result<Vec<u8>> {
    role(config)?;
    let db = crate::alarms::database(state)?;
    let mut keys = BTreeMap::new();
    let folder = state.join("keys");
    if folder.is_dir() {
        for entry in fs::read_dir(&folder)? {
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().into_owned();
            if entry.file_type()?.is_file() && key_name(&name).is_ok() {
                keys.insert(name, STANDARD.encode(fs::read(entry.path())?));
            }
        }
    }
    let contents = Contents {
        created_at: now(),
        node_id: config.node_id.clone(),
        label: config.label.clone(),
        roles: config.roles.clone(),
        os: std::env::consts::OS.into(),
        data_dir: state.to_string_lossy().into_owned(),
        keys,
        tables: dump(&db)?,
    };
    let bytes = seal(&contents, passphrase, FORMAT_VERSION)?;
    let mut record = read_record(state)?;
    record.last_backup_at = Some(contents.created_at);
    write_record(state, &record)?;
    Ok(bytes)
}

fn summary(version: u16, contents: &Contents) -> Value {
    let rows = |table: &str| contents.tables.get(table).map_or(0, Vec::len);
    json!({"formatVersion":version,"createdAt":contents.created_at,"nodeId":contents.node_id,"label":contents.label,
        "os":contents.os,"hosts":rows("targets"),"merges":rows("merges"),"folders":rows("folders")})
}

pub fn inspect(bytes: &[u8], passphrase: &str) -> Result<Value> {
    let (version, contents) = open(bytes, passphrase)?;
    Ok(summary(version, &contents))
}

/// Writes everything or nothing: the fleet rows go in one transaction, the key files are staged first and
/// put back if anything later fails, and config.json is replaced last.
pub fn restore(state: &Path, config: &Config, bytes: &[u8], passphrase: &str) -> Result<Value> {
    role(config)?;
    let mut db = crate::alarms::database(state)?;
    ensure!(
        is_empty(&db)?,
        "E_RESTORE_NOT_EMPTY: this Master already has hosts; restore only works on a fresh install"
    );
    let (version, mut contents) = open(bytes, passphrase)?;
    for name in contents.keys.keys() {
        key_name(name)?;
    }
    let reached = reached_by(&contents.tables);
    let mut targets = contents.tables.remove("targets").unwrap_or_default();
    let check_paths = adapt_targets(&mut targets, &contents, &state.join("keys"))?;
    contents.tables.insert("targets".into(), targets);
    let mut merges = contents.tables.remove("merges").unwrap_or_default();
    let two_way = adapt_merges(&mut merges)?;
    contents.tables.insert("merges".into(), merges);
    let keys = contents
        .keys
        .iter()
        .map(|(name, data)| Ok((name.clone(), STANDARD.decode(data).context("E_BACKUP_BAD_PASSPHRASE_OR_DAMAGED: invalid key data")?)))
        .collect::<Result<Vec<_>>>()?;

    let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
    for (table, columns) in TABLES {
        tx.execute(&format!("DELETE FROM {table}"), [])?;
        let insert = format!(
            "INSERT INTO {table}({}) VALUES({})",
            columns.join(","),
            (1..=columns.len()).map(|i| format!("?{i}")).collect::<Vec<_>>().join(",")
        );
        for row in contents.tables.get(*table).into_iter().flatten() {
            let values = columns
                .iter()
                .map(|column| json_to_sql(row.get(*column).unwrap_or(&Value::Null)))
                .collect::<Result<Vec<_>>>()?;
            tx.execute(&insert, rusqlite::params_from_iter(values))?;
        }
    }

    let folder = state.join("keys");
    fs::create_dir_all(&folder)?;
    security::protect(&folder, true)?;
    let stage = folder.join(format!(".restore-{}", uuid::Uuid::new_v4()));
    fs::create_dir(&stage)?;
    security::protect(&stage, true)?;
    let previous_config = fs::read(state.join("config.json"))?;
    let mut moved: Vec<(PathBuf, Option<PathBuf>)> = Vec::new();
    let result = (|| -> Result<()> {
        for (name, data) in &keys {
            let staged = stage.join(name);
            fs::write(&staged, data)?;
            security::protect(&staged, false)?;
        }
        for (name, _) in &keys {
            let path = folder.join(name);
            let kept = if path.exists() {
                let old = stage.join(format!("{name}.previous"));
                fs::rename(&path, &old)?;
                Some(old)
            } else {
                None
            };
            moved.push((path.clone(), kept));
            fs::rename(stage.join(name), &path)?;
        }
        config::adopt(state, &contents.node_id, &contents.label, &contents.roles)?;
        Ok(())
    })()
    .and_then(|()| tx.commit().map_err(Into::into));
    if let Err(error) = result {
        for (path, kept) in moved.iter().rev() {
            let _ = fs::remove_file(path);
            if let Some(old) = kept {
                let _ = fs::rename(old, path);
            }
        }
        let _ = fs::write(state.join("config.json"), &previous_config);
        let _ = fs::remove_dir_all(&stage);
        return Err(error);
    }
    let _ = fs::remove_dir_all(&stage);
    write_record(state, &Record { last_backup_at: Some(contents.created_at.clone()), restored_at: Some(now()), keys_changed_at: None })?;
    let mut result = summary(version, &contents);
    result["restartRequired"] = json!(true);
    result["reachedBy"] = json!(reached);
    result["twoWayAgain"] = json!(two_way);
    result["checkPaths"] = json!(check_paths);
    Ok(result)
}

/// Called when a change of keys finishes: backups made before it no longer control any host.
pub(crate) fn keys_changed(state: &Path, when: &str) -> Result<()> {
    let mut record = read_record(state)?;
    record.keys_changed_at = Some(when.into());
    write_record(state, &record)
}

pub fn status(state: &Path, owner: bool) -> Result<Value> {
    let record = read_record(state)?;
    let db = crate::alarms::database(state)?;
    // RFC 3339 times in UTC compare as text.
    let stale = matches!((&record.keys_changed_at, &record.last_backup_at), (Some(changed), Some(backup)) if backup < changed)
        || (record.keys_changed_at.is_some() && record.last_backup_at.is_none());
    Ok(json!({"lastBackupAt":record.last_backup_at,"restoredAt":record.restored_at,"keysChangedAt":record.keys_changed_at,
        "backupBeforeKeys":stale,"empty":is_empty(&db)?,"owner":owner}))
}

/// Service entry point; `owner` is true when the local caller is this Master's Owner (or the system).
pub fn handle(state: &Path, config: &Config, req: &Request, owner: bool) -> Result<Option<Value>> {
    if !req.method.starts_with("backup.") {
        return Ok(None);
    }
    let p = &req.params;
    if req.method == "backup.status" {
        return status(state, owner).map(Some);
    }
    ensure!(owner, "E_BACKUP_NOT_OWNER: only this Master's Owner can back up or restore it");
    Ok(Some(match req.method.as_str() {
        "backup.create" => json!({"file": STANDARD.encode(create(state, config, passphrase(p)?)?)}),
        "backup.inspect" => inspect(&file(p)?, required(p)?)?,
        "backup.restore" => restore(state, config, &file(p)?, required(p)?)?,
        _ => bail!("E_ARGUMENT: unknown backup method"),
    }))
}
/// Opening a file takes whatever passphrase was set; the length rule is for new backups only.
fn required(p: &Value) -> Result<&str> {
    p.get("passphrase").and_then(Value::as_str).context("E_ARGUMENT: missing passphrase")
}

#[cfg(test)]
mod tests {
    use super::{old_key_name, restore, seal, Contents};
    use crate::{config, security};

    /// A format 1 file (from before Tags) still restores, with no Tags.
    #[test]
    fn a_format_one_file_restores_without_tags() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = config::init(dir.path(), "master", &security::current_principal().unwrap(), "New", &[]).unwrap();
        let node = uuid::Uuid::new_v4().to_string();
        let tables = serde_json::from_value(serde_json::json!({"folders":[{"id":"f1","master":"local","name":"Web","created_at":"2026-01-01T00:00:00Z"}]})).unwrap();
        let old = Contents { created_at: "2026-01-01T00:00:00Z".into(), node_id: node.clone(), label: "Old".into(), roles: vec!["master".into()],
            os: "linux".into(), data_dir: "/var/lib/sshcc".into(), keys: Default::default(), tables };
        let file = seal(&old, "a long passphrase", 1).unwrap();
        let result = restore(dir.path(), &cfg, &file, "a long passphrase").unwrap();
        assert_eq!((result["formatVersion"].as_u64(), result["folders"].as_u64()), (Some(1), Some(1)));
        assert_eq!(config::load(dir.path()).unwrap().node_id, node);
        assert_eq!(crate::tags::list(dir.path()).unwrap()["tags"], serde_json::json!([]));
    }
    /// A format 2 file (from before Merge Roles) restores its merges as Full control, all hosts, pass-through on.
    #[test]
    fn a_format_two_file_restores_merges_as_full_control() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = config::init(dir.path(), "master", &security::current_principal().unwrap(), "New", &[]).unwrap();
        let peer = uuid::Uuid::new_v4().to_string();
        let tables = serde_json::from_value(serde_json::json!({"relays":[{"master_id":peer,"label":"Peer","joined_at":"2026-01-01T00:00:00Z","last_used_at":null,"revoked":0}]})).unwrap();
        let old = Contents { created_at: "2026-01-01T00:00:00Z".into(), node_id: uuid::Uuid::new_v4().to_string(), label: "Old".into(), roles: vec!["master".into()],
            os: "linux".into(), data_dir: "/var/lib/sshcc".into(), keys: Default::default(), tables };
        let file = seal(&old, "a long passphrase", 2).unwrap();
        restore(dir.path(), &cfg, &file, "a long passphrase").unwrap();
        let state = crate::fleet::handle(dir.path(), &cfg, &crate::protocol::Request { method: "relays.list".into(), params: serde_json::json!({}) }).unwrap().unwrap();
        assert_eq!((state[0]["role"].as_str(), state[0]["passThrough"].as_bool(), state[0]["tags"].is_null()), (Some("full"), Some(true), true));
    }
    /// A format 3 file (from before alarms) restores with the three default rules; a format 4 one keeps its own.
    #[test]
    fn alarm_rules_travel_from_format_four_and_older_files_get_the_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = config::init(dir.path(), "master", &security::current_principal().unwrap(), "New", &[]).unwrap();
        let rule = serde_json::json!({"id":"r1","scope":{"tag":"web"},"condition":"disk","threshold":80,"containers":[],"level":"yellow"});
        let tables = serde_json::from_value(serde_json::json!({"alarm_rules":[{"id":"r1","document":rule.to_string(),"created_at":"2026-01-01T00:00:00Z"}],
            "alarm_settings":[{"key":"seeded","value":"1"},{"key":"interval","value":"15"}]})).unwrap();
        let contents = |tables| Contents { created_at: "2026-01-01T00:00:00Z".into(), node_id: uuid::Uuid::new_v4().to_string(), label: "Old".into(),
            roles: vec!["master".into()], os: "linux".into(), data_dir: "/var/lib/sshcc".into(), keys: Default::default(), tables };
        restore(dir.path(), &cfg, &seal(&contents(tables), "a long passphrase", 4).unwrap(), "a long passphrase").unwrap();
        assert_eq!(crate::alarms::rules(dir.path()).unwrap(), vec![rule]);
        assert_eq!(crate::alarms::interval(&crate::alarms::database(dir.path()).unwrap()).unwrap(), 15);
        let older = tempfile::tempdir().unwrap();
        let cfg = config::init(older.path(), "master", &security::current_principal().unwrap(), "New", &[]).unwrap();
        restore(older.path(), &cfg, &seal(&contents(Default::default()), "a long passphrase", 3).unwrap(), "a long passphrase").unwrap();
        assert_eq!(crate::alarms::rules(older.path()).unwrap().len(), 3);
    }
    #[test]
    fn jump_keys_move_across_windows_and_ubuntu() {
        let win = r"C:\ProgramData\SSHControlCenter";
        assert_eq!(old_key_name(r"C:\ProgramData\SSHControlCenter\keys\Jump.ed25519", win, "windows").as_deref(), Some("Jump.ed25519"));
        assert_eq!(old_key_name("c:/programdata/sshcontrolcenter/keys/jump.ed25519", win, "windows").as_deref(), Some("jump.ed25519"));
        assert_eq!(old_key_name(r"\\?\C:\ProgramData\SSHControlCenter\keys\jump.ed25519", win, "windows").as_deref(), Some("jump.ed25519"));
        assert_eq!(old_key_name("/var/lib/sshcc/keys/jump.ed25519", "/var/lib/sshcc", "linux").as_deref(), Some("jump.ed25519"));
        assert_eq!(old_key_name("/var/lib/sshcc/keys/jump.ed25519", "/var/lib/sshcc/", "linux").as_deref(), Some("jump.ed25519"));
        // Case matters on Ubuntu; nested folders and other places are not the old keys directory.
        assert_eq!(old_key_name("/var/lib/SSHCC/keys/jump.ed25519", "/var/lib/sshcc", "linux"), None);
        assert_eq!(old_key_name("/var/lib/sshcc/keys/sub/jump.ed25519", "/var/lib/sshcc", "linux"), None);
        assert_eq!(old_key_name("/opt/keys/jump.ed25519", "/var/lib/sshcc", "linux"), None);
        assert_eq!(old_key_name(r"D:\keys\jump.ed25519", win, "windows"), None);
    }
}
