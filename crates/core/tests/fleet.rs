use base64::{engine::general_purpose::STANDARD, Engine};
use serde_json::{json, Value};
use sshcc_core::{config::Config, fleet, protocol::Request};
use std::{fs, path::PathBuf};
use uuid::Uuid;

struct Fixture {
    state: PathBuf,
    config: Config,
}
impl Fixture {
    fn new() -> Self {
        let state = std::env::temp_dir().join(format!("sshcc-fleet-test-{}", Uuid::new_v4()));
        fs::create_dir(&state).unwrap();
        let config = Config {
            schema_version: 1,
            node_id: Uuid::new_v4().to_string(),
            label: "Test Master".into(),
            roles: vec!["master".into(), "target".into()],
            owner: "test".into(),
            execution_mode: "service".into(),
            allowed_principals: vec![],
        };
        Self { state, config }
    }
    fn call(&self, method: &str, params: Value) -> anyhow::Result<Value> {
        fleet::handle(
            &self.state,
            &self.config,
            &Request {
                method: method.into(),
                params,
            },
        )?
        .ok_or_else(|| anyhow::anyhow!("unhandled method"))
    }
    fn allow_import(&self) {
        fs::create_dir_all(self.state.join("keys")).unwrap();
        // Registry tests do not use SSH or inspect any real key.
        fs::write(
            self.state.join("keys/company.ed25519"),
            "test key placeholder",
        )
        .unwrap();
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        if self.state.parent() == Some(std::env::temp_dir().as_path())
            && self
                .state
                .file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("sshcc-fleet-test-")
        {
            let _ = fs::remove_dir_all(&self.state);
        }
    }
}
fn key(seed: u8) -> String {
    let mut wire = Vec::new();
    wire.extend_from_slice(&11u32.to_be_bytes());
    wire.extend_from_slice(b"ssh-ed25519");
    wire.extend_from_slice(&32u32.to_be_bytes());
    wire.extend_from_slice(&[seed; 32]);
    format!("ssh-ed25519 {}", STANDARD.encode(wire))
}
fn registration(id: &str, address: &str) -> Value {
    json!({
        "schemaVersion":2,"id":id,"nodeId":Uuid::new_v4().to_string(),"label":id,
        "tenant":"company","os":"linux","user":"sshcc-bridge","hostKeys":[key(1)],
        "routes":[{"kind":"lan","address":address,"port":22}],
        "bridgePath":"/usr/bin/sshcc","runnerInstalled":true
    })
}
fn v1_record(id: &str, address: &str) -> Value {
    json!({"schemaVersion":1,"id":id,"hostname":id,"mode":"company","tenant":"company",
        "address":address,"port":22,"user":"sshcc-bridge","os":"linux","hostKeys":[key(1)],
        "shareRequired":false})
}
#[test]
fn registry_enforces_limit_and_idempotent_reimport() {
    let fx = Fixture::new();
    fx.allow_import();
    let first = registration("node-0", "10.0.0.1");
    fx.call("targets.import", json!({"registration":first}))
        .unwrap();
    fx.call("targets.import", json!({"registration":first}))
        .unwrap();
    for n in 1..50 {
        fx.call(
            "targets.import",
            json!({"registration":registration(&format!("node-{n}"),&format!("10.0.0.{}",n+1))}),
        )
        .unwrap();
    }
    assert_eq!(
        fx.call("targets.list", json!({}))
            .unwrap()
            .as_array()
            .unwrap()
            .len(),
        50
    );
    let error = fx
        .call(
            "targets.import",
            json!({"registration":registration("overflow","10.0.1.1")}),
        )
        .unwrap_err();
    assert!(error.to_string().contains("at most 50"));
}
#[test]
fn replacement_pins_identity_and_tenant_need_explicit_reenrollment() {
    let fx = Fixture::new();
    fx.allow_import();
    let first = registration("one", "10.0.0.1");
    fx.call("targets.import", json!({"registration":first}))
        .unwrap();
    let mut changed = first.clone();
    changed["hostKeys"] = json!([key(2)]);
    assert!(fx
        .call("targets.import", json!({"registration":changed}))
        .unwrap_err()
        .to_string()
        .contains("HOST_KEY_MISMATCH"));
    changed = first.clone();
    changed["nodeId"] = json!(Uuid::new_v4().to_string());
    assert!(fx
        .call("targets.import", json!({"registration":changed}))
        .unwrap_err()
        .to_string()
        .contains("TARGET_IDENTITY_MISMATCH"));
    changed = first.clone();
    changed["label"] = json!("new display name");
    assert_eq!(
        fx.call("targets.update", json!({"target":changed}))
            .unwrap()["label"],
        "new display name"
    );
}
#[test]
fn registry_rejects_duplicate_machine_routes_and_config_injection() {
    let fx = Fixture::new();
    fx.allow_import();
    let first = registration("one", "10.0.0.1");
    fx.call("targets.import", json!({"registration":first}))
        .unwrap();
    assert!(fx
        .call(
            "targets.import",
            json!({"registration":registration("duplicate","10.0.0.1")})
        )
        .is_err());
    let mut malicious = registration("bad", "10.0.0.2");
    malicious["routes"][0]["address"] = json!("127.0.0.1\nProxyCommand evil");
    assert!(fx
        .call("targets.import", json!({"registration":malicious}))
        .is_err());
    malicious = registration("bad", "10.0.0.2");
    malicious["privateKey"] = json!("must never accept secrets");
    assert!(fx
        .call("targets.import", json!({"registration":malicious}))
        .is_err());
    malicious = registration("bad", "10.0.0.2");
    malicious["hostKeys"] = json!(["ssh-ed25519 aGVsbG8="]);
    assert!(fx
        .call("targets.import", json!({"registration":malicious}))
        .is_err());
}
#[test]
fn v1_import_is_atomic_preserves_tenant_and_does_not_claim_runner() {
    let fx = Fixture::new();
    let before = v1_record("host-one", "10.0.0.1");
    let imported = fx
        .call(
            "targets.import_legacy",
            json!({"document":{"host-one":before}}),
        )
        .unwrap();
    assert_eq!(imported["imported"], 1);
    assert_eq!(imported["targets"][0]["runnerInstalled"], false);
    assert_eq!(imported["targets"][0]["tenant"], "company");
    assert_eq!(imported["targets"][0]["hostKeys"], json!([key(1)]));
    assert_eq!(imported["targets"][0]["nodeId"], "");
    let error = fx
        .call(
            "jobs.start",
            json!({"targetId":"host-one","cwd":"/tmp","script":"true"}),
        )
        .unwrap_err();
    assert!(error.to_string().contains("RUNNER_NOT_INSTALLED"));
    let mut invalid = v1_record("bad", "10.0.0.3");
    invalid["hostKeys"] = json!([]);
    assert!(fx
        .call(
            "targets.import_legacy",
            json!({"document":[v1_record("two","10.0.0.2"),invalid]})
        )
        .is_err());
    assert_eq!(
        fx.call("targets.list", json!({}))
            .unwrap()
            .as_array()
            .unwrap()
            .len(),
        1
    );
}
#[test]
fn v1_proxy_jump_requires_explicit_pins_and_identity() {
    let fx = Fixture::new();
    let identity = fx
        .state
        .join("keys/company.ed25519")
        .to_string_lossy()
        .into_owned();
    let imported=fx.call("targets.import_legacy",json!({
        "document":v1_record("one","10.0.0.1"),
        "proxyJump":{"address":"10.0.0.10","port":2222,"user":"sshcc","hostKeys":[key(2)],"identityFile":identity}
    })).unwrap();
    assert_eq!(
        imported["targets"][0]["routes"][0]["proxyJump"]["port"],
        2222
    );
    assert_eq!(
        imported["targets"][0]["routes"][0]["proxyJump"]["hostKeys"],
        json!([key(2)])
    );
}
#[test]
fn independent_masters_can_be_revoked_and_reenrolled() {
    let fx = Fixture::new();
    let a = json!({"id":Uuid::new_v4().to_string(),"label":"A","publicKey":key(1)});
    let b = json!({"id":Uuid::new_v4().to_string(),"label":"B","publicKey":key(2)});
    fleet::register_master(&fx.state, &a).unwrap();
    fleet::register_master(&fx.state, &b).unwrap();
    let a_id = a["id"].as_str().unwrap();
    let b_id = b["id"].as_str().unwrap();
    assert!(fleet::authorized_master(&fx.state, a_id).unwrap());
    let mut changed = a.clone();
    changed["publicKey"] = json!(key(3));
    assert!(fleet::register_master(&fx.state, &changed).is_err());
    fleet::revoke_master(&fx.state, a_id).unwrap();
    assert!(!fleet::authorized_master(&fx.state, a_id).unwrap());
    assert!(fleet::authorized_master(&fx.state, b_id).unwrap());
    fleet::register_master(&fx.state, &changed).unwrap();
    assert!(fleet::authorized_master(&fx.state, a_id).unwrap());
    assert_eq!(
        fx.call("masters.list", json!({}))
            .unwrap()
            .as_array()
            .unwrap()
            .len(),
        2
    );
}
#[test]
fn job_id_conflict_is_detected_before_a_network_attempt() {
    let fx = Fixture::new();
    fx.allow_import();
    fx.call(
        "targets.import",
        json!({"registration":registration("one","10.0.0.1")}),
    )
    .unwrap();
    // Missing managed key prevents networking while exercising durable dispatch intent.
    fs::remove_file(fx.state.join("keys/company.ed25519")).unwrap();
    let job = Uuid::new_v4().to_string();
    let params = json!({"targetId":"one","jobId":job,"cwd":"/tmp","script":"true"});
    let error = fx.call("jobs.start", params.clone()).unwrap_err();
    assert!(format!("{error:#}").contains(&job));
    let mut changed = params;
    changed["script"] = json!("false");
    assert!(fx
        .call("jobs.start", changed)
        .unwrap_err()
        .to_string()
        .contains("JOB_ID_CONFLICT"));
}
#[test]
fn enrollment_uses_actual_target_identity_and_rejects_secret_fields() {
    let fx = Fixture::new();
    let registration = fx
        .call(
            "enrollment.registration",
            json!({
                "targetId":"one","tenant":"company","os":"linux","user":"sshcc-bridge",
                "routes":[{"kind":"lan","address":"10.0.0.1"}],"hostKeys":[key(1)]
            }),
        )
        .unwrap();
    assert_eq!(registration["nodeId"], fx.config.node_id);
    assert_eq!(registration["bridgePath"], "/usr/bin/sshcc");
    let mut bundle = json!({"schemaVersion":2,"targetId":"one","tenant":"company","os":"linux","user":"sshcc-bridge",
        "routes":[{"kind":"lan","address":"10.0.0.1"}],
        "master":{"id":Uuid::new_v4().to_string(),"label":"A","publicKey":key(1)}
    });
    fx.call("enrollment.accept", json!({"bundle":bundle}))
        .unwrap();
    bundle["master"]["privateKey"] = json!("forbidden");
    assert!(fx
        .call("enrollment.accept", json!({"bundle":bundle}))
        .is_err());
}
#[test]
fn route_priority_follows_explicit_registration_order() {
    let fx = Fixture::new();
    fx.allow_import();
    let mut value = registration("one", "10.0.0.1");
    value["routes"] = json!([
        {"kind":"tailscale","address":"100.64.0.1"},
        {"kind":"lan","address":"10.0.0.1"},
        {"kind":"public","address":"example.invalid"}
    ]);
    let imported = fx
        .call("targets.import", json!({"registration":value}))
        .unwrap();
    assert_eq!(imported["routes"][0]["kind"], "tailscale");
}

#[test]
fn key_rotation_never_reauthorizes_stale_ssh_forced_commands() {
    let fx = Fixture::new();
    let original = json!({"id":Uuid::new_v4().to_string(),"label":"Master","publicKey":key(1)});
    let id = original["id"].as_str().unwrap();
    let old = fleet::public_key_fingerprint(&key(1)).unwrap();
    let new = fleet::public_key_fingerprint(&key(2)).unwrap();
    let result = fleet::register_master(&fx.state, &original).unwrap();
    assert_eq!(result["fingerprint"], old);
    assert!(fleet::authorized_master_key(&fx.state, id, &old).unwrap());
    assert!(!fleet::authorized_master_key(&fx.state, id, &new).unwrap());
    fleet::revoke_master(&fx.state, id).unwrap();
    assert!(!fleet::authorized_master_key(&fx.state, id, &old).unwrap());
    // Explicitly re-enrolling the same key is supported.
    fleet::register_master(&fx.state, &original).unwrap();
    assert!(fleet::authorized_master_key(&fx.state, id, &old).unwrap());
    fleet::revoke_master(&fx.state, id).unwrap();
    let mut rotated = original.clone();
    rotated["publicKey"] = json!(key(2));
    fleet::register_master(&fx.state, &rotated).unwrap();
    assert!(!fleet::authorized_master_key(&fx.state, id, &old).unwrap());
    assert!(fleet::authorized_master_key(&fx.state, id, &new).unwrap());
}
#[test]
fn export_generates_private_local_keys_and_public_only_repeatable_bundles() {
    let fx = Fixture::new();
    let params = json!({"targetId":"one","tenant":"company","os":"linux","user":"sshcc-bridge",
        "routes":[{"kind":"lan","address":"10.0.0.1"}]});
    let a = fx.call("enrollment.export", params.clone()).unwrap();
    let b = fx.call("enrollment.export", params.clone()).unwrap();
    assert_eq!(a, b);
    assert_eq!(a["master"]["id"], fx.config.node_id);
    assert!(a["master"]["publicKey"]
        .as_str()
        .unwrap()
        .starts_with("ssh-ed25519 "));
    assert_eq!(a["master"].as_object().unwrap().len(), 3);
    let public = fs::read_to_string(fx.state.join("keys/company.ed25519.pub")).unwrap();
    assert_eq!(
        fleet::public_key_fingerprint(&public).unwrap(),
        fleet::public_key_fingerprint(a["master"]["publicKey"].as_str().unwrap()).unwrap()
    );
    assert!(fx.state.join("keys/company.ed25519").is_file());
    let mut other = params;
    other["tenant"] = json!("customer");
    let c = fx.call("enrollment.export", other).unwrap();
    assert_ne!(a["master"]["publicKey"], c["master"]["publicKey"]);
    assert_eq!(a["master"]["id"], c["master"]["id"]);
}
#[test]
fn retrying_uncertain_dispatch_keeps_existing_job_id_and_payload() {
    let fx = Fixture::new();
    fx.allow_import();
    fx.call(
        "targets.import",
        json!({"registration":registration("one","10.0.0.1")}),
    )
    .unwrap();
    fs::remove_file(fx.state.join("keys/company.ed25519")).unwrap();
    let id = Uuid::new_v4().to_string();
    let p = json!({"targetId":"one","jobId":id,"cwd":"/tmp","script":"echo deploy"});
    for _ in 0..2 {
        let error = fx.call("jobs.start", p.clone()).unwrap_err();
        let text = format!("{error:#}");
        assert!(text.contains(&id));
        assert!(!text.contains("JOB_ID_CONFLICT"));
    }
}

#[test]
fn enrollment_metadata_accepts_a_windows_master_key_path_on_any_target_os() {
    let fx = Fixture::new();
    let bundle = json!({
        "schemaVersion":2,"targetId":"one","tenant":"company","os":"linux","user":"sshcc-bridge",
        "routes":[{"kind":"lan","address":"10.0.0.1","proxyJump":{
            "address":"10.0.0.2","user":"jump","hostKeys":[key(1)],
            "identityFile":"C:/ProgramData/SSHControlCenter/keys/company.ed25519"
        }}],
        "master":{"id":Uuid::new_v4().to_string(),"label":"Windows Master","publicKey":key(2)}
    });
    fx.call("enrollment.accept", json!({"bundle":bundle}))
        .unwrap();
}
#[test]
fn enrollment_metadata_accepts_a_linux_master_key_path_on_any_target_os() {
    let fx = Fixture::new();
    let mut bundle = json!({
        "schemaVersion":2,"targetId":"one","tenant":"company","os":"windows","user":"sshcc-bridge",
        "routes":[{"kind":"lan","address":"10.0.0.1","proxyJump":{
            "address":"10.0.0.2","user":"jump","hostKeys":[key(1)],
            "identityFile":"/var/lib/sshcc/keys/company.ed25519"
        }}],
        "master":{"id":Uuid::new_v4().to_string(),"label":"Linux Master","publicKey":key(2)}
    });
    fx.call("enrollment.accept", json!({"bundle":bundle}))
        .unwrap();
    bundle["routes"][0]["proxyJump"]["identityFile"] = json!("relative/key");
    assert!(fx
        .call("enrollment.accept", json!({"bundle":bundle}))
        .is_err());
}
fn child(id: &str) -> Value {
    json!({"id":id,"label":id,"tenant":"company","os":"linux","nodeId":Uuid::new_v4().to_string()})
}
#[test]
fn relay_join_shares_only_public_targets_and_skips_the_merging_master() {
    let fx = Fixture::new();
    fx.allow_import();
    let merging = Uuid::new_v4().to_string();
    fx.call(
        "targets.import",
        json!({"registration":registration("child-a","10.0.0.1")}),
    )
    .unwrap();
    let mut itself = registration("merging-master", "10.0.0.2");
    itself["nodeId"] = json!(merging);
    fx.call("targets.import", json!({ "registration": itself }))
        .unwrap();
    let joined = fleet::relay_join(&fx.state, &fx.config, &merging).unwrap();
    assert_eq!(joined["nodeId"], fx.config.node_id.as_str());
    let targets = joined["targets"].as_array().unwrap();
    assert_eq!(targets.len(), 1);
    assert_eq!(targets[0]["id"], "child-a");
    for secret in ["routes", "hostKeys", "user", "bridgePath"] {
        assert!(targets[0].get(secret).is_none(), "{secret} must stay private");
    }
    let mut target_only = fx.config.clone();
    target_only.roles = vec!["target".into()];
    assert!(fleet::relay_join(&fx.state, &target_only, &merging)
        .unwrap_err()
        .to_string()
        .contains("master role is not enabled"));
}
#[test]
fn disabled_merge_blocks_join_and_relay_until_allowed_again() {
    let fx = Fixture::new();
    let merging = Uuid::new_v4().to_string();
    let request = json!({"targetId":"child-a","method":"jobs.list","params":{}});
    assert!(
        fleet::relay_request(&fx.state, &fx.config, &merging, &request)
            .unwrap_err()
            .to_string()
            .contains("merge this Master before relaying")
    );
    fleet::relay_join(&fx.state, &fx.config, &merging).unwrap();
    let relays = fx.call("relays.list", json!({})).unwrap();
    assert_eq!(relays[0]["masterId"], merging.as_str());
    assert_eq!(relays[0]["revoked"], false);
    fx.call("relays.revoke", json!({ "masterId": merging }))
        .unwrap();
    for error in [
        fleet::relay_join(&fx.state, &fx.config, &merging).unwrap_err(),
        fleet::relay_request(&fx.state, &fx.config, &merging, &request).unwrap_err(),
    ] {
        assert!(error.to_string().contains("merge was disabled"));
    }
    fx.call("relays.allow", json!({ "masterId": merging }))
        .unwrap();
    fleet::relay_join(&fx.state, &fx.config, &merging).unwrap();
}
#[test]
fn relay_requests_are_one_hop_and_limited_to_fleet_work() {
    let fx = Fixture::new();
    let merging = Uuid::new_v4().to_string();
    fleet::relay_join(&fx.state, &fx.config, &merging).unwrap();
    let relay = |target: &str, method: &str| {
        fleet::relay_request(
            &fx.state,
            &fx.config,
            &merging,
            &json!({"targetId":target,"method":method,"params":{}}),
        )
        .unwrap_err()
        .to_string()
    };
    assert!(relay("peer/child", "jobs.list").contains("relayed targetId"));
    for method in ["merges.add", "targets.remove", "relays.allow", "masters.revoke"] {
        assert!(relay("child", method).contains("not available over relay"));
    }
    assert!(relay("missing", "jobs.list").contains("unknown target"));
}
#[test]
fn merged_children_are_listed_with_composite_ids_and_validated() {
    let fx = Fixture::new();
    fx.allow_import();
    let peer = registration("peer", "10.0.0.1");
    fx.call("targets.import", json!({"registration":peer}))
        .unwrap();
    let direct = registration("direct", "10.0.0.2");
    fx.call("targets.import", json!({"registration":direct}))
        .unwrap();
    let mut duplicate = child("same-machine");
    duplicate["nodeId"] = direct["nodeId"].clone();
    let join = json!({"nodeId":peer["nodeId"],"label":"Peer Master","targets":[child("child-a"),duplicate]});
    let mut forged = join.clone();
    forged["nodeId"] = json!(Uuid::new_v4().to_string());
    assert!(fleet::record_merge(&fx.state, "peer", &forged)
        .unwrap_err()
        .to_string()
        .contains("E_IDENTITY"));
    let mut nested = join.clone();
    nested["targets"] = json!([child("other/child")]);
    assert!(fleet::record_merge(&fx.state, "peer", &nested).is_err());
    let merge = fleet::record_merge(&fx.state, "peer", &join).unwrap();
    assert_eq!(merge["status"], "merged");
    let list = fx.call("targets.list", json!({})).unwrap();
    let ids: Vec<_> = list
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["id"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(ids, ["direct", "peer", "peer/child-a"]);
    let relayed = &list[2];
    assert_eq!(relayed["relayed"], true);
    assert_eq!(relayed["via"], "peer");
    // The badge uses this Master's own name for the merged Master, not its self-label.
    assert_eq!(relayed["viaLabel"], "peer");
    assert_eq!(fx.call("merges.list", json!({})).unwrap()[0]["children"].as_array().unwrap().len(), 2);
    fx.call("merges.remove", json!({"targetId":"peer"}))
        .unwrap();
    assert_eq!(
        fx.call("targets.list", json!({}))
            .unwrap()
            .as_array()
            .unwrap()
            .len(),
        2
    );
}
#[test]
fn composite_dispatch_requires_a_merge_and_a_known_child() {
    let fx = Fixture::new();
    fx.allow_import();
    let peer = registration("peer", "10.0.0.1");
    fx.call("targets.import", json!({"registration":peer}))
        .unwrap();
    let start = |target: &str| {
        let params = json!({"targetId":target,"jobId":Uuid::new_v4().to_string(),"cwd":"/tmp","script":"true"});
        format!("{:#}", fx.call("jobs.start", params).unwrap_err())
    };
    assert!(start("peer/child-a").contains("is not merged"));
    fleet::record_merge(
        &fx.state,
        "peer",
        &json!({"nodeId":peer["nodeId"],"label":"Peer","targets":[child("child-a")]}),
    )
    .unwrap();
    assert!(start("peer/unknown").contains("sync the merge"));
    assert!(start("peer/a/b").contains("merged target id"));
    assert!(start("nobody/child-a").contains("unknown target"));
    // Missing managed key stops before networking, proving the relay route was chosen.
    fs::remove_file(fx.state.join("keys/company.ed25519")).unwrap();
    let job = Uuid::new_v4().to_string();
    let params = json!({"targetId":"peer/child-a","jobId":job,"cwd":"/tmp","script":"true"});
    let error = format!("{:#}", fx.call("jobs.start", params).unwrap_err());
    assert!(error.contains(&job));
    assert!(fx
        .call("targets.doctor", json!({"targetId":"peer/missing"}))
        .unwrap_err()
        .to_string()
        .contains("sync the merge"));
}
#[test]
fn pairing_back_requires_an_active_merge_and_the_callers_own_code() {
    let fx = Fixture::new();
    let merging = Uuid::new_v4().to_string();
    let pair = json!({"os":"windows","routes":[{"kind":"lan","address":"10.0.0.9","port":22}]});
    let complete = json!({"code":"SSHCC1-not-a-code"});
    for error in [
        fleet::relay_pair(&fx.state, &fx.config, &merging, &pair).unwrap_err(),
        fleet::relay_pair_complete(&fx.state, &fx.config, &merging, &complete).unwrap_err(),
        fleet::relay_health(&fx.state, &fx.config, &merging).unwrap_err(),
    ] {
        assert!(error.to_string().contains("merge this Master before relaying"));
    }
    fleet::relay_join(&fx.state, &fx.config, &merging).unwrap();
    fx.call("relays.revoke", json!({ "masterId": merging }))
        .unwrap();
    for error in [
        fleet::relay_pair(&fx.state, &fx.config, &merging, &pair).unwrap_err(),
        fleet::relay_pair_complete(&fx.state, &fx.config, &merging, &complete).unwrap_err(),
    ] {
        assert!(error.to_string().contains("merge was disabled"));
    }
    fx.call("relays.allow", json!({ "masterId": merging }))
        .unwrap();
    assert!(fleet::relay_pair_complete(&fx.state, &fx.config, &merging, &json!({}))
        .unwrap_err()
        .to_string()
        .contains("not paired back yet"));
    assert_eq!(
        fleet::relay_health(&fx.state, &fx.config, &merging).unwrap(),
        json!([])
    );
    assert!(fx
        .call("merges.pair_request", json!({"targetId":"nobody"}))
        .is_err());
}
#[test]
fn health_lists_merged_children_by_composite_id_and_unknown_when_the_peer_fails() {
    let fx = Fixture::new();
    assert_eq!(fx.call("fleet.health", json!({})).unwrap(), json!([]));
    fx.allow_import();
    let peer = registration("peer", "10.0.0.1");
    fx.call("targets.import", json!({"registration":peer}))
        .unwrap();
    fleet::record_merge(
        &fx.state,
        "peer",
        &json!({"nodeId":peer["nodeId"],"label":"Peer","targets":[child("child-a")]}),
    )
    .unwrap();
    // Without the managed key every probe fails before networking.
    fs::remove_file(fx.state.join("keys/company.ed25519")).unwrap();
    let rows = fx.call("fleet.health", json!({"refresh":true})).unwrap();
    let rows = rows.as_array().unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["targetId"], "peer");
    assert_eq!(rows[0]["online"], false);
    assert_eq!(rows[1]["targetId"], "peer/child-a");
    assert_eq!(rows[1]["online"], Value::Null);
    assert_eq!(rows[1]["via"], "peer");
    assert!(rows[1]["error"].as_str().unwrap().starts_with("via peer:"));
    // Fresh rows are served from the cache without probing again.
    assert_eq!(fx.call("fleet.health", json!({})).unwrap().as_array().unwrap().len(), 2);
}
#[test]
fn routine_health_reads_the_ssh_banner_without_logging_in() {
    use std::{io::Write, net::TcpListener};
    let fx = Fixture::new();
    fx.allow_import();
    let sshd = TcpListener::bind("127.0.0.1:0").unwrap();
    let open = sshd.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for mut stream in sshd.incoming().flatten() {
            let _ = stream.write_all(b"SSH-2.0-OpenSSH_test\r\n");
        }
    });
    let closed = TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    for (id, port) in [("up", open), ("down", closed)] {
        let mut reg = registration(id, "127.0.0.1");
        reg["routes"][0]["port"] = json!(port);
        fx.call("targets.import", json!({ "registration": reg }))
            .unwrap();
    }
    // The managed key is gone, so any SSH attempt would fail; only the banner can succeed.
    fs::remove_file(fx.state.join("keys/company.ed25519")).unwrap();
    let rows = fx.call("fleet.health", json!({})).unwrap();
    let row = |id: &str| {
        rows.as_array()
            .unwrap()
            .iter()
            .find(|r| r["targetId"] == id)
            .unwrap()
            .clone()
    };
    assert_eq!(row("up")["online"], true);
    assert_eq!(row("up")["failures"], 0);
    assert_eq!(row("down")["online"], false);
    assert_eq!(row("down")["failures"], 1);
    assert!(row("down")["error"]
        .as_str()
        .unwrap()
        .contains(&closed.to_string()));
    // A failing Target backs off; refresh asks over SSH, which fails here without the key.
    let cached = fx.call("fleet.health", json!({})).unwrap();
    assert_eq!(cached, rows);
    let refreshed = fx.call("fleet.health", json!({"refresh":true})).unwrap();
    assert!(refreshed
        .as_array()
        .unwrap()
        .iter()
        .all(|r| r["online"] == false));
}
