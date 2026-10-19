//! Privileged service launches isolated workers; user work never opens service state.
use crate::{
    config::Config,
    protocol::{self, Request, Response},
};
use anyhow::{bail, ensure, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    io::{BufReader, Read, Write},
    sync::{Arc, Mutex},
    time::Duration,
};
#[cfg(windows)]
#[path = "executor_windows.rs"]
mod native;
#[cfg(unix)]
#[path = "executor_linux.rs"]
mod native;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Session {
    pub id: String,
    pub generation: String,
    pub owner: String,
    pub account: String,
    pub state: String,
    pub home: String,
    pub shell: String,
    pub elevated: bool,
}
pub struct Executor {
    pub config: Config,
}
pub(crate) fn enter(session: &Session, admin: bool) -> Result<()> {
    native::enter(session, admin)
}
pub struct Worker {
    pub input: Arc<Mutex<Box<dyn Write + Send>>>,
    pub output: BufReader<Box<dyn Read + Send>>,
    pub process: native::Process,
    pub session: Session,
}
impl Worker {
    pub fn send(&self, request: &Request) -> Result<()> {
        protocol::write_frame(request, &mut *self.input.lock().unwrap())
    }
    pub fn receive(&mut self) -> Result<Value> {
        self.receive_timeout(Duration::from_secs(20))
    }
    fn receive_timeout<T: serde::de::DeserializeOwned + Send + 'static>(
        &mut self,
        timeout: Duration,
    ) -> Result<T> {
        let mut reader =
            std::mem::replace(&mut self.output, BufReader::new(Box::new(std::io::empty())));
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        std::thread::spawn(move || {
            let result = protocol::read_frame::<T>(&mut reader);
            let _ = tx.send((result, reader));
        });
        match rx.recv_timeout(timeout) {
            Ok((result, reader)) => {
                self.output = reader;
                result
            }
            Err(error) => {
                self.terminate();
                Err(error).context("E_EXEC_TIMEOUT: Owner worker did not become ready")
            }
        }
    }
    pub fn terminate(&mut self) {
        self.process.terminate();
    }
}
impl Executor {
    pub fn new(config: Config) -> Arc<Self> {
        Arc::new(Self { config })
    }
    pub fn owner_mode(&self) -> bool {
        self.config.execution_mode == "owner-session"
    }
    pub fn sessions(&self) -> Result<Vec<Session>> {
        native::sessions(&self.config.owner)
    }
    pub fn select(&self, params: &Value) -> Result<Session> {
        ensure!(
            self.owner_mode(),
            "E_EXECUTION_MODE: select owner-session in installer first"
        );
        let sessions = self.sessions()?;
        let selected = if let Some(id) = params.get("ownerSessionId").filter(|v| !v.is_null()) {
            let id = id
                .as_str()
                .context("E_ARGUMENT: ownerSessionId must be a string")?;
            let selected = sessions
                .into_iter()
                .find(|s| s.id == id)
                .context("E_OWNER_NOT_LOGGED_IN: selected Owner session is absent")?;
            #[cfg(windows)]
            ensure!(
                selected.elevated,
                "E_OWNER_NOT_ADMIN: Owner has no elevated session token"
            );
            selected
        } else {
            if sessions.is_empty() {
                bail!("E_OWNER_NOT_LOGGED_IN: Owner must be logged in");
            }
            let eligible = eligible_sessions(sessions);
            if eligible.is_empty() {
                bail!("E_OWNER_NOT_ADMIN: Owner has no elevated session token");
            }
            pick_default_session(eligible)
                .expect("eligible_sessions non-empty guarantees a default")
        };
        if params.get("asAdmin").is_some_and(|v| !v.is_boolean()) {
            bail!("E_ARGUMENT: asAdmin must be boolean");
        }
        Ok(selected)
    }
    pub fn alive(&self, session: &Session) -> bool {
        self.sessions().is_ok_and(|sessions| {
            sessions
                .iter()
                .any(|s| s.id == session.id && s.generation == session.generation)
        })
    }
    pub fn spawn(&self, session: &Session, method: &str, params: &Value) -> Result<Worker> {
        ensure!(
            self.alive(session),
            "E_OWNER_NOT_LOGGED_IN: bound Owner session has ended"
        );
        let mut worker = native::spawn(session)?;
        worker.send(&Request { method: method.into(), params: json!({"session":session,"asAdmin":params["asAdmin"].as_bool().unwrap_or(false),"request":params}) })?;
        let response: Response = worker.receive_timeout(Duration::from_secs(15))?;
        let attestation = response.into_result()?;
        let expected = if cfg!(unix) && params["asAdmin"] == true {
            "0"
        } else {
            &session.owner
        };
        ensure!(
            attestation["principal"].as_str() == Some(expected),
            "E_IDENTITY: worker principal mismatch"
        );
        #[cfg(windows)]
        ensure!(
            attestation["elevated"] == true,
            "E_OWNER_NOT_ADMIN: worker token is not elevated"
        );
        Ok(worker)
    }
    pub fn call(&self, method: &str, params: &Value) -> Result<Value> {
        let session = self.select(params)?;
        let mut worker = self.spawn(&session, method, params)?;
        // A monitor bounds filesystem/inspection work to the original login session too.
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        let mut output = std::mem::replace(
            &mut worker.output,
            BufReader::new(Box::new(std::io::empty())),
        );
        std::thread::spawn(move || {
            let _ = tx.send(protocol::read_frame::<Response>(&mut output));
        });
        let deadline = std::time::Instant::now() + Duration::from_secs(30);
        let result = loop {
            match rx.recv_timeout(Duration::from_millis(250)) {
                Ok(result) => break result?.into_result(),
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                    bail!("E_EXEC: worker disconnected")
                }
                Err(_) if !self.alive(&session) => {
                    worker.terminate();
                    bail!("E_OWNER_NOT_LOGGED_IN: Owner session ended during request");
                }
                Err(_) if std::time::Instant::now() >= deadline => {
                    worker.terminate();
                    bail!("E_EXEC_TIMEOUT: Owner file/CLI inspection exceeded 30 seconds");
                }
                Err(_) => {}
            }
        };
        worker.process.wait()?;
        result.map(|mut value| {
            if let Some(object) = value.as_object_mut() {
                object.insert("executor".into(), self.metadata(&session, params));
            }
            value
        })
    }
    pub fn metadata(&self, session: &Session, params: &Value) -> Value {
        json!({"mode":"owner-session","owner":session.owner,"ownerAccount":session.account,"account":if cfg!(unix) && params["asAdmin"] == true {"root"} else {&session.account},"ownerSessionId":session.id,
            "principal":if cfg!(unix) && params["asAdmin"] == true {"0"} else {&session.owner},
            "elevated":cfg!(windows) || params["asAdmin"] == true,"home":session.home})
    }
    pub fn readiness(&self, params: &Value) -> Value {
        if !self.owner_mode() {
            return json!({"mode":"service","ready":true,"owner":self.config.owner});
        }
        match self.sessions() {
            Err(error) => {
                json!({"mode":"owner-session","ready":false,"owner":self.config.owner,"error":format!("{error:#}")})
            }
            Ok(sessions) => {
                let selected = self.select(params);
                let ready = selected.is_ok();
                let error = selected.as_ref().err().map(|e| format!("{e:#}"));
                let mut value = json!({"mode":"owner-session","ready":ready,"owner":self.config.owner,"sessions":sessions,
                    "error":error});
                if let Ok(session) = selected {
                    value["selected"] = self.metadata(&session, params);
                    value["error"] = Value::Null;
                }
                if params["inspectCli"] == true && ready {
                    value["cli"] = match self.call("worker.inspect", params) {
                        Ok(v) => v,
                        Err(e) => json!({"error":format!("{e:#}")}),
                    };
                }
                value
            }
        }
    }
}

fn eligible_sessions(sessions: Vec<Session>) -> Vec<Session> {
    if cfg!(windows) {
        sessions.into_iter().filter(|session| session.elevated).collect()
    } else {
        sessions
    }
}

fn preferred_state(state: &str) -> bool {
    // Linux logind uses "active"; Windows WTSActive formats as "0".
    state.eq_ignore_ascii_case("active") || state == "0"
}

fn session_recency_key(session: &Session) -> u64 {
    if cfg!(windows) {
        session.id.parse().unwrap_or(0)
    } else {
        session
            .generation
            .rsplit_once(':')
            .and_then(|(_, start)| start.parse().ok())
            .unwrap_or_else(|| session.id.parse().unwrap_or(0))
    }
}

fn pick_default_session(mut sessions: Vec<Session>) -> Option<Session> {
    if sessions.is_empty() {
        return None;
    }
    sessions.sort_by(|left, right| {
        preferred_state(&left.state)
            .cmp(&preferred_state(&right.state))
            .then_with(|| session_recency_key(left).cmp(&session_recency_key(right)))
    });
    sessions.pop()
}

#[cfg(test)]
mod tests {
    use super::{pick_default_session, Session};

    fn session(id: &str, generation: &str, state: &str, elevated: bool) -> Session {
        Session {
            id: id.into(),
            generation: generation.into(),
            owner: "1000".into(),
            account: "owner".into(),
            state: state.into(),
            home: "/home/owner".into(),
            shell: "/bin/bash".into(),
            elevated,
        }
    }

    #[test]
    fn pick_default_prefers_active_then_newest() {
        let picked = pick_default_session(vec![
            session("1", "10:100", "online", true),
            session("2", "20:50", "active", true),
            session("3", "30:200", "active", true),
        ])
        .unwrap();
        if cfg!(windows) {
            // Windows ranks by session id; active still beats non-active before id.
            assert_eq!(picked.id, "3");
        } else {
            // Linux ranks by generation starttime among active sessions.
            assert_eq!(picked.id, "3");
            assert_eq!(picked.generation, "30:200");
        }
    }

    #[test]
    fn pick_default_falls_back_when_no_active() {
        let picked = pick_default_session(vec![
            session("1", "10:100", "online", true),
            session("9", "20:50", "online", true),
        ])
        .unwrap();
        if cfg!(windows) {
            assert_eq!(picked.id, "9");
        } else {
            assert_eq!(picked.id, "1");
        }
    }
}
