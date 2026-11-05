//! Health alarms (roadmap entry 7). The step that asks a host is a closure and "now" is passed in, so these tests
//! drive passes over minutes and months with neither SSH nor waiting.
use base64::{engine::general_purpose::STANDARD, Engine};
use chrono::{DateTime, Duration, TimeZone, Utc};
use serde_json::{json, Value};
use sshcc_core::{
    alarms::{self, Ask},
    config, fleet, folders,
    protocol::Request,
    security, tags,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::Path,
    sync::Mutex,
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
        "hostKeys":[key(1)],"routes":[{"kind":"lan","address":format!("10.0.0.{n}"),"port":22}],"bridgePath":if os == "windows" { "C:\\Program Files\\SSH Control Center\\sshcc.exe" } else { "/usr/bin/sshcc" },"runnerInstalled":true})
}
/// web-1 and web-2 (Tag `web`, folder Web), db-1, and win-1 (Windows).
fn master(dir: &Path) -> config::Config {
    let cfg = config::init(dir, "master", &security::current_principal().unwrap(), "Office", &[]).unwrap();
    fs::create_dir_all(dir.join("keys")).unwrap();
    fs::write(dir.join("keys/company.ed25519"), "test key placeholder").unwrap();
    for (i, (id, os)) in [("web-1", "linux"), ("web-2", "linux"), ("db-1", "linux"), ("win-1", "windows")].iter().enumerate() {
        fleet::handle(dir, &cfg, &request("targets.import", json!({"registration":host(id, i as u8 + 1, os)}))).unwrap();
    }
    for id in ["web-1", "web-2"] {
        tags::set(dir, &cfg, id, &["web".into()]).unwrap();
    }
    cfg
}
/// A fake fleet: what each host reports, and a log of what was asked.
#[derive(Default)]
struct Fleet {
    disk: BTreeMap<String, f64>,
    memory: BTreeMap<String, f64>,
    down: BTreeSet<String>,
    containers: BTreeMap<String, Vec<(String, bool)>>,
    updates: BTreeMap<String, Vec<String>>,
    asked: Vec<(String, Ask)>,
}
impl Fleet {
    fn ask(&mut self, host: &Value, what: Ask) -> anyhow::Result<Value> {
        let id = host["id"].as_str().unwrap().to_owned();
        self.asked.push((id.clone(), what));
        anyhow::ensure!(!self.down.contains(&id), "ALL_ROUTES_FAILED: lan {id}:22: No route to host");
        Ok(match what {
            Ask::Stats => {
                let disk = self.disk.get(&id).copied().unwrap_or(40.0);
                let mem = self.memory.get(&id).copied().unwrap_or(30.0);
                json!({"disk":{"total":1000,"free":1000.0 - disk * 10.0},"memory":{"total":1000,"available":1000.0 - mem * 10.0}})
            }
            Ask::Containers => json!({"containers":self.containers.get(&id).cloned().unwrap_or_default().into_iter()
                .map(|(name, running)| json!({"name":name,"state":if running { "running" } else { "exited" }})).collect::<Vec<_>>()}),
            Ask::Updates => { let p = self.updates.get(&id).cloned().unwrap_or_default(); json!({"count":p.len(),"packages":p}) }
        })
    }
}
struct World {
    _dir: tempfile::TempDir,
    cfg: config::Config,
    fleet: Mutex<Fleet>,
    now: Mutex<DateTime<Utc>>,
}
impl World {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let cfg = master(dir.path());
        Self { _dir: dir, cfg, fleet: Mutex::new(Fleet::default()), now: Mutex::new(Utc.with_ymd_and_hms(2026, 10, 26, 8, 0, 0).unwrap()) }
    }
    fn state(&self) -> &Path {
        self._dir.path()
    }
    fn call(&self, method: &str, params: Value) -> anyhow::Result<Value> {
        alarms::handle(self.state(), &self.cfg, &request(method, params)).map(|v| v.unwrap())
    }
    /// Only these rules (the defaults go).
    fn only(&self, rules: &[Value]) {
        for rule in alarms::rules(self.state()).unwrap() {
            alarms::delete_rule(self.state(), rule["id"].as_str().unwrap()).unwrap();
        }
        for rule in rules {
            self.call("alarms.rule_set", rule.clone()).unwrap();
        }
    }
    /// One pass, 5 minutes after the previous one; what was asked.
    fn pass(&self) -> Vec<(String, Ask)> {
        let now = { let mut now = self.now.lock().unwrap(); *now += Duration::minutes(5); *now };
        self.fleet.lock().unwrap().asked.clear();
        alarms::pass(self.state(), &self.cfg, now, &|h, a| self.fleet.lock().unwrap().ask(h, a), None).unwrap();
        let mut asked = self.fleet.lock().unwrap().asked.clone();
        asked.sort();
        asked
    }
    fn active(&self) -> Vec<(String, String, String)> {
        let list = self.call("alarms.list", json!({})).unwrap();
        let mut rows: Vec<_> = list["alarms"].as_array().unwrap().iter().map(|a| (a["host"].as_str().unwrap().to_owned(),
            a["condition"].as_str().unwrap().to_owned(), a["level"].as_str().unwrap().to_owned())).collect();
        rows.sort();
        rows
    }
    fn set(&self, f: impl FnOnce(&mut Fleet)) {
        f(&mut self.fleet.lock().unwrap());
    }
}
fn rule(scope: Value, condition: &str, threshold: Value, level: &str) -> Value {
    json!({"scope":scope,"condition":condition,"threshold":threshold,"level":level})
}
fn hosts_of(asked: &[(String, Ask)]) -> BTreeSet<String> {
    asked.iter().map(|(h, _)| h.clone()).collect()
}

#[test]
fn a_new_master_watches_three_things_on_every_host_and_deleted_defaults_stay_deleted() {
    let w = World::new();
    let rules = alarms::rules(w.state()).unwrap();
    let mut shape: Vec<(String, Value)> = rules.iter().map(|r| (r["condition"].as_str().unwrap().to_owned(), r["threshold"].clone())).collect();
    shape.sort_by(|a, b| a.0.cmp(&b.0));
    assert_eq!(shape, [("disk".to_owned(), json!(90)), ("memory".to_owned(), json!(90)), ("unreachable".to_owned(), Value::Null)]);
    assert!(rules.iter().all(|r| r["level"] == "red" && r["scope"] == json!({"all":true})));
    w.only(&[]);
    assert!(alarms::rules(w.state()).unwrap().is_empty(), "deleting the defaults sticks");
}

#[test]
fn no_rules_means_nothing_is_asked_and_only_what_rules_need_is() {
    let w = World::new();
    w.only(&[]);
    assert!(w.pass().is_empty());
    w.only(&[rule(json!({"tag":"WEB"}), "disk", json!(90), "red")]);
    assert_eq!(w.pass(), [("web-1".to_owned(), Ask::Stats), ("web-2".to_owned(), Ask::Stats)], "Tag matched without case; stats only");
    w.only(&[rule(json!({"hosts":["db-1"]}), "container", Value::Null, "yellow")]);
    assert_eq!(w.pass(), [("db-1".to_owned(), Ask::Containers)]);
}

#[test]
fn every_scope_covers_the_right_hosts() {
    let w = World::new();
    let folder = folders::handle(w.state(), &w.cfg, &request("folders.create", json!({"name":"Web"}))).unwrap().unwrap();
    for id in ["web-1", "db-1"] {
        folders::handle(w.state(), &w.cfg, &request("folders.assign", json!({"targetId":id,"folderId":folder["id"]}))).unwrap();
    }
    let cases = [
        (json!({"all":true}), vec!["db-1", "web-1", "web-2", "win-1"]),
        (json!({"folder":folder["id"]}), vec!["db-1", "web-1"]),
        (json!({"tag":"web"}), vec!["web-1", "web-2"]),
        (json!({"hosts":["win-1","db-1"]}), vec!["db-1", "win-1"]),
    ];
    for (scope, hosts) in cases {
        w.only(&[rule(scope.clone(), "disk", json!(90), "red")]);
        assert_eq!(hosts_of(&w.pass()), hosts.into_iter().map(str::to_owned).collect(), "{scope}");
    }
}

#[test]
fn disk_fires_at_the_line_and_clears_two_points_under_it() {
    let w = World::new();
    w.only(&[rule(json!({"tag":"web"}), "disk", json!(90), "red")]);
    w.set(|f| { f.disk.insert("web-1".into(), 91.0); });
    w.pass();
    assert_eq!(w.active(), [("web-1".to_owned(), "disk".to_owned(), "red".to_owned())]);
    w.set(|f| { f.disk.insert("web-1".into(), 89.0); });
    w.pass();
    assert_eq!(w.active().len(), 1, "89% is within the margin: still active");
    w.set(|f| { f.disk.insert("web-1".into(), 87.5); });
    w.pass();
    assert!(w.active().is_empty());
}

#[test]
fn a_disk_rule_watches_the_fullest_partition_or_a_named_one() {
    let w = World::new();
    w.only(&[rule(json!({"hosts":["web-1"]}), "disk", json!(90), "red"), json!({"scope":{"hosts":["web-1","db-1"]},"condition":"disk","threshold":80,"level":"yellow","partition":"/var/"})]);
    // Hosts updated in entry 8 report every partition; db-1 doesn't have /var.
    let stats = |root: f64, var: f64| json!({"disks":[{"path":"/","total":100,"free":100.0 - root},{"path":"/var","total":100,"free":100.0 - var}],"memory":{"total":1,"available":1}});
    let fleet = Mutex::new(stats(40.0, 95.0));
    let pass = |now| alarms::pass(w.state(), &w.cfg, now, &|h, _| Ok(if h["id"] == "web-1" { fleet.lock().unwrap().clone() } else { json!({"disk":{"path":"/","total":100,"free":5},"memory":{"total":1,"available":1}}) }), None).unwrap();
    pass(Utc::now());
    let list = w.call("alarms.list", json!({})).unwrap();
    let mut seen: Vec<(String, String, String)> = list["alarms"].as_array().unwrap().iter()
        .map(|a| (a["host"].as_str().unwrap().into(), a["level"].as_str().unwrap().into(), a["partition"].as_str().unwrap_or("").into())).collect();
    seen.sort();
    assert_eq!(seen, [("web-1".into(), "red".into(), "/var".into()), ("web-1".into(), "yellow".into(), "/var".into())],
        "the fullest (/var 95%) fires the any-partition rule; db-1 has no /var, so the named rule stays quiet there");
    *fleet.lock().unwrap() = stats(40.0, 50.0);
    pass(Utc::now() + Duration::minutes(5));
    assert!(w.active().is_empty());
    // An older host reports only `disk`: the any-partition rule still reads it.
    assert!(format!("{:#}", w.call("alarms.rule_set", json!({"scope":{"all":true},"condition":"disk","threshold":90,"level":"red","partition":"/x\u{1}"})).unwrap_err()).contains("E_ALARM_RULE"));
}

#[test]
fn memory_and_unreachable_need_two_checks_in_a_row() {
    let w = World::new();
    w.only(&[rule(json!({"all":true}), "memory", json!(90), "red"), rule(json!({"all":true}), "unreachable", Value::Null, "red")]);
    w.set(|f| { f.memory.insert("db-1".into(), 95.0); f.down.insert("web-2".into()); });
    w.pass();
    assert!(w.active().is_empty(), "one reading is a spike, one failure a blip");
    w.set(|f| { f.memory.insert("db-1".into(), 60.0); });
    w.pass();
    assert_eq!(w.active(), [("web-2".to_owned(), "unreachable".to_owned(), "red".to_owned())], "the spike reset; web-2 failed twice");
    w.set(|f| { f.memory.insert("db-1".into(), 95.0); });
    w.pass();
    w.pass();
    assert!(w.active().contains(&("db-1".to_owned(), "memory".to_owned(), "red".to_owned())));
    w.set(|f| { f.down.clear(); f.memory.insert("db-1".into(), 80.0); });
    w.pass();
    assert!(w.active().is_empty(), "back and under the margin");
}

#[test]
fn only_containers_that_were_running_count_and_names_filter() {
    let w = World::new();
    w.only(&[rule(json!({"hosts":["db-1"]}), "container", Value::Null, "yellow"), json!({"scope":{"hosts":["web-1"]},"condition":"container","containers":["api"],"level":"red"})]);
    w.set(|f| {
        f.containers.insert("db-1".into(), vec![("pg".into(), true), ("old".into(), false)]);
        f.containers.insert("web-1".into(), vec![("api".into(), true), ("cron".into(), true)]);
    });
    w.pass();
    assert!(w.active().is_empty(), "a container stopped on purpose doesn't count");
    w.set(|f| {
        f.containers.insert("db-1".into(), vec![("pg".into(), false), ("old".into(), false)]);
        f.containers.insert("web-1".into(), vec![("api".into(), true), ("cron".into(), false)]);
    });
    w.pass();
    assert_eq!(w.active(), [("db-1".to_owned(), "container".to_owned(), "yellow".to_owned())], "cron isn't watched on web-1");
    let value = w.call("alarms.list", json!({})).unwrap()["alarms"][0]["value"].clone();
    assert_eq!(value, json!(["pg"]));
    w.set(|f| { f.containers.insert("db-1".into(), vec![("pg".into(), true)]); });
    w.pass();
    assert!(w.active().is_empty());
}

#[test]
fn updates_are_checked_monthly_on_demand_and_never_on_a_view_only_host() {
    let w = World::new();
    w.only(&[rule(json!({"hosts":["web-1"]}), "updates", Value::Null, "yellow")]);
    w.set(|f| { f.updates.insert("web-1".into(), vec!["openssl".into(), "curl".into()]); });
    assert_eq!(w.pass(), [("web-1".to_owned(), Ask::Updates)]);
    assert_eq!(w.active(), [("web-1".to_owned(), "updates".to_owned(), "yellow".to_owned())]);
    assert!(w.pass().is_empty(), "checked this month already");
    let info = w.call("alarms.host", json!({"targetId":"web-1"})).unwrap();
    assert_eq!((info["updates"]["count"].as_u64(), info["updates"]["packages"].clone()), (Some(2), json!(["openssl", "curl"])));
    // A month later it asks again; installed meanwhile, so the alarm clears.
    w.set(|f| { f.updates.insert("web-1".into(), vec![]); });
    *w.now.lock().unwrap() += Duration::days(30);
    assert_eq!(w.pass(), [("web-1".to_owned(), Ask::Updates)]);
    assert!(w.active().is_empty());
    // Check now, on a host no rule covers, and on a host this Master may only view.
    let ids: BTreeSet<String> = ["db-1".to_owned()].into();
    w.set(|f| { f.updates.insert("db-1".into(), vec!["vim".into()]); });
    alarms::check_now(w.state(), &w.cfg, &ids, &[Ask::Updates], &|h, a| w.fleet.lock().unwrap().ask(h, a)).unwrap();
    assert_eq!(w.call("alarms.host", json!({"targetId":"db-1"})).unwrap()["updates"]["count"], 1);
}

#[test]
fn the_update_output_of_both_systems_is_read() {
    let apt = "Listing...\nopenssl/noble-updates 3.0.13-0ubuntu3.4 amd64 [upgradable from: 3.0.13-0ubuntu3.1]\ncurl/noble-security 8.5.0-2ubuntu10.5 amd64 [upgradable from: 8.5.0-2ubuntu10.4]\n";
    assert_eq!(alarms::parse_apt(apt), json!({"count":2,"packages":["openssl","curl"]}));
    assert_eq!(alarms::parse_apt("Listing...\n"), json!({"count":0,"packages":[]}));
    // Recorded from a real Ubuntu 24.04 container (2026-10-26) after `apt-get update`, with the image's older packages.
    let recorded = "Listing...\nliblzma5/noble-updates,noble-security 5.6.1+really5.4.5-1ubuntu0.4 arm64 [upgradable from: 5.6.1+really5.4.5-1ubuntu0.3]\nlibsystemd0/noble-updates,noble-security 255.4-1ubuntu8.18 arm64 [upgradable from: 255.4-1ubuntu8.17]\nlibudev1/noble-updates,noble-security 255.4-1ubuntu8.18 arm64 [upgradable from: 255.4-1ubuntu8.17]\nzlib1g/noble-updates,noble-security 1:1.3.dfsg-3.1ubuntu2.3 arm64 [upgradable from: 1:1.3.dfsg-3.1ubuntu2.2]\n";
    assert_eq!(alarms::parse_apt(recorded), json!({"count":4,"packages":["liblzma5","libsystemd0","libudev1","zlib1g"]}));
    assert_eq!(alarms::parse_windows("[\"2026-10 Cumulative Update for Windows 11\",\"Defender update\"]").unwrap()["count"], 2);
    assert_eq!(alarms::parse_windows("\"One update\"").unwrap()["packages"], json!(["One update"]));
    assert_eq!(alarms::parse_windows("[]").unwrap()["count"], 0);
    assert_eq!(alarms::parse_windows("").unwrap()["count"], 0);
    assert!(alarms::parse_windows("Access is denied").is_err());
}

#[test]
fn acknowledged_alarms_stop_counting_until_they_clear_and_a_recurrence_is_new() {
    let w = World::new();
    w.only(&[rule(json!({"hosts":["web-1"]}), "disk", json!(80), "yellow"), rule(json!({"hosts":["web-1"]}), "disk", json!(90), "red")]);
    w.set(|f| { f.disk.insert("web-1".into(), 95.0); });
    w.pass();
    let list = w.call("alarms.list", json!({})).unwrap();
    assert_eq!((list["red"].as_u64(), list["yellow"].as_u64()), (Some(1), Some(1)), "two rules, two levels");
    let red = list["alarms"][0]["id"].as_str().unwrap().to_owned();
    assert_eq!(list["alarms"][0]["level"], "red", "red first");
    w.call("alarms.ack", json!({"id":red})).unwrap();
    let list = w.call("alarms.list", json!({})).unwrap();
    assert_eq!((list["red"].as_u64(), list["alarms"].as_array().unwrap().len()), (Some(0), 2), "still listed, greyed");
    w.set(|f| { f.disk.insert("web-1".into(), 50.0); });
    w.pass();
    w.set(|f| { f.disk.insert("web-1".into(), 95.0); });
    w.pass();
    assert_eq!(w.call("alarms.list", json!({})).unwrap()["red"], 1, "a new occurrence alarms again");
}

#[test]
fn rules_and_the_interval_are_checked_and_the_interval_is_honoured() {
    let w = World::new();
    for bad in [json!({"scope":{},"condition":"disk","threshold":90,"level":"red"}), json!({"scope":{"all":true},"condition":"cpu","level":"red"}),
        json!({"scope":{"all":true},"condition":"disk","threshold":100,"level":"red"}), json!({"scope":{"all":true},"condition":"disk","threshold":90,"level":"blue"})] {
        assert!(format!("{:#}", w.call("alarms.rule_set", bad).unwrap_err()).contains("E_ALARM_RULE"));
    }
    assert!(format!("{:#}", w.call("alarms.settings", json!({"interval":10})).unwrap_err()).contains("E_ALARM_INTERVAL"));
    w.call("alarms.settings", json!({"interval":15})).unwrap();
    let ask = |_: &Value, _: Ask| Ok(json!({"disk":{"total":1,"free":1},"memory":{"total":1,"available":1}}));
    let start = Utc.with_ymd_and_hms(2026, 10, 26, 8, 0, 0).unwrap();
    assert!(alarms::tick(w.state(), &w.cfg, start, &ask).unwrap(), "first pass");
    assert!(!alarms::tick(w.state(), &w.cfg, start + Duration::minutes(10), &ask).unwrap());
    assert!(!alarms::due(w.state(), &w.cfg, start + Duration::minutes(10)));
    assert!(alarms::due(w.state(), &w.cfg, start + Duration::minutes(15)));
    assert!(alarms::tick(w.state(), &w.cfg, start + Duration::minutes(15), &ask).unwrap());
}

#[test]
fn a_view_only_merged_host_reports_that_updates_cannot_be_checked() {
    let w = World::new();
    let peer = host("branch", 9, "linux");
    fleet::handle(w.state(), &w.cfg, &request("targets.import", json!({"registration":peer}))).unwrap();
    fleet::record_merge(w.state(), "branch", &json!({"nodeId":peer["nodeId"],"label":"Branch","grant":{"role":"view"},"relayVersion":2,
        "targets":[{"id":"far","label":"far","tenant":"company","os":"linux","nodeId":Uuid::new_v4().to_string()}]})).unwrap();
    w.only(&[rule(json!({"hosts":["branch/far"]}), "updates", Value::Null, "yellow")]);
    assert!(w.pass().is_empty(), "no job is tried on a View-only host");
    assert_eq!(w.call("alarms.host", json!({"targetId":"branch/far"})).unwrap()["updates"]["error"], "view-only");
}
