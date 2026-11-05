//! Human approved pairing carries only public enrollment and registration data.
//! Checksums detect damaged copy/paste; the operator authenticates the handoff.
use crate::{
    config::Config,
    fleet::{self, Bundle, Route, Target},
    protocol::Request,
};
use anyhow::{bail, ensure, Context, Result};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use chrono::Utc;
use rusqlite::{params, Connection, OpenFlags, OptionalExtension, TransactionBehavior};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{fs, io::Read, path::Path};
use uuid::Uuid;

pub const MAX_CODE_BYTES: usize = 64 * 1024;
const CODE_PREFIX: &str = "SSHCC1";
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Envelope {
    pub schema_version: u32,
    pub request_id: String,
    pub master_id: String,
    pub bundle_digest: String,
    pub target_label: String,
    pub bundle: Value,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Receipt {
    pub schema_version: u32,
    pub request_id: String,
    pub master_id: String,
    pub bundle_digest: String,
    pub registration: Target,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Pending {
    envelope: Envelope,
    state: String,
    created_at: String,
    updated_at: String,
    #[serde(default)]
    receipt_digest: Option<String>,
    #[serde(default)]
    diagnostics: Option<Value>,
    /// The other request of a pair made for an unknown OS (Windows + Ubuntu). Absent on older
    /// requests and on single ones. Not written when absent, so an older Master can still read
    /// single requests after a downgrade.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    sibling: Option<String>,
    /// The pairing code the Master fetched from the new machine itself (entry 8), waiting for the person to compare
    /// the 6 digits. Not written when absent, like `sibling`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    fetched: Option<String>,
}
fn digest(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}
fn bundle(envelope: &Envelope) -> Result<Bundle> {
    let mut value: Bundle = serde_json::from_value(envelope.bundle.clone())
        .context("invalid public enrollment bundle")?;
    fleet::validate_bundle(&mut value)?;
    Ok(value)
}
fn canonical(value: &impl Serialize) -> Result<Vec<u8>> {
    // Value uses a sorted map, making the hash independent of caller JSON key order.
    Ok(serde_json::to_vec(&serde_json::to_value(value)?)?)
}
fn validate_envelope(envelope: &Envelope) -> Result<Bundle> {
    ensure!(
        envelope.schema_version == 1,
        "E_PAIRING_VERSION: unsupported setup request"
    );
    Uuid::parse_str(&envelope.request_id).context("E_PAIRING_REQUEST: invalid request ID")?;
    Uuid::parse_str(&envelope.master_id).context("E_PAIRING_MASTER: invalid Master ID")?;
    valid_label(&envelope.target_label)?;
    let value = bundle(envelope)?;
    ensure!(
        envelope.master_id == value.master.id,
        "E_PAIRING_MASTER: setup request belongs to another Master"
    );
    ensure!(
        envelope.bundle_digest == digest(&canonical(&value)?),
        "E_PAIRING_DIGEST: setup request is damaged"
    );
    Ok(value)
}
/// Offline inspection deliberately loads no service state and authorizes no Master.
pub fn inspect(value: &Value) -> Result<Value> {
    ensure!(
        canonical(value)?.len() <= MAX_CODE_BYTES,
        "E_PAIRING_SIZE: setup request exceeds 64 KiB"
    );
    let envelope: Envelope =
        serde_json::from_value(value.clone()).context("invalid setup request envelope")?;
    let bundle = validate_envelope(&envelope)?;
    Ok(
        json!({"envelope":envelope,"bundle":bundle,"masterFingerprint":fleet::public_key_fingerprint(&bundle.master.public_key)?}),
    )
}
pub fn load_envelope(path: &Path) -> Result<Value> {
    let mut bytes = Vec::new();
    fs::File::open(path)?
        .take((MAX_CODE_BYTES + 1) as u64)
        .read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() <= MAX_CODE_BYTES,
        "E_PAIRING_SIZE: setup request exceeds 64 KiB"
    );
    let value = serde_json::from_slice(&bytes)?;
    inspect(&value)?;
    Ok(value)
}
fn validate_binding(envelope: &Envelope, receipt: &Receipt) -> Result<()> {
    let requested = validate_envelope(envelope)?;
    ensure!(
        receipt.schema_version == 1,
        "E_PAIRING_VERSION: unsupported pairing code"
    );
    ensure!(
        receipt.request_id == envelope.request_id,
        "E_PAIRING_REQUEST: code belongs to another setup request"
    );
    ensure!(
        receipt.master_id == envelope.master_id,
        "E_PAIRING_MASTER: code belongs to another Master"
    );
    ensure!(
        receipt.bundle_digest == envelope.bundle_digest,
        "E_PAIRING_DIGEST: code belongs to a different setup bundle"
    );
    let mut registration = receipt.registration.clone();
    fleet::validate_target(&mut registration, false)?;
    ensure!(
        registration.id == requested.target_id
            && registration.tenant == requested.tenant
            && registration.os == requested.os
            && registration.user == requested.user
            && canonical(&registration.routes)? == canonical(&requested.routes)?,
        "E_PAIRING_BINDING: machine, group, OS or connection differs from approved request"
    );
    Ok(())
}
pub fn encode_receipt(envelope: &Value, registration: &Value) -> Result<String> {
    let envelope: Envelope = serde_json::from_value(envelope.clone())?;
    let mut registration: Target = serde_json::from_value(registration.clone())?;
    fleet::validate_target(&mut registration, false)?;
    let receipt = Receipt {
        schema_version: 1,
        request_id: envelope.request_id.clone(),
        master_id: envelope.master_id.clone(),
        bundle_digest: envelope.bundle_digest.clone(),
        registration,
    };
    validate_binding(&envelope, &receipt)?;
    let bytes = canonical(&receipt)?;
    let code = format!(
        "{CODE_PREFIX}.{}.{}",
        URL_SAFE_NO_PAD.encode(&bytes),
        digest(&bytes)
    );
    ensure!(
        code.len() <= MAX_CODE_BYTES,
        "E_PAIRING_SIZE: pairing code exceeds 64 KiB"
    );
    Ok(code)
}
pub fn decode_receipt(code: &str) -> Result<Receipt> {
    ensure!(
        code.len() <= MAX_CODE_BYTES,
        "E_PAIRING_SIZE: pairing code exceeds 64 KiB"
    );
    let mut parts = code.trim().split('.');
    ensure!(
        parts.next() == Some(CODE_PREFIX),
        "E_PAIRING_VERSION: invalid pairing code prefix"
    );
    let encoded = parts
        .next()
        .context("E_PAIRING_CODE: missing pairing data")?;
    let checksum = parts
        .next()
        .context("E_PAIRING_CODE: missing pairing checksum")?;
    ensure!(
        parts.next().is_none(),
        "E_PAIRING_CODE: invalid pairing format"
    );
    let bytes = URL_SAFE_NO_PAD
        .decode(encoded)
        .context("E_PAIRING_CODE: pairing code is damaged")?;
    ensure!(
        checksum == digest(&bytes),
        "E_PAIRING_DIGEST: pairing code is damaged"
    );
    let mut receipt: Receipt =
        serde_json::from_slice(&bytes).context("E_PAIRING_CODE: invalid pairing data")?;
    ensure!(
        receipt.schema_version == 1,
        "E_PAIRING_VERSION: unsupported pairing code"
    );
    Uuid::parse_str(&receipt.request_id)?;
    Uuid::parse_str(&receipt.master_id)?;
    fleet::validate_target(&mut receipt.registration, false)?;
    Ok(receipt)
}
/// The 6 digits both screens show (entry 8): from the request, this Master, the approved bundle and the new
/// machine's host key, so a machine in the middle (with its own host key) shows different digits. "123 456".
pub fn short_code(receipt: &Receipt) -> Result<String> {
    let key = receipt.registration.host_keys.first().context("E_PAIRING_CODE: pairing code has no host key")?;
    let bound = canonical(&json!({"requestId":receipt.request_id,"masterId":receipt.master_id,
        "bundleDigest":receipt.bundle_digest,"hostKey":fleet::public_key_fingerprint(key)?}))?;
    let mut hasher = Sha256::new();
    hasher.update(b"sshcc-pairing-compare-v1\n");
    hasher.update(&bound);
    let hash = hasher.finalize();
    let n = u64::from_be_bytes(hash[..8].try_into().unwrap()) % 1_000_000;
    Ok(format!("{:03} {:03}", n / 1000, n % 1000))
}
pub fn short_code_of(code: &str) -> Result<String> {
    short_code(&decode_receipt(code)?)
}
fn receipts_dir(state: &Path) -> std::path::PathBuf {
    state.join("pairing-receipts")
}
/// On the new machine: keep the pairing code where its Master can fetch it (`pairing.receipt`).
pub fn store_receipt(state: &Path, code: &str) -> Result<()> {
    let receipt = decode_receipt(code)?;
    let dir = receipts_dir(state);
    fs::create_dir_all(&dir)?;
    crate::security::protect(&dir, true)?;
    let path = dir.join(format!("{}.code", receipt.request_id));
    fs::write(&path, code)?;
    crate::security::protect(&path, false)
}
/// On the new machine, for the Master `master_id` only: its pairing code for `request_id`, once Approve finished.
pub fn stored_receipt(state: &Path, master_id: &str, request_id: &str) -> Result<Value> {
    Uuid::parse_str(request_id).context("E_PAIRING_REQUEST: invalid request ID")?;
    let Ok(code) = fs::read_to_string(receipts_dir(state).join(format!("{request_id}.code"))) else {
        return Ok(json!({"ready":false}));
    };
    let receipt = decode_receipt(&code)?;
    ensure!(receipt.master_id == master_id, "E_PERMISSION: this pairing belongs to another Master");
    Ok(json!({"ready":true,"code":code.trim()}))
}

/// Verify public cached codes without granting rights or assuming service health.
pub fn inspect_receipt(code: &str, envelope: Option<&Value>) -> Result<Value> {
    let receipt = decode_receipt(code)?;
    if let Some(envelope) = envelope {
        let envelope: Envelope = serde_json::from_value(envelope.clone())?;
        validate_binding(&envelope, &receipt)?;
    }
    Ok(serde_json::to_value(receipt)?)
}
/// Installer runs this after its service health check. Live proof must be from
/// owner/SYSTEM local IPC, not an elevation user's claimed account or JSON file.
pub fn installer_receipt(
    state: &Path,
    config: &Config,
    envelope: &Value,
    registration: &Value,
    status: &Value,
) -> Result<String> {
    let code = encode_receipt(envelope, registration)?;
    let receipt = decode_receipt(&code)?;
    let envelope: Envelope = serde_json::from_value(envelope.clone())?;
    let requested = validate_envelope(&envelope)?;
    ensure!(
        config.roles.iter().any(|r| r == "target")
            && config.node_id == receipt.registration.node_id,
        "E_PAIRING_IDENTITY: registration does not match this Target"
    );
    ensure!(
        status["nodeId"].as_str() == Some(&config.node_id)
            && status["privileged"] == true
            && status["executionPrincipal"].as_str()
                == Some(if requested.os == "windows" {
                    "S-1-5-18"
                } else {
                    "0"
                }),
        "E_PAIRING_SERVICE: Target service must be running as SYSTEM/root"
    );
    let fingerprint = fleet::public_key_fingerprint(&requested.master.public_key)?;
    ensure!(
        fleet::authorized_master_key(state, &requested.master.id, &fingerprint)?,
        "E_PAIRING_APPROVAL: this Master has not been approved"
    );
    Ok(code)
}
/// Read only; never opens the runner, recovers jobs or requires owner IPC.
pub fn installer_preflight(state: &Path) -> Result<Value> {
    let terminals = state.join("terminals.sqlite");
    let terminal_count: usize = if terminals.exists() {
        Connection::open_with_flags(terminals, OpenFlags::SQLITE_OPEN_READ_ONLY)?.query_row(
            "SELECT COUNT(*) FROM terminals WHERE json_extract(record,'$.status')='running'",
            [],
            |row| row.get(0),
        )?
    } else {
        0
    };
    let path = state.join("jobs.sqlite");
    if !path.exists() {
        return Ok(json!({"runningJobs":0,"runningTerminals":terminal_count}));
    }
    let db = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let count: usize = db.query_row(
        "SELECT COUNT(*) FROM jobs WHERE json_extract(record, '$.status') IN ('queued','running')",
        [],
        |row| row.get(0),
    )?;
    Ok(json!({"runningJobs":count,"runningTerminals":terminal_count}))
}
fn database(state: &Path) -> Result<Connection> {
    let db = fleet::database(state)?;
    db.execute_batch("CREATE TABLE IF NOT EXISTS pairing_requests (id TEXT PRIMARY KEY, document TEXT NOT NULL);")?;
    Ok(db)
}
fn read_pending(db: &Connection, id: &str) -> Result<Pending> {
    Uuid::parse_str(id).context("E_PAIRING_REQUEST: invalid request ID")?;
    let document = db
        .query_row(
            "SELECT document FROM pairing_requests WHERE id=?1",
            [id],
            |row| row.get::<_, String>(0),
        )
        .optional()?;
    let Some(document) = document else {
        // Its sibling was answered first, which cancelled this one.
        let replaced = all_pending(db)?
            .iter()
            .any(|p| p.sibling.as_deref() == Some(id) && p.receipt_digest.is_some());
        if replaced {
            bail!("E_PAIRING_REPLACED: unknown setup request on this Master; the other installer of the pair was used first");
        }
        bail!("E_PAIRING_REQUEST: unknown setup request on this Master");
    };
    Ok(serde_json::from_str(&document)?)
}
/// Every stored request, newest first.
fn all_pending(db: &Connection) -> Result<Vec<Pending>> {
    let mut stmt = db.prepare("SELECT document FROM pairing_requests ORDER BY rowid DESC")?;
    let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
    rows.map(|row| Ok(serde_json::from_str(&row?)?)).collect()
}
fn write_pending(db: &Connection, pending: &Pending) -> Result<()> {
    db.execute("INSERT INTO pairing_requests(id,document) VALUES(?1,?2) ON CONFLICT(id) DO UPDATE SET document=excluded.document", params![pending.envelope.request_id, serde_json::to_string(pending)?])?;
    Ok(())
}
/// Forget a request that no Target has approved yet. An approved Target keeps its
/// authorization; that is revoked on the Target (Allowed Masters), never from here.
fn cancel(state: &Path, p: &Value) -> Result<Value> {
    let id = string(p, "requestId")?;
    let db = database(state)?;
    let pending = read_pending(&db, id)?;
    ensure!(
        pending.state == "awaiting_approval",
        "E_PAIRING_STATE: only requests still awaiting approval can be cancelled"
    );
    db.execute("DELETE FROM pairing_requests WHERE id=?1", [id])?;
    Ok(json!({"requestId": id, "cancelled": true}))
}
fn view(pending: &Pending) -> Result<Value> {
    let requested = bundle(&pending.envelope)?;
    Ok(
        json!({"requestId":pending.envelope.request_id,"targetId":requested.target_id,"label":pending.envelope.target_label,"tenant":requested.tenant,"os":requested.os,"state":pending.state,"createdAt":pending.created_at,"updatedAt":pending.updated_at,"envelope":pending.envelope,"diagnostics":pending.diagnostics,"sibling":pending.sibling}),
    )
}
fn string<'a>(params: &'a Value, name: &str) -> Result<&'a str> {
    params
        .get(name)
        .and_then(Value::as_str)
        .with_context(|| format!("E_ARGUMENT: missing string {name}"))
}
pub(crate) fn route_kind(address: &str) -> &'static str {
    if address.ends_with(".ts.net")
        || address
            .parse::<std::net::Ipv4Addr>()
            .is_ok_and(|ip| ip.octets()[0] == 100 && (64..=127).contains(&ip.octets()[1]))
    {
        "tailscale"
    } else if address
        .parse::<std::net::IpAddr>()
        .is_ok_and(|ip| match ip {
            std::net::IpAddr::V4(ip) => ip.is_private() || ip.is_loopback() || ip.is_link_local(),
            std::net::IpAddr::V6(ip) => {
                ip.is_loopback() || ip.is_unique_local() || ip.is_unicast_link_local()
            }
        })
        || !address.contains('.')
    {
        "lan"
    } else {
        "public"
    }
}
/// A machine name as typed: 1-256 characters, no control characters.
fn valid_label(label: &str) -> Result<&str> {
    ensure!(
        !label.is_empty() && label.len() <= 256 && !label.chars().any(char::is_control),
        "E_ARGUMENT: invalid machine name"
    );
    Ok(label)
}
fn create(state: &Path, config: &Config, p: &Value) -> Result<Value> {
    let label = valid_label(string(p, "label")?)?;
    let generated = label
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect::<String>();
    let slug = generated
        .trim_matches('-')
        .chars()
        .take(48)
        .collect::<String>();
    let stem = if slug.is_empty() { "target" } else { &slug };
    let id = p
        .get("targetId")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .unwrap_or_else(|| format!("{stem}-{}", &Uuid::new_v4().simple().to_string()[..8]));
    let routes = if let Some(routes) = p.get("routes") {
        routes.clone()
    } else {
        let address = string(p, "address")?;
        json!([{"kind":p.get("routeKind").and_then(Value::as_str).unwrap_or_else(|| route_kind(address)),"address":address,"port":p.get("port").cloned().unwrap_or(json!(22))}])
    };
    let os = string(p, "os")?;
    // An unknown OS (no SSH banner yet) gets one request per installer, Windows and Ubuntu, for the
    // same host; whichever is answered first wins and cancels the other.
    let systems: &[&str] = if os == "unknown" {
        &["windows", "linux"]
    } else {
        &[os]
    };
    let now = Utc::now().to_rfc3339();
    let mut requests = systems
        .iter()
        .map(|os| -> Result<Pending> {
            let bundle = fleet::enrollment_export(
                state,
                config,
                &json!({"targetId":id,"tenant":string(p,"tenant")?,"os":os,"user":p.get("user").and_then(Value::as_str).unwrap_or("sshcc-bridge"),"routes":routes}),
            )?;
            let envelope = Envelope {
                schema_version: 1,
                request_id: Uuid::new_v4().to_string(),
                master_id: config.node_id.clone(),
                bundle_digest: digest(&canonical(&bundle)?),
                target_label: label.into(),
                bundle,
            };
            validate_envelope(&envelope)?;
            ensure!(
                canonical(&envelope)?.len() <= MAX_CODE_BYTES,
                "E_PAIRING_SIZE: setup request exceeds 64 KiB"
            );
            Ok(Pending {
                envelope,
                state: "awaiting_approval".into(),
                created_at: now.clone(),
                updated_at: now.clone(),
                receipt_digest: None,
                diagnostics: None,
                sibling: None,
                fetched: None,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    if let [windows, linux] = &mut requests[..] {
        windows.sibling = Some(linux.envelope.request_id.clone());
        linux.sibling = Some(windows.envelope.request_id.clone());
    }
    let mut db = database(state)?;
    let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let targets = fleet::list_targets(&tx)?;
    ensure!(
        targets.len() < fleet::MAX_TARGETS || targets.iter().any(|t| t.id == id),
        "Master supports at most 50 targets"
    );
    for pending in &requests {
        write_pending(&tx, pending)?;
    }
    tx.commit()?;
    match &requests[..] {
        [single] => view(single),
        pair => Ok(Value::Array(pair.iter().map(view).collect::<Result<_>>()?)),
    }
}
fn diagnose(state: &Path, config: &Config, id: &str) -> Result<Value> {
    let db = database(state)?;
    let pending = read_pending(&db, id)?;
    ensure!(
        pending.receipt_digest.is_some(),
        "E_PAIRING_APPROVAL: paste the code from the approved machine first"
    );
    let target_id = bundle(&pending.envelope)?.target_id;
    let diagnosed_target = fleet::target(&db, &target_id)?;
    // Do not retain a write transaction across a slow SSH call.
    let diagnostics = match fleet::handle(
        state,
        config,
        &Request {
            method: "targets.doctor".into(),
            params: json!({"targetId":target_id}),
        },
    ) {
        Ok(Some(value)) => value,
        Ok(None) => {
            json!({"ok":false,"stages":[{"name":"diagnostics","ok":false,"message":"diagnostics unavailable"}]})
        }
        Err(error) => {
            json!({"ok":false,"stages":[{"name":"diagnostics","ok":false,"message":format!("{error:#}")}]})
        }
    };
    let expected = fleet::target(&db, &target_id)?;
    let ready = diagnostics["ok"] == true
        && diagnostics["result"]["nodeId"].as_str() == Some(&expected.node_id)
        && diagnostics["result"]["privileged"] == true
        && diagnostics["result"]["executionPrincipal"].as_str()
            == Some(if expected.os == "windows" {
                "S-1-5-18"
            } else {
                "0"
            });
    let mut db = database(state)?;
    let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let mut pending = read_pending(&tx, id)?;
    if canonical(&fleet::target(&tx, &target_id)?)? != canonical(&diagnosed_target)? {
        // A newer address edit has its own check. Never publish stale readiness.
        return view(&pending);
    }
    pending.state = if ready { "ready" } else { "needs_attention" }.into();
    pending.diagnostics = Some(diagnostics);
    pending.updated_at = Utc::now().to_rfc3339();
    write_pending(&tx, &pending)?;
    tx.commit()?;
    view(&pending)
}
/// One validation path for `preview` and `complete`, so they never disagree.
/// Returns the stored request, the receipt and the receipt digest; writes nothing.
fn validate_code(
    db: &Connection,
    config: &Config,
    code: &str,
) -> Result<(Pending, Receipt, String)> {
    let receipt = decode_receipt(code)?;
    ensure!(
        receipt.master_id == config.node_id,
        "E_PAIRING_MASTER: code belongs to another Master"
    );
    let receipt_digest = digest(&canonical(&receipt)?);
    let pending = read_pending(db, &receipt.request_id)?;
    validate_binding(&pending.envelope, &receipt)?;
    if let Some(old) = &pending.receipt_digest {
        ensure!(
            old == &receipt_digest,
            "E_PAIRING_CONFLICT: a different machine already answered this request"
        );
    }
    Ok((pending, receipt, receipt_digest))
}
/// Read-only facts for the review card, taken from the code before anything is saved.
fn preview(state: &Path, config: &Config, p: &Value) -> Result<Value> {
    let db = database(state)?;
    let (pending, receipt, _) = validate_code(&db, config, string(p, "code")?)?;
    let mut target = receipt.registration;
    fleet::validate_target(&mut target, false)?;
    let key = target
        .host_keys
        .first()
        .context("E_PAIRING_CODE: pairing code has no host key")?;
    Ok(
        json!({"requestId":pending.envelope.request_id,"targetId":target.id,"label":pending.envelope.target_label,"hostName":target.label,"os":target.os,"hostKeySha256":fleet::public_key_fingerprint(key)?}),
    )
}
fn complete(state: &Path, config: &Config, p: &Value) -> Result<Value> {
    let label = match p.get("label") {
        None | Some(Value::Null) => None,
        Some(value) => {
            let label = value
                .as_str()
                .context("E_ARGUMENT: label must be a string")?;
            Some(valid_label(label)?.to_owned())
        }
    };
    let mut db = database(state)?;
    let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let (mut pending, receipt, receipt_digest) = validate_code(&tx, config, string(p, "code")?)?;
    if pending.receipt_digest.is_some() {
        return view(&pending);
    }
    let mut target = receipt.registration;
    fleet::validate_target(&mut target, false)?;
    target.label = label.unwrap_or_else(|| pending.envelope.target_label.clone());
    // This is the same strict host-key/node/group collision guard as import.
    fleet::save_target(&tx, &target)?;
    pending.receipt_digest = Some(receipt_digest);
    pending.state = "needs_attention".into();
    pending.updated_at = Utc::now().to_rfc3339();
    write_pending(&tx, &pending)?;
    // The other installer of an unknown-OS pair is no longer needed.
    if let Some(sibling) = &pending.sibling {
        if read_pending(&tx, sibling).is_ok_and(|other| other.state == "awaiting_approval") {
            tx.execute("DELETE FROM pairing_requests WHERE id=?1", [sibling])?;
        }
    }
    tx.commit()?;
    diagnose(state, config, &pending.envelope.request_id)
}
/// One look at the new machine for its pairing code (entry 8). `ask` gets the requested bundle and the request ID and
/// returns the code with the host key the connection saw, `None` while Approve isn't done yet, or an error. An older
/// installer that can't hand over the code answers "not available": then the person pastes the code, as before.
/// Returns `{state: waiting | ready | old | done}`; `ready` carries the 6 digits and the review facts.
/// How fetch_with reaches the Target for its (code, key); tests pass a fake.
pub type AskReceipt<'a> = dyn Fn(&Value, &str) -> Result<Option<(String, String)>> + 'a;
pub fn fetch_with(state: &Path, config: &Config, p: &Value, ask: &AskReceipt) -> Result<Value> {
    let id = string(p, "requestId")?;
    let db = database(state)?;
    let mut pending = read_pending(&db, id)?;
    if pending.receipt_digest.is_some() {
        return Ok(json!({"state":"done","requestId":id}));
    }
    match ask(&pending.envelope.bundle, id) {
        Ok(Some((code, seen_key))) => {
            let (_, receipt, _) = validate_code(&db, config, &code)?;
            ensure!(receipt.registration.host_keys.iter().any(|k| same_key(k, &seen_key)),
                "E_PAIRING_HOSTKEY: the machine that answered isn't the one in its pairing code");
            pending.fetched = Some(code.clone());
            write_pending(&db, &pending)?;
            let preview = preview(state, config, &json!({"code":code}))?;
            Ok(json!({"state":"ready","requestId":id,"digits":short_code(&receipt)?,"preview":preview}))
        }
        Ok(None) => Ok(json!({"state":"waiting","requestId":id})),
        Err(error) => {
            let message = format!("{error:#}");
            if message.contains("not available over bridge") || message.contains("E_METHOD") {
                Ok(json!({"state":"old","requestId":id}))
            } else {
                Ok(json!({"state":"waiting","requestId":id,"error":message}))
            }
        }
    }
}
fn same_key(a: &str, b: &str) -> bool {
    let body = |k: &str| k.split_whitespace().take(2).collect::<Vec<_>>().join(" ");
    body(a) == body(b)
}
/// The person compared the digits: the same finishes the pairing as a pasted code would; different cancels it.
fn confirm(state: &Path, config: &Config, p: &Value) -> Result<Value> {
    let id = string(p, "requestId")?;
    let db = database(state)?;
    let pending = read_pending(&db, id)?;
    let code = pending.fetched.clone().context("E_PAIRING_STATE: no number to compare yet")?;
    if p["same"] == true {
        drop(db);
        let mut params = json!({"code":code});
        if let Some(label) = p.get("label") {
            params["label"] = label.clone();
        }
        return complete(state, config, &params);
    }
    // The pair made for an unknown OS goes too: the same address answered with the wrong machine.
    db.execute("DELETE FROM pairing_requests WHERE id=?1", [id])?;
    if let Some(sibling) = &pending.sibling {
        db.execute("DELETE FROM pairing_requests WHERE id=?1", [sibling])?;
    }
    bail!("E_PAIRING_MISMATCH: the numbers differ, so this pairing was cancelled · something may be between this Master and the new machine; check the network and start again")
}

fn status(state: &Path, config: &Config, p: &Value) -> Result<Value> {
    let mut db = database(state)?;
    let Some(id) = p.get("requestId").and_then(Value::as_str) else {
        ensure!(
            p.get("routes").is_none() && p.get("retry").and_then(Value::as_bool) != Some(true),
            "E_ARGUMENT: requestId required for retry or address changes"
        );
        return Ok(json!(all_pending(&db)?
            .iter()
            .map(view)
            .collect::<Result<Vec<_>>>()?));
    };
    if let Some(routes) = p.get("routes") {
        let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut pending = read_pending(&tx, id)?;
        ensure!(
            pending.receipt_digest.is_some(),
            "E_PAIRING_APPROVAL: transport can change after approval"
        );
        let mut target = fleet::target(&tx, &bundle(&pending.envelope)?.target_id)?;
        target.routes = serde_json::from_value::<Vec<Route>>(routes.clone())?;
        fleet::validate_target(&mut target, false)?;
        fleet::save_target(&tx, &target)?;
        pending.state = "needs_attention".into();
        pending.diagnostics = None;
        pending.updated_at = Utc::now().to_rfc3339();
        write_pending(&tx, &pending)?;
        tx.commit()?;
        return diagnose(state, config, id);
    }
    if p.get("retry").and_then(Value::as_bool) == Some(true) {
        return diagnose(state, config, id);
    }
    view(&read_pending(&db, id)?)
}
/// The OS an SSH identification line reports, if this tool can tell.
fn banner_os(banner: &str) -> Option<&'static str> {
    let software = banner.splitn(3, '-').nth(2)?;
    if software.starts_with("OpenSSH_for_Windows") {
        Some("windows")
    } else if software.starts_with("OpenSSH_")
        && (software.contains(" Ubuntu") || software.contains(" Debian"))
    {
        Some("linux")
    } else {
        None
    }
}
/// Read-only live check for an address typed in "Add host": does SSH answer, which OS its banner
/// reports, and is it already a saved Target (same route rule as `fleet::save_target`) or a request
/// awaiting approval. Banner only, never logs in; writes nothing.
fn check(state: &Path, p: &Value) -> Result<Value> {
    let address = string(p, "address")?.trim();
    ensure!(
        !address.is_empty() && address.len() <= 253 && !address.chars().any(char::is_control),
        "E_ARGUMENT: invalid address"
    );
    let port = match p.get("port") {
        None | Some(Value::Null) => 22,
        Some(value) => value
            .as_u64()
            .and_then(|port| u16::try_from(port).ok())
            .filter(|port| *port > 0)
            .context("E_ARGUMENT: invalid port")?,
    };
    let same = |route: &Route| route.address.eq_ignore_ascii_case(address) && route.port == port;
    // Read the database first and let go of it before the slow network probe.
    let (mut already_as, mut target_id, mut request_id) = (None, None, None);
    {
        let db = database(state)?;
        if let Some(target) = fleet::list_targets(&db)?
            .into_iter()
            .find(|t| t.routes.iter().any(same))
        {
            already_as = Some(target.label);
            target_id = Some(target.id);
        } else {
            // Checked for an open request (`requestId`): it and its sibling are not "already added".
            let own = p.get("requestId").and_then(Value::as_str);
            for pending in all_pending(&db)? {
                let mine = own.is_some()
                    && (Some(pending.envelope.request_id.as_str()) == own
                        || pending.sibling.as_deref() == own);
                if !mine
                    && pending.state == "awaiting_approval"
                    && bundle(&pending.envelope)?.routes.iter().any(same)
                {
                    already_as = Some(pending.envelope.target_label.clone());
                    request_id = Some(pending.envelope.request_id.clone());
                    break;
                }
            }
        }
    }
    let banner = crate::ssh_banner::read(address, port).ok();
    let os = banner.as_deref().and_then(banner_os);
    Ok(
        json!({"address":address,"port":port,"reachable":banner.is_some(),"banner":banner,"os":os,"alreadyAs":already_as,"alreadyTargetId":target_id,"alreadyRequestId":request_id}),
    )
}
pub fn handle(state: &Path, config: &Config, req: &Request) -> Result<Option<Value>> {
    if !req.method.starts_with("pairing.") {
        return Ok(None);
    }
    ensure!(
        config.roles.iter().any(|r| r == "master"),
        "E_ROLE: master role is required"
    );
    Ok(Some(match req.method.as_str() {
        "pairing.create" => create(state, config, &req.params)?,
        "pairing.status" => status(state, config, &req.params)?,
        "pairing.complete" => complete(state, config, &req.params)?,
        "pairing.preview" => preview(state, config, &req.params)?,
        "pairing.cancel" => cancel(state, &req.params)?,
        "pairing.check" => check(state, &req.params)?,
        "pairing.fetch" => fetch_with(state, config, &req.params, &|bundle, id| fleet::fetch_receipt(state, config, bundle, id))?,
        "pairing.confirm" => confirm(state, config, &req.params)?,
        _ => bail!("E_METHOD: unsupported pairing method"),
    }))
}
