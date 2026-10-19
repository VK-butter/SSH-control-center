use base64::{
    engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD},
    Engine,
};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use sshcc_core::{config::Config, fleet, pairing, protocol::Request};
use std::{fs, path::Path};
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
        fs::write(
            dir.path().join("keys/company.ed25519"),
            "test private key placeholder",
        )
        .unwrap();
        fs::write(dir.path().join("keys/company.ed25519.pub"), key(1)).unwrap();
        let config = Config {
            schema_version: 1,
            node_id: Uuid::new_v4().to_string(),
            label: "เครื่องหลัก ไทย".into(),
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
    fn create(&self, id: Option<&str>) -> Value {
        let mut p =
            json!({"label":"เครื่องทดสอบ ไทย","tenant":"company","os":"linux","address":"127.0.0.1"});
        if let Some(id) = id {
            p["targetId"] = json!(id);
        }
        self.call("pairing.create", p).unwrap()
    }
    fn registration(&self, envelope: &Value) -> Value {
        let bundle = &envelope["bundle"];
        json!({"schemaVersion":2,"id":bundle["targetId"],"nodeId":Uuid::new_v4().to_string(),"label":"physical-hostname","tenant":bundle["tenant"],"os":bundle["os"],"user":bundle["user"],"hostKeys":[key(2)],"routes":bundle["routes"],"bridgePath":"/usr/bin/sshcc","runnerInstalled":true})
    }
    fn no_network(&self) {
        fs::remove_file(self.state().join("keys/company.ed25519")).unwrap();
    }
}
fn raw_code(receipt: &Value) -> String {
    let bytes = serde_json::to_vec(receipt).unwrap();
    format!(
        "SSHCC1.{}.{}",
        URL_SAFE_NO_PAD.encode(&bytes),
        hex::encode(Sha256::digest(&bytes))
    )
}
fn targets(fx: &Fixture) -> Value {
    fleet::handle(
        fx.state(),
        &fx.config,
        &Request {
            method: "targets.list".into(),
            params: json!({}),
        },
    )
    .unwrap()
    .unwrap()
}

#[test]
fn public_request_survives_restart_without_registering_a_target() {
    let fx = Fixture::new();
    let created = fx.create(None);
    assert_eq!(created["state"], "awaiting_approval");
    assert!(created["targetId"].as_str().unwrap().starts_with("target-"));
    assert_eq!(created["label"], "เครื่องทดสอบ ไทย");
    assert_eq!(
        created["envelope"]["bundle"]["master"]["id"],
        fx.config.node_id
    );
    assert_eq!(created["envelope"]["bundle"]["user"], "sshcc-bridge");
    assert_eq!(created["envelope"]["bundle"]["routes"][0]["port"], 22);
    let inspected = pairing::inspect(&created["envelope"]).unwrap();
    assert!(inspected["masterFingerprint"]
        .as_str()
        .unwrap()
        .starts_with("SHA256:"));
    assert_eq!(inspected["bundle"]["master"].as_object().unwrap().len(), 3);
    assert!(!serde_json::to_string(&created)
        .unwrap()
        .contains("PRIVATE KEY"));
    assert_eq!(targets(&fx), json!([]));
    let restarted = pairing::handle(
        fx.state(),
        &fx.config.clone(),
        &Request {
            method: "pairing.status".into(),
            params: json!({"requestId":created["requestId"]}),
        },
    )
    .unwrap()
    .unwrap();
    assert_eq!(created, restarted);
    assert_eq!(
        fx.call("pairing.status", json!({})).unwrap(),
        json!([created])
    );
}
#[test]
fn receipt_round_trips_thai_and_rejects_damaged_or_oversized_codes() {
    let fx = Fixture::new();
    let request = fx.create(Some("one"));
    let envelope = &request["envelope"];
    let mut registration = fx.registration(envelope);
    registration["label"] = json!("เครื่องปลายทาง ภาษาไทย");
    let code = pairing::encode_receipt(envelope, &registration).unwrap();
    let receipt = pairing::decode_receipt(&format!("\n{code}\n")).unwrap();
    assert_eq!(receipt.registration.node_id, registration["nodeId"]);
    assert_eq!(receipt.registration.label, "เครื่องปลายทาง ภาษาไทย");
    assert_eq!(
        pairing::inspect_receipt(&code, Some(envelope)).unwrap()["registration"],
        registration
    );
    let other = fx.create(Some("other-request"));
    assert!(pairing::inspect_receipt(&code, Some(&other["envelope"]))
        .unwrap_err()
        .to_string()
        .contains("REQUEST"));
    let mut bad = code.clone();
    bad.pop();
    bad.push('x');
    assert!(pairing::decode_receipt(&bad).is_err());
    assert!(pairing::decode_receipt(&code.replacen("SSHCC1", "SSHCC2", 1)).is_err());
    assert!(pairing::decode_receipt(&"x".repeat(pairing::MAX_CODE_BYTES + 1)).is_err());
    let mut secret = envelope.clone();
    secret["bundle"]["master"]["privateKey"] = json!("forbidden");
    assert!(pairing::inspect(&secret).is_err());
    let mut changed = envelope.clone();
    changed["targetLabel"] = json!("x\n");
    assert!(pairing::inspect(&changed).is_err());
    changed = envelope.clone();
    changed["bundle"]["tenant"] = json!("other");
    assert!(pairing::inspect(&changed)
        .unwrap_err()
        .to_string()
        .contains("DIGEST"));
}
#[test]
fn receipt_is_bound_to_every_approved_machine_setting() {
    let fx = Fixture::new();
    let request = fx.create(Some("one"));
    let env = &request["envelope"];
    let registration = fx.registration(env);
    for field in ["id", "tenant", "os", "user", "routes"] {
        let mut changed = registration.clone();
        changed[field] = match field {
            "os" => json!("windows"),
            "routes" => json!([{"kind":"lan","address":"127.0.0.2","port":22}]),
            _ => json!("other"),
        };
        if field == "os" {
            changed["bridgePath"] = json!("C:/Program Files/SSH Control Center/sshcc.exe");
        }
        assert!(pairing::encode_receipt(env, &changed).is_err(), "{field}");
    }
    let code = pairing::encode_receipt(env, &registration).unwrap();
    let mut receipt = serde_json::to_value(pairing::decode_receipt(&code).unwrap()).unwrap();
    receipt["requestId"] = json!(Uuid::new_v4().to_string());
    assert!(fx
        .call("pairing.complete", json!({"code":raw_code(&receipt)}))
        .unwrap_err()
        .to_string()
        .contains("unknown setup"));
    receipt["requestId"] = request["requestId"].clone();
    receipt["masterId"] = json!(Uuid::new_v4().to_string());
    assert!(fx
        .call("pairing.complete", json!({"code":raw_code(&receipt)}))
        .unwrap_err()
        .to_string()
        .contains("MASTER"));
    receipt["masterId"] = json!(fx.config.node_id);
    receipt["bundleDigest"] = json!("00");
    assert!(fx
        .call("pairing.complete", json!({"code":raw_code(&receipt)}))
        .unwrap_err()
        .to_string()
        .contains("DIGEST"));
    assert_eq!(targets(&fx), json!([]));
}
#[test]
fn approved_but_unreachable_is_persisted_retryable_and_idempotent() {
    let fx = Fixture::new();
    let created = fx.create(Some("one"));
    let registration = fx.registration(&created["envelope"]);
    let code = pairing::encode_receipt(&created["envelope"], &registration).unwrap();
    fx.no_network();
    let result = fx.call("pairing.complete", json!({"code":code})).unwrap();
    assert_eq!(result["state"], "needs_attention");
    assert_eq!(result["diagnostics"]["ok"], false);
    assert_eq!(targets(&fx)[0]["nodeId"], registration["nodeId"]);
    assert_eq!(targets(&fx)[0]["label"], "เครื่องทดสอบ ไทย");
    assert_eq!(
        fx.call("pairing.complete", json!({"code":format!(" {code} ")}))
            .unwrap(),
        result
    );
    let retried = fx
        .call(
            "pairing.status",
            json!({"requestId":created["requestId"],"retry":true}),
        )
        .unwrap();
    assert_eq!(retried["state"], "needs_attention");
    fx.call("pairing.status",json!({"requestId":created["requestId"],"routes":[{"kind":"tailscale","address":"100.64.0.10","port":22}]})).unwrap();
    assert_eq!(targets(&fx)[0]["routes"][0]["address"], "100.64.0.10");
    fx.call("pairing.complete", json!({"code":code})).unwrap();
    assert_eq!(
        targets(&fx)[0]["routes"][0]["address"],
        "100.64.0.10",
        "repeated code must not undo corrected transport"
    );
    let mut another = registration.clone();
    another["nodeId"] = json!(Uuid::new_v4().to_string());
    let another = pairing::encode_receipt(&created["envelope"], &another).unwrap();
    assert!(fx
        .call("pairing.complete", json!({"code":another}))
        .unwrap_err()
        .to_string()
        .contains("CONFLICT"));
}
#[test]
fn pairing_never_replaces_existing_pins_identity_or_group() {
    for field in ["hostKeys", "nodeId", "tenant"] {
        let fx = Fixture::new();
        let created = fx.create(Some("one"));
        let registered = fx.registration(&created["envelope"]);
        let mut existing = registered.clone();
        existing[field] = match field {
            "hostKeys" => json!([key(3)]),
            "nodeId" => json!(Uuid::new_v4().to_string()),
            _ => json!("other"),
        };
        // Existing imported node with its own key tenant is allowed by old API.
        if field == "tenant" {
            fs::write(fx.state().join("keys/other.ed25519"), "placeholder").unwrap();
        }
        fleet::handle(
            fx.state(),
            &fx.config,
            &Request {
                method: "targets.import".into(),
                params: json!({"registration":existing}),
            },
        )
        .unwrap();
        let code = pairing::encode_receipt(&created["envelope"], &registered).unwrap();
        assert!(
            fx.call("pairing.complete", json!({"code":code})).is_err(),
            "{field}"
        );
        assert_eq!(targets(&fx)[0], existing);
        assert_eq!(
            fx.call("pairing.status", json!({"requestId":created["requestId"]}))
                .unwrap()["state"],
            "awaiting_approval"
        );
    }
}
#[test]
fn installer_requires_real_node_privilege_and_explicit_master_approval() {
    let fx = Fixture::new();
    let request = fx.create(Some("one"));
    let env = &request["envelope"];
    let mut registration = fx.registration(env);
    registration["nodeId"] = json!(fx.config.node_id);
    let status = json!({"nodeId":fx.config.node_id,"privileged":true,"executionPrincipal":"0"});
    assert!(
        pairing::installer_receipt(fx.state(), &fx.config, env, &registration, &status)
            .unwrap_err()
            .to_string()
            .contains("APPROVAL")
    );
    fleet::register_master(fx.state(), &env["bundle"]["master"]).unwrap();
    let other = json!({"id":Uuid::new_v4().to_string(),"label":"second","publicKey":key(4)});
    fleet::register_master(fx.state(), &other).unwrap();
    assert!(
        pairing::installer_receipt(fx.state(), &fx.config, env, &registration, &status).is_ok()
    );
    assert!(fleet::authorized_master(fx.state(), other["id"].as_str().unwrap()).unwrap());
    let mut unprivileged = status.clone();
    unprivileged["privileged"] = json!(false);
    assert!(
        pairing::installer_receipt(fx.state(), &fx.config, env, &registration, &unprivileged)
            .is_err()
    );
    unprivileged = status.clone();
    unprivileged["executionPrincipal"] = json!("1000");
    assert!(
        pairing::installer_receipt(fx.state(), &fx.config, env, &registration, &unprivileged)
            .is_err()
    );
    unprivileged = status.clone();
    unprivileged["nodeId"] = json!(Uuid::new_v4().to_string());
    assert!(
        pairing::installer_receipt(fx.state(), &fx.config, env, &registration, &unprivileged)
            .is_err()
    );
}
#[test]
fn installer_preflight_is_read_only_and_does_not_recover_running_jobs() {
    let dir = tempfile::tempdir().unwrap();
    assert_eq!(
        pairing::installer_preflight(dir.path()).unwrap(),
        json!({"runningJobs":0,"runningTerminals":0})
    );
    let db = rusqlite::Connection::open(dir.path().join("jobs.sqlite")).unwrap();
    db.execute_batch(
        "CREATE TABLE jobs(id TEXT PRIMARY KEY,payload TEXT NOT NULL,record TEXT NOT NULL);",
    )
    .unwrap();
    for (id, status) in [("a", "running"), ("b", "queued"), ("c", "succeeded")] {
        db.execute(
            "INSERT INTO jobs VALUES (?1,'{}',?2)",
            rusqlite::params![id, json!({"status":status}).to_string()],
        )
        .unwrap();
    }
    assert_eq!(
        pairing::installer_preflight(dir.path()).unwrap(),
        json!({"runningJobs":2,"runningTerminals":0})
    );
    let record: String = db
        .query_row("SELECT record FROM jobs WHERE id='a'", [], |row| row.get(0))
        .unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(&record).unwrap()["status"],
        "running"
    );
}
#[test]
fn mcp_exposes_pairing_without_relaxing_owner_authorization() {
    let definitions = sshcc_core::mcp::tools();
    for name in ["pairing_create", "pairing_status", "pairing_complete"] {
        assert!(definitions.iter().any(|tool| tool["name"] == name));
    }
    let fx = Fixture::new();
    let mut cfg = fx.config.clone();
    cfg.roles = vec!["target".into()];
    assert!(pairing::handle(
        fx.state(),
        &cfg,
        &Request {
            method: "pairing.create".into(),
            params: json!({})
        }
    )
    .unwrap_err()
    .to_string()
    .contains("E_ROLE"));
}

#[test]
fn pairing_rejects_a_node_already_registered_under_another_name() {
    let fx = Fixture::new();
    let created = fx.create(Some("new-name"));
    let registration = fx.registration(&created["envelope"]);
    let mut old = registration.clone();
    old["id"] = json!("original-name");
    old["routes"] = json!([{"kind":"lan","address":"127.0.0.2","port":22}]);
    fleet::handle(
        fx.state(),
        &fx.config,
        &Request {
            method: "targets.import".into(),
            params: json!({"registration":old}),
        },
    )
    .unwrap();
    let code = pairing::encode_receipt(&created["envelope"], &registration).unwrap();
    assert!(fx
        .call("pairing.complete", json!({"code":code}))
        .unwrap_err()
        .to_string()
        .contains("nodeId already registered"));
    assert_eq!(targets(&fx), json!([old]));
}

#[test]
fn installer_guard_blocks_submissions_releases_and_refuses_active_jobs() {
    use sshcc_core::{config, security, service::Service};
    use std::sync::{atomic::AtomicBool, Arc};
    let dir = tempfile::tempdir().unwrap();
    let owner = security::current_principal().unwrap();
    let cfg = config::init(dir.path(), "target", &owner, "guard test", &[]).unwrap();
    config::set_execution_mode(dir.path(), "service").unwrap();
    let service = Service::new(dir.path().into(), Arc::new(AtomicBool::new(false))).unwrap();
    let context = json!({"expectedNodeId":cfg.node_id,"dataDir":dir.path()});
    let begin = Request {
        method: "system.install-begin".into(),
        params: context.clone(),
    };
    let end = Request {
        method: "system.install-end".into(),
        params: context.clone(),
    };
    assert!(service
        .handle(&begin, "unauthorized")
        .unwrap_err()
        .to_string()
        .contains("PERMISSION"));
    let mut wrong = begin.clone();
    wrong.params["expectedNodeId"] = json!(Uuid::new_v4().to_string());
    assert!(service
        .handle(&wrong, &owner)
        .unwrap_err()
        .to_string()
        .contains("IDENTITY"));
    let other_dir = tempfile::tempdir().unwrap();
    let mut wrong = begin.clone();
    wrong.params["dataDir"] = json!(other_dir.path());
    assert!(service
        .handle(&wrong, &owner)
        .unwrap_err()
        .to_string()
        .contains("IDENTITY"));
    let master_id = Uuid::new_v4().to_string();
    fleet::register_master(
        dir.path(),
        &json!({"id":master_id,"label":"remote Master","publicKey":key(1)}),
    )
    .unwrap();
    let bridge = Request {
        method: "bridge.request".into(),
        params: json!({"masterId":master_id,"masterKey":fleet::public_key_fingerprint(&key(1)).unwrap(),"expectedTargetId":cfg.node_id,"request":begin}),
    };
    assert!(service
        .handle(&bridge, &owner)
        .unwrap_err()
        .to_string()
        .contains("PERMISSION"));
    assert_eq!(service.handle(&begin, &owner).unwrap()["guarded"], true);
    let id = Uuid::new_v4().to_string();
    let start = Request {
        method: "runner.start".into(),
        params: json!({"jobId":id,"masterId":"guard-test","cwd":dir.path(),"script":if cfg!(windows) {"Start-Sleep -Seconds 30"} else {"sleep 30"}}),
    };
    assert!(service
        .handle(&start, &owner)
        .unwrap_err()
        .to_string()
        .contains("INSTALLING"));
    assert!(service
        .handle(&begin, &owner)
        .unwrap_err()
        .to_string()
        .contains("INSTALLING"));
    service.handle(&end, &owner).unwrap();
    service.handle(&start, &owner).unwrap();
    assert!(service
        .handle(&begin, &owner)
        .unwrap_err()
        .to_string()
        .contains("BUSY"));
    service
        .handle(
            &Request {
                method: "runner.cancel".into(),
                params: json!({"jobId":id}),
            },
            &owner,
        )
        .unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        let status = service
            .handle(
                &Request {
                    method: "runner.status".into(),
                    params: json!({"jobId":id}),
                },
                &owner,
            )
            .unwrap();
        if status["status"] == "cancelled" {
            break;
        }
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    assert_eq!(service.handle(&begin, &owner).unwrap()["guarded"], true);
    service.handle(&end, &owner).unwrap();
}
