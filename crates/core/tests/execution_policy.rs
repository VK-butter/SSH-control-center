use serde_json::json;
use sshcc_core::{config, executor::Executor, pairing};
#[test]
fn schema_one_remains_service_and_migration_preserves_identity() {
    let dir = tempfile::tempdir().unwrap();
    let owner = sshcc_core::security::current_principal().unwrap();
    let original = config::init(dir.path(), "both", &owner, "Owner test", &[]).unwrap();
    assert_eq!(original.schema_version, 2);
    assert_eq!(original.execution_mode, "owner-session");
    let mut old = serde_json::to_value(&original).unwrap();
    old["schemaVersion"] = json!(1);
    old.as_object_mut().unwrap().remove("executionMode");
    std::fs::write(
        dir.path().join("config.json"),
        serde_json::to_vec(&old).unwrap(),
    )
    .unwrap();
    let preserved = config::init(dir.path(), "both", &owner, "Owner test", &[]).unwrap();
    assert_eq!(preserved.execution_mode, "service");
    assert_eq!(preserved.schema_version, 1);
    let migrated = config::set_execution_mode(dir.path(), "owner-session").unwrap();
    assert_eq!(migrated.owner, original.owner);
    assert_eq!(migrated.node_id, original.node_id);
    assert_eq!(migrated.schema_version, 2);
    assert!(config::set_execution_mode(dir.path(), "root-fallback").is_err());
}
#[test]
fn terminal_metadata_blocks_installer_without_recovering_work() {
    let dir = tempfile::tempdir().unwrap();
    let db = rusqlite::Connection::open(dir.path().join("terminals.sqlite")).unwrap();
    db.execute_batch("CREATE TABLE terminals(id TEXT PRIMARY KEY,record TEXT);INSERT INTO terminals VALUES('test','{\"status\":\"running\"}');").unwrap();
    assert_eq!(
        pairing::installer_preflight(dir.path()).unwrap()["runningTerminals"],
        1
    );
}
#[cfg(unix)]
#[test]
fn absent_owner_never_falls_back_to_root() {
    let dir = tempfile::tempdir().unwrap();
    // nobody is a real account without a login session on the build host.
    let cfg = config::init(dir.path(), "target", "65534", "Absent Owner", &[]).unwrap();
    let executor = Executor::new(cfg);
    assert!(executor
        .select(&json!({}))
        .unwrap_err()
        .to_string()
        .contains("E_OWNER_NOT_LOGGED_IN"));
}
