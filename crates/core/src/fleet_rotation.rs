//! Change keys: give this Master new tenant keys and move every host to them, so the old machine and old backups
//! stop working. Needs no Target update: each host runs one elevated job built from commands it already has.
//! A host counts as moved only after this Master has connected to it with the new key. Until every host has
//! moved, the old private keys stay, and each connection uses the key that host accepts now.
use super::{database, list_targets, transport, Target};
use crate::{config::Config, protocol::Request};
use anyhow::{bail, ensure, Context, Result};
use base64::{engine::general_purpose::STANDARD, Engine};
use rusqlite::params;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};
use uuid::Uuid;

const JOB_TIMEOUT: Duration = Duration::from_secs(180);
/// Hosts still on the old key are tried again at most this often (when a window asks, or on the service's timer).
pub const RETRY_EVERY: chrono::TimeDelta = chrono::TimeDelta::seconds(60);

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TenantKeys {
    old: String,
    new: String,
}
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Host {
    label: String,
    /// "pending": accepts the old key. "installed": the job ran; the new key is used and still being checked.
    /// "moved": checked with the new key.
    state: String,
    error: Option<String>,
    updated_at: Option<String>,
    /// The key change job, followed with the new key once the host switched to it.
    #[serde(default)]
    job: Option<String>,
}
#[derive(Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Rotation {
    started_at: String,
    finished_at: Option<String>,
    #[serde(default)]
    last_pass_at: Option<String>,
    tenants: BTreeMap<String, TenantKeys>,
    hosts: BTreeMap<String, Host>,
}

/// One step of moving a host, carried out by `act` in `pass`: start the job (with the old key), then follow it
/// with the new key until it has finished. A Check that fails with `E_STILL_OLD_KEY` means the job put the old key
/// back (the host stays on it); any other failure leaves the host to be checked again on the next pass.
pub enum Step<'a> {
    Install { script: &'a str, job: &'a str },
    Check { job: Option<&'a str> },
}
std::thread_local! {
    /// Set by `act` to reach a host with one specific key (the old one, to read why a job failed).
    static FORCED: std::cell::RefCell<Option<PathBuf>> = const { std::cell::RefCell::new(None) };
}
fn with_identity<T>(path: PathBuf, work: impl FnOnce() -> T) -> T {
    FORCED.with(|cell| *cell.borrow_mut() = Some(path));
    let result = work();
    FORCED.with(|cell| *cell.borrow_mut() = None);
    result
}

fn now() -> String {
    chrono::Utc::now().to_rfc3339()
}
fn record_path(state: &Path) -> PathBuf {
    state.join("rotation.json")
}
fn read(state: &Path) -> Result<Option<Rotation>> {
    match fs::read(record_path(state)) {
        Ok(bytes) => Ok(Some(serde_json::from_slice(&bytes)?)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}
fn write(state: &Path, rotation: &Rotation) -> Result<()> {
    let path = record_path(state);
    let tmp = state.join(format!(".rotation-{}.tmp", Uuid::new_v4()));
    fs::write(&tmp, serde_json::to_vec_pretty(rotation)?)?;
    crate::security::protect(&tmp, false)?;
    crate::security::replace(&tmp, &path)?;
    Ok(())
}
fn active(state: &Path) -> Result<Option<Rotation>> {
    Ok(read(state)?.filter(|rotation| rotation.finished_at.is_none()))
}
fn next_key(state: &Path, tenant: &str) -> PathBuf {
    state.join("keys").join(format!("{tenant}.next.ed25519"))
}

/// The private key to connect to `target` with: the new one once its job ran, the current one otherwise.
pub fn identity(state: &Path, target: &Target) -> Result<PathBuf> {
    if let Some(forced) = FORCED.with(|cell| cell.borrow().clone()) {
        return Ok(forced);
    }
    if let Some(rotation) = active(state)? {
        if rotation.hosts.get(&target.id).is_some_and(|host| host.state != "pending") {
            return Ok(next_key(state, &target.tenant));
        }
    }
    Ok(super::key_path(state, &target.tenant))
}

/// Hosts this Master moves itself: direct ones. Merged hosts are moved by the Master they belong to.
fn own_hosts(state: &Path) -> Result<Vec<Target>> {
    list_targets(&database(state)?)
}

fn keygen(path: &Path) -> Result<String> {
    for stale in [path.to_path_buf(), path.with_extension("ed25519.pub")] {
        let _ = fs::remove_file(stale);
    }
    let output = super::quiet_command("ssh-keygen")
        .args(["-q", "-t", "ed25519", "-N", "", "-C", "sshcc-service"])
        .arg("-f")
        .arg(path)
        .output()
        .context("ssh-keygen is required to make new Master keys")?;
    ensure!(output.status.success(), "ssh-keygen failed: {}", String::from_utf8_lossy(&output.stderr));
    crate::security::protect(path, false)?;
    let public = path.with_extension("ed25519.pub");
    crate::security::protect(&public, false)?;
    super::public_key(&fs::read_to_string(public)?)
}

/// Starts (or resumes) a change of keys: one new key per tenant in use, every own host pending.
pub fn start(state: &Path, config: &Config) -> Result<Value> {
    super::role(config, "master")?;
    if active(state)?.is_none() {
        let hosts = own_hosts(state)?;
        let mut rotation = Rotation { started_at: now(), ..Default::default() };
        for target in &hosts {
            if rotation.tenants.contains_key(&target.tenant) {
                continue;
            }
            let current = super::key_path(state, &target.tenant).with_extension("ed25519.pub");
            let old = super::public_key(&fs::read_to_string(&current).with_context(|| format!("no key for tenant {}", target.tenant))?)?;
            let new = keygen(&next_key(state, &target.tenant))?;
            rotation.tenants.insert(target.tenant.clone(), TenantKeys { old, new });
        }
        include_hosts(state, &mut rotation)?;
        write(state, &rotation)?;
    }
    status(state, config)
}

/// Every own host is in the record: hosts paired since the change started (enrolled with the old key) are added,
/// removed hosts are dropped.
fn include_hosts(state: &Path, rotation: &mut Rotation) -> Result<()> {
    let hosts = own_hosts(state)?;
    for target in &hosts {
        if rotation.tenants.contains_key(&target.tenant) && !rotation.hosts.contains_key(&target.id) {
            rotation.hosts.insert(target.id.clone(), Host { label: target.label.clone(), state: "pending".into(), ..Default::default() });
        }
    }
    rotation.hosts.retain(|id, _| hosts.iter().any(|target| &target.id == id));
    Ok(())
}

/// One attempt at every host still on the old key (or not yet checked), in order. `act` runs the job or the
/// check; the record is saved after every step, so a crash never loses a host's state. Finishes the change when
/// no host is left on the old key.
pub fn pass(state: &Path, config: &Config, act: &mut dyn FnMut(&Target, Step) -> Result<()>) -> Result<Value> {
    super::role(config, "master")?;
    let Some(mut rotation) = active(state)? else { return status(state, config) };
    include_hosts(state, &mut rotation)?;
    rotation.last_pass_at = Some(now());
    write(state, &rotation)?;
    for target in own_hosts(state)? {
        let Some(keys) = rotation.tenants.get(&target.tenant).cloned() else { continue };
        let Some(host) = rotation.hosts.get(&target.id).cloned() else { continue };
        if host.state == "moved" {
            continue;
        }
        let mut host = host;
        if host.state == "pending" {
            if !target.runner_installed {
                host.error = Some("E_RUNNER_NOT_INSTALLED: this host has no SSH Control Center service to change its key".into());
                rotation.hosts.insert(target.id.clone(), host);
                write(state, &rotation)?;
                continue;
            }
            let script = install_script(config, &target, &keys.old, &keys.new)?;
            let job = Uuid::new_v4().to_string();
            match act(&target, Step::Install { script: &script, job: &job }) {
                Ok(()) => {
                    host.state = "installed".into();
                    host.job = Some(job);
                }
                Err(error) => {
                    host.error = Some(format!("{error:#}"));
                    host.updated_at = Some(now());
                    rotation.hosts.insert(target.id.clone(), host);
                    write(state, &rotation)?;
                    continue;
                }
            }
            rotation.hosts.insert(target.id.clone(), host.clone());
            write(state, &rotation)?;
        }
        match act(&target, Step::Check { job: host.job.as_deref() }) {
            Ok(()) => {
                host.state = "moved".into();
                host.error = None;
            }
            Err(error) if format!("{error:#}").contains("E_STILL_OLD_KEY") => {
                host.state = "pending".into();
                host.job = None;
                host.error = Some(format!("{error:#}"));
            }
            Err(error) => host.error = Some(format!("E_NEW_KEY_NOT_ACCEPTED: {error:#}")),
        }
        host.updated_at = Some(now());
        rotation.hosts.insert(target.id.clone(), host);
        write(state, &rotation)?;
    }
    if rotation.hosts.values().all(|host| host.state == "moved") {
        finish(state, &mut rotation)?;
    }
    status(state, config)
}

/// Every host moved: the new keys replace the old ones, which are deleted.
fn finish(state: &Path, rotation: &mut Rotation) -> Result<()> {
    let db = database(state)?;
    for (tenant, keys) in &rotation.tenants {
        let current = super::key_path(state, tenant);
        let next = next_key(state, tenant);
        if next.exists() {
            crate::security::replace(&next, &current)?;
            crate::security::replace(&next.with_extension("ed25519.pub"), &current.with_extension("ed25519.pub"))?;
        }
        db.execute("UPDATE tenant_keys SET public_key=?1 WHERE tenant=?2", params![keys.new, tenant])?;
    }
    rotation.finished_at = Some(now());
    write(state, rotation)?;
    crate::backup::keys_changed(state, rotation.finished_at.as_deref().unwrap_or_default())
}

/// True when a change of keys is unfinished and its last pass is older than `RETRY_EVERY`.
pub fn retry_due(state: &Path) -> Result<bool> {
    let Some(rotation) = active(state)? else { return Ok(false) };
    let last = rotation.last_pass_at.as_deref().and_then(|at| chrono::DateTime::parse_from_rfc3339(at).ok());
    Ok(last.is_none_or(|at| chrono::Utc::now().signed_duration_since(at) >= RETRY_EVERY))
}
/// Merged hosts keep the keys of the Master they belong to: that Master changes them, not this one.
fn merged(state: &Path) -> Result<Vec<Value>> {
    let db = database(state)?;
    let mut stmt = db.prepare("SELECT document FROM merges ORDER BY target_id")?;
    let mut rows = Vec::new();
    for document in stmt.query_map([], |row| row.get::<_, String>(0))? {
        let merge: Value = serde_json::from_str(&document?)?;
        for child in merge["children"].as_array().into_iter().flatten() {
            rows.push(json!({"label":child["label"],"master":merge["label"]}));
        }
    }
    Ok(rows)
}

pub fn status(state: &Path, config: &Config) -> Result<Value> {
    super::role(config, "master")?;
    let Some(rotation) = read(state)? else { return Ok(json!({"active":false})) };
    let hosts: Vec<Value> = rotation
        .hosts
        .iter()
        .map(|(id, host)| json!({"targetId":id,"label":host.label,"state":host.state,"error":host.error,"updatedAt":host.updated_at}))
        .collect();
    let old = rotation.hosts.values().filter(|host| host.state == "pending").count();
    let moved = rotation.hosts.values().filter(|host| host.state == "moved").count();
    Ok(json!({"active":rotation.finished_at.is_none(),"startedAt":rotation.started_at,"finishedAt":rotation.finished_at,
        "lastPassAt":rotation.last_pass_at,"total":rotation.hosts.len(),"moved":moved,"oldKey":old,"hosts":hosts,"merged":merged(state)?}))
}

fn fingerprint(public: &str) -> Result<String> {
    super::public_key_fingerprint(public)
}
/// The forced-command line the installers write for this Master, for the new key.
fn authorized_line(config: &Config, target: &Target, public: &str) -> Result<String> {
    let fp = fingerprint(public)?;
    let command = if target.os == "windows" {
        let script = format!(
            "$utf8=New-Object Text.UTF8Encoding($false); [Console]::InputEncoding=$utf8; [Console]::OutputEncoding=$utf8; $OutputEncoding=$utf8; & '{}' bridge --master-id {} --master-key {}; exit $LASTEXITCODE",
            target.bridge_path.replace('\'', "''"), config.node_id, fp
        );
        let utf16: Vec<u8> = script.encode_utf16().flat_map(u16::to_le_bytes).collect();
        format!("powershell.exe -NoProfile -NonInteractive -EncodedCommand {}", STANDARD.encode(utf16))
            .replace('\\', "\\\\")
            .replace('"', "\\\"")
    } else {
        format!("{} bridge --master-id {} --master-key {}", target.bridge_path, config.node_id, fp)
    };
    Ok(format!("restrict,command=\"{command}\" {public} sshcc:master:{}", config.node_id))
}
fn bundle(config: &Config, target: &Target, public: &str) -> Result<String> {
    let mut bundle = super::Bundle {
        schema_version: 2,
        target_id: target.id.clone(),
        tenant: target.tenant.clone(),
        os: target.os.clone(),
        user: target.user.clone(),
        routes: target.routes.clone(),
        master: super::Master { id: config.node_id.clone(), label: config.label.clone(), public_key: public.into() },
    };
    super::validate_bundle(&mut bundle)?;
    Ok(serde_json::to_string(&bundle)?)
}

/// The elevated job for one host. In order: add the new key's line (unused while the host's Master record still
/// names the old key), switch the record to the new key (put back on failure), then remove the old key's line.
pub fn install_script(config: &Config, target: &Target, old: &str, new: &str) -> Result<String> {
    let new_line = authorized_line(config, target, new)?;
    let (new_bundle, old_bundle) = (bundle(config, target, new)?, bundle(config, target, old)?);
    let old_b64 = old.split_whitespace().nth(1).context("invalid old key")?;
    let id = &config.node_id;
    Uuid::parse_str(id).context("invalid Master ID")?;
    if target.os == "windows" {
        let q = |text: &str| format!("'{}'", text.replace('\'', "''"));
        Ok(format!(
            r#"$ErrorActionPreference = 'Stop'
$enc = New-Object Text.UTF8Encoding($false)
$keys = Join-Path $env:ProgramData ('ssh\sshcc\' + {user} + '.authorized_keys')
$exe = {exe}
$data = Join-Path $env:ProgramData 'SSHControlCenter'
$newLine = {new_line}
$marker = ' sshcc:master:{id}'
$oldKey = ' ' + {old_b64} + ' '
$newBundle = Join-Path $env:TEMP ('sshcc-key-' + [guid]::NewGuid() + '.json')
$oldBundle = Join-Path $env:TEMP ('sshcc-key-' + [guid]::NewGuid() + '.json')
try {{
  [IO.File]::WriteAllText($newBundle, {new_bundle}, $enc)
  [IO.File]::WriteAllText($oldBundle, {old_bundle}, $enc)
  $lines = @([IO.File]::ReadAllLines($keys))
  if ($lines -notcontains $newLine) {{ [IO.File]::WriteAllLines($keys, [string[]]@($lines + $newLine), $enc) }}
  & $exe --data-dir $data configure revoke-master {id} | Out-Null
  if ($LASTEXITCODE) {{ throw 'Could not update this Master on the host' }}
  & $exe --data-dir $data configure accept-bundle $newBundle | Out-Null
  if ($LASTEXITCODE) {{
    & $exe --data-dir $data configure accept-bundle $oldBundle | Out-Null
    [IO.File]::WriteAllLines($keys, [string[]]@($lines | Where-Object {{ $_ -ne $newLine }}), $enc)
    throw 'The host refused the new key; it still uses the old one'
  }}
  $kept = @([IO.File]::ReadAllLines($keys) | Where-Object {{ -not ($_.EndsWith($marker) -and $_.Contains($oldKey)) }})
  [IO.File]::WriteAllLines($keys, [string[]]$kept, $enc)
  'SSHCC key changed'
}} finally {{ Remove-Item -LiteralPath $newBundle, $oldBundle -Force -ErrorAction SilentlyContinue }}
"#,
            user = q(&target.user), exe = q(&target.bridge_path), new_line = q(&new_line), old_b64 = q(old_b64),
            new_bundle = q(&new_bundle), old_bundle = q(&old_bundle), id = id
        ))
    } else {
        let q = |text: &str| format!("'{}'", text.replace('\'', r"'\''"));
        Ok(format!(
            r#"set -eu
keys=/etc/ssh/sshcc/{user}.authorized_keys
sshcc={exe}
data=/var/lib/sshcc
new_line={new_line}
dir=$(mktemp -d)
trap 'rm -rf "$dir"' EXIT
printf '%s' {new_bundle} > "$dir/new.json"
printf '%s' {old_bundle} > "$dir/old.json"
test -f "$keys"
grep -qxF "$new_line" "$keys" || {{ cp -p "$keys" "$dir/keys"; printf '%s\n' "$new_line" >> "$dir/keys"; mv -f "$dir/keys" "$keys"; }}
"$sshcc" --data-dir "$data" configure revoke-master {id} >/dev/null
if ! "$sshcc" --data-dir "$data" configure accept-bundle "$dir/new.json" >/dev/null; then
  "$sshcc" --data-dir "$data" configure accept-bundle "$dir/old.json" >/dev/null || true
  grep -vxF "$new_line" "$keys" > "$dir/keys" || true
  chmod 0644 "$dir/keys"; mv -f "$dir/keys" "$keys"
  echo 'The host refused the new key; it still uses the old one' >&2
  exit 1
fi
awk -v k=" {old_b64} " -v m=" sshcc:master:{id}" '!(index($0, k) && substr($0, length($0) - length(m) + 1) == m)' "$keys" > "$dir/keys"
chmod 0644 "$dir/keys"; mv -f "$dir/keys" "$keys"
echo 'SSHCC key changed'
"#,
            user = target.user, exe = q(&target.bridge_path), new_line = q(&new_line),
            new_bundle = q(&new_bundle), old_bundle = q(&old_bundle), id = id, old_b64 = old_b64
        ))
    }
}
/// The real steps. Install starts the elevated job with the old key. Check follows it with the new key, which
/// the host accepts from the moment the job switched it; the old key is tried only to read why a job failed.
pub fn act(state: &Path, config: &Config, target: &Target, step: Step) -> Result<()> {
    let call = |method: &str, params: Value| {
        super::handle(state, config, &Request { method: method.into(), params })?.context("job runner unavailable")
    };
    let (old, new) = (super::key_path(state, &target.tenant), next_key(state, &target.tenant));
    match step {
        Step::Install { script, job } => {
            let cwd = if target.os == "windows" { "C:\\" } else { "/" };
            with_identity(old, || call("jobs.start", json!({"targetId":target.id,"jobId":job,"script":script,"cwd":cwd,"asAdmin":true}))).map(|_| ())
        }
        Step::Check { job: None } => with_identity(new, || transport::request(state, config, target, "system.status", json!({}))).map(|_| ()),
        Step::Check { job: Some(job) } => {
            let status = |key: &PathBuf| with_identity(key.clone(), || call("jobs.status", json!({"targetId":target.id,"jobId":job})));
            let logs = |key: &PathBuf| with_identity(key.clone(), || call("jobs.logs", json!({"targetId":target.id,"jobId":job,"stream":"stderr","offset":0,"limit":4096})))
                .ok().and_then(|logs| logs["content"].as_str().map(|text| text.trim().to_owned())).unwrap_or_default();
            let deadline = Instant::now() + JOB_TIMEOUT;
            while Instant::now() < deadline {
                match status(&new) {
                    Ok(row) if row["status"] == "succeeded" => return Ok(()),
                    Ok(row) if !matches!(row["status"].as_str(), Some("queued" | "running")) => {
                        bail!("the key change job ended {}: {}", row["status"].as_str().unwrap_or("unexpectedly"), logs(&new))
                    }
                    Ok(_) => {}
                    // Not switched yet, or the job failed and put the old key back.
                    Err(_) => {
                        if let Ok(row) = status(&old) {
                            if !matches!(row["status"].as_str(), Some("queued" | "running" | "succeeded")) {
                                bail!("E_STILL_OLD_KEY: the key change job ended {}: {}", row["status"].as_str().unwrap_or("unexpectedly"), logs(&old));
                            }
                        }
                    }
                }
                std::thread::sleep(Duration::from_secs(1));
            }
            bail!("the key change job did not finish in time")
        }
    }
}

static RUNNING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
/// One pass in a background thread, unless one is already running. Jobs take seconds per host, so the window
/// polls `keys.rotation_status` instead of waiting on one long request.
pub fn pass_in_background(state: &Path, config: &Config) {
    use std::sync::atomic::Ordering;
    if RUNNING.swap(true, Ordering::SeqCst) {
        return;
    }
    let (state, config) = (state.to_path_buf(), config.clone());
    std::thread::spawn(move || {
        let result = pass(&state, &config, &mut |target, step| act(&state, &config, target, step));
        if let Err(error) = result {
            eprintln!("SSH Control Center: changing keys stopped: {error:#}");
        }
        RUNNING.store(false, Ordering::SeqCst);
    });
}

/// Service entry point, Owner-only and local (never routed through a bridge or relay).
pub fn handle(state: &Path, config: &Config, req: &Request) -> Result<Option<Value>> {
    Ok(Some(match req.method.as_str() {
        "keys.rotate" => {
            let started = start(state, config)?;
            pass_in_background(state, config);
            started
        }
        "keys.rotation_status" => {
            // A window watching the change also retries hosts that were offline.
            if retry_due(state)? {
                pass_in_background(state, config);
            }
            status(state, config)?
        }
        _ => return Ok(None),
    }))
}
