//! `sshcc backup <file>` and `sshcc restore <file>`: the same service methods as the window. The passphrase is
//! typed at a prompt with no echo (or read from a pipe), never taken as an argument.
use anyhow::{bail, ensure, Context, Result};
use base64::{engine::general_purpose::STANDARD, Engine};
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use serde_json::{json, Value};
use std::{
    io::{IsTerminal, Write},
    path::Path,
};

/// One line from the terminal without echo; from a pipe, one line as is.
fn secret(label: &str) -> Result<String> {
    if !std::io::stdin().is_terminal() {
        let mut line = String::new();
        std::io::stdin().read_line(&mut line)?;
        return Ok(line.trim_end_matches(['\r', '\n']).to_owned());
    }
    eprint!("{label}");
    std::io::stderr().flush()?;
    struct Raw;
    impl Drop for Raw {
        fn drop(&mut self) {
            let _ = crossterm::terminal::disable_raw_mode();
            eprintln!();
        }
    }
    crossterm::terminal::enable_raw_mode()?;
    let _raw = Raw;
    let mut text = String::new();
    loop {
        if let Event::Key(key) = event::read()? {
            if key.kind != KeyEventKind::Press {
                continue;
            }
            match key.code {
                KeyCode::Enter => return Ok(text),
                KeyCode::Backspace => {
                    text.pop();
                }
                KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => bail!("cancelled"),
                KeyCode::Char(c) => text.push(c),
                KeyCode::Esc => bail!("cancelled"),
                _ => {}
            }
        }
    }
}
fn confirm(question: &str) -> Result<bool> {
    eprint!("{question} [y/N] ");
    std::io::stderr().flush()?;
    let mut line = String::new();
    std::io::stdin().read_line(&mut line)?;
    Ok(matches!(line.trim(), "y" | "Y" | "yes" | "Yes"))
}

pub fn backup(file: &Path, call: impl Fn(&str, Value) -> Result<Value>) -> Result<Value> {
    ensure!(!file.exists(), "{} already exists; choose another file name", file.display());
    let first = secret("Passphrase for the backup (at least 12 characters): ")?;
    let again = secret("Type the passphrase again: ")?;
    ensure!(first == again, "the two passphrases are different");
    eprintln!("The file gives full control of every host. Keep it like a key. A lost passphrase cannot be recovered.");
    let made = call("backup.create", json!({ "passphrase": first }))?;
    let bytes = STANDARD
        .decode(made["file"].as_str().context("the service returned no backup")?)?;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(file)?.write_all(&bytes)?;
    Ok(json!({"file": file.display().to_string(), "bytes": bytes.len()}))
}

pub fn restore(file: &Path, call: impl Fn(&str, Value) -> Result<Value>) -> Result<Value> {
    let bytes = std::fs::read(file).with_context(|| format!("cannot read {}", file.display()))?;
    let encoded = STANDARD.encode(&bytes);
    let passphrase = secret("Passphrase of the backup: ")?;
    let summary = call("backup.inspect", json!({ "file": encoded, "passphrase": passphrase }))?;
    eprintln!(
        "Backup of {} from {}: {} hosts, {} Merges, {} folders.",
        summary["label"].as_str().unwrap_or("?"),
        summary["createdAt"].as_str().unwrap_or("?"),
        summary["hosts"], summary["merges"], summary["folders"]
    );
    eprintln!("This machine becomes that Master. Two Masters with the same keys must never run at once.");
    if !confirm("Is the old machine off?")? {
        bail!("restore cancelled: turn the old machine off first");
    }
    let result = call("backup.restore", json!({ "file": encoded, "passphrase": passphrase }))?;
    for master in result["reachedBy"].as_array().into_iter().flatten() {
        eprintln!("Update this Master's address on {}", master["label"].as_str().unwrap_or("?"));
    }
    for merge in result["twoWayAgain"].as_array().into_iter().flatten() {
        eprintln!("Two-way with {} needs enabling again (Masters page, Enable two-way)", merge["label"].as_str().unwrap_or("?"));
    }
    for path in result["checkPaths"].as_array().into_iter().flatten() {
        eprintln!("Check this path on {}: {}", path["label"].as_str().unwrap_or("?"), path["path"].as_str().unwrap_or("?"));
    }
    Ok(result)
}
