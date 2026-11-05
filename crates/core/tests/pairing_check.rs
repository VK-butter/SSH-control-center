//! `pairing.check`: the read-only live check behind step 1 of "Add host".
use base64::{engine::general_purpose::STANDARD, Engine};
use serde_json::{json, Value};
use sshcc_core::{config::Config, pairing, protocol::Request};
use std::{fs, io::Write, net::TcpListener, path::Path, thread};
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
    fn create(&self, label: &str, address: &str, port: u16) -> Value {
        self.call(
            "pairing.create",
            json!({"label":label,"tenant":"company","os":"linux","address":address,"port":port}),
        )
        .unwrap()
    }
    /// Pairs a request so a Target is saved (the connection check fails offline; the Target stays).
    fn pair(&self, label: &str, address: &str, port: u16) -> Value {
        let created = self.create(label, address, port);
        let bundle = &created["envelope"]["bundle"];
        let registration = json!({"schemaVersion":2,"id":bundle["targetId"],"nodeId":Uuid::new_v4().to_string(),"label":"hostname","tenant":bundle["tenant"],"os":bundle["os"],"user":bundle["user"],"hostKeys":[key(2)],"routes":bundle["routes"],"bridgePath":"/usr/bin/sshcc","runnerInstalled":true});
        let code = pairing::encode_receipt(&created["envelope"], &registration).unwrap();
        fs::remove_file(self.state().join("keys/company.ed25519")).ok();
        self.call("pairing.complete", json!({"code":code})).unwrap()
    }
    fn check(&self, params: Value) -> Value {
        self.call("pairing.check", params).unwrap()
    }
}
/// A local listener that writes `reply` to every connection.
fn listener(reply: &'static [u8]) -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    thread::spawn(move || {
        for mut stream in listener.incoming().flatten() {
            let _ = stream.write_all(reply);
        }
    });
    port
}
fn closed_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.local_addr().unwrap().port()
}

#[test]
fn a_local_ssh_listener_is_reachable_and_reports_its_banner() {
    let fx = Fixture::new();
    let port = listener(b"SSH-2.0-OpenSSH_for_Windows_9.5\r\n");
    let result = fx.check(json!({"address":"127.0.0.1","port":port}));
    assert_eq!(result["reachable"], true);
    assert_eq!(result["banner"], "SSH-2.0-OpenSSH_for_Windows_9.5");
    assert_eq!(result["os"], "windows");
    assert_eq!(result["alreadyAs"], Value::Null);
}

#[test]
fn a_closed_port_is_not_reachable() {
    let fx = Fixture::new();
    let result = fx.check(json!({"address":"127.0.0.1","port":closed_port()}));
    assert_eq!(result["reachable"], false);
    assert_eq!(result["banner"], Value::Null);
    assert_eq!(result["os"], Value::Null);
}

#[test]
fn the_banner_names_the_os() {
    for (banner, os) in [
        (
            &b"SSH-2.0-OpenSSH_for_Windows_9.5\r\n"[..],
            json!("windows"),
        ),
        (
            b"SSH-2.0-OpenSSH_9.6p1 Ubuntu-3ubuntu13.5\r\n",
            json!("linux"),
        ),
        (
            b"SSH-2.0-OpenSSH_9.2p1 Debian-2+deb12u3\r\n",
            json!("linux"),
        ),
        (b"SSH-2.0-OpenSSH_9.6\r\n", Value::Null),
        (b"SSH-2.0-dropbear_2022.83\r\n", Value::Null),
        (b"SSH-\r\n", Value::Null),
    ] {
        let fx = Fixture::new();
        let port = listener(Box::leak(banner.to_vec().into_boxed_slice()));
        let result = fx.check(json!({"address":"127.0.0.1","port":port}));
        assert_eq!(result["reachable"], true);
        assert_eq!(result["os"], os, "{}", String::from_utf8_lossy(banner));
    }
}

#[test]
fn a_saved_target_with_the_same_route_is_already_added() {
    let fx = Fixture::new();
    let port = closed_port();
    let paired = fx.pair("Office PC", "127.0.0.1", port);
    let result = fx.check(json!({"address":"127.0.0.1","port":port}));
    assert_eq!(result["alreadyAs"], "Office PC");
    assert_eq!(result["alreadyTargetId"], paired["targetId"]);
    assert_eq!(result["alreadyRequestId"], Value::Null);
    // Same address, other port: a different route.
    let other = fx.check(json!({"address":"127.0.0.1","port":closed_port()}));
    assert_eq!(other["alreadyAs"], Value::Null);
}

#[test]
fn a_request_awaiting_approval_for_the_address_is_already_added() {
    let fx = Fixture::new();
    let created = fx.create("Lab box", "Box.Example", 22);
    let result = fx.check(json!({"address":"box.example"}));
    assert_eq!(result["alreadyAs"], "Lab box");
    assert_eq!(result["alreadyRequestId"], created["requestId"]);
    assert_eq!(result["alreadyTargetId"], Value::Null);
    assert_eq!(result["reachable"], false);
}

#[test]
fn another_address_is_not_already_added() {
    let fx = Fixture::new();
    fx.create("Lab box", "192.0.2.10", 22);
    let result = fx.check(json!({"address":"192.0.2.11","port":closed_port()}));
    assert_eq!(result["alreadyAs"], Value::Null);
    assert_eq!(result["alreadyTargetId"], Value::Null);
    assert_eq!(result["alreadyRequestId"], Value::Null);
}

#[test]
fn an_empty_address_is_refused() {
    let fx = Fixture::new();
    assert!(fx.call("pairing.check", json!({"address":""})).is_err());
    assert!(fx.call("pairing.check", json!({})).is_err());
}
