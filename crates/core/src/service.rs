use crate::{
    config::{self, Config},
    executor::Executor,
    files, fleet, ipc,
    jobs::Jobs,
    pairing,
    protocol::{self, Request, Response},
    security,
    terminal::Terminals,
};
use anyhow::{bail, Result};
use serde_json::{json, Value};
use std::{
    io::BufReader,
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc, Mutex,
    },
    time::Instant,
};
pub struct Service {
    pub config: Config,
    state: PathBuf,
    jobs: Arc<Jobs>,
    started: Instant,
    stop: Arc<AtomicBool>,
    installing: Mutex<bool>,
    executor: Arc<Executor>,
    terminals: Arc<Terminals>,
    file_mutation: Mutex<()>,
}
impl Service {
    pub fn new(state: PathBuf, stop: Arc<AtomicBool>) -> Result<Self> {
        let config = config::load(&state)?;
        let executor = Executor::new(config.clone());
        let jobs = Jobs::open_for(&state, Some(executor.clone()))?;
        let terminals = Terminals::new(&state, executor.clone())?;
        Ok(Self {
            config,
            state,
            jobs,
            started: Instant::now(),
            stop,
            installing: Mutex::new(false),
            executor,
            terminals,
            file_mutation: Mutex::new(()),
        })
    }
    fn target(&self) -> Result<()> {
        if !self.config.roles.iter().any(|r| r == "target") {
            bail!("E_ROLE: target role is required");
        }
        Ok(())
    }
    fn serve_stream(
        self: &Arc<Self>,
        req: Request,
        caller: String,
        mut reader: BufReader<ipc::Connection>,
        mut writer: ipc::Connection,
    ) -> Result<()> {
        #[cfg(unix)]
        {
            reader.get_ref().set_read_timeout(None)?;
        }
        if req.method == "terminal.attach" {
            // Authenticate the local Master caller before opening its protected SSH identity.
            self.handle(
                &Request {
                    method: "targets.list".into(),
                    params: json!({}),
                },
                &caller,
            )?;
            let remote = fleet::attach(&self.state, &self.config, &req.params)?;
            return pipe_remote(remote, reader, writer);
        }
        if req.params["request"]["method"] == "relay.attach" {
            let master = self.bridge_master(&req, &caller)?;
            let remote = fleet::relay_attach(
                &self.state,
                &self.config,
                master,
                &req.params["request"]["params"],
            )?;
            return pipe_remote(remote, reader, writer);
        }
        anyhow::ensure!(
            req.params["request"]["method"] == "pty.attach",
            "E_PERMISSION: stream attachment required"
        );
        let mut envelope = req.clone();
        envelope.method = "bridge.request".into();
        let initial = self.handle(&envelope, &caller)?;
        let id = protocol::string(&initial, "terminalId")?.to_owned();
        let base = envelope.params["request"]["params"].clone();
        let mut offset = base["offset"]
            .as_u64()
            .unwrap_or(initial["offset"].as_u64().unwrap_or(0));
        let (tx, rx) = std::sync::mpsc::sync_channel(64);
        std::thread::spawn(move || {
            while let Ok(frame) = protocol::read_frame::<Request>(&mut reader) {
                if tx.send(frame).is_err() {
                    break;
                }
            }
        });
        let result = (|| -> Result<()> {
            ipc::reply(&mut writer, &Response::from_result(Ok(initial.clone())))?;
            loop {
                if self.stop.load(Ordering::SeqCst) {
                    break;
                }
                match rx.recv_timeout(std::time::Duration::from_millis(20)) {
                    Ok(input) => {
                        let method = match input.method.as_str() {
                            "input" => "pty.write",
                            "resize" => "pty.resize",
                            "close" => "pty.close",
                            "detach" => break,
                            _ => bail!("E_PROTOCOL: unsupported stream frame"),
                        };
                        let mut params = input.params;
                        params["terminalId"] = json!(id);
                        envelope.params["request"] = json!({"method":method,"params":params});
                        let result = self.handle(&envelope, &caller).map(|mut result| {
                            result["event"] = json!("ack");
                            result
                        });
                        ipc::reply(&mut writer, &Response::from_result(result))?;
                    }
                    Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
                    Err(_) => {}
                }
                envelope.params["request"] = json!({"method":"pty.read","params":{"terminalId":id,"offset":offset,"limit":65536}});
                let mut chunk = self.handle(&envelope, &caller)?;
                offset = chunk["nextOffset"].as_u64().unwrap_or(offset);
                let ended = chunk["status"] != "running";
                if chunk["contentBase64"]
                    .as_str()
                    .is_some_and(|s| !s.is_empty())
                    || ended
                {
                    chunk["event"] = json!(if ended { "end" } else { "output" });
                    ipc::reply(&mut writer, &Response::from_result(Ok(chunk)))?;
                }
                if ended {
                    break;
                }
            }
            Ok(())
        })();
        let _ = self.terminals.handle(
            "pty.detach",
            &json!({"terminalId":id,"masterId":req.params["masterId"]}),
        );
        if result.is_ok() {
            let _ = ipc::reply(
                &mut writer,
                &Response::from_result(Ok(
                    json!({"event":"end","status":"detached","terminalId":id,"nextOffset":offset}),
                )),
            );
        }
        result
    }
    pub fn handle(&self, req: &Request, caller: &str) -> Result<Value> {
        let owner = caller == self.config.owner || security::is_system(caller);
        if !owner
            && (!self.config.allowed_principals.iter().any(|p| p == caller)
                || req.method != "bridge.request")
        {
            bail!("E_PERMISSION: this local principal is not authorized");
        }
        if req.method == "bridge.request" {
            let master = self.bridge_master(req, caller)?;
            let mut inner: Request = serde_json::from_value(req.params["request"].clone())?;
            match inner.method.as_str() {
                "relay.join" => return fleet::relay_join(&self.state, &self.config, master),
                "relay.health" => return fleet::relay_health(&self.state, &self.config, master),
                "relay.pair" => {
                    return fleet::relay_pair(&self.state, &self.config, master, &inner.params)
                }
                "relay.pair_complete" => {
                    return fleet::relay_pair_complete(
                        &self.state,
                        &self.config,
                        master,
                        &inner.params,
                    )
                }
                "relay.request" => {
                    return fleet::relay_request(&self.state, &self.config, master, &inner.params)
                }
                _ => {}
            }
            let allowed = [
                "system.status",
                "runner.start",
                "runner.list",
                "runner.status",
                "runner.logs",
                "runner.cancel",
                "fs.list",
                "fs.read",
                "fs.search",
                "fs.grep",
                "fs.write",
                "fs.mkdir",
                "fs.remove",
                "pty.open",
                "pty.read",
                "pty.write",
                "pty.status",
                "pty.resize",
                "pty.close",
                "pty.attach",
                "pty.detach",
            ];
            if !allowed.contains(&inner.method.as_str()) {
                bail!("E_PERMISSION: method is not available over bridge");
            }
            if inner.method == "runner.start" || inner.method.starts_with("pty.") {
                let object = inner
                    .params
                    .as_object_mut()
                    .ok_or_else(|| anyhow::anyhow!("E_ARGUMENT: object params required"))?;
                object.insert("masterId".into(), json!(master));
                object.insert("masterKey".into(), req.params["masterKey"].clone());
            }
            return self.local(&inner);
        }
        if req.method.starts_with("targets.")
            || req.method.starts_with("jobs.")
            || req.method.starts_with("files.")
            || req.method.starts_with("terminal.")
            || req.method == "enrollment.export"
            || req.method.starts_with("pairing.")
        {
            if !self.config.roles.iter().any(|r| r == "master") {
                bail!("E_ROLE: master role is required");
            }
        }
        if let Some(value) = pairing::handle(&self.state, &self.config, req)? {
            return Ok(value);
        }
        if let Some(value) = fleet::handle(&self.state, &self.config, req)? {
            return Ok(value);
        }
        self.local(req)
    }
    /// Authenticated Master ID of a bridge envelope addressed to this Target.
    fn bridge_master<'a>(&self, req: &'a Request, caller: &str) -> Result<&'a str> {
        let owner = caller == self.config.owner || security::is_system(caller);
        if !owner && !self.config.allowed_principals.iter().any(|p| p == caller) {
            bail!("E_PERMISSION: this local principal is not authorized");
        }
        self.target()?;
        let master = protocol::string(&req.params, "masterId")?;
        if !fleet::authorized_master_key(
            &self.state,
            master,
            protocol::string(&req.params, "masterKey")?,
        )? {
            bail!("E_PERMISSION: master is not enrolled or has been revoked");
        }
        if protocol::string(&req.params, "expectedTargetId")? != self.config.node_id {
            bail!("E_IDENTITY: target identity does not match registration");
        }
        Ok(master)
    }
    fn installer_context(&self, req: &Request) -> Result<()> {
        if protocol::string(&req.params, "expectedNodeId")? != self.config.node_id {
            bail!("E_IDENTITY: installer is connected to a different node");
        }
        let supplied = std::fs::canonicalize(protocol::string(&req.params, "dataDir")?)?;
        let expected = std::fs::canonicalize(&self.state)?;
        #[cfg(windows)]
        let matches = supplied
            .to_string_lossy()
            .eq_ignore_ascii_case(&expected.to_string_lossy());
        #[cfg(not(windows))]
        let matches = supplied == expected;
        if !matches {
            bail!("E_IDENTITY: installer data directory differs from this service");
        }
        Ok(())
    }
    fn local(&self, req: &Request) -> Result<Value> {
        match req.method.as_str() {
            "system.status" => Ok(
                json!({"nodeId":self.config.node_id,"label":self.config.label,"roles":self.config.roles,"owner":self.config.owner,"version":env!("CARGO_PKG_VERSION"),"uptimeSeconds":self.started.elapsed().as_secs(), "executionPrincipal":security::current_principal()?, "privileged":security::is_system(&security::current_principal()?),"executor":self.executor.readiness(&req.params),"activeTerminals":self.terminals.active()}),
            ),
            "system.install-begin" => {
                self.installer_context(req)?;
                let mut installing = self
                    .installing
                    .lock()
                    .map_err(|_| anyhow::anyhow!("E_OPERATION: installer guard unavailable"))?;
                if *installing {
                    bail!("E_INSTALLING: another installer already paused submissions");
                }
                let preflight = pairing::installer_preflight(&self.state)?;
                if preflight["runningJobs"].as_u64() != Some(0) || self.terminals.active() != 0 {
                    bail!("E_BUSY: queued or running jobs must finish before installing");
                }
                *installing = true;
                Ok(json!({"guarded":true,"runningJobs":0,"nodeId":self.config.node_id}))
            }
            "system.install-end" => {
                self.installer_context(req)?;
                *self
                    .installing
                    .lock()
                    .map_err(|_| anyhow::anyhow!("E_OPERATION: installer guard unavailable"))? =
                    false;
                Ok(json!({"guarded":false,"nodeId":self.config.node_id}))
            }
            "system.stop" => {
                self.stop.store(true, Ordering::SeqCst);
                wake();
                Ok(json!({"stopping":true}))
            }
            "runner.start" => {
                self.target()?;
                let installing = self
                    .installing
                    .lock()
                    .map_err(|_| anyhow::anyhow!("E_OPERATION: installer guard unavailable"))?;
                if *installing {
                    bail!("E_INSTALLING: retry after the service installer finishes");
                }
                self.jobs.start(&req.params)
            }
            "runner.list" => {
                self.target()?;
                Ok(json!(self.jobs.list()?))
            }
            "runner.status" => {
                self.target()?;
                self.jobs.get(protocol::string(&req.params, "jobId")?)
            }
            "runner.cancel" => {
                self.target()?;
                self.jobs.cancel(protocol::string(&req.params, "jobId")?)
            }
            "runner.logs" => {
                self.target()?;
                self.jobs.logs(&req.params)
            }
            name if name.starts_with("fs.") => {
                self.target()?;
                let _mutation = if matches!(name, "fs.write" | "fs.mkdir" | "fs.remove") {
                    Some(self.file_mutation.lock().unwrap())
                } else {
                    None
                };
                if self.executor.owner_mode() {
                    self.executor.call(name, &req.params)
                } else {
                    files::handle(name, &req.params)
                }
            }
            name if name.starts_with("pty.") => {
                self.target()?;
                let guard = self.installing.lock().unwrap();
                if *guard {
                    bail!("E_INSTALLING: retry after the service installer finishes");
                }
                let mut params = req.params.clone();
                if params["masterId"].is_null() {
                    params["masterId"] = json!(self.config.node_id);
                }
                if name == "pty.open" {
                    self.terminals.open(&params)
                } else {
                    self.terminals.handle(name, &params)
                }
            }
            _ => bail!("E_METHOD: unsupported method {}", req.method),
        }
    }
}
/// Forward client frames to a remote terminal stream and its replies back until it ends.
fn pipe_remote(
    mut remote: fleet::RemoteStream,
    mut reader: BufReader<ipc::Connection>,
    mut writer: ipc::Connection,
) -> Result<()> {
    ipc::reply(&mut writer, &remote.initial)?;
    let remote_input = remote.input.take().unwrap();
    std::thread::spawn(move || {
        let mut input = remote_input;
        while let Ok(frame) = protocol::read_frame::<Request>(&mut reader) {
            if protocol::write_frame(&frame, &mut input).is_err() {
                break;
            }
        }
        let _ = protocol::write_frame(
            &Request {
                method: "detach".into(),
                params: json!({}),
            },
            &mut input,
        );
    });
    while let Ok(frame) = protocol::read_frame::<Response>(&mut remote.output) {
        ipc::reply(&mut writer, &frame)?;
        if !frame.ok || frame.result.as_ref().is_some_and(|v| v["event"] == "end") {
            break;
        }
    }
    Ok(())
}
pub fn wake() {
    std::thread::spawn(|| {
        let _ = crate::client::call(&Request {
            method: "system.status".into(),
            params: json!({}),
        });
    });
}
pub fn run(stop: Arc<AtomicBool>) -> Result<()> {
    let state = config::data_dir();
    let config = config::load(&state)?;
    let mut listener = ipc::Listener::bind(&config)?;
    let env_path = state.join("service-env.json");
    if env_path.exists() {
        let vars: std::collections::BTreeMap<String, String> =
            serde_json::from_slice(&std::fs::read(env_path)?)?;
        for (key, value) in vars {
            if key.is_empty()
                || key.starts_with("SSHCC_")
                || key.contains(['=', '\0'])
                || value.contains('\0')
            {
                bail!("E_CONFIG: invalid service environment");
            }
            std::env::set_var(key, value);
        }
    }
    let service = Arc::new(Service::new(state, stop.clone())?);
    let connections = Arc::new(AtomicUsize::new(0));
    while !stop.load(Ordering::SeqCst) {
        let mut connection = listener.accept()?;
        if stop.load(Ordering::SeqCst) {
            break;
        }
        #[cfg(unix)]
        {
            // Reject untrusted UIDs before they can retain a parser or worker thread.
            let caller = ipc::peer(&connection)?;
            if caller != service.config.owner
                && !security::is_system(&caller)
                && !service.config.allowed_principals.contains(&caller)
            {
                let _ = ipc::reply(
                    &mut connection,
                    &Response::from_result(Err(anyhow::anyhow!(
                        "E_PERMISSION: unauthorized local principal"
                    ))),
                );
                continue;
            }
            connection.set_read_timeout(Some(std::time::Duration::from_secs(15)))?;
            connection.set_write_timeout(Some(std::time::Duration::from_secs(15)))?;
        }
        // Bound local transports, without serializing jobs or locking Targets/projects.
        if connections.load(Ordering::SeqCst) >= 64 {
            let _ = ipc::reply(
                &mut connection,
                &Response::from_result(Err(anyhow::anyhow!(
                    "E_BUSY: local request limit reached; retry later"
                ))),
            );
            continue;
        }
        connections.fetch_add(1, Ordering::SeqCst);
        let connections = connections.clone();
        let service = service.clone();
        std::thread::spawn(move || {
            struct ConnectionGuard(Arc<AtomicUsize>);
            impl Drop for ConnectionGuard {
                fn drop(&mut self) {
                    self.0.fetch_sub(1, Ordering::SeqCst);
                }
            }
            let _guard = ConnectionGuard(connections);
            let result = (|| -> Result<Option<Value>> {
                let mut reader = BufReader::new(connection.try_clone()?);
                let req: Request = protocol::read_frame(&mut reader)?;
                let caller = ipc::peer(&connection)?;
                if matches!(req.method.as_str(), "terminal.attach" | "bridge.stream") {
                    service.serve_stream(req, caller, reader, connection.try_clone()?)?;
                    return Ok(None);
                }
                Ok(Some(service.handle(&req, &caller)?))
            })();
            if let Ok(None) = result {
            } else {
                let _ = ipc::reply(
                    &mut connection,
                    &Response::from_result(result.map(|v| v.unwrap())),
                );
            }
        });
    }
    service.jobs.shutdown();
    service.terminals.shutdown();
    Ok(())
}
