//! Multi-hop merge (roadmap entry 6, Stage B). Masters run in this process; the step that sends a request to the
//! next Master (`fleet::set_hop`) calls that Master's service through its bridge, as SSH would. Only the last step,
//! from the last Master to its own host, is real SSH, to a closed local port, so a request that gets there fails
//! with that Master's route error.
use base64::{engine::general_purpose::STANDARD, Engine};
use serde_json::{json, Value};
use sshcc_core::{config, fleet, protocol::Request, security, service::Service};
use std::{
    collections::BTreeMap,
    fs,
    path::Path,
    sync::{atomic::AtomicBool, Arc, Mutex},
    time::{Duration, Instant},
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
/// A host on a closed port of its own loopback address.
fn registration(id: &str, node: &str, n: u8) -> Value {
    json!({"schemaVersion":2,"id":id,"nodeId":node,"label":id,"tenant":"company","os":"linux","user":"sshcc-bridge",
        "hostKeys":[key(1)],"routes":[{"kind":"lan","address":format!("127.0.{n}.1"),"port":1}],"bridgePath":"/usr/bin/sshcc","runnerInstalled":true})
}

struct Master {
    dir: tempfile::TempDir,
    service: Arc<Service>,
    seed: u8,
    owner: String,
}
impl Master {
    fn state(&self) -> &Path {
        self.dir.path()
    }
    fn config(&self) -> &config::Config {
        &self.service.config
    }
    fn node(&self) -> String {
        self.config().node_id.clone()
    }
    fn call(&self, method: &str, params: Value) -> anyhow::Result<Value> {
        self.service.handle_as(&request(method, params), &self.owner, Some("desktop"))
    }
    fn hosts(&self) -> Vec<Value> {
        self.call("targets.list", json!({})).unwrap().as_array().unwrap().clone()
    }
    fn host(&self, id: &str) -> Option<Value> {
        self.hosts().into_iter().find(|h| h["id"] == id)
    }
    fn audit(&self) -> Vec<Value> {
        fs::read_to_string(self.state().join("audit.jsonl")).unwrap_or_default().lines().map(|l| serde_json::from_str(l).unwrap()).collect()
    }
}

/// In-process Masters by node ID, and Masters whose service doesn't answer.
#[derive(Default)]
struct Net {
    masters: BTreeMap<String, Arc<Service>>,
    keys: BTreeMap<String, u8>,
    silent: Vec<String>,
}
static NET: Mutex<Option<Net>> = Mutex::new(None);
static SERIAL: Mutex<()> = Mutex::new(());

/// The network: Masters named by `names` (each with one host of its own, `t-<name>`), `links` saying who merges whom.
struct World {
    masters: BTreeMap<String, Master>,
    _serial: std::sync::MutexGuard<'static, ()>,
}
impl World {
    fn new(names: &[&str]) -> Self {
        let serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        *NET.lock().unwrap() = Some(Net::default());
        fleet::set_hop(Some(Arc::new(|_state: &Path, cfg: &config::Config, peer: &fleet::Target, method: &str, params: Value, _timeout: Duration| {
            let (service, seed, silent) = {
                let net = NET.lock().unwrap();
                let net = net.as_ref().unwrap();
                (net.masters.get(&peer.node_id).cloned(), net.keys[&cfg.node_id], net.silent.contains(&peer.node_id))
            };
            let service = service.ok_or_else(|| anyhow::anyhow!("ALL_ROUTES_FAILED: no such Master"))?;
            if silent {
                std::thread::sleep(Duration::from_secs(30));
            }
            let fingerprint = fleet::public_key_fingerprint(&key(seed))?;
            let owner = security::current_principal()?;
            service.handle(&request("bridge.request", json!({"masterId":cfg.node_id,"masterKey":fingerprint,"expectedTargetId":service.config.node_id,
                "request":{"method":method,"params":params}})), &owner)
        })));
        let owner = security::current_principal().unwrap();
        let mut masters = BTreeMap::new();
        for (i, name) in names.iter().enumerate() {
            let dir = tempfile::tempdir().unwrap();
            let label = name.to_uppercase();
            let cfg = config::init(dir.path(), "both", &owner, &label, &[]).unwrap();
            fs::create_dir_all(dir.path().join("keys")).unwrap();
            fs::write(dir.path().join("keys/company.ed25519"), "test key placeholder").unwrap();
            let host = format!("t-{name}");
            fleet::handle(dir.path(), &cfg, &request("targets.import", json!({"registration":registration(&host, &Uuid::new_v4().to_string(), i as u8 + 10)}))).unwrap();
            let service = Arc::new(Service::new(dir.path().into(), Arc::new(AtomicBool::new(false))).unwrap());
            let seed = i as u8 + 20;
            let net = &mut NET.lock().unwrap();
            let net = net.as_mut().unwrap();
            net.masters.insert(cfg.node_id.clone(), service.clone());
            net.keys.insert(cfg.node_id.clone(), seed);
            masters.insert(name.to_string(), Master { dir, service, seed, owner: owner.clone() });
        }
        Self { masters, _serial: serial }
    }
    fn m(&self, name: &str) -> &Master {
        &self.masters[name]
    }
    /// `from` merges `to`: `to` is one of `from`'s hosts (ID = `to`'s name), and `to` allows `from`'s key.
    fn link(&self, from: &str, to: &str, n: u8) {
        let (a, b) = (self.m(from), self.m(to));
        fleet::handle(a.state(), a.config(), &request("targets.import", json!({"registration":registration(to, &b.node(), n)}))).unwrap();
        fleet::register_master(b.state(), &json!({"id":a.node(),"label":a.config().label,"publicKey":key(a.seed)})).unwrap();
        self.sync(from, to);
    }
    fn sync(&self, from: &str, to: &str) {
        self.m(from).call("merges.sync", json!({"targetId":to})).unwrap();
    }
    fn grant(&self, on: &str, to: &str, params: Value) {
        let mut params = params;
        params["masterId"] = json!(self.m(to).node());
        self.m(on).call("relays.set", params).unwrap();
    }
    fn silence(&self, name: &str) {
        NET.lock().unwrap().as_mut().unwrap().silent.push(self.m(name).node());
    }
}
impl Drop for World {
    fn drop(&mut self) {
        fleet::set_hop(None);
    }
}
fn error(result: anyhow::Result<Value>) -> String {
    format!("{:#}", result.unwrap_err())
}

#[test]
fn a_host_two_masters_away_is_listed_with_its_path_and_reached_through_both() {
    let w = World::new(&["a", "b", "c"]);
    w.link("b", "c", 50);
    w.link("a", "b", 51);
    let far = w.m("a").host("b/c/t-c").expect("listed after one sync, no other action");
    assert_eq!((far["hops"].as_u64(), far["path"].clone(), far["access"].as_str()), (Some(2), json!(["B", "C"]), Some("full")));
    assert_eq!(w.m("a").host("b/t-b").unwrap()["path"], json!(["B"]));
    // A change on the far host goes A → B → C; C's own SSH call to its host is what fails.
    let failed = error(w.m("a").call("files.remove", json!({"targetId":"b/c/t-c","path":"/tmp/x"})));
    assert!(failed.contains("127.0."), "reached C's own route: {failed}");
    let at = |name: &str| w.m(name).audit().into_iter().find(|l| l["action"] == "files.remove").unwrap();
    assert_eq!((at("b")["channel"].as_str(), at("b")["object"].as_str()), (Some("merge: A"), Some("c/t-c · /tmp/x")));
    assert_eq!((at("c")["channel"].as_str(), at("c")["object"].as_str()), (Some("merge: A → B"), Some("t-c · /tmp/x")));
}

#[test]
fn hosts_up_to_four_masters_away_are_listed_and_the_rest_counted() {
    let w = World::new(&["a", "b", "c", "d", "e", "f"]);
    for (i, (from, to)) in [("e", "f"), ("d", "e"), ("c", "d"), ("b", "c"), ("a", "b")].iter().enumerate() {
        w.link(from, to, 60 + i as u8);
    }
    let mut ids: Vec<String> = w.m("a").hosts().iter().filter(|h| h["relayed"] == true).map(|h| h["id"].as_str().unwrap().to_owned()).collect();
    ids.sort();
    // Each Master's machine is a host too (b/c is C itself, reached through B).
    assert_eq!(ids, ["b/c", "b/c/d", "b/c/d/e", "b/c/d/e/f", "b/c/d/e/t-e", "b/c/d/t-d", "b/c/t-c", "b/t-b"]);
    let merge = w.m("a").call("merges.list", json!({})).unwrap()[0].clone();
    assert_eq!(merge["beyond"], 1, "t-f is 5 Masters away");
    // A request past four Masters is refused even when addressed by hand.
    assert!(error(w.m("a").call("jobs.list", json!({"targetId":"b/c/d/e/f/t-f"}))).contains("E_RELAY_TOO_FAR"));
}

#[test]
fn loops_are_dropped_from_the_list_and_refused_on_the_wire() {
    let w = World::new(&["a", "b"]);
    w.link("a", "b", 70);
    w.link("b", "a", 71);
    w.sync("a", "b");
    let relayed: Vec<String> = w.m("a").hosts().iter().filter(|h| h["relayed"] == true).map(|h| h["id"].as_str().unwrap().to_owned()).collect();
    assert_eq!(relayed, ["b/t-b"], "A's own host doesn't come back through B");
    // A forged path that already holds B.
    let (a, b) = (w.m("a"), w.m("b"));
    let fingerprint = fleet::public_key_fingerprint(&key(a.seed)).unwrap();
    let forged = b.service.handle(&request("bridge.request", json!({"masterId":a.node(),"masterKey":fingerprint,"expectedTargetId":b.node(),
        "request":{"method":"relay.request","params":{"targetId":"t-b","method":"jobs.list","params":{},
            "path":[{"id":b.node(),"label":"B"},{"id":a.node(),"label":"A"}]}}})), &b.owner);
    assert!(error(forged).contains("E_RELAY_LOOP"));
}

#[test]
fn the_lowest_grant_along_the_path_wins_and_is_reported_with_who_set_it() {
    let w = World::new(&["a", "b", "c"]);
    w.link("b", "c", 80);
    w.grant("c", "b", json!({"role":"view"}));
    w.sync("b", "c");
    w.link("a", "b", 81);
    let far = w.m("a").host("b/c/t-c").unwrap();
    assert_eq!((far["access"].as_str(), far["accessFrom"].as_str()), (Some("view"), Some("C")));
    assert_eq!(w.m("a").host("b/t-b").unwrap()["access"], "full", "B itself grants Full control");
    let refused = error(w.m("a").call("files.remove", json!({"targetId":"b/c/t-c","path":"/tmp/x"})));
    assert!(refused.contains("E_MERGE_ROLE: C gives this Master View only"), "{refused}");
    // B's own grant to A is lower still for everything behind it.
    w.grant("b", "a", json!({"role":"view"}));
    w.sync("a", "b");
    assert_eq!(w.m("a").host("b/t-b").unwrap()["accessFrom"], "B");
    assert!(error(w.m("a").call("files.remove", json!({"targetId":"b/t-b","path":"/tmp/x"}))).contains("E_MERGE_ROLE: B gives"));
}

#[test]
fn pass_through_off_keeps_merged_hosts_to_the_granting_master() {
    let w = World::new(&["a", "b", "c"]);
    w.link("b", "c", 90);
    w.link("a", "b", 91);
    assert!(w.m("a").host("b/c/t-c").is_some());
    w.grant("b", "a", json!({"passThrough":false}));
    w.sync("a", "b");
    assert!(w.m("a").host("b/c/t-c").is_none());
    assert!(w.m("a").host("b/t-b").is_some(), "B's own hosts still");
    let fingerprint = fleet::public_key_fingerprint(&key(w.m("a").seed)).unwrap();
    let by_hand = w.m("b").service.handle(&request("bridge.request", json!({"masterId":w.m("a").node(),"masterKey":fingerprint,"expectedTargetId":w.m("b").node(),
        "request":{"method":"relay.request","params":{"targetId":"c/t-c","method":"jobs.list","params":{}}}})), &w.m("b").owner);
    assert!(error(by_hand).contains("doesn't pass on"));
}

#[test]
fn the_route_with_fewest_hops_wins_and_a_host_is_listed_once() {
    let w = World::new(&["a", "b", "c"]);
    w.link("b", "c", 100);
    w.link("a", "b", 101);
    w.link("a", "c", 102);
    let t_c: Vec<Value> = w.m("a").hosts().into_iter().filter(|h| h["label"] == "t-c").collect();
    assert_eq!(t_c.len(), 1);
    assert_eq!((t_c[0]["id"].as_str(), t_c[0]["hops"].as_u64()), (Some("c/t-c"), Some(1)), "direct merge beats the way through B");
}

#[test]
fn a_master_that_does_not_answer_is_named_within_the_time_given() {
    let w = World::new(&["a", "b", "c"]);
    w.link("b", "c", 110);
    w.link("a", "b", 111);
    w.silence("c");
    // Ask B as A would, with 7 seconds left: B keeps 5 for itself and gives C the rest.
    let fingerprint = fleet::public_key_fingerprint(&key(w.m("a").seed)).unwrap();
    let started = Instant::now();
    let answer = w.m("b").service.handle(&request("bridge.request", json!({"masterId":w.m("a").node(),"masterKey":fingerprint,"expectedTargetId":w.m("b").node(),
        "request":{"method":"relay.request","params":{"targetId":"c/t-c","method":"jobs.list","params":{},"timeoutMs":7000,
            "path":[{"id":w.m("a").node(),"label":"A"}]}}})), &w.m("b").owner);
    let took = started.elapsed();
    assert!(error(answer).contains("E_RELAY_TIMEOUT: c didn't answer in time"), "named by the host name B gave it");
    assert!(took < Duration::from_secs(10), "{took:?}");
}

#[test]
fn an_older_master_counts_as_full_and_its_merge_is_marked() {
    let w = World::new(&["a"]);
    let a = w.m("a");
    let peer = Uuid::new_v4().to_string();
    fleet::handle(a.state(), a.config(), &request("targets.import", json!({"registration":registration("old", &peer, 120)}))).unwrap();
    fleet::record_merge(a.state(), "old", &json!({"nodeId":peer,"label":"Old","targets":[{"id":"x","label":"x","tenant":"company","os":"linux","nodeId":Uuid::new_v4().to_string()}]})).unwrap();
    let merge = a.call("merges.list", json!({})).unwrap()[0].clone();
    assert!(merge["relayVersion"].is_null(), "the UI shows it as outdated");
    assert_eq!(a.host("old/x").unwrap()["access"], "full");
}
