//! Read and control containers through the host's docker or podman CLI.
//! Each request runs at most two bounded CLI calls in the service process; there is
//! no daemon client, cache or background polling. Rootless Owner containers are not seen.
use anyhow::{bail, Context, Result};
use serde_json::{json, Value};
use std::{
    env,
    io::Read,
    path::{Path, PathBuf},
    process::{Child, Stdio},
    thread,
    time::{Duration, Instant},
};

const LIST_TIMEOUT: Duration = Duration::from_secs(10);
/// `stop` waits up to 10 s for the container before the runtime kills it.
const ACTION_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_OUTPUT: usize = 2 * 1024 * 1024;
const MAX_ROWS: usize = 500;
const MAX_LOG_LINES: u64 = 500;
const MAX_LOG_BYTES: usize = 256 * 1024;

pub fn handle(method: &str, p: &Value) -> Result<Value> {
    match method {
        "ct.list" => list(),
        "ct.logs" => logs(p),
        "ct.action" => action(p),
        _ => bail!("E_ARGUMENT: unknown container method"),
    }
}

/// Docker first, then Podman, found on the service PATH (podman-docker's shim counts as docker).
fn runtime() -> Option<(&'static str, PathBuf)> {
    let path = env::var_os("PATH")?;
    for name in ["docker", "podman"] {
        for dir in env::split_paths(&path) {
            let candidate = dir.join(if cfg!(windows) { format!("{name}.exe") } else { name.into() });
            if candidate.is_file() {
                return Some((name, candidate));
            }
        }
    }
    None
}

fn require_runtime() -> Result<(&'static str, PathBuf)> {
    runtime().context("E_NO_RUNTIME: docker or podman was not found on this machine")
}

struct Output {
    status: i32,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

fn drain(mut stream: impl Read + Send + 'static, limit: usize) -> thread::JoinHandle<Vec<u8>> {
    thread::spawn(move || {
        let mut kept = Vec::new();
        let _ = (&mut stream).take(limit as u64 + 1).read_to_end(&mut kept);
        // Keep reading so a chatty child never blocks on a full pipe.
        let _ = std::io::copy(&mut stream, &mut std::io::sink());
        kept
    })
}

fn run(program: &Path, args: &[&str], timeout: Duration) -> Result<Output> {
    let mut child: Child = crate::fleet::quiet_command(&program.to_string_lossy())
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("E_RUNTIME: could not start {}", program.display()))?;
    let stdout = drain(child.stdout.take().unwrap(), MAX_OUTPUT);
    let stderr = drain(child.stderr.take().unwrap(), 64 * 1024);
    let started = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if started.elapsed() >= timeout {
            let _ = child.kill();
            let _ = child.wait();
            bail!("E_TIMEOUT: {} did not answer within {} s", args.first().unwrap_or(&""), timeout.as_secs());
        }
        thread::sleep(Duration::from_millis(25));
    };
    let stdout = stdout.join().unwrap_or_default();
    let stderr = stderr.join().unwrap_or_default();
    if stdout.len() > MAX_OUTPUT {
        bail!("E_TOO_LARGE: container runtime output exceeds {} MiB", MAX_OUTPUT / 1024 / 1024);
    }
    Ok(Output { status: status.code().unwrap_or(-1), stdout, stderr })
}

fn failure(output: &Output) -> anyhow::Error {
    let message = String::from_utf8_lossy(&output.stderr);
    anyhow::anyhow!("E_RUNTIME: {}", message.trim().chars().take(500).collect::<String>())
}

fn checked(output: Output) -> Result<String> {
    if output.status != 0 {
        return Err(failure(&output));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn list() -> Result<Value> {
    let Some((name, program)) = runtime() else {
        return Ok(json!({"runtime": null, "containers": [], "images": []}));
    };
    let template = "{{json .}}";
    // A stopped daemon is reported, not raised, so the page can say why the list is empty.
    let containers = match run(&program, &["ps", "-a", "--no-trunc", "--format", template], LIST_TIMEOUT).and_then(checked) {
        Ok(text) => parse_lines(&text, container_row)?,
        Err(error) => return Ok(json!({"runtime": name, "error": format!("{error:#}"), "containers": [], "images": []})),
    };
    let images = parse_lines(&checked(run(&program, &["images", "--format", template], LIST_TIMEOUT)?)?, image_row)?;
    let truncated = containers.len() >= MAX_ROWS || images.len() >= MAX_ROWS;
    Ok(json!({"runtime": name, "containers": containers, "images": images, "truncated": truncated}))
}

fn parse_lines(text: &str, row: fn(&Value) -> Value) -> Result<Vec<Value>> {
    let mut rows = Vec::new();
    for line in text.lines().map(str::trim).filter(|line| !line.is_empty()) {
        if rows.len() >= MAX_ROWS {
            break;
        }
        let value: Value = serde_json::from_str(line).context("E_RUNTIME: unexpected container runtime output")?;
        rows.push(row(&value));
    }
    Ok(rows)
}

/// First present field among Docker (`ID`, `Names`) and Podman (`Id`, `Names[]`) spellings.
fn field<'a>(value: &'a Value, keys: &[&str]) -> Option<&'a Value> {
    keys.iter().find_map(|key| value.get(*key).filter(|v| !v.is_null()))
}

fn text(value: &Value, keys: &[&str]) -> String {
    match field(value, keys) {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(items)) => items.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(", "),
        Some(Value::Number(n)) => n.to_string(),
        _ => String::new(),
    }
}

fn ports(value: &Value) -> String {
    match field(value, &["Ports"]) {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(items)) => items
            .iter()
            .map(|port| {
                let container = text(port, &["container_port", "containerPort"]);
                let protocol = text(port, &["protocol"]);
                match text(port, &["host_port", "hostPort"]).as_str() {
                    "" | "0" => format!("{container}/{protocol}"),
                    host => format!("{host}->{container}/{protocol}"),
                }
            })
            .collect::<Vec<_>>()
            .join(", "),
        _ => String::new(),
    }
}

fn container_row(value: &Value) -> Value {
    let id = text(value, &["ID", "Id"]);
    json!({
        "id": id,
        "name": text(value, &["Names", "Name"]).trim_start_matches('/'),
        "image": text(value, &["Image"]),
        "state": text(value, &["State"]).to_ascii_lowercase(),
        "status": text(value, &["Status"]),
        "ports": ports(value),
        "created": text(value, &["CreatedAt", "Created"]),
    })
}

fn image_row(value: &Value) -> Value {
    let (mut repository, mut tag) = (text(value, &["Repository"]), text(value, &["Tag"]));
    if repository.is_empty() {
        // Podman lists references as Names: ["docker.io/library/nginx:1.27"].
        let reference = field(value, &["Names"]).and_then(|v| v.get(0)).and_then(Value::as_str).unwrap_or("");
        match reference.rsplit_once(':').filter(|(_, t)| !t.contains('/')) {
            Some((repo, t)) => (repository, tag) = (repo.into(), t.into()),
            None => repository = reference.into(),
        }
    }
    let size = field(value, &["Size", "VirtualSize"]);
    json!({
        "id": text(value, &["ID", "Id"]),
        "repository": if repository.is_empty() { "<none>".into() } else { repository },
        "tag": if tag.is_empty() { "<none>".into() } else { tag },
        "size": size.and_then(Value::as_str),
        "sizeBytes": size.and_then(Value::as_u64),
        "created": text(value, &["CreatedSince", "CreatedAt", "Created"]),
    })
}

/// Container names and IDs only: no shell is used, but option-looking or path input is refused.
fn container_ref(p: &Value) -> Result<String> {
    let reference = p.get("container").and_then(Value::as_str).context("E_ARGUMENT: container is required")?;
    let mut chars = reference.chars();
    let valid = reference.len() <= 255
        && chars.next().is_some_and(|c| c.is_ascii_alphanumeric())
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-'));
    if !valid || reference.contains("..") {
        bail!("E_ARGUMENT: invalid container name or ID");
    }
    Ok(reference.into())
}

fn logs(p: &Value) -> Result<Value> {
    let reference = container_ref(p)?;
    let tail = p.get("tail").and_then(Value::as_u64).unwrap_or(200).clamp(1, MAX_LOG_LINES).to_string();
    let (name, program) = require_runtime()?;
    let output = run(&program, &["logs", "--tail", &tail, "--timestamps", &reference], LIST_TIMEOUT)?;
    if output.status != 0 {
        return Err(failure(&output));
    }
    // The runtime splits the container's stdout/stderr; RFC 3339 timestamps sort them back together.
    let mut lines: Vec<&str> = Vec::new();
    let (stdout, stderr) = (String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
    lines.extend(stdout.lines());
    lines.extend(stderr.lines());
    lines.sort_by(|a, b| a.split(' ').next().cmp(&b.split(' ').next()));
    let mut content = lines.join("\n");
    let truncated = content.len() > MAX_LOG_BYTES;
    if truncated {
        let mut start = content.len() - MAX_LOG_BYTES;
        while !content.is_char_boundary(start) {
            start += 1;
        }
        content = content[start..].to_string();
    }
    Ok(json!({"runtime": name, "container": reference, "content": content, "truncated": truncated}))
}

fn action(p: &Value) -> Result<Value> {
    let reference = container_ref(p)?;
    let verb = p.get("action").and_then(Value::as_str).unwrap_or("");
    let args: Vec<&str> = match verb {
        "start" => vec!["start", &reference],
        "stop" => vec!["stop", "--time", "10", &reference],
        "restart" => vec!["restart", "--time", "10", &reference],
        _ => bail!("E_ARGUMENT: action must be start, stop or restart"),
    };
    let (name, program) = require_runtime()?;
    checked(run(&program, &args, ACTION_TIMEOUT)?)?;
    Ok(json!({"runtime": name, "container": reference, "action": verb, "ok": true}))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn docker_and_podman_rows_normalize_to_the_same_keys() {
        let docker = r#"{"ID":"4f1c2a","Names":"web","Image":"nginx:1.27","State":"running","Status":"Up 3 hours","Ports":"0.0.0.0:8080->80/tcp","CreatedAt":"2026-10-17 08:00:00 +0700 +07"}"#;
        let podman = r#"{"Id":"9a8b7c","Names":["db"],"Image":"docker.io/library/postgres:16","State":"exited","Status":"Exited (0) 2 days ago","Ports":[{"host_ip":"","container_port":5432,"host_port":15432,"range":1,"protocol":"tcp"}],"Created":1760680800}"#;
        let rows = parse_lines(&format!("{docker}\n\n{podman}\n"), container_row).unwrap();
        assert_eq!(rows[0]["name"], "web");
        assert_eq!(rows[0]["state"], "running");
        assert_eq!(rows[0]["ports"], "0.0.0.0:8080->80/tcp");
        assert_eq!(rows[1]["id"], "9a8b7c");
        assert_eq!(rows[1]["name"], "db");
        assert_eq!(rows[1]["ports"], "15432->5432/tcp");
        assert_eq!(rows[1]["created"], "1760680800");
    }

    #[test]
    fn image_rows_from_both_runtimes() {
        let docker = r#"{"Repository":"nginx","Tag":"1.27","ID":"sha256:abc","Size":"192MB","CreatedSince":"2 weeks ago"}"#;
        let podman = r#"{"Id":"def","Names":["docker.io/library/alpine:3.20"],"Size":7800000,"Created":1760000000}"#;
        let dangling = r#"{"Id":"0ff","Names":null,"Size":10}"#;
        let rows = parse_lines(&[docker, podman, dangling].join("\n"), image_row).unwrap();
        assert_eq!((rows[0]["repository"].as_str(), rows[0]["tag"].as_str(), rows[0]["size"].as_str()), (Some("nginx"), Some("1.27"), Some("192MB")));
        assert_eq!((rows[1]["repository"].as_str(), rows[1]["tag"].as_str()), (Some("docker.io/library/alpine"), Some("3.20")));
        assert_eq!(rows[1]["sizeBytes"], 7800000);
        assert_eq!(rows[2]["repository"], "<none>");
    }

    #[test]
    fn container_references_are_strict() {
        for good in ["web", "4f1c2a9e", "my_app.1-blue"] {
            assert_eq!(container_ref(&json!({"container": good})).unwrap(), good);
        }
        for bad in ["", "-rm", "--all", "a b", "a;rm", "../x", "a..b", "/web", "é"] {
            assert!(container_ref(&json!({"container": bad})).is_err(), "{bad}");
        }
        assert!(action(&json!({"container": "web", "action": "rm"})).is_err());
    }

    #[test]
    fn row_cap_is_enforced() {
        let line = r#"{"ID":"a","Names":"n","Image":"i","State":"running","Status":"Up"}"#;
        let text = vec![line; MAX_ROWS + 20].join("\n");
        assert_eq!(parse_lines(&text, container_row).unwrap().len(), MAX_ROWS);
    }
}
