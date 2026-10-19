//! Hidden subprocess entry. Credentials are established before threads or user work.
use crate::{
    executor::{self, Session},
    files,
    protocol::{self, Request, Response},
    security,
};
use anyhow::{bail, ensure, Context, Result};
use base64::{engine::general_purpose::STANDARD, Engine};
use serde_json::{json, Value};
use std::{
    fs,
    io::{Read, Write},
    path::PathBuf,
    process::{Command, Stdio},
    sync::{Arc, Mutex},
};

pub fn run() -> Result<()> {
    let launch: Request = protocol::read_frame(&mut std::io::stdin().lock())?;
    let session: Session = serde_json::from_value(launch.params["session"].clone())?;
    let admin = launch.params["asAdmin"].as_bool().unwrap_or(false);
    let entered = executor::enter(&session, admin).map(|_| json!({"principal":security::current_principal().unwrap_or_default(),"elevated":cfg!(windows)||admin}));
    let success = entered.is_ok();
    protocol::write_frame(
        &Response::from_result(entered),
        &mut std::io::stdout().lock(),
    )?;
    if !success {
        return Ok(());
    }
    let params = &launch.params["request"];
    match launch.method.as_str() {
        name if name.starts_with("fs.") => protocol::write_frame(
            &Response::from_result(files::handle(name, params)),
            &mut std::io::stdout().lock(),
        ),
        "worker.inspect" => protocol::write_frame(
            &Response::from_result(inspect()),
            &mut std::io::stdout().lock(),
        ),
        "worker.job" => job(params),
        "worker.terminal" => terminal(&session, params),
        _ => bail!("E_METHOD: unsupported worker operation"),
    }
}
fn output(value: &Value, writer: &Arc<Mutex<std::io::Stdout>>) -> Result<()> {
    protocol::write_frame(value, &mut *writer.lock().unwrap())
}
fn capture(
    mut input: impl Read,
    stream: &'static str,
    writer: Arc<Mutex<std::io::Stdout>>,
) -> Result<()> {
    let mut bytes = [0u8; 16384];
    loop {
        let n = input.read(&mut bytes)?;
        if n == 0 {
            break;
        }
        output(
            &json!({"event":"output","stream":stream,"data":STANDARD.encode(&bytes[..n])}),
            &writer,
        )?;
    }
    Ok(())
}
struct Script(PathBuf);
impl Drop for Script {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}
fn job(params: &Value) -> Result<()> {
    let cwd = protocol::string(params, "cwd")?;
    security::ensure_absolute(std::path::Path::new(cwd))?;
    let path = std::env::temp_dir().join(format!(
        "sshcc-{}.{}",
        uuid::Uuid::new_v4(),
        if cfg!(windows) { "ps1" } else { "sh" }
    ));
    let script = Script(path);
    // The service delivered this script through the pipe, and keeps all durable logs.
    let mut file = fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&script.0)?;
    security::protect(&script.0, false)?;
    #[cfg(windows)]
    file.write_all(&[0xef, 0xbb, 0xbf])?;
    file.write_all(protocol::string(params, "script")?.as_bytes())?;
    drop(file);
    let mut command;
    #[cfg(windows)]
    {
        let path = script.0.to_string_lossy().replace('\'', "''");
        let source=format!("$null=[Console]::ReadLine(); $ErrorActionPreference='Stop'; [Console]::OutputEncoding=[Text.UTF8Encoding]::new($false); $global:LASTEXITCODE=0; & ([scriptblock]::Create([IO.File]::ReadAllText('{path}'))); if ($LASTEXITCODE -ne 0) {{ exit $LASTEXITCODE }}");
        let encoded: Vec<u8> = source
            .encode_utf16()
            .flat_map(|c| c.to_le_bytes())
            .collect();
        command = Command::new("powershell.exe");
        command.args([
            "-NoLogo",
            "-NonInteractive",
            "-EncodedCommand",
            &STANDARD.encode(encoded),
        ]);
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command = Command::new("/bin/bash");
        command
            .process_group(0)
            .args([
                "-l",
                "-c",
                "IFS= read -r _sshcc_start; source \"$1\"",
                "sshcc",
            ])
            .arg(&script.0);
    }
    if let Some(vars) = params["env"].as_object() {
        for (key, val) in vars {
            command.env(key, val.as_str().context("E_ARGUMENT: invalid env")?);
        }
    }
    command
        .current_dir(cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(windows_sys::Win32::System::Threading::CREATE_NO_WINDOW);
    }
    let writer = Arc::new(Mutex::new(std::io::stdout()));
    let result = (|| -> Result<Value> {
        let mut child = command.spawn()?;
        let guard = crate::jobs::ProcessGuard::new(child.id())?;
        child
            .stdin
            .take()
            .context("E_EXEC: missing execution gate")?
            .write_all(b"\n")?;
        let out = child.stdout.take().unwrap();
        let err = child.stderr.take().unwrap();
        let a = writer.clone();
        let b = writer.clone();
        let out = std::thread::spawn(move || capture(out, "stdout", a));
        let err = std::thread::spawn(move || capture(err, "stderr", b));
        let status = child.wait()?;
        guard.terminate();
        out.join()
            .map_err(|_| anyhow::anyhow!("E_LOG: worker stdout failed"))??;
        err.join()
            .map_err(|_| anyhow::anyhow!("E_LOG: worker stderr failed"))??;
        Ok(json!({"event":"exit","exitCode":status.code(),"success":status.success()}))
    })();
    output(
        &result.unwrap_or_else(|e| json!({"event":"error","message":format!("{e:#}")})),
        &writer,
    )
}
fn terminal(session: &Session, params: &Value) -> Result<()> {
    use portable_pty::CommandBuilder;
    let cwd = params["cwd"].as_str().unwrap_or(&session.home);
    security::ensure_absolute(std::path::Path::new(cwd))?;
    ensure!(
        std::path::Path::new(cwd).is_dir(),
        "E_ARGUMENT: terminal cwd must be a directory"
    );
    let shell = params["shell"].as_str().unwrap_or(&session.shell);
    let mut command;
    #[cfg(windows)]
    {
        command = match shell {
            "powershell" => CommandBuilder::new("powershell.exe"),
            "cmd" => CommandBuilder::new("cmd.exe"),
            _ => bail!("E_ARGUMENT: shell must be powershell or cmd"),
        };
        if shell == "powershell" {
            command.arg("-NoLogo");
        } else {
            command.arg("/Q");
        }
    }
    #[cfg(unix)]
    {
        let executable = match shell {
            "bash" => "/bin/bash",
            "sh" => "/bin/sh",
            value if value.starts_with('/') => value,
            _ => bail!("E_ARGUMENT: shell must be an absolute executable path"),
        };
        command = CommandBuilder::new(executable);
        command.arg("-l");
        command.env("TERM", "xterm-256color");
    }
    command.cwd(cwd);
    let size = pty_size(params)?;
    let pair = portable_pty::native_pty_system().openpty(size)?;
    let mut child = pair.slave.spawn_command(command)?;
    drop(pair.slave);
    let reader = pair.master.try_clone_reader()?;
    let writer = Arc::new(Mutex::new(pair.master.take_writer()?));
    let output_writer = Arc::new(Mutex::new(std::io::stdout()));
    output(
        &json!({"event":"ready","cwd":cwd,"shell":shell}),
        &output_writer,
    )?;
    let out = output_writer.clone();
    #[cfg(windows)]
    let reply = writer.clone();
    let reader = std::thread::spawn(move || {
        #[cfg(windows)]
        {
            capture_conpty(reader, out, reply)
        }
        #[cfg(unix)]
        {
            capture(reader, "terminal", out)
        }
    });
    let (tx, rx) = std::sync::mpsc::sync_channel(32);
    std::thread::spawn(move || {
        let mut input = std::io::stdin().lock();
        while let Ok(request) = protocol::read_frame::<Request>(&mut input) {
            if tx.send(request).is_err() {
                break;
            }
        }
    });
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        match rx.recv_timeout(std::time::Duration::from_millis(25)) {
            Ok(request) => {
                let result = match request.method.as_str() {
                    "input" => STANDARD
                        .decode(request.params["data"].as_str().unwrap_or(""))
                        .map_err(anyhow::Error::from)
                        .and_then(|bytes| {
                            let mut writer = writer.lock().unwrap();
                            writer.write_all(&bytes)?;
                            writer.flush()?;
                            Ok(())
                        }),
                    "resize" => pty_size(&request.params).and_then(|s| pair.master.resize(s)),
                    _ => {
                        child.kill()?;
                        break child.wait()?;
                    }
                };
                if result.is_err() {
                    child.kill()?;
                    break child.wait()?;
                }
            }
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                child.kill()?;
                break child.wait()?;
            }
            Err(_) => {}
        }
    };
    // Notify the service before closing/joining a PTY held by orphan descendants.
    // The service then terminates the enclosing Job Object/cgroup, including them.
    output(
        &json!({"event":"exit","exitCode":status.exit_code(),"success":status.success()}),
        &output_writer,
    )?;
    drop(writer);
    drop(pair.master);
    // ConPTY closes its output once the shell and descendants have exited.
    reader
        .join()
        .map_err(|_| anyhow::anyhow!("E_TERMINAL: reader failed"))??;
    Ok(())
}
#[cfg(windows)]
fn capture_conpty(
    mut reader: Box<dyn Read + Send>,
    output_writer: Arc<Mutex<std::io::Stdout>>,
    input: Arc<Mutex<Box<dyn Write + Send>>>,
) -> Result<()> {
    // portable-pty requests an inherited cursor at ConPTY startup. The remote
    // console is new, so answer at 1,1; MCP clients have no terminal emulator.
    let mut initial = true;
    let mut pending = Vec::new();
    let mut bytes = [0u8; 8192];
    loop {
        let count = reader.read(&mut bytes)?;
        if count == 0 {
            break;
        }
        pending.extend_from_slice(&bytes[..count]);
        if initial {
            if let Some(index) = pending.windows(4).position(|s| s == b"\x1b[6n") {
                input.lock().unwrap().write_all(b"\x1b[1;1R")?;
                pending.drain(index..index + 4);
                initial = false;
            }
        }
        let emit = if initial {
            pending.len().saturating_sub(3)
        } else {
            pending.len()
        };
        if emit > 0 {
            output(
                &json!({"event":"output","stream":"terminal","data":STANDARD.encode(&pending[..emit])}),
                &output_writer,
            )?;
            pending.drain(..emit);
        }
    }
    if !pending.is_empty() {
        output(
            &json!({"event":"output","stream":"terminal","data":STANDARD.encode(pending)}),
            &output_writer,
        )?;
    }
    Ok(())
}
fn pty_size(params: &Value) -> Result<portable_pty::PtySize> {
    let dimension = |key, default| -> Result<u16> {
        let n = params[key].as_u64().unwrap_or(default);
        ensure!(
            (1..=4096).contains(&n),
            "E_ARGUMENT: terminal dimensions must be 1..4096"
        );
        Ok(n as u16)
    };
    Ok(portable_pty::PtySize {
        rows: dimension("rows", 24)?,
        cols: dimension("cols", 80)?,
        pixel_width: 0,
        pixel_height: 0,
    })
}
fn inspect() -> Result<Value> {
    let mut command;
    #[cfg(windows)]
    {
        command = Command::new("powershell.exe");
        command.args(["-NoLogo","-NonInteractive","-Command","$git=Get-Command git -ErrorAction SilentlyContinue; $codex=Get-Command codex -ErrorAction SilentlyContinue; $version=$null; $signed=$false; if($codex){ $version=(& $codex --version 2>$null | Select-Object -First 1); $login=(& $codex login status 2>&1 | Out-String); $signed=($LASTEXITCODE -eq 0 -and $login -match 'Logged in') }; @{git=if($git){$git.Source}else{$null};codex=if($codex){$codex.Source}else{$null};codexVersion=$version;codexSignedIn=$signed;path=$env:PATH;home=$env:USERPROFILE} | ConvertTo-Json -Compress"]);
    }
    #[cfg(unix)]
    {
        command = Command::new("/bin/bash");
        command.args(["-l","-c","python3 - <<'SSHCC_INSPECT'\nimport os,shutil,subprocess,json\nc=shutil.which('codex')\ndef run(args):\n try: return subprocess.run(args,capture_output=True,text=True,timeout=8)\n except (OSError,subprocess.TimeoutExpired): return None\nv=run([c,'--version']) if c else None\ns=run([c,'login','status']) if c else None\nprint(json.dumps(dict(git=shutil.which('git'),codex=c,codexVersion=v.stdout.strip() if v else None,codexSignedIn=bool(s and s.returncode==0 and 'Logged in' in s.stdout+s.stderr),path=os.environ.get('PATH'),home=os.environ.get('HOME'))))\nSSHCC_INSPECT"]);
    }
    let result = command.stdin(Stdio::null()).output()?;
    ensure!(
        result.status.success(),
        "E_CLI_INSPECT: cannot inspect Owner profile"
    );
    Ok(serde_json::from_slice(&result.stdout)?)
}
