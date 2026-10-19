//! Target-owned PTYs. Only metadata is durable; input and output stay in memory.
use crate::{
    executor::{Executor, Worker},
    fleet,
    protocol::{self, Request},
    security,
};
use anyhow::{bail, ensure, Context, Result};
use base64::{engine::general_purpose::STANDARD, Engine};
use chrono::Utc;
use rusqlite::{params, Connection};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::{HashMap, VecDeque},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
const BUFFER_CAP: usize = 4 * 1024 * 1024;
const DETACH_TTL: Duration = Duration::from_secs(30 * 60);
struct State {
    record: Value,
    bytes: VecDeque<u8>,
    offset: u64,
    ack: u64,
    last_hash: String,
    detached: Option<Instant>,
    attached: bool,
}
struct Terminal {
    state: Mutex<State>,
    worker: Mutex<Worker>,
    creator: String,
    key: Option<String>,
}
pub struct Terminals {
    entries: Mutex<HashMap<String, Arc<Terminal>>>,
    db: Mutex<Connection>,
    executor: Arc<Executor>,
    state: PathBuf,
}
impl Terminals {
    pub fn new(path: &Path, executor: Arc<Executor>) -> Result<Arc<Self>> {
        let db = Connection::open(path.join("terminals.sqlite"))?;
        db.execute_batch("PRAGMA journal_mode=WAL; CREATE TABLE IF NOT EXISTS terminals(id TEXT PRIMARY KEY, record TEXT NOT NULL); UPDATE terminals SET record=json_set(record,'$.status','interrupted','$.message','Target service restarted') WHERE json_extract(record,'$.status')='running';")?;
        security::protect(&path.join("terminals.sqlite"), false)?;
        Ok(Arc::new(Self {
            entries: Mutex::new(HashMap::new()),
            db: Mutex::new(db),
            executor,
            state: path.into(),
        }))
    }
    fn save(&self, record: &Value) -> Result<()> {
        self.db.lock().unwrap().execute("INSERT INTO terminals(id,record) VALUES(?1,?2) ON CONFLICT(id) DO UPDATE SET record=excluded.record",params![record["terminalId"].as_str(),serde_json::to_string(record)?])?;
        Ok(())
    }
    pub fn active(&self) -> usize {
        self.entries
            .lock()
            .unwrap()
            .values()
            .filter(|e| e.state.lock().unwrap().record["status"] == "running")
            .count()
    }
    fn authorize(entry: &Terminal, params: &Value) -> Result<()> {
        ensure!(
            params["masterId"].as_str() == Some(&entry.creator),
            "E_PERMISSION: terminal belongs to another Master"
        );
        Ok(())
    }
    pub fn open(self: &Arc<Self>, params: &Value) -> Result<Value> {
        let creator = protocol::string(params, "masterId")?.to_owned();
        let session = self.executor.select(params)?;
        let mut entries = self.entries.lock().unwrap();
        entries.retain(|_, e| {
            let s = e.state.lock().unwrap();
            s.record["status"] == "running" || s.detached.is_some_and(|t| t.elapsed() < DETACH_TTL)
        });
        ensure!(
            entries
                .values()
                .filter(|e| e.state.lock().unwrap().record["status"] == "running")
                .count()
                < 8,
            "E_LIMIT: maximum 8 active terminals per Target"
        );
        let mut worker = self.executor.spawn(&session, "worker.terminal", params)?;
        let ready = worker.receive()?;
        ensure!(
            ready["event"] == "ready",
            "E_TERMINAL: PTY did not start: {ready}"
        );
        let id = uuid::Uuid::new_v4().to_string();
        let record = json!({"terminalId":id,"masterId":creator,"ownerSessionId":session.id,"executor":self.executor.metadata(&session,params),"status":"running","createdAt":Utc::now().to_rfc3339(),"cwd":ready["cwd"],"shell":ready["shell"],"exitCode":null});
        self.save(&record)?;
        let output = std::mem::replace(
            &mut worker.output,
            std::io::BufReader::new(Box::new(std::io::empty())),
        );
        let entry = Arc::new(Terminal {
            state: Mutex::new(State {
                record: record.clone(),
                bytes: VecDeque::new(),
                offset: 0,
                ack: 0,
                last_hash: String::new(),
                detached: Some(Instant::now()),
                attached: false,
            }),
            worker: Mutex::new(worker),
            creator,
            key: params["masterKey"].as_str().map(str::to_owned),
        });
        entries.insert(id, entry.clone());
        drop(entries);
        let this = self.clone();
        let item = entry.clone();
        std::thread::spawn(move || {
            let mut output = output;
            while let Ok(frame) = protocol::read_frame::<Value>(&mut output) {
                let mut state = item.state.lock().unwrap();
                match frame["event"].as_str() {
                    Some("output") => {
                        if let Some(bytes) =
                            frame["data"].as_str().and_then(|v| STANDARD.decode(v).ok())
                        {
                            state.bytes.extend(bytes);
                            while state.bytes.len() > BUFFER_CAP {
                                state.bytes.pop_front();
                                state.offset += 1;
                            }
                        }
                    }
                    Some("exit") => {
                        if state.record["status"] == "running" {
                            state.record["status"] = json!("exited");
                            state.record["exitCode"] = frame["exitCode"].clone();
                        }
                        break;
                    }
                    _ => {}
                }
            }
            let mut state = item.state.lock().unwrap();
            if state.record["status"] == "running" {
                state.record["status"] = json!("interrupted");
                state.record["message"] = json!("Owner worker disconnected");
            }
            state.record["finishedAt"] = json!(Utc::now().to_rfc3339());
            let _ = this.save(&state.record);
            drop(state);
            item.worker.lock().unwrap().terminate();
        });
        let this = self.clone();
        std::thread::spawn(move || loop {
            std::thread::sleep(Duration::from_millis(500));
            let session = entry.worker.lock().unwrap().session.clone();
            let ended = !this.executor.alive(&session);
            let revoked = entry.key.as_ref().is_some_and(|key| {
                !fleet::authorized_master_key(&this.state, &entry.creator, key).unwrap_or(false)
            });
            let mut state = entry.state.lock().unwrap();
            if state.record["status"] != "running" {
                break;
            }
            let expired = state
                .detached
                .is_some_and(|time| time.elapsed() >= DETACH_TTL);
            if ended || revoked || expired {
                state.record["status"] = json!(if expired { "expired" } else { "interrupted" });
                state.record["message"] = json!(if ended {
                    "Owner session ended"
                } else if revoked {
                    "Master access revoked"
                } else {
                    "Detached for 30 minutes"
                });
                let _ = this.save(&state.record);
                drop(state);
                entry.worker.lock().unwrap().terminate();
                break;
            }
        });
        Ok(record)
    }
    pub fn handle(&self, method: &str, params: &Value) -> Result<Value> {
        let id = protocol::string(params, "terminalId")?;
        uuid::Uuid::parse_str(id).context("E_ARGUMENT: terminalId must be UUID")?;
        let entry = self.entries.lock().unwrap().get(id).cloned();
        let Some(entry) = entry else {
            let record: String = self
                .db
                .lock()
                .unwrap()
                .query_row("SELECT record FROM terminals WHERE id=?1", [id], |r| {
                    r.get(0)
                })
                .context("E_NOT_FOUND: terminal not found")?;
            let record: Value = serde_json::from_str(&record)?;
            ensure!(
                record["masterId"] == params["masterId"],
                "E_PERMISSION: terminal belongs to another Master"
            );
            if method == "pty.status" {
                return Ok(record);
            }
            bail!("E_TERMINAL_INTERRUPTED: terminal is no longer running after service restart");
        };
        Self::authorize(&entry, params)?;
        let mut state = entry.state.lock().unwrap();
        if !state.attached {
            state.detached = Some(Instant::now());
        }
        match method {
            "pty.status" => {
                let mut value = state.record.clone();
                value["ack"] = json!(state.ack);
                value["attached"] = json!(state.attached);
                value["nextOffset"] = json!(state.offset + state.bytes.len() as u64);
                Ok(value)
            }
            "pty.read" => {
                let requested = params["offset"].as_u64().unwrap_or(state.offset);
                let offset = requested.max(state.offset);
                let skip = offset
                    .saturating_sub(state.offset)
                    .min(state.bytes.len() as u64) as usize;
                let limit = params["limit"].as_u64().unwrap_or(65536).min(1024 * 1024) as usize;
                let bytes: Vec<u8> = state.bytes.iter().skip(skip).take(limit).copied().collect();
                Ok(
                    json!({"terminalId":id,"contentBase64":STANDARD.encode(&bytes),"offset":state.offset+skip as u64,"nextOffset":state.offset+skip as u64+bytes.len() as u64,"truncated":requested<state.offset,"status":state.record["status"],"ack":state.ack}),
                )
            }
            "pty.write" => {
                ensure!(
                    state.record["status"] == "running",
                    "E_TERMINAL_CLOSED: terminal is not running"
                );
                let sequence = params["sequence"]
                    .as_u64()
                    .context("E_ARGUMENT: sequence must be an integer")?;
                let bytes = STANDARD.decode(protocol::string(params, "contentBase64")?)?;
                ensure!(
                    bytes.len() <= 65536,
                    "E_LIMIT: terminal input exceeds 64 KiB"
                );
                let hash = hex::encode(Sha256::digest(&bytes));
                if sequence <= state.ack {
                    ensure!(
                        sequence != state.ack || hash == state.last_hash,
                        "E_CONFLICT: sequence reused with different input"
                    );
                    return Ok(json!({"ack":state.ack,"duplicate":true}));
                }
                ensure!(
                    sequence == state.ack + 1,
                    "E_SEQUENCE: expected sequence {}",
                    state.ack + 1
                );
                let result = entry.worker.lock().unwrap().send(&Request {
                    method: "input".into(),
                    params: json!({"data":STANDARD.encode(bytes)}),
                });
                if let Err(e) = result {
                    state.record["status"] = json!("interrupted");
                    self.save(&state.record)?;
                    entry.worker.lock().unwrap().terminate();
                    return Err(e);
                }
                state.ack = sequence;
                state.last_hash = hash;
                Ok(json!({"ack":state.ack,"duplicate":false}))
            }
            "pty.resize" => {
                ensure!(
                    state.record["status"] == "running",
                    "E_TERMINAL_CLOSED: terminal is not running"
                );
                for key in ["rows", "cols"] {
                    ensure!(
                        params[key]
                            .as_u64()
                            .is_some_and(|v| (1..=4096).contains(&v)),
                        "E_ARGUMENT: {key} must be 1..4096"
                    );
                }
                entry.worker.lock().unwrap().send(&Request {
                    method: "resize".into(),
                    params: params.clone(),
                })?;
                Ok(json!({"resized":true}))
            }
            "pty.close" => {
                state.record["status"] = json!("closed");
                state.record["finishedAt"] = json!(Utc::now().to_rfc3339());
                self.save(&state.record)?;
                entry.worker.lock().unwrap().terminate();
                Ok(state.record.clone())
            }
            "pty.attach" => {
                ensure!(
                    state.record["status"] == "running",
                    "E_TERMINAL_CLOSED: terminal is not running"
                );
                ensure!(
                    !state.attached,
                    "E_BUSY: terminal already has a streaming attachment"
                );
                state.attached = true;
                state.detached = None;
                Ok(
                    json!({"terminalId":id,"ack":state.ack,"offset":state.offset,"executor":state.record["executor"]}),
                )
            }
            "pty.detach" => {
                state.attached = false;
                state.detached = Some(Instant::now());
                Ok(json!({"detached":true}))
            }
            _ => bail!("E_METHOD: unsupported terminal operation"),
        }
    }
    pub fn shutdown(&self) {
        for entry in self.entries.lock().unwrap().values() {
            let mut state = entry.state.lock().unwrap();
            if state.record["status"] == "running" {
                state.record["status"] = json!("interrupted");
                state.record["message"] = json!("Target service stopped");
                let _ = self.save(&state.record);
            }
            drop(state);
            entry.worker.lock().unwrap().terminate();
        }
    }
}
