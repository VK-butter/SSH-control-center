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

#[test]
fn container_tools_are_bounded_by_role_and_hints() {
    let definitions = sshcc_core::mcp::tools();
    let tool = |name: &str| definitions.iter().find(|tool| tool["name"] == name).cloned().unwrap();
    for (name, readonly) in [("containers_list", true), ("containers_logs", true), ("containers_action", false)] {
        let definition = tool(name);
        assert_eq!(definition["annotations"]["readOnlyHint"], readonly, "{name}");
        assert!(!definition["inputSchema"]["required"].as_array().unwrap().iter().any(|f| f == "targetId"), "{name}");
    }
    assert_eq!(tool("containers_action")["inputSchema"]["properties"]["action"]["enum"], json!(["start", "stop", "restart"]));
    let fx = Fixture::new();
    let call = |config: &Config, params: Value| {
        fleet::handle(fx.state(), config, &Request { method: "containers.list".into(), params })
    };
    // Without targetId the machine's own runtime answers, whatever its role.
    let mut target_only = fx.config.clone();
    target_only.roles = vec!["target".into()];
    let local = call(&target_only, json!({})).unwrap().unwrap();
    assert!(local.get("runtime").is_some() && local["containers"].is_array());
    // Asking about another machine needs the Master role.
    assert!(call(&target_only, json!({"targetId": "elsewhere"})).unwrap_err().to_string().contains("master role"));
    let invalid = fleet::handle(fx.state(), &fx.config, &Request { method: "containers.action".into(), params: json!({"container": "--all", "action": "stop"}) });
    assert!(invalid.unwrap_err().to_string().contains("E_ARGUMENT"));
}

#[test]
fn awaiting_request_can_be_cancelled_once() {
    let fx = Fixture::new();
    let created = fx.create(None);
    let id = created["requestId"].as_str().unwrap().to_string();
    let cancelled = fx.call("pairing.cancel", json!({"requestId": id})).unwrap();
    assert_eq!(cancelled["cancelled"], true);
    let rows = fx.call("pairing.status", json!({})).unwrap();
    assert!(!rows.as_array().unwrap().iter().any(|row| row["requestId"] == id));
    assert!(fx.call("pairing.cancel", json!({"requestId": id})).unwrap_err().to_string().contains("E_PAIRING_REQUEST"));
}

#[test]
fn folders_are_optional_and_scoped_to_their_master() {
    let fx = Fixture::new();
    let call = |method: &str, params: Value| sshcc_core::folders::handle(fx.state(), &fx.config, &Request { method: method.into(), params });
    assert_eq!(call("folders.list", json!({})).unwrap().unwrap()["folders"], json!([]));
    let web = call("folders.create", json!({"name": "Web servers"})).unwrap().unwrap();
    let id = web["id"].as_str().unwrap().to_string();
    assert!(call("folders.create", json!({"name": "web SERVERS"})).unwrap_err().to_string().contains("E_EXISTS"));
    assert!(call("folders.create", json!({"name": "  "})).is_err());
    call("folders.assign", json!({"targetId": "dev-01", "folderId": id})).unwrap();
    // A machine behind merged Master "cnx" cannot join a local folder.
    assert!(call("folders.assign", json!({"targetId": "cnx/bot-9", "folderId": id})).is_err());
    let listed = call("folders.list", json!({})).unwrap().unwrap();
    assert_eq!(listed["members"]["dev-01"], id);
    call("folders.rename", json!({"id": id, "name": "Web"})).unwrap();
    call("folders.assign", json!({"targetId": "dev-01", "folderId": null})).unwrap();
    assert!(call("folders.list", json!({})).unwrap().unwrap()["members"].as_object().unwrap().is_empty());
    call("folders.delete", json!({"id": id})).unwrap();
    assert_eq!(call("folders.list", json!({})).unwrap().unwrap()["folders"], json!([]));
    let mut target_only = fx.config.clone();
    target_only.roles = vec!["target".into()];
    assert!(sshcc_core::folders::handle(fx.state(), &target_only, &Request { method: "folders.list".into(), params: json!({}) }).is_err());
}

#[test]
fn datacenters_group_masters_optionally() {
    let fx = Fixture::new();
    let call = |method: &str, params: Value| sshcc_core::folders::handle(fx.state(), &fx.config, &Request { method: method.into(), params });
    assert_eq!(call("datacenters.list", json!({})).unwrap().unwrap()["datacenters"], json!([]));
    let a = call("datacenters.create", json!({"name": "DC-A"})).unwrap().unwrap()["id"].as_str().unwrap().to_string();
    let b = call("datacenters.create", json!({"name": "DC-B"})).unwrap().unwrap()["id"].as_str().unwrap().to_string();
    assert!(call("datacenters.create", json!({"name": "dc-a"})).unwrap_err().to_string().contains("E_EXISTS"));
    call("datacenters.assign", json!({"master": "local", "datacenterId": a})).unwrap();
    call("datacenters.assign", json!({"master": "master-b", "datacenterId": b})).unwrap();
    assert!(call("datacenters.assign", json!({"master": "peer/child", "datacenterId": b})).is_err());
    let listed = call("datacenters.list", json!({})).unwrap().unwrap();
    assert_eq!((listed["members"]["local"].as_str(), listed["members"]["master-b"].as_str()), (Some(a.as_str()), Some(b.as_str())));
    call("datacenters.delete", json!({"id": b})).unwrap();
    let listed = call("datacenters.list", json!({})).unwrap().unwrap();
    assert!(listed["members"].get("master-b").is_none());
    call("datacenters.assign", json!({"master": "local", "datacenterId": null})).unwrap();
    assert!(call("datacenters.list", json!({})).unwrap().unwrap()["members"].as_object().unwrap().is_empty());
}

#[test]
fn preview_reads_the_code_without_saving_anything() {
    let fx = Fixture::new();
    let created = fx.create(Some("one"));
    let registration = fx.registration(&created["envelope"]);
    let code = pairing::encode_receipt(&created["envelope"], &registration).unwrap();
    let status = || {
        fx.call("pairing.status", json!({"requestId":created["requestId"]}))
            .unwrap()
    };
    let before = status();
    let preview = fx.call("pairing.preview", json!({"code":code})).unwrap();
    assert_eq!(
        preview,
        json!({"requestId":created["requestId"],"targetId":"one","label":"เครื่องทดสอบ ไทย","hostName":"physical-hostname","os":"linux","hostKeySha256":fleet::public_key_fingerprint(&key(2)).unwrap()})
    );
    assert_eq!(targets(&fx), json!([]));
    assert_eq!(status(), before);
}

#[test]
fn preview_refuses_bad_codes_with_the_same_error_as_complete() {
    let fx = Fixture::new();
    let created = fx.create(Some("one"));
    let registration = fx.registration(&created["envelope"]);
    let code = pairing::encode_receipt(&created["envelope"], &registration).unwrap();
    let mut receipt = serde_json::to_value(pairing::decode_receipt(&code).unwrap()).unwrap();
    receipt["masterId"] = json!(Uuid::new_v4().to_string());
    let other_master = raw_code(&receipt);
    let mut damaged = code.clone();
    damaged.pop();
    damaged.push('x');
    let same_error = |code: &str| {
        let preview = fx
            .call("pairing.preview", json!({"code":code}))
            .unwrap_err()
            .to_string();
        let complete = fx
            .call("pairing.complete", json!({"code":code}))
            .unwrap_err()
            .to_string();
        assert_eq!(preview, complete);
        preview
    };
    assert!(same_error(&other_master).contains("another Master"));
    assert!(same_error(&damaged).contains("damaged"));
    assert_eq!(targets(&fx), json!([]));
    fx.no_network();
    fx.call("pairing.complete", json!({"code":code})).unwrap();
    let mut another = registration.clone();
    another["nodeId"] = json!(Uuid::new_v4().to_string());
    let another = pairing::encode_receipt(&created["envelope"], &another).unwrap();
    assert!(same_error(&another).contains("different machine already answered"));
}
#[test]
fn complete_saves_the_name_edited_on_the_review_card() {
    let fx = Fixture::new();
    let created = fx.create(Some("one"));
    let code =
        pairing::encode_receipt(&created["envelope"], &fx.registration(&created["envelope"]))
            .unwrap();
    fx.no_network();
    fx.call(
        "pairing.complete",
        json!({"code":code,"label":"web-01 ใหม่"}),
    )
    .unwrap();
    assert_eq!(targets(&fx)[0]["label"], "web-01 ใหม่");
}

/// Entry 8: the 6 digits both screens show.
#[test]
fn the_six_digits_are_stable_and_change_with_each_bound_part() {
    let fx = Fixture::new();
    let request = fx.create(None);
    let envelope = request["envelope"].clone();
    let registration = fx.registration(&envelope);
    let code = pairing::encode_receipt(&envelope, &registration).unwrap();
    let digits = pairing::short_code_of(&code).unwrap();
    assert!(digits.len() == 7 && digits.as_bytes()[3] == b' ' && digits.replace(' ', "").bytes().all(|b| b.is_ascii_digit()), "{digits}");
    assert_eq!(pairing::short_code_of(&code).unwrap(), digits, "the same code, the same digits");
    // Another host key (a machine in the middle) gives other digits.
    let mut other = registration.clone();
    other["hostKeys"] = json!([key(9)]);
    assert_ne!(pairing::short_code_of(&pairing::encode_receipt(&envelope, &other).unwrap()).unwrap(), digits);
    // Another request gives other digits too.
    let second = fx.create(Some("second-host"));
    let reg2 = fx.registration(&second["envelope"]);
    assert_ne!(pairing::short_code_of(&pairing::encode_receipt(&second["envelope"], &reg2).unwrap()).unwrap(), digits);
}

#[test]
fn the_master_fetches_the_code_compares_and_finishes_or_falls_back() {
    let fx = Fixture::new();
    let request = fx.create(None);
    let id = request["requestId"].as_str().unwrap().to_owned();
    let envelope = request["envelope"].clone();
    let registration = fx.registration(&envelope);
    let code = pairing::encode_receipt(&envelope, &registration).unwrap();
    let fetch = |answer: &dyn Fn() -> anyhow::Result<Option<(String, String)>>| pairing::fetch_with(fx.state(), &fx.config, &json!({"requestId":id}), &|_, _| answer()).unwrap();
    // Before Approve, and while the machine isn't reachable: keep waiting.
    assert_eq!(fetch(&|| Ok(None))["state"], "waiting");
    let unreachable = fetch(&|| anyhow::bail!("ALL_ROUTES_FAILED: lan 127.0.0.1:22: Connection refused"));
    assert_eq!((unreachable["state"].as_str(), unreachable["error"].is_string()), (Some("waiting"), true));
    // An older installer doesn't know the method: the paste box.
    assert_eq!(fetch(&|| anyhow::bail!("E_PERMISSION: method is not available over bridge"))["state"], "old");
    // A code whose host key isn't the one the connection saw is refused.
    let wrong = pairing::fetch_with(fx.state(), &fx.config, &json!({"requestId":id}), &|_, _| Ok(Some((code.clone(), key(7)))));
    assert!(format!("{:#}", wrong.unwrap_err()).contains("E_PAIRING_HOSTKEY"));
    // Ready: the digits match the installer's, with the review facts.
    let ready = fetch(&|| Ok(Some((code.clone(), key(2)))));
    assert_eq!(ready["state"], "ready");
    assert_eq!(ready["digits"], pairing::short_code_of(&code).unwrap());
    assert_eq!(ready["preview"]["targetId"], registration["id"]);
    // "No, it's different" cancels the request.
    let other = fx.create(Some("other-host"));
    let other_id = other["requestId"].as_str().unwrap().to_owned();
    let other_code = pairing::encode_receipt(&other["envelope"], &fx.registration(&other["envelope"])).unwrap();
    pairing::fetch_with(fx.state(), &fx.config, &json!({"requestId":other_id}), &|_, _| Ok(Some((other_code.clone(), key(2))))).unwrap();
    let different = fx.call("pairing.confirm", json!({"requestId":other_id,"same":false})).unwrap_err();
    assert!(format!("{different:#}").contains("E_PAIRING_MISMATCH"));
    assert!(fx.call("pairing.status", json!({"requestId":other_id})).is_err(), "cancelled");
    // "Yes, same number" finishes it as a pasted code would (the connection check then fails: no network here).
    fx.no_network();
    let _ = fx.call("pairing.confirm", json!({"requestId":id,"same":true}));
    assert!(targets(&fx).as_array().unwrap().iter().any(|t| t["id"] == registration["id"]), "registered");
    assert_eq!(fetch(&|| Ok(None))["state"], "done");
}

#[test]
fn the_new_machine_hands_its_code_only_to_its_own_master() {
    let fx = Fixture::new();
    let request = fx.create(None);
    let envelope = request["envelope"].clone();
    let code = pairing::encode_receipt(&envelope, &fx.registration(&envelope)).unwrap();
    let id = request["requestId"].as_str().unwrap();
    assert_eq!(pairing::stored_receipt(fx.state(), &fx.config.node_id, id).unwrap(), json!({"ready":false}));
    pairing::store_receipt(fx.state(), &code).unwrap();
    assert_eq!(pairing::stored_receipt(fx.state(), &fx.config.node_id, id).unwrap(), json!({"ready":true,"code":code}));
    assert!(format!("{:#}", pairing::stored_receipt(fx.state(), &Uuid::new_v4().to_string(), id).unwrap_err()).contains("another Master"));
    assert!(pairing::stored_receipt(fx.state(), &fx.config.node_id, "../../etc/passwd").is_err());
}
