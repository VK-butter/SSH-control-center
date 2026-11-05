//! Master backup and restore (roadmap entry 3): one encrypted file moves a Master to a fresh install.
use base64::{engine::general_purpose::STANDARD, Engine};
use serde_json::{json, Value};
use sshcc_core::{backup, config, fleet, folders, jobs::Jobs, protocol::Request, security, service::Service, tags};
use std::{
    fs,
    path::Path,
    sync::{atomic::AtomicBool, Arc},
};
use uuid::Uuid;

const PASSPHRASE: &str = "correct horse battery";

fn key(seed: u8) -> String {
    let mut wire = Vec::new();
    wire.extend_from_slice(&11u32.to_be_bytes());
    wire.extend_from_slice(b"ssh-ed25519");
    wire.extend_from_slice(&32u32.to_be_bytes());
    wire.extend_from_slice(&[seed; 32]);
    format!("ssh-ed25519 {}", STANDARD.encode(wire))
}
fn call(state: &Path, config: &config::Config, method: &str, params: Value) -> Value {
    let request = Request { method: method.into(), params };
    fleet::handle(state, config, &request)
        .unwrap()
        .unwrap_or_else(|| panic!("{method} unhandled"))
}
fn folders_call(state: &Path, config: &config::Config, method: &str, params: Value) -> Value {
    folders::handle(state, config, &Request { method: method.into(), params }).unwrap().unwrap()
}
fn backup_call(state: &Path, config: &config::Config, method: &str, params: Value) -> anyhow::Result<Value> {
    backup::handle(state, config, &Request { method: method.into(), params }, true).map(|value| value.unwrap())
}
fn fresh(dir: &Path, label: &str) -> config::Config {
    config::init(dir, "both", &security::current_principal().unwrap(), label, &[]).unwrap()
}
fn host(id: &str, address: &str) -> Value {
    json!({"schemaVersion":2,"id":id,"nodeId":Uuid::new_v4().to_string(),"label":id,"tenant":"company","os":"linux",
        "user":"sshcc-bridge","hostKeys":[key(1)],"routes":[{"kind":"lan","address":address,"port":22}],
        "bridgePath":"/usr/bin/sshcc","runnerInstalled":true})
}

/// A Master with two hosts (one a Master it merged), a tenant key, an allowed Master, a relay, a folder and a
/// datacenter, plus state that must not travel: a job, health and an unfinished pairing request.
fn old_master(dir: &Path) -> config::Config {
    let cfg = fresh(dir, "Old Master");
    config::set_execution_mode(dir, "service").unwrap();
    let keys = dir.join("keys");
    fs::create_dir_all(&keys).unwrap();
    fs::write(keys.join("company.ed25519"), "PRIVATE KEY company").unwrap();
    fs::write(keys.join("company.ed25519.pub"), key(7) + " sshcc-service\n").unwrap();
    fs::write(keys.join("jump.ed25519"), "PRIVATE KEY jump").unwrap();
    call(dir, &cfg, "enrollment.export", json!({"targetId":"web-1","tenant":"company","os":"linux","user":"sshcc-bridge",
        "routes":[{"kind":"lan","address":"10.0.0.1","port":22}]}));
    call(dir, &cfg, "targets.import", json!({"registration":host("web-1","10.0.0.1")}));
    call(dir, &cfg, "targets.import", json!({"registration":host("branch","10.0.0.2")}));
    fleet::record_merge(dir, "branch", &json!({"nodeId":fleet_node(dir, &cfg, "branch"),"label":"Branch","targets":[
        {"id":"child","label":"Child","tenant":"lab","os":"linux","nodeId":Uuid::new_v4().to_string()}]})).unwrap();
    let allowed = Uuid::new_v4().to_string();
    fleet::register_master(dir, &json!({"id":allowed,"label":"Peer Master","publicKey":key(3)})).unwrap();
    fleet::relay_join(dir, &cfg, &allowed).unwrap();
    let folder = folders_call(dir, &cfg, "folders.create", json!({"name":"Web"}));
    folders_call(dir, &cfg, "folders.assign", json!({"targetId":"web-1","folderId":folder["id"]}));
    let site = folders_call(dir, &cfg, "datacenters.create", json!({"name":"Bangkok"}));
    folders_call(dir, &cfg, "datacenters.assign", json!({"master":"local","datacenterId":site["id"]}));
    tags::set(dir, &cfg, "web-1", &["Web".into(), "บางกอก".into()]).unwrap();
    tags::set(dir, &cfg, "branch/child", &["web".into()]).unwrap();
    Jobs::open(dir).unwrap();
    cfg
}
fn fleet_node(dir: &Path, cfg: &config::Config, id: &str) -> String {
    call(dir, cfg, "targets.list", json!({})).as_array().unwrap().iter()
        .find(|row| row["id"] == id).unwrap()["nodeId"].as_str().unwrap().into()
}
fn backup_file(dir: &Path, cfg: &config::Config) -> Vec<u8> {
    let made = backup_call(dir, cfg, "backup.create", json!({"passphrase":PASSPHRASE})).unwrap();
    STANDARD.decode(made["file"].as_str().unwrap()).unwrap()
}
fn restore(dir: &Path, cfg: &config::Config, file: &[u8], passphrase: &str) -> anyhow::Result<Value> {
    backup_call(dir, cfg, "backup.restore", json!({"file":STANDARD.encode(file),"passphrase":passphrase}))
}
fn error<T: std::fmt::Debug>(result: anyhow::Result<T>) -> String {
    format!("{:#}", result.unwrap_err())
}
fn ids(value: &Value, field: &str) -> Vec<String> {
    let mut ids: Vec<String> = value.as_array().unwrap().iter().map(|row| row[field].as_str().unwrap().to_owned()).collect();
    ids.sort();
    ids
}

#[test]
fn restore_moves_the_master_to_a_fresh_install() {
    let old = tempfile::tempdir().unwrap();
    let old_cfg = old_master(old.path());
    let file = backup_file(old.path(), &old_cfg);
    assert!(!String::from_utf8_lossy(&file).contains("PRIVATE KEY"), "the file must be encrypted");

    let new = tempfile::tempdir().unwrap();
    let new_cfg = fresh(new.path(), "New install");
    let result = restore(new.path(), &new_cfg, &file, PASSPHRASE).unwrap();
    assert_eq!(result["hosts"], 2);
    assert_eq!(result["restartRequired"], true);

    let restored = config::load(new.path()).unwrap();
    assert_eq!(restored.node_id, old_cfg.node_id);
    assert_eq!(restored.label, "Old Master");
    assert_eq!(restored.roles, old_cfg.roles);
    // The new machine's installer chose its Owner and execution mode; the backup never carries them.
    assert_eq!(restored.owner, new_cfg.owner);
    assert_eq!(restored.execution_mode, "owner-session");

    assert_eq!(fs::read_to_string(new.path().join("keys/company.ed25519")).unwrap(), "PRIVATE KEY company");
    assert_eq!(fs::read_to_string(new.path().join("keys/jump.ed25519")).unwrap(), "PRIVATE KEY jump");
    assert!(fs::read_to_string(new.path().join("keys/company.ed25519.pub")).unwrap().starts_with(&key(7)));

    let list = |method: &str| call(new.path(), &restored, method, json!({}));
    let old_list = |method: &str| call(old.path(), &old_cfg, method, json!({}));
    assert_eq!(list("targets.list"), old_list("targets.list"));
    assert_eq!(ids(&list("masters.list"), "id"), ids(&old_list("masters.list"), "id"));
    assert_eq!(ids(&list("relays.list"), "masterId"), ids(&old_list("relays.list"), "masterId"));
    assert_eq!(ids(&list("merges.list"), "targetId"), vec!["branch"]);
    assert_eq!(folders_call(new.path(), &restored, "folders.list", json!({})), folders_call(old.path(), &old_cfg, "folders.list", json!({})));
    assert_eq!(folders_call(new.path(), &restored, "datacenters.list", json!({})), folders_call(old.path(), &old_cfg, "datacenters.list", json!({})));
    assert_eq!(tags::list(new.path()).unwrap(), tags::list(old.path()).unwrap(), "Tags travel with the Master");
    assert_eq!(tags::list(new.path()).unwrap()["hosts"]["branch/child"], json!(["Web"]));
    // Enrolling another host keeps using the restored tenant key, not a new one.
    let bundle = call(new.path(), &restored, "enrollment.export", json!({"targetId":"web-2","tenant":"company","os":"linux",
        "user":"sshcc-bridge","routes":[{"kind":"lan","address":"10.0.0.3","port":22}]}));
    assert_eq!(bundle["master"]["publicKey"], key(7));
    assert_eq!(bundle["master"]["id"], old_cfg.node_id.as_str());
    // Job history stays behind.
    assert!(!new.path().join("jobs.sqlite").exists());
}

#[test]
fn inspect_and_status_describe_without_writing() {
    let old = tempfile::tempdir().unwrap();
    let cfg = old_master(old.path());
    let before = backup_call(old.path(), &cfg, "backup.status", json!({})).unwrap();
    assert_eq!(before["lastBackupAt"], Value::Null);
    assert_eq!(before["empty"], false);
    let file = backup_file(old.path(), &cfg);
    let after = backup_call(old.path(), &cfg, "backup.status", json!({})).unwrap();
    assert!(after["lastBackupAt"].is_string());

    let new = tempfile::tempdir().unwrap();
    let new_cfg = fresh(new.path(), "New install");
    assert_eq!(backup_call(new.path(), &new_cfg, "backup.status", json!({})).unwrap()["empty"], true);
    let summary = backup_call(new.path(), &new_cfg, "backup.inspect", json!({"file":STANDARD.encode(&file),"passphrase":PASSPHRASE})).unwrap();
    assert_eq!(summary["label"], "Old Master");
    assert_eq!(summary["hosts"], 2);
    assert_eq!(summary["formatVersion"], backup::FORMAT_VERSION);
    assert!(summary["createdAt"].is_string());
    assert_eq!(config::load(new.path()).unwrap().node_id, new_cfg.node_id);
}

#[test]
fn every_failure_has_its_own_code_and_writes_nothing() {
    let old = tempfile::tempdir().unwrap();
    let cfg = old_master(old.path());
    let file = backup_file(old.path(), &cfg);
    let new = tempfile::tempdir().unwrap();
    let new_cfg = fresh(new.path(), "New install");
    let untouched = |dir: &Path| {
        assert_eq!(config::load(dir).unwrap().node_id, new_cfg.node_id);
        assert!(!dir.join("keys/company.ed25519").exists());
    };

    assert!(error(backup_call(old.path(), &cfg, "backup.create", json!({"passphrase":"short pass"}))).contains("E_BACKUP_PASSPHRASE_SHORT"));
    assert!(error(backup::handle(old.path(), &cfg, &Request { method: "backup.create".into(), params: json!({"passphrase":PASSPHRASE}) }, false))
        .contains("E_BACKUP_NOT_OWNER"));
    assert!(error(backup::handle(new.path(), &new_cfg, &Request { method: "backup.restore".into(),
        params: json!({"file":STANDARD.encode(&file),"passphrase":PASSPHRASE}) }, false)).contains("E_BACKUP_NOT_OWNER"));

    assert!(error(restore(new.path(), &new_cfg, &file, "wrong passphrase!")).contains("E_BACKUP_BAD_PASSPHRASE_OR_DAMAGED"));
    let mut damaged = file.clone();
    let last = damaged.len() - 1;
    damaged[last] ^= 1;
    assert!(error(restore(new.path(), &new_cfg, &damaged, PASSPHRASE)).contains("E_BACKUP_BAD_PASSPHRASE_OR_DAMAGED"));
    assert!(error(restore(new.path(), &new_cfg, b"{\"not\":\"a backup\"}", PASSPHRASE)).contains("E_BACKUP_NOT_A_BACKUP"));
    let mut newer = file.clone();
    newer[8..10].copy_from_slice(&(backup::FORMAT_VERSION + 1).to_be_bytes());
    assert!(error(restore(new.path(), &new_cfg, &newer, PASSPHRASE)).contains("E_BACKUP_NEWER"));
    untouched(new.path());

    // A Master that already has hosts refuses, before the passphrase is even tried.
    assert!(error(restore(old.path(), &cfg, &file, "wrong passphrase!")).contains("E_RESTORE_NOT_EMPTY"));
}

#[test]
fn backup_and_restore_never_cross_a_bridge_or_relay() {
    let dir = tempfile::tempdir().unwrap();
    let owner = security::current_principal().unwrap();
    config::init(dir.path(), "both", &owner, "Test", &[]).unwrap();
    let service = Service::new(dir.path().into(), Arc::new(AtomicBool::new(false))).unwrap();
    // An enrolled Master, so the refusal comes from the method list and not from authentication.
    let peer = Uuid::new_v4().to_string();
    fleet::register_master(dir.path(), &json!({"id":peer,"label":"Peer","publicKey":key(3)})).unwrap();
    let fingerprint = fleet::public_key_fingerprint(&key(3)).unwrap();
    for method in ["backup.create", "backup.restore", "backup.inspect", "backup.status"] {
        let bridged = Request { method: "bridge.request".into(), params: json!({"masterId":peer,"masterKey":fingerprint,
            "expectedTargetId":service.config.node_id,"request":{"method":method,"params":{"passphrase":PASSPHRASE}}}) };
        assert!(error(service.handle(&bridged, &owner)).contains("not available over bridge"), "{method} over a bridge");
        let relayed = fleet::relay_request(dir.path(), &service.config, &Uuid::new_v4().to_string(),
            &json!({"targetId":"child","method":method,"params":{}}));
        assert!(format!("{:#}", relayed.unwrap_err()).contains("E_PERMISSION"), "{method} over a relay");
    }
    let status = service.handle(&Request { method: "backup.status".into(), params: json!({}) }, &owner).unwrap();
    assert_eq!(status["empty"], true);
}

fn jump_host(id: &str, address: &str, identity: &str) -> Value {
    let mut value = host(id, address);
    value["routes"][0]["proxyJump"] = json!({"address":"10.9.9.9","port":22,"user":"jump","hostKeys":[key(1)],"identityFile":identity});
    value
}

#[test]
fn restore_reports_what_the_new_machine_cannot_fix_alone() {
    let old = tempfile::tempdir().unwrap();
    let cfg = old_master(old.path());
    let inside = old.path().join("keys").join("jump.ed25519");
    call(old.path(), &cfg, "targets.import", json!({"registration":jump_host("db-1","10.0.0.5",&inside.to_string_lossy())}));
    call(old.path(), &cfg, "targets.import", json!({"registration":jump_host("db-2","10.0.0.6","/opt/keys/outside.ed25519")}));
    // Two-way with the merged Branch Master; a disabled relay and a revoked Master no longer reach this one.
    let request = Request { method: "merges.list".into(), params: json!({}) };
    let merge = fleet::handle(old.path(), &cfg, &request).unwrap().unwrap()[0].clone();
    assert_eq!(merge["targetId"], "branch");
    let mut two_way = merge.clone();
    two_way["reverse"] = json!({"status":"two-way","peerTargetId":"this-master","updatedAt":"2026-10-01T00:00:00Z"});
    rusqlite::Connection::open(old.path().join("fleet.sqlite")).unwrap()
        .execute("UPDATE merges SET document=?1 WHERE target_id='branch'", [two_way.to_string()]).unwrap();
    let gone = Uuid::new_v4().to_string();
    fleet::register_master(old.path(), &json!({"id":gone,"label":"Old laptop","publicKey":key(4)})).unwrap();
    fleet::revoke_master(old.path(), &gone).unwrap();
    let file = backup_file(old.path(), &cfg);

    let new = tempfile::tempdir().unwrap();
    let new_cfg = fresh(new.path(), "New install");
    let result = restore(new.path(), &new_cfg, &file, PASSPHRASE).unwrap();
    let restored = config::load(new.path()).unwrap();
    let targets = call(new.path(), &restored, "targets.list", json!({}));
    let jump = |id: &str| targets.as_array().unwrap().iter().find(|t| t["id"] == id).unwrap()["routes"][0]["proxyJump"]["identityFile"].clone();
    assert_eq!(jump("db-1"), new.path().join("keys").join("jump.ed25519").to_string_lossy().as_ref());
    assert_eq!(jump("db-2"), "/opt/keys/outside.ed25519");
    assert_eq!(result["checkPaths"], json!([{"targetId":"db-2","label":"db-2","path":"/opt/keys/outside.ed25519"}]));

    let reached: Vec<&str> = result["reachedBy"].as_array().unwrap().iter().map(|m| m["label"].as_str().unwrap()).collect();
    assert_eq!(reached, vec!["Peer Master"], "allowed and merging Masters, not revoked ones");
    assert_eq!(result["reachedBy"][0]["ways"], json!(["direct", "merge"]));
    assert_eq!(result["twoWayAgain"], json!([{"targetId":"branch","label":"Branch"}]));
    let merges = call(new.path(), &restored, "merges.list", json!({}));
    assert_eq!(merges[0]["reverse"], Value::Null, "two-way is offered again, as one-way until enabled");
}

// Change keys (roadmap entry 3, after a restore). The elevated job itself is not run here: `pass` takes the steps as
// a closure, so these tests check the bookkeeping only.
use sshcc_core::fleet::rotation::{self, Step};
fn own_target(dir: &Path, cfg: &config::Config, id: &str) -> fleet::Target {
    let rows = call(dir, cfg, "targets.list", json!({}));
    serde_json::from_value(rows.as_array().unwrap().iter().find(|row| row["id"] == id).unwrap().clone()).unwrap()
}
fn public(path: &Path) -> String {
    fs::read_to_string(path).unwrap().split_whitespace().take(2).collect::<Vec<_>>().join(" ")
}

#[test]
fn hosts_move_only_after_the_check_and_old_keys_stay_until_all_moved() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = old_master(dir.path());
    let (current, next) = (dir.path().join("keys/company.ed25519"), dir.path().join("keys/company.next.ed25519"));
    let started = rotation::start(dir.path(), &cfg).unwrap();
    assert_eq!((started["total"].as_u64(), started["oldKey"].as_u64(), started["active"].as_bool()), (Some(2), Some(2), Some(true)));
    let new_public = public(&next.with_extension("ed25519.pub"));
    assert_ne!(new_public, key(7));

    // web-1 moves; branch runs its job but the check with the new key fails.
    let mut steps = Vec::new();
    let first = rotation::pass(dir.path(), &cfg, &mut |target, step| {
        let check = matches!(step, Step::Check { .. });
        if let Step::Install { script, .. } = &step {
            assert!(script.contains(&fleet::public_key_fingerprint(&new_public).unwrap()), "the job installs the new key");
        }
        steps.push(format!("{} {}", target.id, if check { "check" } else { "install" }));
        if check && target.id == "branch" { anyhow::bail!("timed out") } else { Ok(()) }
    }).unwrap();
    assert_eq!(steps, ["branch install", "branch check", "web-1 install", "web-1 check"]);
    assert_eq!((first["moved"].as_u64(), first["oldKey"].as_u64(), first["active"].as_bool()), (Some(1), Some(0), Some(true)));
    let branch = first["hosts"].as_array().unwrap().iter().find(|h| h["targetId"] == "branch").unwrap().clone();
    assert_eq!(branch["state"], "installed");
    assert!(branch["error"].as_str().unwrap().contains("E_NEW_KEY_NOT_ACCEPTED"));
    // Each host is reached with the key it accepts now; the old key stays until no host needs it.
    assert_eq!(rotation::identity(dir.path(), &own_target(dir.path(), &cfg, "web-1")).unwrap(), next);
    assert_eq!(rotation::identity(dir.path(), &own_target(dir.path(), &cfg, "branch")).unwrap(), next);
    assert_eq!(fs::read_to_string(&current).unwrap(), "PRIVATE KEY company");
    assert_eq!(backup_call(dir.path(), &cfg, "backup.status", json!({})).unwrap()["keysChangedAt"], Value::Null);

    // The next pass only checks branch again, then the change finishes.
    let mut again = Vec::new();
    let done = rotation::pass(dir.path(), &cfg, &mut |target, step| {
        again.push(format!("{} {}", target.id, if matches!(step, Step::Check { .. }) { "check" } else { "install" }));
        Ok(())
    }).unwrap();
    assert_eq!(again, ["branch check"]);
    assert_eq!((done["moved"].as_u64(), done["active"].as_bool()), (Some(2), Some(false)));
    assert_eq!(public(&current.with_extension("ed25519.pub")), new_public, "the new key is now the key");
    assert_ne!(fs::read_to_string(&current).unwrap(), "PRIVATE KEY company", "the old private key is gone");
    assert!(!next.exists());
    assert_eq!(rotation::identity(dir.path(), &own_target(dir.path(), &cfg, "web-1")).unwrap(), current);
    let bundle = call(dir.path(), &cfg, "enrollment.export", json!({"targetId":"web-3","tenant":"company","os":"linux","user":"sshcc-bridge",
        "routes":[{"kind":"lan","address":"10.0.0.9","port":22}]}));
    assert_eq!(bundle["master"]["publicKey"], new_public.as_str(), "new hosts get the new key");
    let status = backup_call(dir.path(), &cfg, "backup.status", json!({})).unwrap();
    assert!(status["keysChangedAt"].is_string());
    assert_eq!(status["backupBeforeKeys"], true, "old backups no longer work");
}

#[test]
fn a_failed_job_keeps_the_host_on_the_old_key() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = old_master(dir.path());
    rotation::start(dir.path(), &cfg).unwrap();
    // web-1's job can't start; branch's job ran but put the old key back.
    let mut jobs = Vec::new();
    let status = rotation::pass(dir.path(), &cfg, &mut |target, step| match step {
        Step::Install { .. } if target.id == "web-1" => anyhow::bail!("E_OWNER_NOT_LOGGED_IN: nobody is logged in"),
        Step::Install { job, .. } => { jobs.push(job.to_owned()); Ok(()) }
        Step::Check { job } => { assert_eq!(job.map(str::to_owned), jobs.last().cloned(), "the check follows that job"); anyhow::bail!("E_STILL_OLD_KEY: the key change job ended failed: no key file") }
    }).unwrap();
    assert_eq!((status["moved"].as_u64(), status["oldKey"].as_u64(), status["active"].as_bool()), (Some(0), Some(2), Some(true)));
    let host = |id: &str| status["hosts"].as_array().unwrap().iter().find(|h| h["targetId"] == id).unwrap().clone();
    assert_eq!(host("web-1")["state"], "pending");
    assert!(host("web-1")["error"].as_str().unwrap().contains("E_OWNER_NOT_LOGGED_IN"));
    assert_eq!(host("branch")["state"], "pending");
    assert!(host("branch")["error"].as_str().unwrap().contains("no key file"));
    assert_eq!(rotation::identity(dir.path(), &own_target(dir.path(), &cfg, "web-1")).unwrap(), dir.path().join("keys/company.ed25519"));
    assert_eq!(fs::read_to_string(dir.path().join("keys/company.ed25519")).unwrap(), "PRIVATE KEY company");
}

#[test]
fn the_job_adds_the_new_key_before_it_drops_the_old_one() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = old_master(dir.path());
    let mut target = own_target(dir.path(), &cfg, "web-1");
    let fp = fleet::public_key_fingerprint(&key(9)).unwrap();
    let old_b64 = key(7).split_whitespace().nth(1).unwrap().to_owned();
    let linux = rotation::install_script(&cfg, &target, &key(7), &key(9)).unwrap();
    let order = |script: &str, parts: &[&str]| parts.iter().map(|part| script.find(part).unwrap_or_else(|| panic!("missing {part}"))).collect::<Vec<_>>();
    let at = order(&linux, &["/usr/bin/sshcc bridge --master-id", "configure revoke-master", "configure accept-bundle \"$dir/new.json\"", &format!("-v k=\" {old_b64} \"")]);
    assert!(at.windows(2).all(|w| w[0] < w[1]), "add, switch, then remove: {at:?}");
    assert!(linux.contains(&fp) && linux.contains(&cfg.node_id));
    target.os = "windows".into();
    target.bridge_path = "C:/Program Files/SSH Control Center/sshcc.exe".into();
    let windows = rotation::install_script(&cfg, &target, &key(7), &key(9)).unwrap();
    let at = order(&windows, &["powershell.exe -NoProfile -NonInteractive -EncodedCommand", "configure revoke-master", "accept-bundle $newBundle", "$_.Contains($oldKey)"]);
    assert!(at.windows(2).all(|w| w[0] < w[1]), "add, switch, then remove: {at:?}");
}

#[test]
fn offline_hosts_are_retried_and_merged_hosts_belong_to_their_master() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = old_master(dir.path());
    rotation::start(dir.path(), &cfg).unwrap();
    assert!(rotation::retry_due(dir.path()).unwrap(), "a new change is due at once");
    let status = rotation::pass(dir.path(), &cfg, &mut |target, step| match step {
        Step::Install { .. } if target.id == "web-1" => anyhow::bail!("ALL_ROUTES_FAILED: lan 10.0.0.1:22: No route to host"),
        _ => Ok(()),
    }).unwrap();
    assert_eq!(status["oldKey"], 1);
    assert!(!rotation::retry_due(dir.path()).unwrap(), "not again right away");
    assert_eq!(status["merged"], json!([{"label":"Child","master":"Branch"}]), "merged hosts are listed, not counted");
    assert_eq!(status["total"], 2);
    // A minute later the host is back: the next pass moves it and the change finishes.
    let path = dir.path().join("rotation.json");
    let mut record: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    record["lastPassAt"] = json!("2026-01-01T00:00:00Z");
    fs::write(&path, record.to_string()).unwrap();
    assert!(rotation::retry_due(dir.path()).unwrap());
    let done = rotation::pass(dir.path(), &cfg, &mut |_, _| Ok(())).unwrap();
    assert_eq!((done["moved"].as_u64(), done["active"].as_bool()), (Some(2), Some(false)));
    assert!(!rotation::retry_due(dir.path()).unwrap(), "nothing left to retry");
}

#[test]
fn hosts_without_the_service_and_hosts_paired_meanwhile() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = old_master(dir.path());
    call(dir.path(), &cfg, "targets.import_legacy", json!({"document":{"schemaVersion":1,"id":"old-box","hostname":"old-box","mode":"company",
        "tenant":"company","address":"10.0.0.8","port":22,"user":"sshcc-bridge","os":"linux","hostKeys":[key(1)],"shareRequired":false}}));
    rotation::start(dir.path(), &cfg).unwrap();
    call(dir.path(), &cfg, "targets.import", json!({"registration":host("web-2","10.0.0.4")}));
    let mut installed = Vec::new();
    let status = rotation::pass(dir.path(), &cfg, &mut |target, step| {
        if matches!(step, Step::Install { .. }) { installed.push(target.id.clone()); }
        Ok(())
    }).unwrap();
    assert!(installed.contains(&"web-2".to_string()), "a host paired during the change is moved too");
    assert!(!installed.contains(&"old-box".to_string()));
    let old_box = status["hosts"].as_array().unwrap().iter().find(|h| h["targetId"] == "old-box").unwrap().clone();
    assert_eq!(old_box["state"], "pending");
    assert!(old_box["error"].as_str().unwrap().contains("E_RUNNER_NOT_INSTALLED"));
    assert_eq!(status["active"], true, "the change stays open while a host is on the old key");
}
