use crate::{
    executor::{Executor, Session},
    protocol, security,
};
use anyhow::{bail, Context, Result};
use chrono::Utc;
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    fs,
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    thread,
    time::Duration,
};
const LOG_CAP: u64 = 64 * 1024 * 1024;
pub struct Jobs {
    db: Mutex<Connection>,
    state: PathBuf,
    active: Mutex<HashMap<String, Arc<AtomicBool>>>,
    stopping: AtomicBool,
    executor: Option<Arc<Executor>>,
}
impl Jobs {
    pub fn open(state: &Path) -> Result<Arc<Self>> {
        Self::open_for(state, None)
    }
    pub fn open_for(state: &Path, executor: Option<Arc<Executor>>) -> Result<Arc<Self>> {
        fs::create_dir_all(state.join("jobs"))?;
        security::protect(&state.join("jobs"), true)?;
        let db = Connection::open(state.join("jobs.sqlite"))?;
        db.busy_timeout(Duration::from_secs(10))?;
        db.execute_batch(
            "PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL;
CREATE TABLE IF NOT EXISTS jobs(id TEXT PRIMARY KEY,payload TEXT NOT NULL,record TEXT NOT NULL);",
        )?;
        security::protect(&state.join("jobs.sqlite"), false)?;
        let jobs = Arc::new(Self {
            db: Mutex::new(db),
            state: state.into(),
            active: Mutex::new(HashMap::new()),
            stopping: AtomicBool::new(false),
            executor,
        });
        // A service start never silently resumes arbitrary administrative scripts.
        let unfinished = {
            let db = jobs.db.lock().unwrap();
            let mut statement = db.prepare("SELECT record FROM jobs WHERE json_extract(record, '$.status') IN ('queued','running')")?;
            let rows = statement.query_map([], |row| row.get::<_, String>(0))?;
            rows.map(|row| Ok(serde_json::from_str::<Value>(&row?)?))
                .collect::<Result<Vec<_>>>()?
        };
        for mut record in unfinished {
            if matches!(record["status"].as_str(), Some("queued" | "running")) {
                record["status"] = json!("interrupted");
                record["finishedAt"] = json!(Utc::now().to_rfc3339());
                record["message"] =
                    json!("Target service restarted; inspect state before starting another job");
                jobs.update(&record)?;
            }
        }
        Ok(jobs)
    }
    fn update(&self, record: &Value) -> Result<()> {
        self.db.lock().unwrap().execute(
            "UPDATE jobs SET record=?1 WHERE id=?2",
            params![
                serde_json::to_string(record)?,
                protocol::string(record, "id")?
            ],
        )?;
        Ok(())
    }
    pub fn get(&self, id: &str) -> Result<Value> {
        let result: Option<String> = self
            .db
            .lock()
            .unwrap()
            .query_row("SELECT record FROM jobs WHERE id=?1", [id], |r| r.get(0))
            .optional()?;
        Ok(serde_json::from_str(
            &result.context("E_NOT_FOUND: job not found")?,
        )?)
    }
    pub fn list(&self) -> Result<Vec<Value>> {
        let db = self.db.lock().unwrap();
        let mut statement = db.prepare("SELECT record FROM jobs ORDER BY rowid DESC LIMIT 1000")?;
        let rows = statement.query_map([], |r| r.get::<_, String>(0))?;
        rows.map(|row| Ok(serde_json::from_str(&row?)?)).collect()
    }
    pub fn start(self: &Arc<Self>, p: &Value) -> Result<Value> {
        if self.stopping.load(Ordering::SeqCst) {
            bail!("E_STOPPING: service is stopping");
        }
        let id = protocol::string(p, "jobId")?;
        uuid::Uuid::parse_str(id).context("E_ARGUMENT: jobId must be UUID")?;
        let script = protocol::string(p, "script")?;
        if script.len() > 1024 * 1024 {
            bail!("E_LIMIT: script exceeds 1 MiB");
        }
        let cwd = protocol::string(p, "cwd")?;
        security::ensure_absolute(Path::new(cwd))?;
        let master = protocol::string(p, "masterId")?;
        let shell =
            p["shell"]
                .as_str()
                .unwrap_or(if cfg!(windows) { "powershell" } else { "bash" });
        if shell != if cfg!(windows) { "powershell" } else { "bash" } {
            bail!("E_ARGUMENT: shell is not supported on this target");
        }
        let env = p.get("env").cloned().unwrap_or(json!({}));
        let vars = env
            .as_object()
            .context("E_ARGUMENT: env must be an object")?;
        for (key, val) in vars {
            if key.is_empty()
                || key.contains(['=', '\0'])
                || val.as_str().is_none_or(|v| v.contains('\0'))
            {
                bail!("E_ARGUMENT: invalid environment entry");
            }
        }
        let mut payload =
            json!({"jobId":id,"script":script,"cwd":cwd,"masterId":master,"shell":shell,"env":env});
        for field in ["ownerSessionId", "asAdmin"] {
            if let Some(value) = p.get(field) {
                payload[field] = value.clone();
            }
        }
        let serialized = serde_json::to_string(&payload)?;
        let mut record = json!({"id":id,"masterId":master,"script":script.chars().take(4096).collect::<String>(),"scriptTruncated":script.chars().count()>4096,"cwd":cwd,"shell":shell,"status":"queued","createdAt":Utc::now().to_rfc3339(),"startedAt":null,"finishedAt":null,"exitCode":null});
        if self.executor.as_ref().is_none_or(|e| !e.owner_mode()) {
            let principal = security::current_principal()?;
            record["executor"] = json!({"mode":"service","principal":principal,"elevated":security::is_system(&principal)});
        }
        // Hold active registration across the DB insert so cancel cannot miss a visible queued job.
        let mut active = self.active.lock().unwrap();
        {
            let db = self.db.lock().unwrap();
            let prior: Option<(String, String)> = db
                .query_row("SELECT payload,record FROM jobs WHERE id=?1", [id], |r| {
                    Ok((r.get(0)?, r.get(1)?))
                })
                .optional()?;
            if let Some((old, record)) = prior {
                if old != serialized {
                    bail!("E_CONFLICT: jobId already used with different command");
                }
                return Ok(serde_json::from_str(&record)?);
            }
            if let Some(executor) = self.executor.as_ref().filter(|e| e.owner_mode()) {
                let session = executor.select(p)?;
                record["executor"] = executor.metadata(&session, p);
                record["ownerSessionId"] = json!(session.id);
                payload["_executorSession"] = serde_json::to_value(session)?;
            }
            db.execute(
                "INSERT INTO jobs(id,payload,record) VALUES (?1,?2,?3)",
                params![id, serialized, serde_json::to_string(&record)?],
            )?;
        }
        let cancelled = Arc::new(AtomicBool::new(false));
        active.insert(id.into(), cancelled.clone());
        drop(active);
        let this = self.clone();
        let id = id.to_string();
        thread::spawn(move || {
            if let Err(err) = this.execute(&id, &payload, &cancelled) {
                if let Ok(mut record) = this.get(&id) {
                    record["status"] = json!(if this.stopping.load(Ordering::SeqCst) {
                        "interrupted"
                    } else if cancelled.load(Ordering::SeqCst) {
                        "cancelled"
                    } else {
                        "failed"
                    });
                    record["finishedAt"] = json!(Utc::now().to_rfc3339());
                    record["message"] = json!(format!("{err:#}"));
                    let _ = this.update(&record);
                }
            }
            this.active.lock().unwrap().remove(&id);
        });
        Ok(record)
    }
    fn execute(&self, id: &str, p: &Value, cancelled: &AtomicBool) -> Result<()> {
        if let Some(executor) = self.executor.as_ref().filter(|e| e.owner_mode()) {
            return self.execute_owner(executor, id, p, cancelled);
        }
        if cancelled.load(Ordering::SeqCst) {
            bail!("E_CANCELLED: cancelled before start");
        }
        let dir = self.state.join("jobs").join(id);
        fs::create_dir_all(&dir)?;
        security::protect(&dir, true)?;
        let script_path = dir.join(if cfg!(windows) {
            "command.ps1"
        } else {
            "command.sh"
        });
        let script = protocol::string(p, "script")?;
        fs::write(&script_path, script.as_bytes())?;
        security::protect(&script_path, false)?;
        let mut cmd = command(&script_path)?;
        cmd.current_dir(protocol::string(p, "cwd")?)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        for (key, value) in p["env"].as_object().unwrap() {
            cmd.env(key, value.as_str().unwrap());
        }
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            unsafe {
                cmd.pre_exec(|| {
                    if libc::setsid() < 0 {
                        return Err(std::io::Error::last_os_error());
                    }
                    if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL) < 0 {
                        return Err(std::io::Error::last_os_error());
                    }
                    if libc::getppid() == 1 {
                        libc::_exit(125);
                    }
                    Ok(())
                });
            }
        }
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(windows_sys::Win32::System::Threading::CREATE_NO_WINDOW);
        }
        let mut child = cmd.spawn().context("E_EXEC: start job process")?;
        let guard = match ProcessGuard::new(child.id()) {
            Ok(guard) => guard,
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(error);
            }
        };
        if cancelled.load(Ordering::SeqCst) || self.stopping.load(Ordering::SeqCst) {
            guard.terminate();
            let _ = child.kill();
            let _ = child.wait();
            bail!("E_CANCELLED: stopped before execution");
        }
        if let Some(mut gate) = child.stdin.take() {
            gate.write_all(b"\n")?;
        }
        let out = child.stdout.take().context("E_EXEC: missing stdout")?;
        let err = child.stderr.take().context("E_EXEC: missing stderr")?;
        let out_path = dir.join("stdout.log");
        let err_path = dir.join("stderr.log");
        let out_thread = thread::spawn(move || capture(out, &out_path));
        let err_thread = thread::spawn(move || capture(err, &err_path));
        let mut record = self.get(id)?;
        record["status"] = json!("running");
        record["startedAt"] = json!(Utc::now().to_rfc3339());
        record["pid"] = json!(child.id());
        self.update(&record)?;
        let (exit, was_cancelled) = loop {
            if cancelled.load(Ordering::SeqCst) || self.stopping.load(Ordering::SeqCst) {
                guard.terminate();
                let _ = child.kill();
                break (child.wait()?, true);
            }
            if let Some(status) = child.try_wait()? {
                break (status, false);
            }
            thread::sleep(Duration::from_millis(100));
        };
        // Descendants belong to this job; closing a completed job also closes orphan descendants.
        guard.terminate();
        let stdout = out_thread
            .join()
            .map_err(|_| anyhow::anyhow!("E_LOG: stdout reader failed"))??;
        let stderr = err_thread
            .join()
            .map_err(|_| anyhow::anyhow!("E_LOG: stderr reader failed"))??;
        record["status"] = json!(if self.stopping.load(Ordering::SeqCst) {
            "interrupted"
        } else if was_cancelled {
            "cancelled"
        } else if exit.success() {
            "succeeded"
        } else {
            "failed"
        });
        record["exitCode"] = json!(exit.code());
        record["finishedAt"] = json!(Utc::now().to_rfc3339());
        record["stdoutTruncated"] = json!(stdout);
        record["stderrTruncated"] = json!(stderr);
        self.update(&record)?;
        Ok(())
    }
    fn execute_owner(
        &self,
        executor: &Executor,
        id: &str,
        p: &Value,
        cancelled: &AtomicBool,
    ) -> Result<()> {
        use base64::{engine::general_purpose::STANDARD, Engine};
        let session: Session = serde_json::from_value(p["_executorSession"].clone())?;
        let dir = self.state.join("jobs").join(id);
        fs::create_dir_all(&dir)?;
        security::protect(&dir, true)?;
        let mut out = fs::File::create(dir.join("stdout.log"))?;
        security::protect(&dir.join("stdout.log"), false)?;
        let mut err = fs::File::create(dir.join("stderr.log"))?;
        security::protect(&dir.join("stderr.log"), false)?;
        let mut worker = executor.spawn(&session, "worker.job", p)?;
        let mut reader = std::mem::replace(
            &mut worker.output,
            std::io::BufReader::new(Box::new(std::io::empty())),
        );
        let (tx, rx) = std::sync::mpsc::sync_channel(32);
        let reader = thread::spawn(move || {
            while let Ok(frame) = protocol::read_frame::<Value>(&mut reader) {
                if tx.send(frame).is_err() {
                    break;
                }
            }
        });
        let mut record = self.get(id)?;
        record["status"] = json!("running");
        record["startedAt"] = json!(Utc::now().to_rfc3339());
        self.update(&record)?;
        let mut written = [0u64; 2];
        let mut exit = None;
        let mut interrupted = false;
        loop {
            if cancelled.load(Ordering::SeqCst)
                || self.stopping.load(Ordering::SeqCst)
                || !executor.alive(&session)
            {
                interrupted = self.stopping.load(Ordering::SeqCst) || !executor.alive(&session);
                worker.terminate();
                break;
            }
            match rx.recv_timeout(Duration::from_millis(100)) {
                Ok(frame) => match frame["event"].as_str() {
                    Some("output") => {
                        let bytes = STANDARD.decode(
                            frame["data"]
                                .as_str()
                                .context("E_PROTOCOL: worker output missing")?,
                        )?;
                        let index = usize::from(frame["stream"] == "stderr");
                        let keep = (LOG_CAP - written[index]).min(bytes.len() as u64) as usize;
                        if index == 0 {
                            out.write_all(&bytes[..keep])?;
                        } else {
                            err.write_all(&bytes[..keep])?;
                        }
                        written[index] += keep as u64;
                        if keep < bytes.len() {
                            record[if index == 0 {
                                "stdoutTruncated"
                            } else {
                                "stderrTruncated"
                            }] = json!(true);
                        }
                    }
                    Some("exit") => {
                        exit = Some(frame);
                        break;
                    }
                    Some("error") => {
                        record["message"] = frame["message"].clone();
                        break;
                    }
                    _ => {}
                },
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                    record["message"] = json!("Owner worker disconnected");
                    interrupted = true;
                    break;
                }
                Err(_) => {}
            }
        }
        worker.terminate();
        drop(rx);
        let _ = reader.join();
        out.sync_all()?;
        err.sync_all()?;
        record["status"] = json!(if interrupted {
            "interrupted"
        } else if cancelled.load(Ordering::SeqCst) {
            "cancelled"
        } else if exit.as_ref().is_some_and(|e| e["success"] == true) {
            "succeeded"
        } else {
            "failed"
        });
        if interrupted {
            record["message"] = json!("Bound Owner session ended or service stopped");
        }
        record["exitCode"] = exit.map(|e| e["exitCode"].clone()).unwrap_or(Value::Null);
        record["finishedAt"] = json!(Utc::now().to_rfc3339());
        self.update(&record)
    }
    pub fn cancel(&self, id: &str) -> Result<Value> {
        if let Some(flag) = self.active.lock().unwrap().get(id) {
            flag.store(true, Ordering::SeqCst);
        }
        self.get(id)
    }
    pub fn logs(&self, p: &Value) -> Result<Value> {
        let id = protocol::string(p, "jobId")?;
        let _record = self.get(id)?;
        uuid::Uuid::parse_str(id)?;
        let stream = p["stream"].as_str().unwrap_or("stdout");
        if stream != "stdout" && stream != "stderr" {
            bail!("E_ARGUMENT: invalid log stream");
        }
        let offset = p["offset"].as_u64().unwrap_or(0);
        let limit = p["limit"].as_u64().unwrap_or(65536).min(1024 * 1024);
        let path = self
            .state
            .join("jobs")
            .join(id)
            .join(format!("{stream}.log"));
        if !path.exists() {
            return Ok(json!({"content":"","nextOffset":offset,"eof":true}));
        }
        let mut file = fs::File::open(path)?;
        let size = file.metadata()?.len();
        file.seek(SeekFrom::Start(offset))?;
        let mut bytes = Vec::new();
        file.take(limit).read_to_end(&mut bytes)?;
        Ok(
            json!({"content":String::from_utf8_lossy(&bytes),"nextOffset":offset+bytes.len() as u64,"eof":offset+bytes.len() as u64>=size}),
        )
    }
    pub fn shutdown(&self) {
        self.stopping.store(true, Ordering::SeqCst);
        for flag in self.active.lock().unwrap().values() {
            flag.store(true, Ordering::SeqCst);
        }
        for _ in 0..100 {
            if self.active.lock().unwrap().is_empty() {
                break;
            }
            thread::sleep(Duration::from_millis(100));
        }
    }
}
fn command(script: &Path) -> Result<Command> {
    #[cfg(unix)]
    {
        let mut cmd = Command::new("/bin/bash");
        cmd.args([
            "-c",
            "IFS= read -r _sshcc_start; exec /bin/bash \"$1\"",
            "sshcc",
        ])
        .arg(script);
        Ok(cmd)
    }
    #[cfg(windows)]
    {
        use base64::{engine::general_purpose::STANDARD, Engine};
        let quoted = script.to_string_lossy().replace("'", "''");
        let source=format!("$null=[Console]::ReadLine(); $ErrorActionPreference='Stop'; [Console]::OutputEncoding=[Text.UTF8Encoding]::new($false); $global:LASTEXITCODE=0; & ([scriptblock]::Create([IO.File]::ReadAllText('{quoted}'))); if ($LASTEXITCODE -ne 0) {{ exit $LASTEXITCODE }}");
        let encoded: Vec<u8> = source
            .encode_utf16()
            .flat_map(|unit| unit.to_le_bytes())
            .collect();
        let mut cmd = Command::new("powershell.exe");
        cmd.args([
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-EncodedCommand",
            &STANDARD.encode(encoded),
        ]);
        Ok(cmd)
    }
}
fn capture(mut input: impl Read, path: &Path) -> Result<bool> {
    let mut output = fs::File::create(path)?;
    security::protect(path, false)?;
    let mut buffer = [0u8; 16384];
    let mut written = 0;
    let mut truncated = false;
    loop {
        let n = input.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        let keep = (LOG_CAP - written).min(n as u64) as usize;
        if keep > 0 {
            output.write_all(&buffer[..keep])?;
            written += keep as u64;
        }
        if keep < n {
            truncated = true;
        }
    }
    output.sync_all()?;
    Ok(truncated)
}
#[cfg(unix)]
pub(crate) struct ProcessGuard {
    pid: u32,
}
#[cfg(unix)]
impl ProcessGuard {
    pub(crate) fn new(pid: u32) -> Result<Self> {
        Ok(Self { pid })
    }
    pub(crate) fn terminate(&self) {
        unsafe {
            libc::kill(-(self.pid as i32), libc::SIGKILL);
        }
    }
}
#[cfg(unix)]
impl Drop for ProcessGuard {
    fn drop(&mut self) {
        self.terminate();
    }
}
#[cfg(windows)]
pub(crate) struct ProcessGuard {
    handle: windows_sys::Win32::Foundation::HANDLE,
}
#[cfg(windows)]
impl ProcessGuard {
    pub(crate) fn new(pid: u32) -> Result<Self> {
        use windows_sys::Win32::{
            Foundation::CloseHandle,
            System::{
                JobObjects::{
                    AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
                    SetInformationJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
                    JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
                },
                Threading::{OpenProcess, PROCESS_SET_QUOTA, PROCESS_TERMINATE},
            },
        };
        let handle = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
        if handle.is_null() {
            return Err(std::io::Error::last_os_error().into());
        }
        let guard = Self { handle };
        let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        if unsafe {
            SetInformationJobObject(
                handle,
                JobObjectExtendedLimitInformation,
                (&limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
                std::mem::size_of_val(&limits) as u32,
            )
        } == 0
        {
            return Err(std::io::Error::last_os_error().into());
        }
        let process = unsafe { OpenProcess(PROCESS_SET_QUOTA | PROCESS_TERMINATE, 0, pid) };
        if process.is_null() {
            return Err(std::io::Error::last_os_error().into());
        }
        let ok = unsafe { AssignProcessToJobObject(handle, process) };
        let err = std::io::Error::last_os_error();
        unsafe { CloseHandle(process) };
        if ok == 0 {
            return Err(err).context("E_EXEC: cannot contain job process tree");
        }
        Ok(guard)
    }
    pub(crate) fn terminate(&self) {
        unsafe {
            windows_sys::Win32::System::JobObjects::TerminateJobObject(self.handle, 1);
        }
    }
}
#[cfg(windows)]
impl Drop for ProcessGuard {
    fn drop(&mut self) {
        unsafe { windows_sys::Win32::Foundation::CloseHandle(self.handle) };
    }
}
