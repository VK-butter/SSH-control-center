//! Group runs (roadmap entry 4): one job on every host with a Tag or in a folder, one result per host. The step that
//! talks to a host is a closure here, so these tests check the bookkeeping without SSH.
use base64::{engine::general_purpose::STANDARD, Engine};
use serde_json::{json, Value};
use sshcc_core::{
    config, fleet, folders,
    groups::{self, Step},
    protocol::Request,
    security, service::Service, tags,
};
use std::{
    fs,
    path::Path,
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc, Mutex,
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
fn host(id: &str, address: &str) -> Value {
    json!({"schemaVersion":2,"id":id,"nodeId":Uuid::new_v4().to_string(),"label":id,"tenant":"company","os":"linux",
        "user":"sshcc-bridge","hostKeys":[key(1)],"routes":[{"kind":"lan","address":address,"port":22}],
        "bridgePath":"/usr/bin/sshcc","runnerInstalled":true})
}
fn fleet_call(state: &Path, cfg: &config::Config, method: &str, params: Value) -> Value {
    fleet::handle(state, cfg, &Request { method: method.into(), params }).unwrap().unwrap()
}
/// web-1 … web-<n>, a v1 host without the service, and a merged Windows host `branch/child`, all tagged `web`.
fn master(dir: &Path, n: usize) -> config::Config {
    let cfg = config::init(dir, "both", &security::current_principal().unwrap(), "Office", &[]).unwrap();
    fs::create_dir_all(dir.join("keys")).unwrap();
    fs::write(dir.join("keys/company.ed25519"), "test key placeholder").unwrap();
    for i in 1..=n {
        fleet_call(dir, &cfg, "targets.import", json!({"registration":host(&format!("web-{i}"), &format!("10.0.1.{i}"))}));
    }
    fleet_call(dir, &cfg, "targets.import", json!({"registration":host("branch", "10.0.0.3")}));
    fleet_call(dir, &cfg, "targets.import_legacy", json!({"document":{"schemaVersion":1,"id":"old-box","hostname":"old-box","mode":"company",
        "tenant":"company","address":"10.0.0.8","port":22,"user":"sshcc-bridge","os":"linux","hostKeys":[key(1)],"shareRequired":false}}));
    let node = fleet_call(dir, &cfg, "targets.list", json!({})).as_array().unwrap().iter().find(|row| row["id"] == "branch").unwrap()["nodeId"].clone();
    fleet::record_merge(dir, "branch", &json!({"nodeId":node,"label":"Branch","targets":[
        {"id":"child","label":"Child","tenant":"lab","os":"windows","nodeId":Uuid::new_v4().to_string()}]})).unwrap();
    for i in 1..=n {
        tags::set(dir, &cfg, &format!("web-{i}"), &["web".into()]).unwrap();
    }
    tags::set(dir, &cfg, "old-box", &["web".into()]).unwrap();
    tags::set(dir, &cfg, "branch/child", &["Web".into()]).unwrap();
    cfg
}
fn ids(rows: &Value) -> Vec<String> {
    let mut ids: Vec<String> = rows.as_array().unwrap().iter().map(|row| row["targetId"].as_str().unwrap().to_owned()).collect();
    ids.sort();
    ids
}
fn host_of<'a>(run: &'a Value, id: &str) -> &'a Value {
    run["hosts"].as_array().unwrap().iter().find(|h| h["targetId"] == id).unwrap()
}
fn by_tag(script: &str) -> Value {
    json!({"selector":{"tag":"WEB"},"scripts":{"linux":script,"windows":"Get-Date"}})
}

#[test]
fn preview_lists_who_runs_and_who_is_skipped() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = master(dir.path(), 2);
    let preview = groups::preview(dir.path(), &cfg, &json!({"tag":"web"})).unwrap();
    assert_eq!(ids(&preview["hosts"]), ["branch/child", "web-1", "web-2"], "matched without case, merged host included");
    assert_eq!(ids(&preview["skipped"]), ["old-box"]);
    assert!(preview["skipped"][0]["reason"].as_str().unwrap().contains("E_RUNNER_NOT_INSTALLED"));
    // By folder, a merged Master's folder included.
    let folder = folders::handle(dir.path(), &cfg, &Request { method: "folders.create".into(), params: json!({"name":"Far","master":"branch"}) }).unwrap().unwrap();
    folders::handle(dir.path(), &cfg, &Request { method: "folders.assign".into(), params: json!({"targetId":"branch/child","folderId":folder["id"]}) }).unwrap();
    let preview = groups::preview(dir.path(), &cfg, &json!({"folder":folder["id"]})).unwrap();
    assert_eq!(ids(&preview["hosts"]), ["branch/child"]);
    assert!(groups::preview(dir.path(), &cfg, &json!({"tag":"nobody"})).unwrap()["hosts"].as_array().unwrap().is_empty());
}

#[test]
fn a_run_starts_one_job_per_host_and_reads_each_result() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = master(dir.path(), 2);
    let run = groups::start(dir.path(), &cfg, &by_tag("uptime")).unwrap();
    let id = run["id"].as_str().unwrap().to_owned();
    assert_eq!(host_of(&run, "web-1")["state"], "queued");
    assert_eq!(host_of(&run, "old-box")["state"], "skipped");
    let started = Mutex::new(Vec::new());
    groups::drive(dir.path(), &cfg, &id, &|member, step| match step {
        Step::Start { script, cwd, .. } => {
            started.lock().unwrap().push((member.id.clone(), script.to_owned(), cwd.to_owned()));
            if member.id == "web-2" { anyhow::bail!("ALL_ROUTES_FAILED: lan 10.0.1.2:22: No route to host") }
            Ok(json!({}))
        }
        _ => unreachable!(),
    }).unwrap();
    let mut started = started.into_inner().unwrap();
    started.sort();
    assert_eq!(started, [
        ("branch/child".to_owned(), "Get-Date".to_owned(), "C:\\".to_owned()),
        ("web-1".to_owned(), "uptime".to_owned(), "/".to_owned()),
        ("web-2".to_owned(), "uptime".to_owned(), "/".to_owned()),
    ], "each OS gets its script and its default working folder");
    // Status is read per host; an unreachable host is not tried again.
    let status = groups::refresh(dir.path(), &cfg, &id, &|member, step| match step {
        Step::Status { job } => {
            assert_ne!(member.id, "web-2", "no status for a host that never started");
            assert!(Uuid::parse_str(job).is_ok());
            Ok(if member.id == "web-1" { json!({"status":"succeeded","exitCode":0}) } else { json!({"status":"running","exitCode":null}) })
        }
        _ => unreachable!(),
    }).unwrap();
    assert_eq!((host_of(&status, "web-1")["status"].as_str(), host_of(&status, "web-1")["exitCode"].as_i64()), (Some("succeeded"), Some(0)));
    assert_eq!(host_of(&status, "branch/child")["status"], "running");
    assert_eq!(host_of(&status, "web-2")["state"], "unreachable");
    assert!(host_of(&status, "web-2")["error"].as_str().unwrap().contains("No route to host"));
    assert_eq!(status["done"], false);
    // Finished hosts are not asked again.
    let again = groups::refresh(dir.path(), &cfg, &id, &|member, _| {
        assert_eq!(member.id, "branch/child");
        Ok(json!({"status":"failed","exitCode":2}))
    }).unwrap();
    assert_eq!(again["done"], true);
}

#[test]
fn at_most_eight_hosts_start_at_a_time() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = master(dir.path(), 12);
    let run = groups::start(dir.path(), &cfg, &by_tag("true")).unwrap();
    let (now, most, total) = (AtomicUsize::new(0), AtomicUsize::new(0), AtomicUsize::new(0));
    groups::drive(dir.path(), &cfg, run["id"].as_str().unwrap(), &|_, _| {
        let running = now.fetch_add(1, Ordering::SeqCst) + 1;
        most.fetch_max(running, Ordering::SeqCst);
        std::thread::sleep(Duration::from_millis(40));
        now.fetch_sub(1, Ordering::SeqCst);
        total.fetch_add(1, Ordering::SeqCst);
        Ok(json!({}))
    }).unwrap();
    assert_eq!(total.load(Ordering::SeqCst), 13);
    assert!(most.load(Ordering::SeqCst) <= groups::AT_ONCE && most.load(Ordering::SeqCst) > 1, "{} at once", most.load(Ordering::SeqCst));
    let status = groups::status(dir.path(), &cfg, &json!({"runId":run["id"]})).unwrap();
    assert!(status["hosts"].as_array().unwrap().iter().filter(|h| h["state"] != "skipped").all(|h| h["state"] == "started"));
}

#[test]
fn only_the_twenty_newest_runs_are_kept() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = master(dir.path(), 1);
    let first = groups::start(dir.path(), &cfg, &by_tag("echo 0")).unwrap();
    for n in 1..=20 {
        groups::start(dir.path(), &cfg, &by_tag(&format!("echo {n}"))).unwrap();
    }
    let recent = groups::status(dir.path(), &cfg, &json!({})).unwrap();
    assert_eq!(recent["runs"].as_array().unwrap().len(), groups::KEEP);
    assert_eq!(recent["runs"][0]["scripts"]["linux"], "echo 20", "newest first");
    assert!(groups::status(dir.path(), &cfg, &json!({"runId":first["id"]})).unwrap_err().to_string().contains("E_NOT_FOUND"));
}

#[test]
fn group_runs_never_cross_a_bridge_or_relay() {
    let dir = tempfile::tempdir().unwrap();
    let owner = security::current_principal().unwrap();
    config::init(dir.path(), "both", &owner, "Test", &[]).unwrap();
    let service = Service::new(dir.path().into(), Arc::new(AtomicBool::new(false))).unwrap();
    let peer = Uuid::new_v4().to_string();
    fleet::register_master(dir.path(), &json!({"id":peer,"label":"Peer","publicKey":key(3)})).unwrap();
    let fingerprint = fleet::public_key_fingerprint(&key(3)).unwrap();
    for method in ["groups.preview", "groups.run", "groups.status", "groups.cancel", "groups.retry"] {
        let bridged = Request { method: "bridge.request".into(), params: json!({"masterId":peer,"masterKey":fingerprint,
            "expectedTargetId":service.config.node_id,"request":{"method":method,"params":{}}}) };
        assert!(format!("{:#}", service.handle(&bridged, &owner).unwrap_err()).contains("not available over bridge"), "{method} over a bridge");
        let relayed = fleet::relay_request(dir.path(), &service.config, &peer, &json!({"targetId":"child","method":method,"params":{}}));
        assert!(format!("{:#}", relayed.unwrap_err()).contains("E_PERMISSION"), "{method} over a relay");
    }
    assert_eq!(service.handle(&Request { method: "groups.status".into(), params: json!({}) }, &owner).unwrap()["runs"], json!([]));
}

#[test]
fn a_missing_script_for_an_os_in_the_group_starts_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = master(dir.path(), 1);
    let error = groups::start(dir.path(), &cfg, &json!({"selector":{"tag":"web"},"scripts":{"linux":"uptime"}})).unwrap_err();
    assert!(format!("{error:#}").contains("E_GROUP_SCRIPT"), "branch/child is Windows");
    assert_eq!(groups::status(dir.path(), &cfg, &json!({})).unwrap()["runs"], json!([]), "no run recorded");
}

#[test]
fn admin_and_environment_reach_every_job() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = master(dir.path(), 1);
    let run = groups::start(dir.path(), &cfg, &json!({"selector":{"tag":"web"},"scripts":{"linux":"id","windows":"whoami"},
        "asAdmin":true,"env":{"STAGE":"prod"},"cwd":{"linux":"/srv","windows":"D:\\apps"}})).unwrap();
    let seen = Mutex::new(Vec::new());
    groups::drive(dir.path(), &cfg, run["id"].as_str().unwrap(), &|member, step| {
        if let Step::Start { as_admin, env, cwd, .. } = step {
            seen.lock().unwrap().push((member.id.clone(), as_admin, env["STAGE"].as_str().unwrap().to_owned(), cwd.to_owned()));
        }
        Ok(json!({}))
    }).unwrap();
    let mut seen = seen.into_inner().unwrap();
    seen.sort();
    assert_eq!(seen, [
        ("branch/child".into(), false, "prod".into(), "D:\\apps".into()),
        ("web-1".into(), true, "prod".into(), "/srv".into()),
    ], "admin is a Linux option; the working folder is per OS");
}

fn started(dir: &Path, cfg: &config::Config, n: usize) -> String {
    let run = groups::start(dir, cfg, &by_tag("sleep 60")).unwrap();
    let id = run["id"].as_str().unwrap().to_owned();
    groups::drive(dir, cfg, &id, &|member, _| if member.id == format!("web-{n}") { anyhow::bail!("No route to host") } else { Ok(json!({})) }).unwrap();
    id
}

#[test]
fn cancel_all_or_one_host() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = master(dir.path(), 3);
    let id = started(dir.path(), &cfg, 3);
    let cancelled = Mutex::new(Vec::new());
    let cancel = |member: &groups::Member, step: Step| match step {
        Step::Cancel { .. } => { cancelled.lock().unwrap().push(member.id.clone()); Ok(json!({"status":"cancelled","exitCode":null})) }
        _ => unreachable!(),
    };
    let one = groups::cancel(dir.path(), &cfg, &id, Some("web-1"), &cancel).unwrap();
    assert_eq!(host_of(&one, "web-1")["status"], "cancelled");
    assert_eq!(*cancelled.lock().unwrap(), ["web-1"]);
    groups::cancel(dir.path(), &cfg, &id, None, &cancel).unwrap();
    let mut all = cancelled.into_inner().unwrap();
    all.sort();
    assert_eq!(all, ["branch/child", "web-1", "web-2"], "web-1 was already cancelled; web-3 never started");
}

#[test]
fn retry_starts_only_the_hosts_that_failed() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = master(dir.path(), 3);
    let id = started(dir.path(), &cfg, 3);
    groups::refresh(dir.path(), &cfg, &id, &|member, _| Ok(match member.id.as_str() {
        "web-1" => json!({"status":"succeeded","exitCode":0}),
        "web-2" => json!({"status":"failed","exitCode":1}),
        _ => json!({"status":"succeeded","exitCode":0}),
    })).unwrap();
    let before = groups::status(dir.path(), &cfg, &json!({"runId":id})).unwrap();
    let old_job = host_of(&before, "web-2")["jobId"].clone();
    groups::retry(dir.path(), &cfg, &id).unwrap();
    let again = Mutex::new(Vec::new());
    groups::drive(dir.path(), &cfg, &id, &|member, step| {
        if let Step::Start { job, .. } = step { again.lock().unwrap().push((member.id.clone(), job.to_owned())); }
        Ok(json!({}))
    }).unwrap();
    let mut again = again.into_inner().unwrap();
    again.sort();
    assert_eq!(again.iter().map(|(id, _)| id.as_str()).collect::<Vec<_>>(), ["web-2", "web-3"], "failed and unreachable hosts only");
    assert!(again.iter().all(|(_, job)| json!(job) != old_job), "new job IDs");
    let after = groups::status(dir.path(), &cfg, &json!({"runId":id})).unwrap();
    assert_eq!(host_of(&after, "web-1")["status"], "succeeded", "a host that worked is left alone");
    assert_eq!(host_of(&after, "web-3")["state"], "started");
    assert_eq!(host_of(&after, "web-3")["error"], Value::Null);
}
