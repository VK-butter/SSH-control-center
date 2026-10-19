//! One JSON request per SSH connection; target service owns long-running work.
use super::{key_path, quiet_command, Config, Route, Target};
use anyhow::{bail, ensure, Context, Result};
use serde_json::{json, Value};
use std::{
    fs,
    io::{BufReader, Read, Write},
    path::{Path, PathBuf},
    process::{ExitStatus, Stdio},
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};
use uuid::Uuid;
const OUTPUT_LIMIT: usize = 8 * 1024 * 1024;
const REQUEST_LIMIT: usize = 8 * 1024 * 1024 - 1024;
const SSH_TIMEOUT: Duration = Duration::from_secs(45);
/// A relay waits for the merged Master's own SSH call to the Target.
pub(super) const RELAY_TIMEOUT: Duration = Duration::from_secs(100);

struct Session {
    directory: PathBuf,
}
impl Drop for Session {
    fn drop(&mut self) {
        // Only delete this call's two generated files, never recurse through state.
        let _ = fs::remove_file(self.directory.join("config"));
        let _ = fs::remove_file(self.directory.join("known_hosts"));
        let _ = fs::remove_dir(&self.directory);
    }
}
fn config_quote(value: &str) -> Result<String> {
    ensure!(
        !value.chars().any(char::is_control) && !value.contains(['"', '%', '$', '\u{0060}']),
        "path contains characters unsupported by OpenSSH config"
    );
    Ok(format!("\"{}\"", value.replace('\\', "\\\\")))
}
fn managed_key(state: &Path, path: &Path) -> Result<PathBuf> {
    let folder = state
        .join("keys")
        .canonicalize()
        .context("no Master keys; export an enrollment bundle first")?;
    let path = path
        .canonicalize()
        .context("configured service-owned identity does not exist")?;
    ensure!(
        path.starts_with(&folder) && path != folder && path.is_file(),
        "SSH identities must be service-owned files inside this service's keys directory"
    );
    // A copied/junctioned external identity is not allowed to escape service ownership.
    crate::security::protect(&path, false)?;
    Ok(path)
}
fn display_path(path: &Path) -> String {
    // OpenSSH for Windows does not accept the extended-length prefix.
    let value = path.to_string_lossy();
    value
        .strip_prefix(r"\\?\")
        .unwrap_or(&value)
        .replace('\\', "/")
}
fn session_config(state: &Path, target: &Target, route: &Route) -> Result<Session> {
    let identity = managed_key(state, &key_path(state, &target.tenant))?;
    let root = state.join("transport");
    fs::create_dir_all(&root)?;
    crate::security::protect(&root, true)?;
    let directory = root.join(Uuid::new_v4().to_string());
    fs::create_dir(&directory)?;
    crate::security::protect(&directory, true)?;
    let session = Session { directory };
    let pins_path = session.directory.join("known_hosts");
    let config_path = session.directory.join("config");
    let alias = format!("sshcc-{}", target.node_id);
    let mut pins = target
        .host_keys
        .iter()
        .map(|key| format!("{alias} {key}\n"))
        .collect::<String>();
    let mut text = format!(
        "Host target\n  HostName {}\n  Port {}\n  User {}\n  IdentityFile {}\n  HostKeyAlias {}\n",
        route.address,
        route.port,
        config_quote(&target.user)?,
        config_quote(&display_path(&identity))?,
        alias
    );
    if let Some(jump) = &route.proxy_jump {
        let identity = managed_key(state, Path::new(&jump.identity_file))?;
        text.push_str(&format!(
            "  ProxyJump jump\nHost jump\n  HostName {}\n  Port {}\n  User {}\n  IdentityFile {}\n  HostKeyAlias sshcc-jump\n",
            jump.address,jump.port,config_quote(&jump.user)?,config_quote(&display_path(&identity))?
        ));
        for key in &jump.host_keys {
            pins.push_str(&format!("sshcc-jump {key}\n"));
        }
    }
    text.push_str(&format!(
        "Host *\n  BatchMode yes\n  IdentitiesOnly yes\n  IdentityAgent none\n  ForwardAgent no\n  ClearAllForwardings yes\n  PermitLocalCommand no\n  StrictHostKeyChecking yes\n  UserKnownHostsFile {}\n  GlobalKnownHostsFile none\n  UpdateHostKeys no\n  CheckHostIP no\n  PasswordAuthentication no\n  KbdInteractiveAuthentication no\n  ControlMaster no\n  ControlPath none\n  ControlPersist no\n  ConnectTimeout 8\n  ConnectionAttempts 1\n  ServerAliveInterval 5\n  ServerAliveCountMax 2\n  LogLevel ERROR\n",
        config_quote(&display_path(&pins_path))?
    ));
    fs::write(&pins_path, pins)?;
    crate::security::protect(&pins_path, false)?;
    fs::write(&config_path, text)?;
    crate::security::protect(&config_path, false)?;
    Ok(session)
}
struct Output {
    status: ExitStatus,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}
fn read_bounded(mut input: impl Read, max: usize) -> std::io::Result<Vec<u8>> {
    let mut result = Vec::new();
    let mut chunk = [0u8; 8192];
    loop {
        let n = input.read(&mut chunk)?;
        if n == 0 {
            return Ok(result);
        }
        if result.len() + n > max {
            return Err(std::io::Error::other("SSH response exceeds permitted size"));
        }
        result.extend_from_slice(&chunk[..n]);
    }
}
fn run_ssh(
    state: &Path,
    session: &Session,
    target: &Target,
    config: &Config,
    request: &Value,
    timeout: Duration,
) -> Result<Output> {
    let mut payload = serde_json::to_vec(request)?;
    ensure!(
        payload.len() <= REQUEST_LIMIT,
        "SSH request exceeds the 8 MiB IPC frame budget"
    );
    payload.push(b'\n');
    let public = fs::read_to_string(key_path(state, &target.tenant).with_extension("ed25519.pub"))?;
    let fingerprint = super::public_key_fingerprint(&public)?;
    let remote_command = if target.os == "windows" {
        format!(
            "\"{}\" bridge --master-id {} --master-key {}",
            target.bridge_path, config.node_id, fingerprint
        )
    } else {
        format!(
            "'{}' bridge --master-id {} --master-key {}",
            target.bridge_path, config.node_id, fingerprint
        )
    };
    let mut command = quiet_command("ssh");
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    let mut child = command
        .current_dir(&session.directory)
        .arg("-F")
        .arg("config")
        .args(["-T", "target"])
        .arg(remote_command)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("OpenSSH client is required")?;
    let stdout = child.stdout.take().context("missing SSH stdout")?;
    let stderr = child.stderr.take().context("missing SSH stderr")?;
    let mut stdin = child.stdin.take().context("missing SSH stdin")?;
    // All pipes have concurrent readers/writers: large file requests cannot deadlock.
    let (out_tx, out_rx) = mpsc::channel();
    thread::spawn(move || {
        let _ = out_tx.send(read_bounded(stdout, OUTPUT_LIMIT));
    });
    let (err_tx, err_rx) = mpsc::channel();
    thread::spawn(move || {
        let _ = err_tx.send(read_bounded(stderr, 128 * 1024));
    });
    let (in_tx, in_rx) = mpsc::channel();
    thread::spawn(move || {
        let _ = in_tx.send(stdin.write_all(&payload));
    });
    let start = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if start.elapsed() >= timeout {
            #[cfg(windows)]
            {
                let _ = quiet_command("taskkill")
                    .args(["/PID", &child.id().to_string(), "/T", "/F"])
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status();
            }
            #[cfg(unix)]
            unsafe {
                libc::kill(-(child.id() as i32), libc::SIGKILL);
            }
            let _ = child.kill();
            let _ = child.wait();
            bail!(
                "SSH transport timed out after {} seconds; remote job state is unknown",
                timeout.as_secs()
            );
        }
        thread::sleep(Duration::from_millis(20));
    };
    let stdout = out_rx
        .recv_timeout(Duration::from_secs(2))
        .context("SSH stdout did not close")??;
    let stderr = err_rx
        .recv_timeout(Duration::from_secs(2))
        .context("SSH stderr did not close")??;
    // A failed SSH connection can close stdin before accepting the request. Preserve
    // stderr/status so host-key failures still stop fallback.
    let write_result = in_rx
        .recv_timeout(Duration::from_secs(2))
        .context("SSH stdin did not close")?;
    if status.success() {
        write_result.context("failed writing SSH request")?;
    }
    Ok(Output {
        status,
        stdout,
        stderr,
    })
}
fn host_mismatch(message: &str) -> bool {
    let message = message.to_ascii_lowercase();
    [
        "remote host identification has changed",
        "host key verification failed",
        "offending ",
        "no ed25519 host key is known",
        "no ecdsa host key is known",
        "no rsa host key is known",
        "host key for ",
        "possible dns spoofing",
    ]
    .iter()
    .any(|needle| message.contains(needle))
}
pub(crate) struct Stream {
    pub input: Option<std::process::ChildStdin>,
    pub output: BufReader<std::process::ChildStdout>,
    pub initial: crate::protocol::Response,
    child: std::process::Child,
    _session: Session,
}
impl Drop for Stream {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
pub(super) fn attach(
    state: &Path,
    config: &Config,
    target: &Target,
    method: &str,
    params: &Value,
) -> Result<Stream> {
    let handshake = Duration::from_secs(if method == "relay.attach" { 45 } else { 20 });
    let mut failures = Vec::new();
    for route in &target.routes {
        let session = session_config(state, target, route)?;
        let public =
            fs::read_to_string(key_path(state, &target.tenant).with_extension("ed25519.pub"))?;
        let fingerprint = super::public_key_fingerprint(&public)?;
        let command = if target.os == "windows" {
            format!(
                "\"{}\" bridge --master-id {} --master-key {}",
                target.bridge_path, config.node_id, fingerprint
            )
        } else {
            format!(
                "'{}' bridge --master-id {} --master-key {}",
                target.bridge_path, config.node_id, fingerprint
            )
        };
        let mut child = quiet_command("ssh")
            .current_dir(&session.directory)
            .args(["-F", "config", "-T", "target"])
            .arg(command)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        let mut input = child.stdin.take().unwrap();
        let output = child.stdout.take().unwrap();
        let stderr = child.stderr.take().unwrap();
        let (err_tx, err_rx) = mpsc::sync_channel(1);
        thread::spawn(move || {
            let _ = err_tx.send(read_bounded(stderr, 128 * 1024));
        });
        let request = json!({"method":"bridge.stream","params":{"expectedTargetId":target.node_id,"request":{"method":method,"params":params}}});
        crate::protocol::write_frame(&request, &mut input)?;
        let (tx, rx) = mpsc::sync_channel(1);
        thread::spawn(move || {
            let mut output = BufReader::new(output);
            let initial = crate::protocol::read_frame::<crate::protocol::Response>(&mut output);
            let _ = tx.send((output, initial));
        });
        match rx.recv_timeout(handshake) {
            Ok((output, Ok(initial))) => {
                initial.clone().into_result()?;
                return Ok(Stream {
                    input: Some(input),
                    output,
                    initial,
                    child,
                    _session: session,
                });
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                let message = err_rx
                    .recv_timeout(Duration::from_secs(1))
                    .ok()
                    .and_then(Result::ok)
                    .map(|v| String::from_utf8_lossy(&v).into_owned())
                    .unwrap_or_else(|| "SSH stream handshake timed out".into());
                if host_mismatch(&message) {
                    bail!("HOST_KEY_MISMATCH: {message}");
                }
                failures.push(message);
            }
        }
    }
    bail!("ALL_ROUTES_FAILED: {}", failures.join("; "))
}
enum Attempt {
    Success(Value),
    Retry(String),
    Stop(String),
}
fn attempt(
    state: &Path,
    config: &Config,
    target: &Target,
    route: &Route,
    method: &str,
    params: &Value,
    timeout: Duration,
) -> Attempt {
    let session = match session_config(state, target, route) {
        Ok(session) => session,
        Err(error) => return Attempt::Stop(format!("{error:#}")),
    };
    let request = json!({"method":"bridge.request","params":{
        "expectedTargetId":target.node_id,"request":{"method":method,"params":params}
    }});
    let output = match run_ssh(state, &session, target, config, &request, timeout) {
        Ok(output) => output,
        Err(error) => return Attempt::Retry(format!("{error:#}")),
    };
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
    if host_mismatch(&stderr) {
        return Attempt::Stop(format!("HOST_KEY_MISMATCH: {stderr}"));
    }
    if !output.status.success() {
        // A bridge may exit nonzero after emitting a structured error, so parse a
        // JSON reply before treating a failing exit status as a connection failure.
        if output.stdout.is_empty() {
            return Attempt::Retry(format!("SSH exited {}: {stderr}", output.status));
        }
    }
    let response: Value = match serde_json::from_slice(&output.stdout) {
        Ok(value) => value,
        Err(error) => {
            return Attempt::Stop(format!("INVALID_BRIDGE_RESPONSE: {error}; stderr={stderr}"))
        }
    };
    match response.get("ok").and_then(Value::as_bool) {
        Some(true) => match response.get("result") {
            Some(result) => Attempt::Success(result.clone()),
            None => Attempt::Stop("INVALID_BRIDGE_RESPONSE: missing result".into()),
        },
        Some(false) => {
            let code = response
                .pointer("/error/code")
                .and_then(Value::as_str)
                .unwrap_or("REMOTE_ERROR");
            let message = response
                .pointer("/error/message")
                .and_then(Value::as_str)
                .unwrap_or("remote request failed");
            Attempt::Stop(format!("{code}: {message}"))
        }
        None => Attempt::Stop("INVALID_BRIDGE_RESPONSE: missing ok boolean".into()),
    }
}
pub(super) fn request(
    state: &Path,
    config: &Config,
    target: &Target,
    method: &str,
    params: Value,
) -> Result<Value> {
    request_within(state, config, target, method, params, SSH_TIMEOUT)
}
pub(super) fn request_within(
    state: &Path,
    config: &Config,
    target: &Target,
    method: &str,
    params: Value,
    timeout: Duration,
) -> Result<Value> {
    let mut failures = Vec::new();
    for route in &target.routes {
        match attempt(state, config, target, route, method, &params, timeout) {
            Attempt::Success(result) => return Ok(result),
            Attempt::Stop(error) => bail!("{error}"),
            Attempt::Retry(error) => failures.push(format!(
                "{} {}:{}: {error}",
                route.kind, route.address, route.port
            )),
        }
    }
    bail!("ALL_ROUTES_FAILED: {}", failures.join("; "))
}
pub(super) fn doctor(
    state: &Path,
    config: &Config,
    target: &Target,
    params: &Value,
) -> Result<Value> {
    let mut stages = vec![
        json!({"name":"local-registration","ok":true,"message":"Registration and host pins are stored"}),
    ];
    if !target.runner_installed {
        stages.push(json!({"name":"runner-installed","ok":false,"message":"A Target imported from a v1 registry needs the Target installer and a v2 registration"}));
        return Ok(json!({"targetId":target.id,"ok":false,"stages":stages}));
    }
    match managed_key(state,&key_path(state,&target.tenant)) {
        Ok(_)=>stages.push(json!({"name":"local-key","ok":true,"message":"Service-owned tenant identity is available"})),
        Err(error)=>{
            stages.push(json!({"name":"local-key","ok":false,"message":format!("{error:#}")}));
            return Ok(json!({"targetId":target.id,"ok":false,"stages":stages}));
        }
    }
    for route in &target.routes {
        let route_value = json!({"kind":route.kind,"address":route.address,"port":route.port});
        match attempt(
            state,
            config,
            target,
            route,
            "system.status",
            &json!({"inspectCli":true,"ownerSessionId":params["ownerSessionId"].clone()}),
            SSH_TIMEOUT,
        ) {
            Attempt::Success(result) => {
                if result.get("nodeId").and_then(Value::as_str) != Some(&target.node_id) {
                    stages.push(json!({"name":"ssh-runner","ok":false,"route":route_value,"message":"TARGET_IDENTITY_MISMATCH"}));
                    return Ok(json!({"targetId":target.id,"ok":false,"stages":stages}));
                }
                if result.get("privileged").and_then(Value::as_bool) != Some(true) {
                    stages.push(json!({"name":"ssh-runner","ok":false,"route":route_value,
                        "message":"Target service must run as SYSTEM/root","executionPrincipal":result.get("executionPrincipal")}));
                    return Ok(
                        json!({"targetId":target.id,"ok":false,"stages":stages,"result":result}),
                    );
                }
                stages.push(json!({"name":"ssh-runner","ok":true,"route":route_value,"message":"Pinned SSH host, Target identity and SYSTEM/root execution verified"}));
                if let Some(executor) = result.get("executor") {
                    stages.push(
                        json!({"name":"executor","ok":executor["ready"]==true,"executor":executor}),
                    );
                }
                // ok stays transport-level: pairing and the desktop session picker rely on it.
                return Ok(json!({"targetId":target.id,"ok":true,"ready":executor_ready(&result),"stages":stages,"result":result}));
            }
            Attempt::Stop(error) => {
                stages.push(
                    json!({"name":"ssh-runner","ok":false,"route":route_value,"message":error}),
                );
                break;
            }
            Attempt::Retry(error) => stages
                .push(json!({"name":"ssh-runner","ok":false,"route":route_value,"message":error})),
        }
    }
    Ok(json!({"targetId":target.id,"ok":false,"stages":stages}))
}
/// Service-mode Targets report no executor and can always run work.
fn executor_ready(status: &Value) -> bool {
    status
        .get("executor")
        .is_none_or(|executor| executor["ready"] == true)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn doctor_readiness_follows_the_executor() {
        assert!(executor_ready(&json!({"version":"0.4.0"})));
        assert!(executor_ready(
            &json!({"executor":{"mode":"owner-session","ready":true}})
        ));
        assert!(!executor_ready(&json!({"executor":{"mode":"owner-session","ready":false,
            "error":"E_OWNER_NOT_LOGGED_IN: Owner must be logged in"}})));
    }
    #[test]
    fn host_key_failures_never_look_like_transient_disconnects() {
        for stderr in [
            "WARNING: REMOTE HOST IDENTIFICATION HAS CHANGED!",
            "Host key verification failed.",
            "No ED25519 host key is known for example and you have requested strict checking.",
        ] {
            assert!(host_mismatch(stderr));
        }
        assert!(!host_mismatch(
            "connect to host 127.0.0.1 port 22: Connection refused"
        ));
    }
    #[test]
    fn config_path_tokens_are_rejected() {
        assert!(config_quote("C:/state/keys/company").is_ok());
        for path in [
            "C:/%h/key",
            "/tmp/key\nProxyCommand bad",
            "/tmp/\"key",
            "/tmp/$USER/key",
        ] {
            assert!(config_quote(path).is_err());
        }
    }
}

#[test]
fn openssh_accepts_isolated_pins_and_jump_config_without_connecting() {
    use crate::fleet::{ProxyJump, Route, Target};
    let temporary = tempfile::Builder::new()
        .prefix("sshcc config space ")
        .tempdir()
        .unwrap();
    let state = temporary.path();
    fs::create_dir(state.join("keys")).unwrap();
    fs::write(state.join("keys/company.ed25519"), "placeholder").unwrap();
    fs::write(state.join("keys/jump.ed25519"), "placeholder").unwrap();
    let route = Route {
        kind: "lan".into(),
        address: "127.0.0.1".into(),
        port: 2222,
        proxy_jump: Some(ProxyJump {
            address: "127.0.0.2".into(),
            port: 2223,
            user: "DOMAIN\\jump".into(),
            host_keys: vec!["ssh-ed25519 AAAA".into()],
            identity_file: state.join("keys/jump.ed25519").to_string_lossy().into(),
        }),
    };
    let target = Target {
        schema_version: 2,
        id: "config-test".into(),
        node_id: Uuid::new_v4().to_string(),
        label: "Config test".into(),
        tenant: "company".into(),
        os: "linux".into(),
        user: "sshcc-bridge".into(),
        host_keys: vec!["ssh-ed25519 AAAA".into()],
        routes: vec![route.clone()],
        bridge_path: "/usr/bin/sshcc".into(),
        runner_installed: true,
    };
    let session = session_config(state, &target, &route).unwrap();
    for host in ["target", "jump"] {
        let output = quiet_command("ssh")
            .current_dir(&session.directory)
            .args(["-G", "-F", "config", host])
            .output()
            .expect("OpenSSH client prerequisite");
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let config = String::from_utf8(output.stdout).unwrap();
        assert!(
            config.contains("stricthostkeychecking true")
                || config.contains("stricthostkeychecking yes")
        );
        assert!(config.contains("identityagent none"));
        assert!(config.contains("identitiesonly yes"));
        assert!(config.contains("batchmode yes"));
        if host == "target" {
            assert!(config.contains("proxyjump jump"));
        }
        if host == "jump" {
            assert!(config.contains("user DOMAIN\\jump"));
        }
        assert!(!config.contains("proxycommand "));
    }
    let directory = session.directory.clone();
    drop(session);
    assert!(
        !directory.exists(),
        "per-call generated transport config must be removed"
    );
}
