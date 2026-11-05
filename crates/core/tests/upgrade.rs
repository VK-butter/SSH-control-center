//! Updating Targets from the Master (roadmap entry 8). Templates are built here in the release layout; the steps
//! that talk to a host are a closure, and waits are passed in, so nothing uses SSH or the clock.
use base64::{engine::general_purpose::STANDARD, Engine};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use sshcc_core::{activity, config, fleet, protocol::Request, security, upgrade::{self, Step}};
use std::{
    cmp::Ordering,
    fs,
    io::Write,
    path::Path,
    sync::{
        atomic::{AtomicUsize, Ordering as Atomic},
        Mutex,
    },
    time::Duration,
};
use uuid::Uuid;

fn key(seed: u8) -> String {
    let mut wire = Vec::new();
    wire.extend_from_slice(&11u32.to_be_bytes());
    wire.extend_from_slice(b"ssh-ed25519");
    wire.extend_from_slice(&32u32.to_be_bytes());
    wire.extend_from_slice(&[seed; 32]);
    format!("ssh-ed25519 {}", STANDARD.encode(wire))
}
fn request(method: &str, params: Value) -> Request {
    Request { method: method.into(), params }
}
fn host(id: &str, n: u8, os: &str) -> Value {
    json!({"schemaVersion":2,"id":id,"nodeId":Uuid::new_v4().to_string(),"label":id,"tenant":"company","os":os,"user":"sshcc-bridge",
        "hostKeys":[key(1)],"routes":[{"kind":"lan","address":format!("10.0.0.{n}"),"port":22}],
        "bridgePath":if os == "windows" { "C:\\Program Files\\SSH Control Center\\sshcc.exe" } else { "/usr/bin/sshcc" },"runnerInstalled":true})
}
/// A `.deb` (ar archive) whose data.tar.gz holds `files`.
fn deb(files: &[(&str, &[u8])]) -> Vec<u8> {
    let mut tar = tar::Builder::new(flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast()));
    for (path, data) in files {
        let mut header = tar::Header::new_gnu();
        header.set_size(data.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        tar.append_data(&mut header, path, *data).unwrap();
    }
    let data = tar.into_inner().unwrap().finish().unwrap();
    let mut out = b"!<arch>\n".to_vec();
    for (name, body) in [("debian-binary", b"2.0\n".to_vec()), ("data.tar.gz", data)] {
        writeln!(out, "{:<16}{:<12}{:<6}{:<6}{:<8}{:<10}`", name, 0, 0, 0, 100644, body.len()).unwrap();
        out.extend_from_slice(&body);
        if body.len() % 2 == 1 {
            out.push(b'\n');
        }
    }
    out
}
const SERVICE: &[u8] = b"the sshcc service package";
const WINDOWS: &[u8] = b"MZ the windows template";
/// Release templates as a Master package installs them.
fn templates(dir: &Path) {
    fs::create_dir_all(dir).unwrap();
    let ubuntu = deb(&[("./usr/lib/sshcc-setup/assets/manifest.json", b"{}"), ("./usr/lib/sshcc-setup/assets/sshcc-service.deb", SERVICE)]);
    fs::write(dir.join("sshcc-client-ubuntu-template.deb"), &ubuntu).unwrap();
    fs::write(dir.join("sshcc-client-windows-template.exe"), WINDOWS).unwrap();
    let sha = |b: &[u8]| hex::encode(Sha256::digest(b));
    fs::write(dir.join("template-manifest.json"), json!({"schemaVersion":1,"templates":[
        {"os":"linux","fileName":"sshcc-client-ubuntu-template.deb","sha256":sha(&ubuntu)},
        {"os":"windows","fileName":"sshcc-client-windows-template.exe","sha256":sha(WINDOWS)}]}).to_string()).unwrap();
}
struct World {
    _dir: tempfile::TempDir,
    cfg: config::Config,
}
impl World {
    /// old-1 (0.4.0, Ubuntu), old-win (0.4.0, Windows), same (this version), newer (99.0.0), quiet (no version yet),
    /// and branch/child (merged, 0.4.0).
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let cfg = config::init(dir.path(), "master", &security::current_principal().unwrap(), "Office", &[]).unwrap();
        fs::create_dir_all(dir.path().join("keys")).unwrap();
        fs::write(dir.path().join("keys/company.ed25519"), "test key placeholder").unwrap();
        templates(&dir.path().join("templates"));
        for (i, (id, os)) in [("old-1", "linux"), ("old-win", "windows"), ("same", "linux"), ("newer", "linux"), ("quiet", "linux"), ("branch", "linux")].iter().enumerate() {
            fleet::handle(dir.path(), &cfg, &request("targets.import", json!({"registration":host(id, i as u8 + 1, os)}))).unwrap();
        }
        let peer = fleet::handle(dir.path(), &cfg, &request("targets.list", json!({}))).unwrap().unwrap().as_array().unwrap().iter().find(|h| h["id"] == "branch").unwrap()["nodeId"].clone();
        fleet::record_merge(dir.path(), "branch", &json!({"nodeId":peer,"label":"Branch","targets":[{"id":"child","label":"child","tenant":"company","os":"linux","nodeId":Uuid::new_v4().to_string()}]})).unwrap();
        let db = rusqlite::Connection::open(dir.path().join("fleet.sqlite")).unwrap();
        for (id, version) in [("old-1", "0.4.0"), ("old-win", "0.4.0"), ("same", upgrade::ours()), ("newer", "99.0.0"), ("branch", upgrade::ours()), ("branch/child", "0.4.0")] {
            db.execute("INSERT OR REPLACE INTO health(target_id,document) VALUES(?1,?2)", rusqlite::params![id, json!({"targetId":id,"online":true,"version":version}).to_string()]).unwrap();
        }
        Self { _dir: dir, cfg }
    }
    fn state(&self) -> &Path {
        self._dir.path()
    }
    fn templates(&self) -> std::path::PathBuf {
        self.state().join("templates")
    }
}
fn row(rows: &[Value], id: &str) -> Value {
    rows.iter().find(|r| r["targetId"] == id).unwrap().clone()
}
/// A fake host: records each step, and reports `version` after the installer was started.
struct Host {
    steps: Mutex<Vec<(String, String)>>,
    uploaded: Mutex<Vec<u8>>,
    after: Mutex<String>,
}
impl Host {
    fn new(after: &str) -> Self {
        Self { steps: Mutex::new(Vec::new()), uploaded: Mutex::new(Vec::new()), after: Mutex::new(after.into()) }
    }
    fn act(&self, host: &Value, step: Step) -> anyhow::Result<Value> {
        let id = host["id"].as_str().unwrap().to_owned();
        match step {
            Step::Upload { path, bytes } => {
                self.steps.lock().unwrap().push((id, format!("upload {path}")));
                *self.uploaded.lock().unwrap() = bytes.to_vec();
                Ok(json!({}))
            }
            Step::Run { script, windows } => {
                self.steps.lock().unwrap().push((id, format!("run windows={windows} {script}")));
                Ok(json!({"status":"succeeded"}))
            }
            Step::Version => Ok(json!(self.after.lock().unwrap().clone())),
        }
    }
}

#[test]
fn versions_compare_as_numbers_and_a_pre_release_is_older() {
    assert_eq!(upgrade::compare("0.4.0", "0.5.0"), Ordering::Less);
    assert_eq!(upgrade::compare("0.10.0", "0.9.9"), Ordering::Greater);
    assert_eq!(upgrade::compare("v0.5.0", "0.5.0"), Ordering::Equal);
    assert_eq!(upgrade::compare("0.5.0-rc.1", "0.5.0"), Ordering::Less);
    assert_eq!(upgrade::compare("0.5", "0.5.0"), Ordering::Equal);
    assert_eq!(upgrade::ours(), "0.5.2", "this release ships 0.5.2");
}

#[test]
fn each_host_says_whether_this_master_can_update_it_and_why_not() {
    let w = World::new();
    let rows = upgrade::check(w.state(), &w.cfg, &w.templates()).unwrap();
    let says = |id: &str| { let r = row(&rows, id); (r["outdated"].as_bool().unwrap(), r["canUpdate"].as_bool().unwrap(), r["reason"].as_str().map(str::to_owned)) };
    assert_eq!(says("old-1"), (true, true, None));
    assert_eq!(says("old-win"), (true, true, None));
    assert_eq!(says("same"), (false, false, Some("current".into())));
    assert_eq!(says("newer"), (false, false, Some("current".into())), "never a downgrade");
    assert_eq!(says("quiet"), (false, false, Some("unknown".into())));
    assert_eq!(says("branch/child"), (true, false, Some("by:Branch".into())), "its own Master updates it");
    // Without installer templates (a development build): outdated, but nothing to send.
    let none = upgrade::check(w.state(), &w.cfg, &w.state().join("missing")).unwrap();
    assert_eq!(row(&none, "old-1")["reason"], "E_UPGRADE_TEMPLATES");
}

#[test]
fn an_ubuntu_host_gets_the_service_package_started_detached_then_answers_the_new_version() {
    let w = World::new();
    let fake = Host::new(upgrade::ours());
    let seen = Mutex::new(Vec::new());
    let done = upgrade::one(w.state(), &w.cfg, &w.templates(), "old-1", &|h, s| fake.act(h, s), &|p| seen.lock().unwrap().push(p.to_owned()), Duration::from_secs(5), Duration::ZERO).unwrap();
    assert_eq!((done["from"].as_str(), done["version"].as_str()), (Some("0.4.0"), Some(upgrade::ours())));
    assert_eq!(*seen.lock().unwrap(), ["uploading", "installing", "checking"]);
    assert_eq!(fake.uploaded.lock().unwrap().as_slice(), SERVICE, "the service package inside the template, not the pairing installer");
    let steps = fake.steps.lock().unwrap();
    assert_eq!(steps[0].1, "upload /tmp/sshcc-update-old-1.deb");
    assert!(steps[1].1.starts_with("run windows=false ") && steps[1].1.contains("systemd-run --unit 'sshcc-update-old-1' --collect") && steps[1].1.contains("dpkg -i"), "{}", steps[1].1);
}

#[test]
fn a_windows_host_gets_the_template_run_by_a_one_off_system_task() {
    let w = World::new();
    let fake = Host::new(upgrade::ours());
    upgrade::one(w.state(), &w.cfg, &w.templates(), "old-win", &|h, s| fake.act(h, s), &|_| {}, Duration::from_secs(5), Duration::ZERO).unwrap();
    assert_eq!(fake.uploaded.lock().unwrap().as_slice(), WINDOWS);
    let steps = fake.steps.lock().unwrap();
    assert_eq!(steps[0].1, "upload C:\\Windows\\Temp\\sshcc-update-old-win.exe");
    let script = &steps[1].1;
    assert!(script.starts_with("run windows=true ") && script.contains("Register-ScheduledTask") && script.contains("-User 'SYSTEM'") && script.contains("/UPDATE"), "{script}");
}

#[test]
fn a_host_that_doesnt_come_back_new_fails_with_the_reason_and_refusals_are_plain() {
    let w = World::new();
    let stuck = Host::new("0.4.0");
    let error = upgrade::one(w.state(), &w.cfg, &w.templates(), "old-1", &|h, s| stuck.act(h, s), &|_| {}, Duration::from_millis(30), Duration::from_millis(5)).unwrap_err();
    assert!(format!("{error:#}").contains("E_UPGRADE_TIMEOUT"));
    let fake = Host::new(upgrade::ours());
    let refuse = |id: &str, templates: &Path| format!("{:#}", upgrade::one(w.state(), &w.cfg, templates, id, &|h, s| fake.act(h, s), &|_| {}, Duration::ZERO, Duration::ZERO).unwrap_err());
    assert!(refuse("branch/child", &w.templates()).contains("E_UPGRADE_NOT_HERE: branch/child is updated by Branch"));
    assert!(refuse("newer", &w.templates()).contains("E_UPGRADE_CURRENT"));
    assert!(refuse("old-1", &w.state().join("missing")).contains("E_UPGRADE_TEMPLATES"));
    assert!(fake.steps.lock().unwrap().is_empty(), "nothing was sent to a refused host");
    for method in ["targets.upgrade", "targets.upgrade_all"] {
        assert!(activity::audited(method, &json!({})), "{method} is audited");
    }
}

#[test]
fn every_outdated_host_is_updated_one_at_a_time_and_a_failure_moves_on() {
    let w = World::new();
    let busy = AtomicUsize::new(0);
    let most = AtomicUsize::new(0);
    let order = Mutex::new(Vec::new());
    let act = |h: &Value, s: Step| -> anyhow::Result<Value> {
        let id = h["id"].as_str().unwrap().to_owned();
        match s {
            Step::Upload { .. } => {
                let now = busy.fetch_add(1, Atomic::SeqCst) + 1;
                most.fetch_max(now, Atomic::SeqCst);
                order.lock().unwrap().push(id.clone());
                anyhow::ensure!(id != "old-1", "E_FILE: no space left on device");
                Ok(json!({}))
            }
            Step::Run { .. } => Ok(json!({})),
            Step::Version => { busy.fetch_sub(1, Atomic::SeqCst); Ok(json!(upgrade::ours())) }
        }
    };
    let result = upgrade::all(w.state(), &w.cfg, &w.templates(), &|h, s| {
        let failed_upload = matches!(s, Step::Upload { .. }) && h["id"] == "old-1";
        let out = act(h, s);
        if failed_upload { busy.fetch_sub(1, Atomic::SeqCst); }
        out
    }, Duration::from_secs(5), Duration::ZERO).unwrap();
    assert_eq!(most.load(Atomic::SeqCst), 1, "never two at once");
    assert_eq!(*order.lock().unwrap(), ["old-1", "old-win"], "only the hosts this Master can update");
    let hosts = result["hosts"].as_array().unwrap();
    assert_eq!((hosts[0]["state"].as_str(), hosts[1]["state"].as_str()), (Some("failed"), Some("done")));
    assert!(hosts[0]["error"].as_str().unwrap().contains("no space left"));
    assert_eq!(result["done"], true);
    assert_eq!(upgrade::status()["done"], true);
}
