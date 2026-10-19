use serde_json::json;
use sshcc_core::{config, files, jobs::Jobs, protocol::Request, security, service::Service};
use std::{
    sync::{atomic::AtomicBool, Arc},
    time::{Duration, Instant},
};
fn script(text: &str) -> String {
    if cfg!(windows) {
        format!("Write-Output '{text}'")
    } else {
        format!("printf '%s\\n' '{text}'")
    }
}
fn wait(jobs: &Arc<Jobs>, id: &str) -> serde_json::Value {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let r = jobs.get(id).unwrap();
        if !matches!(r["status"].as_str(), Some("queued" | "running")) {
            return r;
        }
        assert!(Instant::now() < deadline, "job timed out: {r}");
        std::thread::sleep(Duration::from_millis(30));
    }
}
#[test]
fn jobs_are_deduplicated_and_persist_exit_and_logs() {
    let dir = tempfile::tempdir().unwrap();
    let jobs = Jobs::open(dir.path()).unwrap();
    let id = uuid::Uuid::new_v4().to_string();
    let request =
        json!({"jobId":id,"masterId":"master-a","script":script("durable ไทย"),"cwd":dir.path()});
    jobs.start(&request).unwrap();
    jobs.start(&request).unwrap();
    let done = wait(&jobs, &id);
    assert_eq!(done["status"], "succeeded");
    assert_eq!(done["exitCode"], 0);
    let log = jobs.logs(&json!({"jobId":id})).unwrap();
    assert!(log["content"].as_str().unwrap().contains("durable ไทย"));
    assert_eq!(jobs.list().unwrap().len(), 1);
    let mut changed = request.clone();
    changed["script"] = json!("different");
    assert!(jobs
        .start(&changed)
        .unwrap_err()
        .to_string()
        .contains("E_CONFLICT"));
}
#[test]
fn cancel_stops_running_job() {
    let dir = tempfile::tempdir().unwrap();
    let jobs = Jobs::open(dir.path()).unwrap();
    let id = uuid::Uuid::new_v4().to_string();
    let source = if cfg!(windows) {
        "Start-Sleep -Seconds 30"
    } else {
        "sleep 30"
    };
    jobs.start(&json!({"jobId":id,"masterId":"master-a","script":source,"cwd":dir.path()}))
        .unwrap();
    std::thread::sleep(Duration::from_millis(200));
    jobs.cancel(&id).unwrap();
    assert_eq!(wait(&jobs, &id)["status"], "cancelled");
}
#[test]
fn service_shutdown_marks_work_interrupted() {
    let dir = tempfile::tempdir().unwrap();
    let jobs = Jobs::open(dir.path()).unwrap();
    let id = uuid::Uuid::new_v4().to_string();
    let source = if cfg!(windows) {
        "Start-Sleep -Seconds 30"
    } else {
        "sleep 30"
    };
    jobs.start(&json!({"jobId":id,"masterId":"master-a","script":source,"cwd":dir.path()}))
        .unwrap();
    std::thread::sleep(Duration::from_millis(150));
    jobs.shutdown();
    assert_eq!(jobs.get(&id).unwrap()["status"], "interrupted");
}
#[test]
fn file_roundtrip_is_binary_and_hash_guarded() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("file.bin");
    let result =
        files::handle("fs.write", &json!({"path":path,"contentBase64":"AAECA//+"})).unwrap();
    let read = files::handle("fs.read", &json!({"path":path})).unwrap();
    assert_eq!(read["contentBase64"], "AAECA//+");
    assert_eq!(read["sha256"], result["sha256"]);
    assert!(files::handle(
        "fs.write",
        &json!({"path":path,"contentBase64":"","expectedSha256":"wrong"})
    )
    .is_err());
    assert_eq!(
        files::handle("fs.read", &json!({"path":path})).unwrap()["contentBase64"],
        "AAECA//+"
    );
}
#[test]
fn service_checks_local_principal_and_role() {
    let dir = tempfile::tempdir().unwrap();
    let owner = security::current_principal().unwrap();
    config::init(dir.path(), "master", &owner, "Test", &[]).unwrap();
    let app = Service::new(dir.path().into(), Arc::new(AtomicBool::new(false))).unwrap();
    let status = Request {
        method: "system.status".into(),
        params: json!({}),
    };
    assert!(app.handle(&status, &owner).is_ok());
    assert!(app
        .handle(&status, "unauthorized")
        .unwrap_err()
        .to_string()
        .contains("E_PERMISSION"));
    assert!(app
        .handle(
            &Request {
                method: "runner.list".into(),
                params: json!({})
            },
            &owner
        )
        .unwrap_err()
        .to_string()
        .contains("E_ROLE"));
}
#[test]
fn mcp_negotiates_and_lists_real_tools_without_stdout_noise() {
    let input=b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\",\"params\":{\"protocolVersion\":\"2025-06-18\"}}\n{\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\"}\n{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/list\"}\n";
    let mut output = Vec::new();
    sshcc_core::mcp::serve(&input[..], &mut output).unwrap();
    let responses: Vec<serde_json::Value> = String::from_utf8(output)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(responses.len(), 2);
    assert_eq!(responses[0]["result"]["protocolVersion"], "2025-06-18");
    assert!(responses[1]["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .any(|t| t["name"] == "jobs_start"));
}

#[test]
fn restart_recovers_unfinished_jobs_older_than_history_page() {
    let dir = tempfile::tempdir().unwrap();
    drop(Jobs::open(dir.path()).unwrap());
    let mut db = rusqlite::Connection::open(dir.path().join("jobs.sqlite")).unwrap();
    let transaction = db.transaction().unwrap();
    for index in 0..1002 {
        let status = if index == 0 {
            "running"
        } else if index == 1 {
            "queued"
        } else {
            "succeeded"
        };
        let record = json!({"id": index.to_string(), "status": status});
        transaction
            .execute(
                "INSERT INTO jobs(id,payload,record) VALUES (?1,'{}',?2)",
                rusqlite::params![index.to_string(), record.to_string()],
            )
            .unwrap();
    }
    transaction.commit().unwrap();
    drop(db);
    let jobs = Jobs::open(dir.path()).unwrap();
    assert_eq!(jobs.list().unwrap().len(), 1000);
    assert_eq!(jobs.get("0").unwrap()["status"], "interrupted");
    assert_eq!(jobs.get("1").unwrap()["status"], "interrupted");
    assert_eq!(jobs.get("1001").unwrap()["status"], "succeeded");
}
