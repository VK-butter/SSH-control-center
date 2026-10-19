use anyhow::{bail, Context, Result};
use base64::{engine::general_purpose::STANDARD, Engine};
use crossterm::{
    event::{
        self, DisableBracketedPaste, EnableBracketedPaste, Event, KeyCode, KeyEventKind,
        KeyModifiers,
    },
    terminal,
};
use serde_json::{json, Value};
use sshcc_core::{
    client, ipc,
    protocol::{self, Request, Response},
};
use std::{
    io::{BufReader, Write},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Duration,
};
fn call(method: &str, params: Value) -> Result<Value> {
    client::call(&Request {
        method: method.into(),
        params,
    })
}
struct Raw;
impl Drop for Raw {
    fn drop(&mut self) {
        let _ = crossterm::execute!(std::io::stdout(), DisableBracketedPaste);
        let _ = terminal::disable_raw_mode();
    }
}
pub fn run(
    target: String,
    cwd: Option<String>,
    session: Option<String>,
    shell: Option<String>,
    resume: Option<String>,
    admin: bool,
) -> Result<()> {
    let targets = call("targets.list", json!({}))?;
    let list = targets
        .as_array()
        .context("E_PROTOCOL: invalid target list")?;
    let exact = list.iter().find(|t| t["id"] == target);
    let matches: Vec<_> = list.iter().filter(|t| t["label"] == target).collect();
    let selected = match exact {
        Some(t) => t,
        None if matches.len() == 1 => matches[0],
        _ => bail!("E_TARGET: target name must match exactly one Target; use Target ID"),
    };
    let target_id = selected["id"].as_str().unwrap().to_owned();
    let (cols, rows) = terminal::size().unwrap_or((80, 24));
    let terminal_id = if let Some(id) = resume {
        uuid::Uuid::parse_str(&id)?;
        id
    } else {
        let mut params = json!({"targetId":target_id,"rows":rows,"cols":cols,"asAdmin":admin});
        if let Some(value) = cwd {
            params["cwd"] = json!(value);
        }
        if let Some(value) = session {
            params["ownerSessionId"] = json!(value);
        }
        if let Some(value) = shell {
            params["shell"] = json!(value);
        }
        call("terminal.open", params)?["terminalId"]
            .as_str()
            .context("E_PROTOCOL: terminal ID missing")?
            .to_owned()
    };
    eprintln!("terminalId={terminal_id}  (Ctrl+] detaches; --resume {terminal_id})");
    let mut connection = ipc::connect()?;
    protocol::write_frame(
        &Request {
            method: "terminal.attach".into(),
            params: json!({"targetId":target_id,"terminalId":terminal_id}),
        },
        &mut connection,
    )?;
    let mut reader = BufReader::new(connection.try_clone()?);
    let initial: Response = protocol::read_frame(&mut reader)?;
    let attached = initial.into_result()?;
    let mut sequence = attached["ack"].as_u64().unwrap_or(0);
    terminal::enable_raw_mode()?;
    let _raw = Raw;
    crossterm::execute!(std::io::stdout(), EnableBracketedPaste)?;
    let stopped = Arc::new(AtomicBool::new(false));
    let stop = stopped.clone();
    std::thread::spawn(move || {
        while !stop.load(Ordering::SeqCst) {
            if !event::poll(Duration::from_millis(100)).unwrap_or(false) {
                continue;
            }
            let Ok(event) = event::read() else {
                break;
            };
            let request = match event {
                Event::Resize(cols, rows) => Some(Request {
                    method: "resize".into(),
                    params: json!({"rows":rows,"cols":cols}),
                }),
                Event::Paste(text) => {
                    sequence += 1;
                    Some(Request {
                        method: "input".into(),
                        params: json!({"sequence":sequence,"contentBase64":STANDARD.encode(text.as_bytes())}),
                    })
                }
                Event::Key(key) if key.kind != KeyEventKind::Release => {
                    if key.modifiers.contains(KeyModifiers::CONTROL)
                        && key.code == KeyCode::Char(']')
                    {
                        let _ = protocol::write_frame(
                            &Request {
                                method: "detach".into(),
                                params: json!({}),
                            },
                            &mut connection,
                        );
                        break;
                    }
                    if let Some(bytes) = key_bytes(key.code, key.modifiers) {
                        sequence += 1;
                        Some(Request {
                            method: "input".into(),
                            params: json!({"sequence":sequence,"contentBase64":STANDARD.encode(bytes)}),
                        })
                    } else {
                        None
                    }
                }
                _ => None,
            };
            if let Some(request) = request {
                if protocol::write_frame(&request, &mut connection).is_err() {
                    break;
                }
            }
        }
    });
    let result = (|| -> Result<()> {
        let mut output = std::io::stdout().lock();
        loop {
            let response: Response = protocol::read_frame(&mut reader)?;
            let frame = response.into_result()?;
            if let Some(text) = frame["contentBase64"].as_str() {
                output.write_all(&STANDARD.decode(text)?)?;
                output.flush()?;
            }
            if frame["event"] == "end" {
                break;
            }
        }
        Ok(())
    })();
    stopped.store(true, Ordering::SeqCst);
    result
}
fn key_bytes(code: KeyCode, modifiers: KeyModifiers) -> Option<Vec<u8>> {
    let text = match code {
        KeyCode::Enter => "\r".into(),
        KeyCode::Tab => "\t".into(),
        KeyCode::BackTab => "\x1b[Z".into(),
        KeyCode::Backspace => "\x7f".into(),
        KeyCode::Esc => "\x1b".into(),
        KeyCode::Up => "\x1b[A".into(),
        KeyCode::Down => "\x1b[B".into(),
        KeyCode::Right => "\x1b[C".into(),
        KeyCode::Left => "\x1b[D".into(),
        KeyCode::Home => "\x1b[H".into(),
        KeyCode::End => "\x1b[F".into(),
        KeyCode::Delete => "\x1b[3~".into(),
        KeyCode::Insert => "\x1b[2~".into(),
        KeyCode::PageUp => "\x1b[5~".into(),
        KeyCode::PageDown => "\x1b[6~".into(),
        KeyCode::Char(c) if modifiers.contains(KeyModifiers::CONTROL) && c.is_ascii() => {
            ((c.to_ascii_uppercase() as u8) & 0x1f).to_string()
        }
        KeyCode::Char(c) => c.to_string(),
        _ => return None,
    };
    // Control characters must be sent as bytes, not their decimal text representation.
    let mut bytes = if let KeyCode::Char(c) = code {
        if modifiers.contains(KeyModifiers::CONTROL) && c.is_ascii() {
            vec![(c.to_ascii_uppercase() as u8) & 0x1f]
        } else {
            text.into_bytes()
        }
    } else {
        text.into_bytes()
    };
    if modifiers.contains(KeyModifiers::ALT) {
        bytes.insert(0, 0x1b);
    }
    Some(bytes)
}
