//! Updating Targets from the Master (roadmap entry 8). A host older than this Master can be updated with the
//! installer this Master carries: the Master uploads it, starts it detached from the host's service (so the
//! service restarting mid-way doesn't stop it), and waits for the new version to answer. Only this Master's own
//! (direct) hosts, never a downgrade. "Update every outdated host" runs one host at a time. The steps that talk to
//! the host are a closure (`Step`), so the orchestration is tested without SSH.
use crate::{config::Config, fleet, protocol::Request, setup_package};
use anyhow::{bail, ensure, Context, Result};
use serde_json::{json, Value};
use std::{
    cmp::Ordering,
    collections::BTreeMap,
    path::Path,
    sync::{Mutex, PoisonError},
    time::{Duration, Instant},
};

/// How long a host may take to answer with the new version.
pub const WAIT: Duration = Duration::from_secs(180);
const POLL: Duration = Duration::from_secs(5);
/// Uploads go in pieces small enough for one request frame.
const CHUNK: usize = 1024 * 1024;

/// Semantic versions, numerically; a pre-release (`0.5.0-rc.1`) is older than its release.
pub fn compare(a: &str, b: &str) -> Ordering {
    let parse = |v: &str| {
        let v = v.trim().trim_start_matches('v');
        let (core, pre) = v.split_once('-').map_or((v, None), |(c, p)| (c, Some(p.to_owned())));
        let nums: Vec<u64> = core.split('.').map(|n| n.parse().unwrap_or(0)).collect();
        (nums, pre)
    };
    let (an, ap) = parse(a);
    let (bn, bp) = parse(b);
    for i in 0..an.len().max(bn.len()) {
        match an.get(i).unwrap_or(&0).cmp(bn.get(i).unwrap_or(&0)) {
            Ordering::Equal => {}
            other => return other,
        }
    }
    match (ap, bp) {
        (None, None) => Ordering::Equal,
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (Some(x), Some(y)) => x.cmp(&y),
    }
}

/// This Master's version (its own binary).
pub fn ours() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// Each host with the version it last reported, whether it is older than this Master, and whether this Master can
/// update it (otherwise why: by its own Master, newer, unknown version, no installer templates).
pub fn check(state: &Path, config: &Config, templates: &Path) -> Result<Vec<Value>> {
    let hosts = fleet::handle(state, config, &Request { method: "targets.list".into(), params: json!({}) })?.context("targets unavailable")?;
    let versions = reported(state)?;
    // Each template is read and checked once per call, not once per host.
    let cache: Mutex<BTreeMap<String, bool>> = Mutex::new(BTreeMap::new());
    let have = |os: &str| *cache.lock().unwrap_or_else(PoisonError::into_inner).entry(os.to_owned())
        .or_insert_with(|| setup_package::template(templates, os).is_ok());
    let mut rows = Vec::new();
    for host in hosts.as_array().into_iter().flatten() {
        let id = host["id"].as_str().unwrap_or_default();
        let os = host["os"].as_str().unwrap_or("linux");
        let version = versions.get(id).cloned();
        let outdated = version.as_deref().is_some_and(|v| compare(v, ours()) == Ordering::Less);
        let reason = if host["runnerInstalled"] == false {
            Some("legacy".to_owned())
        } else if host["relayed"] == true {
            // The Master that owns it: the first on its path (its merged Master's own name).
            Some(format!("by:{}", host["path"][0].as_str().or(host["viaLabel"].as_str()).unwrap_or_default()))
        } else if version.is_none() {
            Some("unknown".into())
        } else if !outdated {
            Some("current".into())
        } else if !have(os) {
            Some("E_UPGRADE_TEMPLATES".into())
        } else {
            None
        };
        rows.push(json!({"targetId":id,"label":host["label"],"os":os,"version":version,"latest":ours(),"outdated":outdated,
            "canUpdate":reason.is_none(),"reason":reason}));
    }
    Ok(rows)
}
/// The version each host last reported to the health check.
fn reported(state: &Path) -> Result<BTreeMap<String, String>> {
    let db = fleet::database(state)?;
    let mut stmt = db.prepare("SELECT target_id,document FROM health")?;
    let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
    Ok(rows.filter_map(|row| row.ok())
        .filter_map(|(id, doc)| Some((id, serde_json::from_str::<Value>(&doc).ok()?["version"].as_str()?.to_owned())))
        .collect())
}

/// What an update asks the host to do.
pub enum Step<'a> {
    /// Write the installer to `path` on the host.
    Upload { path: &'a str, bytes: &'a [u8] },
    /// Run `script` as a job (with admin rights on Linux).
    Run { script: &'a str, windows: bool },
    /// The version the host's service reports now.
    Version,
}
pub type Act<'a> = dyn Fn(&Value, Step) -> Result<Value> + Sync + 'a;

/// The installer for one host's OS, and where it goes on the host.
pub fn package(templates: &Path, os: &str, id: &str) -> Result<(Vec<u8>, String)> {
    let template = setup_package::template(templates, os).map_err(|e| anyhow::anyhow!("E_UPGRADE_TEMPLATES: {e:#}"))?;
    let tag: String = id.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-').take(40).collect();
    Ok(if os == "windows" {
        (template, format!("C:\\Windows\\Temp\\sshcc-update-{tag}.exe"))
    } else {
        (setup_package::service_package(&template)?, format!("/tmp/sshcc-update-{tag}.deb"))
    })
}
/// The job that starts the installer detached from the host's service: a transient systemd unit on Linux, a
/// one-off SYSTEM task on Windows. Both remove the uploaded file when the installer is done.
pub fn script(os: &str, path: &str, id: &str) -> String {
    let unit: String = id.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-').take(40).collect();
    if os == "windows" {
        let quoted = path.replace('\'', "''");
        format!(
            "$ErrorActionPreference='Stop'; $task='sshcc-update-{unit}'; $exe='{quoted}'; \
             $action=New-ScheduledTaskAction -Execute 'cmd.exe' -Argument ('/c \"\"' + $exe + '\" /UPDATE & del /f /q \"' + $exe + '\" & schtasks /Delete /TN ' + $task + ' /F\"'); \
             Register-ScheduledTask -TaskName $task -Action $action -User 'SYSTEM' -RunLevel Highest -Force | Out-Null; \
             Start-ScheduledTask -TaskName $task; 'started'"
        )
    } else {
        let quoted = path.replace('\'', "'\\''");
        format!(
            "set -e; systemd-run --unit 'sshcc-update-{unit}' --collect --quiet sh -c 'sleep 2; dpkg -i \"$0\"; rm -f \"$0\"' '{quoted}'; echo started"
        )
    }
}

/// Updates one host: refuses what can't be updated, uploads, starts the installer detached, then waits for the new
/// version. `wait` and `poll` are passed in so tests don't wait.
#[allow(clippy::too_many_arguments)] // wait/poll are separate so tests can shrink them
pub fn one(state: &Path, config: &Config, templates: &Path, id: &str, act: &Act, progress: &dyn Fn(&str), wait: Duration, poll: Duration) -> Result<Value> {
    let row = check(state, config, templates)?.into_iter().find(|r| r["targetId"] == id).with_context(|| format!("E_TARGET: unknown host {id}"))?;
    match row["reason"].as_str() {
        None => {}
        Some(reason) if reason.starts_with("by:") => bail!("E_UPGRADE_NOT_HERE: {} is updated by {}", id, &reason[3..]),
        Some("current") => bail!("E_UPGRADE_CURRENT: {id} already runs {} (this Master runs {})", row["version"].as_str().unwrap_or("?"), ours()),
        Some("E_UPGRADE_TEMPLATES") => bail!("E_UPGRADE_TEMPLATES: this Master has no installer to send (a development build?)"),
        Some(other) => bail!("E_UPGRADE_UNKNOWN: {id} can't be updated from here ({other})"),
    }
    let os = row["os"].as_str().unwrap_or("linux");
    let host = json!({"id":id,"os":os,"label":row["label"]});
    let (bytes, path) = package(templates, os, id)?;
    progress("uploading");
    act(&host, Step::Upload { path: &path, bytes: &bytes })?;
    progress("installing");
    act(&host, Step::Run { script: &script(os, &path, id), windows: os == "windows" })?;
    progress("checking");
    let started = Instant::now();
    loop {
        // The service restarts during the update: a failed answer means "not back yet".
        if let Ok(version) = act(&host, Step::Version) {
            if version.as_str().is_some_and(|v| compare(v, ours()) != Ordering::Less) {
                return Ok(json!({"targetId":id,"version":version,"from":row["version"]}));
            }
        }
        ensure!(started.elapsed() < wait, "E_UPGRADE_TIMEOUT: {id} didn't come back with {} within {} minutes", ours(), wait.as_secs() / 60);
        std::thread::sleep(poll);
    }
}

/// Progress of the last "Update every outdated host": one row per host, in order.
static RUN: Mutex<Option<Value>> = Mutex::new(None);
pub fn status() -> Value {
    RUN.lock().unwrap_or_else(PoisonError::into_inner).clone().unwrap_or(json!({"hosts":[],"done":true}))
}
fn set_row(id: &str, change: impl FnOnce(&mut Value)) {
    let mut run = RUN.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(row) = run.as_mut().and_then(|r| r["hosts"].as_array_mut()).and_then(|h| h.iter_mut().find(|r| r["targetId"] == id)) {
        change(row);
    }
}
/// Every outdated host this Master can update, one at a time; a failure moves on. Returns the final progress.
pub fn all(state: &Path, config: &Config, templates: &Path, act: &Act, wait: Duration, poll: Duration) -> Result<Value> {
    let ids: Vec<String> = check(state, config, templates)?.into_iter().filter(|r| r["canUpdate"] == true)
        .filter_map(|r| r["targetId"].as_str().map(str::to_owned)).collect();
    {
        let mut run = RUN.lock().unwrap_or_else(PoisonError::into_inner);
        ensure!(run.as_ref().is_none_or(|r| r["done"] == true), "E_UPGRADE_BUSY: an update of every outdated host is already running");
        *run = Some(json!({"done":false,"hosts":ids.iter().map(|id| json!({"targetId":id,"state":"waiting","error":null})).collect::<Vec<_>>()}));
    }
    for id in &ids {
        let result = one(state, config, templates, id, act, &|step| set_row(id, |row| row["state"] = json!(step)), wait, poll);
        set_row(id, |row| match result {
            Ok(done) => { row["state"] = json!("done"); row["version"] = done["version"].clone(); }
            Err(error) => { row["state"] = json!("failed"); row["error"] = json!(format!("{error:#}")); }
        });
    }
    let mut run = RUN.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(r) = run.as_mut() {
        r["done"] = json!(true);
    }
    Ok(run.clone().unwrap_or_default())
}

/// The real host steps: the Master's own fleet methods.
pub fn real_act(state: &Path, config: &Config, host: &Value, step: Step) -> Result<Value> {
    use base64::{engine::general_purpose::STANDARD, Engine};
    let id = host["id"].as_str().unwrap_or_default();
    let call = |method: &str, params: Value| fleet::handle(state, config, &Request { method: method.into(), params })?.context("fleet unavailable");
    match step {
        Step::Upload { path, bytes } => {
            let upload = uuid::Uuid::new_v4().simple().to_string();
            let mut offset = 0;
            for chunk in bytes.chunks(CHUNK) {
                call("files.upload", json!({"targetId":id,"path":path,"uploadId":upload,"offset":offset,"contentBase64":STANDARD.encode(chunk)}))?;
                offset += chunk.len();
            }
            call("files.upload", json!({"targetId":id,"path":path,"uploadId":upload,"offset":offset,"contentBase64":"","done":true}))
        }
        Step::Run { script, windows } => {
            let job = uuid::Uuid::new_v4().to_string();
            let mut params = json!({"targetId":id,"jobId":job,"script":script,"cwd":if windows { "C:\\" } else { "/" }});
            if !windows {
                params["asAdmin"] = json!(true);
            }
            call("jobs.start", params)?;
            for _ in 0..60 {
                let status = call("jobs.status", json!({"targetId":id,"jobId":job}))?;
                match status["status"].as_str().unwrap_or_default() {
                    "succeeded" => return Ok(status),
                    "failed" | "cancelled" | "interrupted" => {
                        let err = call("jobs.logs", json!({"targetId":id,"jobId":job,"stream":"stderr","offset":0,"limit":4096})).ok();
                        bail!("the update couldn't be started: {}", err.and_then(|e| e["content"].as_str().map(str::to_owned)).unwrap_or_default().trim())
                    }
                    _ => std::thread::sleep(Duration::from_secs(1)),
                }
            }
            bail!("the update couldn't be started in time")
        }
        Step::Version => fleet::system_status(state, config, id)?["version"].as_str().map(|v| json!(v)).context("the host didn't report its version"),
    }
}

pub fn handle(state: &Path, config: &Config, req: &Request) -> Result<Option<Value>> {
    if !matches!(req.method.as_str(), "targets.update_check" | "targets.upgrade" | "targets.upgrade_all" | "targets.upgrade_status") {
        return Ok(None);
    }
    ensure!(config.roles.iter().any(|r| r == "master"), "E_ROLE: master role is required");
    let templates = setup_package::default_templates();
    Ok(Some(match req.method.as_str() {
        "targets.update_check" => json!({"latest":ours(),"hosts":check(state, config, &templates)?}),
        "targets.upgrade" => {
            let id = req.params["targetId"].as_str().context("E_ARGUMENT: missing targetId")?;
            let act = |h: &Value, s: Step| real_act(state, config, h, s);
            one(state, config, &templates, id, &act, &|_| {}, WAIT, POLL)?
        }
        "targets.upgrade_all" => {
            ensure!(status()["done"] == true, "E_UPGRADE_BUSY: an update of every outdated host is already running");
            let (state, config) = (state.to_path_buf(), config.clone());
            std::thread::spawn(move || {
                let act = |h: &Value, s: Step| real_act(&state, &config, h, s);
                if let Err(error) = all(&state, &config, &templates, &act, WAIT, POLL) {
                    eprintln!("SSH Control Center: updating every outdated host failed: {error:#}");
                }
            });
            json!({"started":true})
        }
        _ => status(),
    }))
}
