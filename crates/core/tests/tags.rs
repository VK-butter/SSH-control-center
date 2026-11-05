//! Tags (roadmap entry 4): free-text markers this Master puts on its hosts, merged hosts included.
use base64::{engine::general_purpose::STANDARD, Engine};
use serde_json::{json, Value};
use sshcc_core::{config, fleet, protocol::Request, security, service::Service, tags};
use std::{
    fs,
    path::Path,
    sync::{atomic::AtomicBool, Arc},
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
fn call(state: &Path, cfg: &config::Config, method: &str, params: Value) -> anyhow::Result<Value> {
    let request = Request { method: method.into(), params };
    if let Some(value) = tags::handle(state, cfg, &request)? {
        return Ok(value);
    }
    fleet::handle(state, cfg, &request)?.ok_or_else(|| anyhow::anyhow!("{method} unhandled"))
}
/// A Master with two direct hosts and a merged Master (branch) whose host `child` is reached as `branch/child`.
fn master(dir: &Path) -> config::Config {
    let cfg = config::init(dir, "both", &security::current_principal().unwrap(), "Office", &[]).unwrap();
    fs::create_dir_all(dir.join("keys")).unwrap();
    fs::write(dir.join("keys/company.ed25519"), "test key placeholder").unwrap();
    for (id, address) in [("web-1", "10.0.0.1"), ("web-2", "10.0.0.2"), ("branch", "10.0.0.3")] {
        call(dir, &cfg, "targets.import", json!({"registration":host(id, address)})).unwrap();
    }
    let node = call(dir, &cfg, "targets.list", json!({})).unwrap().as_array().unwrap().iter()
        .find(|row| row["id"] == "branch").unwrap()["nodeId"].clone();
    fleet::record_merge(dir, "branch", &json!({"nodeId":node,"label":"Branch","targets":[
        {"id":"child","label":"Child","tenant":"lab","os":"windows","nodeId":Uuid::new_v4().to_string()}]})).unwrap();
    cfg
}
fn set(dir: &Path, cfg: &config::Config, id: &str, list: &[&str]) -> anyhow::Result<Value> {
    call(dir, cfg, "tags.set", json!({"targetId":id,"tags":list}))
}
fn list(dir: &Path, cfg: &config::Config) -> Value {
    call(dir, cfg, "tags.list", json!({})).unwrap()
}
fn error(result: anyhow::Result<Value>) -> String {
    format!("{:#}", result.unwrap_err())
}

#[test]
fn tags_are_trimmed_matched_without_case_and_keep_their_first_spelling() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = master(dir.path());
    assert_eq!(set(dir.path(), &cfg, "web-1", &["  Web ", "บางกอก"]).unwrap()["tags"], json!(["Web", "บางกอก"]));
    // Another host writes it differently: same Tag, shown as first typed. Duplicates in one request collapse.
    assert_eq!(set(dir.path(), &cfg, "web-2", &["WEB", "web"]).unwrap()["tags"], json!(["Web"]));
    assert_eq!(set(dir.path(), &cfg, "branch/child", &["web"]).unwrap()["tags"], json!(["Web"]));
    let all = list(dir.path(), &cfg);
    assert_eq!(all["tags"], json!([{"tag":"Web","hosts":["branch/child","web-1","web-2"]},{"tag":"บางกอก","hosts":["web-1"]}]));
    assert_eq!(all["hosts"]["web-1"], json!(["Web", "บางกอก"]));
    // A Tag no host uses is gone.
    set(dir.path(), &cfg, "web-1", &["Web"]).unwrap();
    assert!(!list(dir.path(), &cfg)["tags"].to_string().contains("บางกอก"));
}

#[test]
fn every_rule_has_its_own_code() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = master(dir.path());
    assert!(error(set(dir.path(), &cfg, "web-1", &[" "])).contains("E_TAG_LENGTH"));
    assert!(error(set(dir.path(), &cfg, "web-1", &[&"x".repeat(33)])).contains("E_TAG_LENGTH"));
    assert!(set(dir.path(), &cfg, "web-1", &[&"ก".repeat(32)]).is_ok(), "32 characters, not bytes");
    assert!(error(set(dir.path(), &cfg, "web-1", &["a,b"])).contains("E_TAG_CHARS"));
    assert!(error(set(dir.path(), &cfg, "web-1", &["a\tb"])).contains("E_TAG_CHARS"));
    let many: Vec<String> = (0..21).map(|n| format!("t{n}")).collect();
    let refs: Vec<&str> = many.iter().map(String::as_str).collect();
    assert!(error(set(dir.path(), &cfg, "web-1", &refs)).contains("E_TAG_LIMIT"));
    assert!(set(dir.path(), &cfg, "web-1", &refs[..20]).is_ok());
    assert!(error(set(dir.path(), &cfg, "nobody", &["web"])).contains("E_TARGET"), "only hosts this Master has");
    assert_eq!(list(dir.path(), &cfg)["hosts"]["web-1"].as_array().unwrap().len(), 20, "a refused set changes nothing");
}

#[test]
fn tags_never_cross_a_bridge_or_relay() {
    let dir = tempfile::tempdir().unwrap();
    let owner = security::current_principal().unwrap();
    config::init(dir.path(), "both", &owner, "Test", &[]).unwrap();
    let service = Service::new(dir.path().into(), Arc::new(AtomicBool::new(false))).unwrap();
    let peer = Uuid::new_v4().to_string();
    fleet::register_master(dir.path(), &json!({"id":peer,"label":"Peer","publicKey":key(3)})).unwrap();
    let fingerprint = fleet::public_key_fingerprint(&key(3)).unwrap();
    for method in ["tags.list", "tags.set"] {
        let bridged = Request { method: "bridge.request".into(), params: json!({"masterId":peer,"masterKey":fingerprint,
            "expectedTargetId":service.config.node_id,"request":{"method":method,"params":{}}}) };
        assert!(error(service.handle(&bridged, &owner)).contains("not available over bridge"), "{method} over a bridge");
        let relayed = fleet::relay_request(dir.path(), &service.config, &peer, &json!({"targetId":"child","method":method,"params":{}}));
        assert!(format!("{:#}", relayed.unwrap_err()).contains("E_PERMISSION"), "{method} over a relay");
    }
    assert_eq!(service.handle(&Request { method: "tags.list".into(), params: json!({}) }, &owner).unwrap()["tags"], json!([]));
}

#[test]
fn removing_a_host_removes_its_tags() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = master(dir.path());
    set(dir.path(), &cfg, "web-1", &["web"]).unwrap();
    set(dir.path(), &cfg, "web-2", &["web", "db"]).unwrap();
    call(dir.path(), &cfg, "targets.remove", json!({"targetId":"web-2"})).unwrap();
    let all = list(dir.path(), &cfg);
    assert_eq!(all["tags"], json!([{"tag":"web","hosts":["web-1"]}]), "db is gone with its only host");
    assert!(all["hosts"].get("web-2").is_none());
}
