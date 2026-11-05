//! `pairing.create` with `os: "unknown"`: two linked requests (Windows and Ubuntu); the first one
//! completed wins and the other is cancelled.
use base64::{engine::general_purpose::STANDARD, Engine};
use serde_json::{json, Value};
use sshcc_core::{config::Config, pairing, protocol::Request};
use std::{fs, net::TcpListener, path::Path};
use uuid::Uuid;

fn key(seed: u8) -> String {
    let mut wire = Vec::new();
    wire.extend_from_slice(&11u32.to_be_bytes());
    wire.extend_from_slice(b"ssh-ed25519");
    wire.extend_from_slice(&32u32.to_be_bytes());
    wire.extend_from_slice(&[seed; 32]);
    format!("ssh-ed25519 {}", STANDARD.encode(wire))
}
struct Fixture {
    dir: tempfile::TempDir,
    config: Config,
}
impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir(dir.path().join("keys")).unwrap();
        fs::write(dir.path().join("keys/company.ed25519"), "placeholder").unwrap();
        fs::write(dir.path().join("keys/company.ed25519.pub"), key(1)).unwrap();
        let config = Config {
            schema_version: 1,
            node_id: Uuid::new_v4().to_string(),
            label: "Master".into(),
            roles: vec!["master".into(), "target".into()],
            owner: "test".into(),
            execution_mode: "service".into(),
            allowed_principals: vec![],
        };
        Self { dir, config }
    }
    fn state(&self) -> &Path {
        self.dir.path()
    }
    fn call(&self, method: &str, params: Value) -> anyhow::Result<Value> {
        pairing::handle(
            self.state(),
            &self.config,
            &Request {
                method: method.into(),
                params,
            },
        )?
        .ok_or_else(|| anyhow::anyhow!("unhandled"))
    }
    fn create(&self, os: &str, address: &str, port: u16) -> Value {
        self.call(
            "pairing.create",
            json!({"label":"New PC","tenant":"company","os":os,"address":address,"port":port}),
        )
        .unwrap()
    }
    /// Pastes the code. Without the private key the connection check fails fast; the Target stays.
    fn complete(&self, request: &Value) -> anyhow::Result<Value> {
        fs::remove_file(self.state().join("keys/company.ed25519")).ok();
        self.call("pairing.complete", json!({"code": code(request)}))
    }
    fn requests(&self) -> Vec<Value> {
        self.call("pairing.status", json!({}))
            .unwrap()
            .as_array()
            .unwrap()
            .clone()
    }
    /// Saved Targets, read straight from the Master's store.
    fn targets(&self) -> Vec<Value> {
        let db = rusqlite::Connection::open(self.state().join("fleet.sqlite")).unwrap();
        let mut stmt = db.prepare("SELECT document FROM targets").unwrap();
        let rows = stmt
            .query_map([], |row| row.get::<_, String>(0))
            .unwrap()
            .map(|row| serde_json::from_str(&row.unwrap()).unwrap())
            .collect();
        rows
    }
}
/// The Pairing code the new host shows after Approve for `request`.
fn code(request: &Value) -> String {
    let bundle = &request["envelope"]["bundle"];
    let bridge = if bundle["os"] == "windows" {
        r"C:\Program Files\SSH Control Center\sshcc.exe"
    } else {
        "/usr/bin/sshcc"
    };
    let registration = json!({"schemaVersion":2,"id":bundle["targetId"],"nodeId":Uuid::new_v4().to_string(),"label":"hostname","tenant":bundle["tenant"],"os":bundle["os"],"user":bundle["user"],"hostKeys":[key(2)],"routes":bundle["routes"],"bridgePath":bridge,"runnerInstalled":true});
    pairing::encode_receipt(&request["envelope"], &registration).unwrap()
}
fn closed_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}
fn pair(fx: &Fixture) -> (Value, Value) {
    let created = fx.create("unknown", "192.0.2.31", 22);
    let rows = created.as_array().expect("both requests").clone();
    assert_eq!(rows.len(), 2);
    (rows[0].clone(), rows[1].clone())
}

#[test]
fn an_unknown_os_makes_a_windows_and_an_ubuntu_request_linked_as_siblings() {
    let fx = Fixture::new();
    let (windows, linux) = pair(&fx);
    assert_eq!(windows["os"], "windows");
    assert_eq!(linux["os"], "linux");
    assert_eq!(windows["sibling"], linux["requestId"]);
    assert_eq!(linux["sibling"], windows["requestId"]);
    for field in ["label", "tenant", "targetId"] {
        assert_eq!(windows[field], linux[field], "{field}");
    }
    assert_eq!(windows["label"], "New PC");
    assert_eq!(
        windows["envelope"]["bundle"]["routes"],
        linux["envelope"]["bundle"]["routes"]
    );
    assert_eq!(
        windows["envelope"]["bundle"]["routes"][0]["address"],
        "192.0.2.31"
    );
    let listed = fx.requests();
    assert_eq!(listed.len(), 2);
    assert!(listed.iter().all(|row| row["sibling"].is_string()));
    // A known OS still makes one request with no sibling.
    let single = fx.create("linux", "192.0.2.40", 22);
    assert_eq!(single["os"], "linux");
    assert_eq!(single["sibling"], Value::Null);
}

#[test]
fn completing_one_removes_its_sibling_and_the_sibling_code_says_replaced() {
    let fx = Fixture::new();
    let (windows, linux) = pair(&fx);
    let done = fx.complete(&windows).unwrap();
    assert_eq!(done["requestId"], windows["requestId"]);
    let listed = fx.requests();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0]["requestId"], windows["requestId"]);
    let error = fx.complete(&linux).unwrap_err().to_string();
    assert!(error.contains("unknown setup request"), "{error}");
    assert!(error.contains("E_PAIRING_REPLACED"), "{error}");
    // The review step reads the code first and says the same.
    let previewed = fx
        .call("pairing.preview", json!({"code": code(&linux)}))
        .unwrap_err()
        .to_string();
    assert!(previewed.contains("E_PAIRING_REPLACED"), "{previewed}");
    let targets = fx.targets();
    assert_eq!(targets.len(), 1);
    assert_eq!(targets[0]["os"], "windows");
}

#[test]
fn the_ubuntu_request_can_win_too() {
    let fx = Fixture::new();
    let (windows, linux) = pair(&fx);
    fx.complete(&linux).unwrap();
    let error = fx.complete(&windows).unwrap_err().to_string();
    assert!(error.contains("E_PAIRING_REPLACED"), "{error}");
    assert_eq!(fx.targets()[0]["os"], "linux");
}

#[test]
fn a_cancelled_sibling_does_not_stop_the_other_from_completing() {
    let fx = Fixture::new();
    let (windows, linux) = pair(&fx);
    fx.call("pairing.cancel", json!({"requestId":linux["requestId"]}))
        .unwrap();
    fx.complete(&windows).unwrap();
    assert_eq!(fx.targets().len(), 1);
}

#[test]
fn a_request_stored_before_the_sibling_field_still_loads_and_completes() {
    let fx = Fixture::new();
    let created = fx.create("linux", "192.0.2.41", 22);
    let db = rusqlite::Connection::open(fx.state().join("fleet.sqlite")).unwrap();
    let id = created["requestId"].as_str().unwrap();
    let document: String = db
        .query_row(
            "SELECT document FROM pairing_requests WHERE id=?1",
            [id],
            |row| row.get(0),
        )
        .unwrap();
    let mut old: Value = serde_json::from_str(&document).unwrap();
    old.as_object_mut().unwrap().remove("sibling");
    db.execute(
        "UPDATE pairing_requests SET document=?1 WHERE id=?2",
        rusqlite::params![old.to_string(), id],
    )
    .unwrap();
    drop(db);
    assert_eq!(fx.requests()[0]["sibling"], Value::Null);
    fx.complete(&created).unwrap();
    assert_eq!(fx.targets().len(), 1);
}

#[test]
fn already_added_ignores_the_requests_own_sibling() {
    let fx = Fixture::new();
    let port = closed_port();
    let created = fx.create("unknown", "127.0.0.1", port);
    let (windows, linux) = (&created[0], &created[1]);
    let check = |params: Value| fx.call("pairing.check", params).unwrap();
    // A plain address check still finds the pair.
    let fresh = check(json!({"address":"127.0.0.1","port":port}));
    assert_eq!(fresh["alreadyAs"], "New PC");
    // Checked for one of the pair, neither it nor its sibling is "already added".
    for own in [windows, linux] {
        let result = check(json!({"address":"127.0.0.1","port":port,"requestId":own["requestId"]}));
        assert_eq!(result["alreadyAs"], Value::Null);
        assert_eq!(result["alreadyRequestId"], Value::Null);
    }
    // Another request for the same address still counts.
    let other = fx.create("linux", "127.0.0.1", port);
    let result = check(json!({"address":"127.0.0.1","port":port,"requestId":windows["requestId"]}));
    assert_eq!(result["alreadyRequestId"], other["requestId"]);
}
