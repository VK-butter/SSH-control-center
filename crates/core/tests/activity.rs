//! Activity (roadmap entry 5): Recent Jobs and the audit log, this Master's own record. "Now" is passed in where a
//! test needs a particular day, and host steps are closures, so nothing here uses SSH.
use base64::{engine::general_purpose::STANDARD, Engine};
use chrono::{DateTime, Duration, TimeZone, Utc};
use serde_json::{json, Value};
use sshcc_core::{
    activity::{self, Sent},
    config, fleet,
    groups::{self, Step},
    protocol::Request,
    security,
    service::Service,
    tags,
};
use std::{
    fs,
    path::Path,
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc,
    },
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
fn request(method: &str, params: Value) -> Request {
    Request { method: method.into(), params }
}
/// A Master (also its own Target) with web-1 and web-2, both tagged `web`.
fn master(dir: &Path) -> (Service, String) {
    let owner = security::current_principal().unwrap();
    let cfg = config::init(dir, "both", &owner, "Office", &[]).unwrap();
    fs::create_dir_all(dir.join("keys")).unwrap();
    fs::write(dir.join("keys/company.ed25519"), "test key placeholder").unwrap();
    for (id, address) in [("web-1", "10.0.0.1"), ("web-2", "10.0.0.2")] {
        fleet::handle(dir, &cfg, &request("targets.import", json!({"registration":host(id, address)}))).unwrap();
        tags::set(dir, &cfg, id, &["web".into()]).unwrap();
    }
    (Service::new(dir.into(), Arc::new(AtomicBool::new(false))).unwrap(), owner)
}
fn lines(dir: &Path) -> Vec<Value> {
    fs::read_to_string(dir.join("audit.jsonl")).unwrap_or_default().lines().map(|l| serde_json::from_str(l).unwrap()).collect()
}
fn all() -> activity::Filter {
    activity::filter(&json!({}), Utc::now()).unwrap()
}
/// 2026-10-25 00:30 in Thailand is 2026-10-24 17:30 UTC.
fn thai(day: u32, hour: u32, minute: u32) -> DateTime<Utc> {
    activity::thai().with_ymd_and_hms(2026, 10, day, hour, minute, 0).unwrap().with_timezone(&Utc)
}
fn sent<'a>(job: &'a str, target: &'a str, status: &'a str) -> Sent<'a> {
    static NO_ENV: Value = Value::Null;
    Sent { job, target, label: None, group: None, script: "uptime", cwd: "/", shell: None, as_admin: false, env: &NO_ENV,
        channel: "desktop", status, error: None }
}
fn job_ids(entries: &[Value]) -> Vec<String> {
    entries.iter().flat_map(|e| match e["kind"].as_str() {
        Some("group") => e["hosts"].as_array().unwrap().iter().map(|h| h["jobId"].as_str().unwrap().to_owned()).collect::<Vec<_>>(),
        _ => vec![e["job"]["jobId"].as_str().unwrap().to_owned()],
    }).collect()
}

#[test]
fn a_change_is_audited_once_with_its_channel_and_a_read_is_not() {
    let dir = tempfile::tempdir().unwrap();
    let (service, owner) = master(dir.path());
    service.handle_as(&request("tags.set", json!({"targetId":"web-1","tags":["web","db"]})), &owner, Some("desktop")).unwrap();
    service.handle_as(&request("tags.list", json!({})), &owner, Some("desktop")).unwrap();
    service.handle_as(&request("targets.list", json!({})), &owner, Some("cli")).unwrap();
    let audit = lines(dir.path());
    assert_eq!(audit.len(), 1, "reads are not audited: {audit:?}");
    let line = &audit[0];
    assert_eq!(line["action"], "tags.set");
    assert_eq!(line["object"], "web-1 · web, db");
    assert_eq!((line["result"].as_str(), line["channel"].as_str(), line["user"].as_str()), (Some("ok"), Some("desktop"), Some(owner.as_str())));
    assert!(line["time"].as_str().unwrap().ends_with("+07:00"), "Thai time: {}", line["time"]);
    // A client that reports no channel.
    service.handle(&request("folders.create", json!({"name":"Web"})), &owner).unwrap();
    assert_eq!(lines(dir.path())[1]["channel"], "unknown");
}

#[test]
fn a_refused_call_is_audited_with_its_error_code() {
    let dir = tempfile::tempdir().unwrap();
    let (service, owner) = master(dir.path());
    assert!(service.handle_as(&request("tags.set", json!({"targetId":"web-1","tags":["a,b"]})), &owner, Some("mcp")).is_err());
    assert!(service.handle_as(&request("tags.set", json!({"targetId":"web-1","tags":["web"]})), "someone-else", Some("cli")).is_err());
    let audit = lines(dir.path());
    assert_eq!(audit[0]["result"], "E_TAG_CHARS");
    assert_eq!(audit[0]["channel"], "mcp");
    assert_eq!((audit[1]["result"].as_str(), audit[1]["user"].as_str()), (Some("E_PERMISSION"), Some("someone-else")));
}

#[test]
fn no_script_content_env_or_passphrase_reaches_the_audit_log() {
    let dir = tempfile::tempdir().unwrap();
    let (service, owner) = master(dir.path());
    let secret = "hunter2-secret";
    let calls = [
        request("jobs.start", json!({"targetId":"web-1","jobId":"j1","script":format!("echo {secret}"),"cwd":"/","env":{"TOKEN":secret}})),
        request("files.write", json!({"targetId":"web-1","path":"/etc/app.conf","contentBase64":STANDARD.encode(secret)})),
        request("backup.create", json!({"passphrase":secret})),
    ];
    for call in &calls {
        let _ = service.handle_as(call, &owner, Some("desktop"));
    }
    let text = fs::read_to_string(dir.path().join("audit.jsonl")).unwrap();
    assert_eq!(text.lines().count(), 3);
    assert!(!text.contains(secret) && !text.contains(&STANDARD.encode(secret)), "{text}");
    assert!(text.contains("web-1 · j1") && text.contains("web-1 · /etc/app.conf"), "{text}");
}

#[test]
fn downloads_and_uploads_are_audited_once() {
    let dir = tempfile::tempdir().unwrap();
    let (service, owner) = master(dir.path());
    for offset in [0, 1048576] {
        let _ = service.handle_as(&request("files.read", json!({"targetId":"web-1","path":"/var/log/app.log","offset":offset,"download":offset == 0})), &owner, Some("desktop"));
    }
    let _ = service.handle_as(&request("files.read", json!({"targetId":"web-1","path":"/etc/hosts","offset":0})), &owner, Some("desktop"));
    for (offset, done) in [(0, false), (10, true)] {
        let _ = service.handle_as(&request("files.upload", json!({"targetId":"web-1","path":"/tmp/a","uploadId":"u","offset":offset,"done":done})), &owner, Some("desktop"));
    }
    let actions: Vec<String> = lines(dir.path()).iter().map(|l| l["action"].as_str().unwrap().to_owned()).collect();
    assert_eq!(actions, ["files.read", "files.upload"], "a preview is not a download");
}

#[test]
fn a_relayed_call_is_labelled_with_the_merged_master() {
    let dir = tempfile::tempdir().unwrap();
    let (service, owner) = master(dir.path());
    let peer = Uuid::new_v4().to_string();
    fleet::register_master(dir.path(), &json!({"id":peer,"label":"Peer","publicKey":key(3)})).unwrap();
    let fingerprint = fleet::public_key_fingerprint(&key(3)).unwrap();
    let bridged = |method: &str, params: Value| request("bridge.request", json!({"masterId":peer,"masterKey":fingerprint,
        "expectedTargetId":service.config.node_id,"request":{"method":method,"params":params}}));
    service.handle(&bridged("relay.join", json!({})), &owner).unwrap();
    let _ = service.handle(&bridged("relay.request", json!({"targetId":"web-1","method":"files.remove","params":{"path":"/tmp/x"}})), &owner);
    let _ = service.handle(&bridged("relay.request", json!({"targetId":"web-1","method":"files.list","params":{"path":"/"}})), &owner);
    // A Master using this machine as its Target is that Master's activity, not this one's.
    let _ = service.handle(&bridged("runner.start", json!({"jobId":"x","script":"id","cwd":"/"})), &owner);
    let audit = lines(dir.path());
    assert_eq!(audit.len(), 1, "{audit:?}");
    assert_eq!((audit[0]["action"].as_str(), audit[0]["object"].as_str()), (Some("files.remove"), Some("web-1 · /tmp/x")));
    assert_eq!(audit[0]["channel"], "merge: Peer");
    assert!(activity::job_list(dir.path(), &all()).unwrap().is_empty(), "relayed and bridged jobs are not Recent Jobs");
}

#[test]
fn a_started_job_is_recorded_with_env_names_only() {
    let dir = tempfile::tempdir().unwrap();
    let (service, owner) = master(dir.path());
    let start = request("jobs.start", json!({"targetId":"web-1","jobId":"j1","script":"x".repeat(5000),"cwd":"/srv","asAdmin":true,"env":{"TOKEN":"secret"}}));
    activity::after(dir.path(), &service.config, &start, &owner, Some("cli"), &Ok(json!({"id":"j1","status":"queued"})));
    activity::after(dir.path(), &service.config, &start, &owner, Some("cli"), &Err(anyhow::anyhow!("E_ROUTE: no route")));
    let jobs = activity::job_list(dir.path(), &all()).unwrap();
    assert_eq!(jobs.len(), 1, "a job that never left isn't listed");
    let job = &jobs[0]["job"];
    assert_eq!((job["targetId"].as_str(), job["cwd"].as_str(), job["channel"].as_str()), (Some("web-1"), Some("/srv"), Some("cli")));
    assert_eq!(job["script"].as_str().unwrap().chars().count(), 4096);
    assert_eq!(job["envNames"], json!(["TOKEN"]));
    assert!(!job.to_string().contains("secret"));
    assert_eq!(lines(dir.path()).len(), 2, "both attempts are in the audit log");
}

#[test]
fn a_group_run_is_one_entry_with_a_row_per_host() {
    let dir = tempfile::tempdir().unwrap();
    let (service, _) = master(dir.path());
    let cfg = &service.config;
    let run = groups::start(dir.path(), cfg, &json!({"selector":{"tag":"web"},"scripts":{"linux":"uptime"},"channel":"desktop"})).unwrap();
    let id = run["id"].as_str().unwrap();
    groups::drive(dir.path(), cfg, id, &|target, step| match (target.id.as_str(), step) {
        ("web-2", Step::Start { .. }) => Err(anyhow::anyhow!("E_ROUTE: ALL_ROUTES_FAILED")),
        _ => Ok(json!({"status":"queued"})),
    }).unwrap();
    groups::refresh(dir.path(), cfg, id, &|_, _| Ok(json!({"status":"succeeded","exitCode":0}))).unwrap();
    let jobs = activity::job_list(dir.path(), &all()).unwrap();
    assert_eq!(jobs.len(), 1);
    let group = &jobs[0];
    assert_eq!((group["kind"].as_str(), group["groupId"].as_str(), group["channel"].as_str()), (Some("group"), Some(id), Some("desktop")));
    assert_eq!(group["selector"], json!({"tag":"web"}));
    let state = |host: &str| group["hosts"].as_array().unwrap().iter().find(|h| h["targetId"] == host).unwrap()["status"].clone();
    assert_eq!((state("web-1"), state("web-2")), (json!("succeeded"), json!("unreachable")));
    // Filtering by one host still shows the run it was in; filtering by status keeps the run if any host matches.
    let only = |p: Value| activity::job_list(dir.path(), &activity::filter(&p, Utc::now()).unwrap()).unwrap().len();
    assert_eq!(only(json!({"hosts":["web-2"]})), 1);
    assert_eq!(only(json!({"hosts":["db-9"]})), 0);
    assert_eq!(only(json!({"status":"failed"})), 1);
    assert_eq!(only(json!({"status":"running"})), 0);
}

#[test]
fn yesterday_and_today_are_thai_calendar_days() {
    let dir = tempfile::tempdir().unwrap();
    master(dir.path());
    activity::record(dir.path(), thai(24, 23, 50), sent("late", "web-1", "succeeded"));
    activity::record(dir.path(), thai(25, 0, 30), sent("early", "web-1", "succeeded"));
    activity::record(dir.path(), thai(18, 12, 0), sent("week", "web-2", "failed"));
    let now = thai(25, 9, 0);
    let pick = |p: Value| job_ids(&activity::job_list(dir.path(), &activity::filter(&p, now).unwrap()).unwrap());
    assert_eq!(pick(json!({"range":"today"})), ["early"], "00:30 Thai time is today even though it is 17:30 UTC yesterday");
    assert_eq!(pick(json!({"range":"yesterday"})), ["late"]);
    assert_eq!(pick(json!({"range":"7d"})), ["early", "late"]);
    assert_eq!(pick(json!({"range":"8d"})), ["early", "late", "week"]);
    assert_eq!(pick(json!({"from":"2026-10-18","to":"2026-10-18"})), ["week"]);
    assert_eq!(pick(json!({"range":"7d","hosts":["web-2"]})), Vec::<String>::new());
    assert_eq!(pick(json!({"status":"failed"})), ["week"]);
    assert!(activity::filter(&json!({"from":"25/10/2026"}), now).is_err());
}

#[test]
fn the_audit_view_matches_hosts_or_text() {
    let dir = tempfile::tempdir().unwrap();
    let (service, owner) = master(dir.path());
    service.handle_as(&request("tags.set", json!({"targetId":"web-1","tags":["web"]})), &owner, Some("desktop")).unwrap();
    service.handle_as(&request("folders.create", json!({"name":"Bangkok"})), &owner, Some("desktop")).unwrap();
    let pick = |p: Value| activity::audit_list(dir.path(), &activity::filter(&p, Utc::now()).unwrap()).iter()
        .map(|l| l["action"].as_str().unwrap().to_owned()).collect::<Vec<_>>();
    assert_eq!(pick(json!({})), ["folders.create", "tags.set"], "newest first");
    assert_eq!(pick(json!({"hosts":["web-1"],"text":"web-1"})), ["tags.set"]);
    assert_eq!(pick(json!({"hosts":[],"text":"bangkok"})), ["folders.create"]);
    assert_eq!(pick(json!({"hosts":[],"text":"folders."})), ["folders.create"]);
    assert_eq!(pick(json!({"range":"yesterday"})), Vec::<String>::new());
}

#[test]
fn finished_jobs_are_not_asked_again() {
    let dir = tempfile::tempdir().unwrap();
    master(dir.path());
    activity::record(dir.path(), Utc::now(), sent("done", "web-1", "succeeded"));
    activity::record(dir.path(), Utc::now(), sent("busy", "web-2", "running"));
    activity::record(dir.path(), Utc::now(), sent("off", "web-1", "queued"));
    let asked = AtomicUsize::new(0);
    let jobs = activity::refresh(dir.path(), &all(), &|_, job| {
        asked.fetch_add(1, Ordering::SeqCst);
        match job {
            "busy" => Ok(json!({"status":"succeeded","exitCode":0,"finishedAt":"2026-10-25T07:00:00Z"})),
            _ => Err(anyhow::anyhow!("E_ROUTE: ALL_ROUTES_FAILED")),
        }
    }).unwrap();
    assert_eq!(asked.load(Ordering::SeqCst), 2);
    let job = |id: &str| jobs.iter().map(|e| &e["job"]).find(|j| j["jobId"] == id).unwrap().clone();
    assert_eq!((job("busy")["status"].as_str(), job("busy")["exitCode"].as_i64()), (Some("succeeded"), Some(0)));
    // An offline host keeps what was recorded, with the reason.
    assert_eq!(job("off")["status"], "queued");
    assert!(job("off")["error"].as_str().unwrap().contains("ALL_ROUTES_FAILED"));
    assert_eq!(job("off")["script"], "uptime");
}

#[test]
fn csv_has_a_bom_thai_times_and_one_row_per_host_job() {
    let dir = tempfile::tempdir().unwrap();
    master(dir.path());
    let selector = json!({"tag":"web"});
    let mut quoted = sent("j1", "web-1", "succeeded");
    quoted.script = "echo \"a, b\"\nuptime";
    activity::record(dir.path(), thai(25, 14, 3), quoted);
    for (job, host) in [("g1", "web-1"), ("g2", "web-2")] {
        let mut row = sent(job, host, "succeeded");
        row.group = Some(("run-1", &selector));
        activity::record(dir.path(), thai(25, 15, 0), row);
    }
    activity::update_status(dir.path(), "g1", &json!({"status":"succeeded","exitCode":0,"startedAt":"2026-10-25T08:00:00Z"})).unwrap();
    let text = activity::jobs_csv(&activity::job_list(dir.path(), &all()).unwrap());
    assert!(text.starts_with('\u{feff}'));
    let rows: Vec<&str> = text.trim_start_matches('\u{feff}').split("\r\n").collect();
    assert_eq!(rows[0], "time,host,group,status,exit_code,started,finished,cwd,as_admin,channel,job_id,script");
    assert!(rows[1].starts_with("2026-10-25T15:00:00+07:00,web-1,run-1,succeeded,0,2026-10-25T15:00:00+07:00,"), "{}", rows[1]);
    assert!(rows[2].starts_with("2026-10-25T15:00:00+07:00,web-2,run-1,"));
    assert!(text.contains("2026-10-25T14:03:00+07:00,web-1,,succeeded") && text.contains("\"echo \"\"a, b\"\"\nuptime\""), "{text}");
    let audit = activity::audit_csv(&[json!({"time":"2026-10-25T14:03:00+07:00","action":"tags.set","object":"web-1 · web, db","result":"ok","channel":"desktop","user":"me"})]);
    assert_eq!(audit, "\u{feff}time,action,object,result,channel,user\r\n2026-10-25T14:03:00+07:00,tags.set,\"web-1 · web, db\",ok,desktop,me\r\n");
}

#[test]
fn old_entries_go_after_keep_days() {
    let dir = tempfile::tempdir().unwrap();
    let (service, owner) = master(dir.path());
    assert_eq!(activity::status(dir.path(), Utc::now()).unwrap()["keepDays"], 14);
    let now = Utc::now();
    activity::record(dir.path(), now - Duration::days(15), sent("old", "web-1", "succeeded"));
    activity::record(dir.path(), now - Duration::days(13), sent("new", "web-1", "succeeded"));
    activity::audit(dir.path(), json!({"time":activity::stamp(now - Duration::days(20)),"action":"tags.set","object":"web-1","result":"ok"}));
    service.handle_as(&request("folders.create", json!({"name":"Web"})), &owner, Some("desktop")).unwrap();
    activity::prune(dir.path(), now).unwrap();
    assert_eq!(job_ids(&activity::job_list(dir.path(), &all()).unwrap()), ["new"]);
    assert_eq!(lines(dir.path()).iter().map(|l| l["action"].as_str().unwrap()).collect::<Vec<_>>(), ["folders.create"]);
    // Fewer days through the service; it's an audited setting, and 0 is refused.
    assert!(service.handle_as(&request("activity.settings", json!({"keepDays":0})), &owner, Some("desktop")).unwrap_err().to_string().contains("E_ACTIVITY_DAYS"));
    service.handle_as(&request("activity.settings", json!({"keepDays":10})), &owner, Some("desktop")).unwrap();
    activity::prune(dir.path(), now).unwrap();
    assert!(activity::job_list(dir.path(), &all()).unwrap().is_empty());
    assert_eq!(lines(dir.path()).last().unwrap()["object"], "10");
    // Once a day.
    activity::daily(dir.path(), now).unwrap();
    activity::record(dir.path(), now - Duration::days(11), sent("again", "web-1", "succeeded"));
    activity::daily(dir.path(), now).unwrap();
    assert_eq!(job_ids(&activity::job_list(dir.path(), &all()).unwrap()), ["again"], "already pruned today");
}

#[test]
fn a_failed_write_keeps_the_action_and_warns_until_it_works() {
    let dir = tempfile::tempdir().unwrap();
    let (service, owner) = master(dir.path());
    fs::create_dir(dir.path().join("audit.jsonl")).unwrap();
    service.handle_as(&request("tags.set", json!({"targetId":"web-1","tags":["db"]})), &owner, Some("desktop")).unwrap();
    assert_eq!(tags::list(dir.path()).unwrap()["hosts"]["web-1"], json!(["db"]), "the action went ahead");
    let status = service.handle(&request("activity.status", json!({})), &owner).unwrap();
    assert_eq!(status["warning"], true);
    fs::remove_dir(dir.path().join("audit.jsonl")).unwrap();
    service.handle_as(&request("tags.set", json!({"targetId":"web-1","tags":["web"]})), &owner, Some("desktop")).unwrap();
    assert_eq!(service.handle(&request("activity.status", json!({})), &owner).unwrap()["warning"], false);
}

#[test]
fn activity_is_owner_only_and_never_crosses_a_bridge() {
    let dir = tempfile::tempdir().unwrap();
    let (service, owner) = master(dir.path());
    let peer = Uuid::new_v4().to_string();
    fleet::register_master(dir.path(), &json!({"id":peer,"label":"Peer","publicKey":key(3)})).unwrap();
    let fingerprint = fleet::public_key_fingerprint(&key(3)).unwrap();
    let bridged = request("bridge.request", json!({"masterId":peer,"masterKey":fingerprint,"expectedTargetId":service.config.node_id,
        "request":{"method":"activity.audit","params":{}}}));
    assert!(service.handle(&bridged, &owner).unwrap_err().to_string().contains("not available over bridge"));
    let relayed = fleet::relay_request(dir.path(), &service.config, &peer, &json!({"targetId":"web-1","method":"activity.jobs","params":{}}));
    assert!(format!("{:#}", relayed.unwrap_err()).contains("E_PERMISSION"));
    let jobs = service.handle(&request("activity.jobs", json!({})), &owner).unwrap();
    assert_eq!(jobs["jobs"], json!([]));
    assert!(jobs["status"]["since"].as_str().unwrap().ends_with("+07:00"));
}

#[test]
fn a_restored_master_starts_a_new_history() {
    let dir = tempfile::tempdir().unwrap();
    let (service, owner) = master(dir.path());
    service.handle_as(&request("folders.create", json!({"name":"Web"})), &owner, Some("desktop")).unwrap();
    activity::record(dir.path(), Utc::now(), sent("j1", "web-1", "succeeded"));
    activity::after(dir.path(), &service.config, &request("backup.restore", json!({"file":"…","passphrase":"secret phrase"})), &owner, Some("desktop"), &Ok(json!({})));
    let audit = lines(dir.path());
    assert_eq!(audit.len(), 1);
    assert_eq!((audit[0]["action"].as_str(), audit[0]["result"].as_str()), (Some("backup.restore"), Some("ok")));
    assert!(!audit[0].to_string().contains("secret"));
    assert!(activity::job_list(dir.path(), &all()).unwrap().is_empty());
}
