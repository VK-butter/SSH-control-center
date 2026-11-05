//! Merge Roles (roadmap entry 6, Stage A): what this Master grants each Master that merged it. Relayed requests go
//! through `Service::handle` as a merging Master's bridge would send them; hosts point at a closed local port, so an
//! allowed request fails on the way to the host, never with `E_MERGE_ROLE`.
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
/// A host on a closed port of its own loopback address (routes must differ).
fn host(id: &str) -> Value {
    let n = id.bytes().fold(7u32, |h, b| h.wrapping_mul(31).wrapping_add(u32::from(b))) % 250 + 2;
    json!({"schemaVersion":2,"id":id,"nodeId":Uuid::new_v4().to_string(),"label":id,"tenant":"company","os":"linux",
        "user":"sshcc-bridge","hostKeys":[key(1)],"routes":[{"kind":"lan","address":format!("127.0.0.{n}"),"port":1}],
        "bridgePath":"/usr/bin/sshcc","runnerInstalled":true})
}
fn request(method: &str, params: Value) -> Request {
    Request { method: method.into(), params }
}
/// "Office", with web-1 (Tag `web`) and db-1, merged by the Master "Peer".
struct Office {
    _dir: tempfile::TempDir,
    service: Service,
    owner: String,
    peer: String,
    fingerprint: String,
}
impl Office {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let owner = security::current_principal().unwrap();
        let cfg = config::init(dir.path(), "both", &owner, "Office", &[]).unwrap();
        fs::create_dir_all(dir.path().join("keys")).unwrap();
        fs::write(dir.path().join("keys/company.ed25519"), "test key placeholder").unwrap();
        for id in ["web-1", "db-1"] {
            fleet::handle(dir.path(), &cfg, &request("targets.import", json!({"registration":host(id)}))).unwrap();
        }
        tags::set(dir.path(), &cfg, "web-1", &["web".into()]).unwrap();
        let peer = Uuid::new_v4().to_string();
        fleet::register_master(dir.path(), &json!({"id":peer,"label":"Peer","publicKey":key(3)})).unwrap();
        let fingerprint = fleet::public_key_fingerprint(&key(3)).unwrap();
        let service = Service::new(dir.path().into(), Arc::new(AtomicBool::new(false))).unwrap();
        let office = Self { _dir: dir, service, owner, peer, fingerprint };
        office.bridge("relay.join", json!({})).unwrap();
        office
    }
    fn state(&self) -> &Path {
        self._dir.path()
    }
    /// A request from Peer through the bridge.
    fn bridge(&self, method: &str, params: Value) -> anyhow::Result<Value> {
        self.service.handle(&request("bridge.request", json!({"masterId":self.peer,"masterKey":self.fingerprint,
            "expectedTargetId":self.service.config.node_id,"request":{"method":method,"params":params}})), &self.owner)
    }
    /// Peer relays `method` to one of Office's hosts; the error text, or "ok".
    fn relay(&self, host: &str, method: &str) -> String {
        match self.bridge("relay.request", json!({"targetId":host,"method":method,"params":{"jobId":"j1","path":"/tmp/x","script":"id","cwd":"/"}})) {
            Ok(_) => "ok".into(),
            Err(error) => format!("{error:#}"),
        }
    }
    fn set(&self, params: Value) -> anyhow::Result<Value> {
        let mut params = params;
        params["masterId"] = json!(self.peer);
        self.service.handle_as(&request("relays.set", params), &self.owner, Some("desktop"))
    }
    fn listed(&self) -> Vec<String> {
        let joined = self.bridge("relay.join", json!({})).unwrap();
        joined["targets"].as_array().unwrap().iter().map(|t| t["id"].as_str().unwrap().to_owned()).collect()
    }
    fn audit(&self) -> Vec<Value> {
        fs::read_to_string(self.state().join("audit.jsonl")).unwrap_or_default().lines().map(|l| serde_json::from_str(l).unwrap()).collect()
    }
}

const READS: &[&str] = &["targets.doctor", "jobs.list", "jobs.status", "jobs.logs", "files.list", "files.read", "files.search", "files.grep",
    "containers.list", "containers.logs", "monitor.stats"];
const CHANGES: &[&str] = &["jobs.start", "jobs.cancel", "files.write", "files.upload", "files.mkdir", "files.remove",
    "terminal.open", "terminal.read", "terminal.write", "terminal.status", "terminal.resize", "terminal.close", "containers.action"];

#[test]
fn a_new_merge_is_full_control_and_says_so_in_its_join_reply() {
    let office = Office::new();
    let joined = office.bridge("relay.join", json!({})).unwrap();
    assert_eq!(joined["grant"], json!({"role":"full","tags":null,"passThrough":true}));
    assert_eq!(joined["relayVersion"], 2);
    for method in READS.iter().chain(CHANGES) {
        assert!(!office.relay("db-1", method).contains("E_MERGE_ROLE"), "{method} under Full control");
    }
}

#[test]
fn view_only_allows_every_read_and_refuses_every_change() {
    let office = Office::new();
    office.set(json!({"role":"view"})).unwrap();
    for method in READS {
        assert!(!office.relay("db-1", method).contains("E_MERGE_ROLE"), "{method} is a read");
    }
    for method in CHANGES {
        let error = office.relay("db-1", method);
        assert!(error.contains("E_MERGE_ROLE: Office gives this Master View only"), "{method}: {error}");
    }
    // The next request after a change uses the new role.
    office.set(json!({"role":"full"})).unwrap();
    assert!(!office.relay("db-1", "terminal.write").contains("E_MERGE_ROLE"));
}

#[test]
fn a_tag_limit_hides_other_hosts_and_follows_the_tags() {
    let office = Office::new();
    assert_eq!(office.listed(), ["db-1", "web-1"]);
    office.set(json!({"tags":["WEB"]})).unwrap();
    assert_eq!(office.listed(), ["web-1"], "matched without case; db-1 isn't listed at all");
    assert!(office.relay("db-1", "jobs.list").contains("E_MERGE_ROLE: Office gives this Master only hosts with Tag WEB"));
    assert!(!office.relay("web-1", "jobs.start").contains("E_MERGE_ROLE"), "Full control on the hosts it reaches");
    // Tagging db-1 shares it at once; View only applies within the limit too.
    tags::set(office.state(), &office.service.config, "db-1", &["web".into()]).unwrap();
    assert_eq!(office.listed(), ["db-1", "web-1"]);
    office.set(json!({"role":"view"})).unwrap();
    assert!(office.relay("db-1", "jobs.start").contains("View only"));
    office.set(json!({"tags":null})).unwrap();
    assert_eq!(office.bridge("relay.join", json!({})).unwrap()["grant"], json!({"role":"view","tags":null,"passThrough":true}));
}

#[test]
fn grants_are_listed_validated_and_audited() {
    let office = Office::new();
    let row = office.set(json!({"role":"view","tags":["web","Web"],"passThrough":false})).unwrap();
    assert_eq!((row["role"].as_str(), row["tags"].clone(), row["passThrough"].as_bool()), (Some("view"), json!(["web"]), Some(false)));
    let listed = office.service.handle(&request("relays.list", json!({})), &office.owner).unwrap();
    assert_eq!(listed[0]["role"], "view");
    assert!(format!("{:#}", office.set(json!({"role":"admin"})).unwrap_err()).contains("role is full or view"));
    assert!(format!("{:#}", office.set(json!({"tags":[]})).unwrap_err()).contains("at least one Tag"));
    assert!(format!("{:#}", office.set(json!({"tags":["x".repeat(33)]})).unwrap_err()).contains("E_TAG_LENGTH"));
    // Never over a bridge or a relay.
    assert!(format!("{:#}", office.bridge("relays.set", json!({"masterId":office.peer,"role":"full"})).unwrap_err()).contains("not available over bridge"));
    assert!(office.relay("db-1", "relays.set").contains("not available over relay"));
    // A refused relay and every change of grant are in the audit log.
    let _ = office.relay("db-1", "files.remove");
    let audit = office.audit();
    let set = audit.iter().find(|l| l["action"] == "relays.set").unwrap();
    assert_eq!(set["object"], format!("{} · view · tags web, Web · pass-through off", office.peer));
    let refused = audit.iter().find(|l| l["action"] == "files.remove").unwrap();
    assert_eq!((refused["result"].as_str(), refused["channel"].as_str()), (Some("E_MERGE_ROLE"), Some("merge: Peer")));
}

#[test]
fn the_merging_master_marks_view_only_hosts() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = config::init(dir.path(), "master", &security::current_principal().unwrap(), "Home", &[]).unwrap();
    fs::create_dir_all(dir.path().join("keys")).unwrap();
    fs::write(dir.path().join("keys/company.ed25519"), "test key placeholder").unwrap();
    let list = |id: &str| -> Vec<(String, String)> {
        fleet::handle(dir.path(), &cfg, &request("targets.list", json!({}))).unwrap().unwrap().as_array().unwrap().iter()
            .filter(|t| t["relayed"] == true && t["via"] == id).map(|t| (t["id"].as_str().unwrap().to_owned(), t["access"].as_str().unwrap().to_owned())).collect()
    };
    let child = |id: &str| json!({"id":id,"label":id,"tenant":"company","os":"linux","nodeId":Uuid::new_v4().to_string()});
    for (id, grant) in [("office", json!({"role":"view","tags":["web"],"passThrough":true})), ("old", Value::Null)] {
        let peer = host(id);
        fleet::handle(dir.path(), &cfg, &request("targets.import", json!({"registration":peer}))).unwrap();
        let mut join = json!({"nodeId":peer["nodeId"],"label":id,"targets":[child("web-1")]});
        if !grant.is_null() {
            join["grant"] = grant;
            join["relayVersion"] = json!(2);
        }
        let merge = fleet::record_merge(dir.path(), id, &join).unwrap();
        assert_eq!(merge["grant"]["role"].as_str(), if id == "office" { Some("view") } else { None });
    }
    assert_eq!(list("office"), [("office/web-1".to_owned(), "view".to_owned())]);
    assert_eq!(list("old"), [("old/web-1".to_owned(), "full".to_owned())], "an older Master grants Full control, as before");
}
