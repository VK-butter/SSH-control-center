use crate::options::benchmark_options;

use serde_json::Value;
use sshcc_core::{client, client_links as links, protocol::Request};
use std::{
    io::Read,
    sync::atomic::{AtomicBool, AtomicU64, Ordering},
    time::Duration,
};
use tauri::{
    image::Image,
    menu::{Menu, MenuItem, PredefinedMenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    AppHandle, Manager, WebviewUrl, WebviewWindowBuilder,
};
use tauri_plugin_dialog::DialogExt;

static OPENING: AtomicBool = AtomicBool::new(false);
static HIDE_GENERATION: AtomicU64 = AtomicU64::new(0);

// A closed window stays hidden for this long so the next tray click skips the
// WebView cold start; afterwards the window process exits and frees its runtime.
fn idle_window_lifetime() -> Duration {
    std::env::var("SSHCC_WINDOW_IDLE_SECS")
        .ok()
        .and_then(|value| value.trim().parse().ok())
        .map(Duration::from_secs)
        .unwrap_or(Duration::from_secs(600))
}

fn hide_until_idle(window: &tauri::Window, lifetime: Duration) {
    let _ = window.hide();
    let generation = HIDE_GENERATION.fetch_add(1, Ordering::AcqRel) + 1;
    let window = window.clone();
    std::thread::spawn(move || {
        std::thread::sleep(lifetime);
        // Checked on the event loop, which also serializes show_window.
        let app = window.app_handle().clone();
        let _ = app.run_on_main_thread(move || {
            if HIDE_GENERATION.load(Ordering::Acquire) == generation {
                let _ = window.destroy();
            }
        });
    });
}

#[cfg(all(test, windows))]
mod live_shell {
    #[test]
    #[ignore = "requires the installed Master and an explicitly selected live Target"]
    fn launch_under_management_job() {
        use windows_sys::Win32::System::{JobObjects::*, Threading::GetCurrentProcess};
        let target = std::env::var("SSHCC_LIVE_SHELL_TARGET").expect("set live Target ID");
        let report = std::env::var("SSHCC_LIVE_SHELL_REPORT").expect("set report path");
        let job = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
        assert!(!job.is_null());
        let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE | JOB_OBJECT_LIMIT_BREAKAWAY_OK;
        assert_ne!(unsafe { SetInformationJobObject(job, JobObjectExtendedLimitInformation,
            (&limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(), std::mem::size_of_val(&limits) as u32) }, 0);
        assert_ne!(unsafe { AssignProcessToJobObject(job, GetCurrentProcess()) }, 0);
        let session = std::env::var("SSHCC_LIVE_SHELL_SESSION").ok();
        let result = tauri::async_runtime::block_on(super::open_shell(target, session));
        std::fs::write(report, serde_json::to_vec_pretty(&serde_json::json!({"result":result})).unwrap()).unwrap();
        assert!(result.is_ok(), "{result:?}");
        // The OS closes this test process's job handle when the runner exits.
        // The caller then verifies that its newly opened shell survived.
    }
}

// OpenSSH and local pipe reads never execute on the window event thread.
#[tauri::command]
async fn rpc(method: String, params: Value) -> Result<Value, String> {
    tauri::async_runtime::spawn_blocking(move || {
        client::call(&Request { method, params }).map_err(|error| format!("{error:#}"))
    })
    .await
    .map_err(|error| error.to_string())?
}

fn client_link_paths() -> Result<(std::path::PathBuf, std::path::PathBuf), String> {
    let exe = std::env::current_exe().map_err(|error| error.to_string())?;
    let sshcc = links::resolve_sshcc(&exe).map_err(|error| format!("{error:#}"))?;
    let home = links::user_home().map_err(|error| format!("{error:#}"))?;
    Ok((sshcc, home))
}

#[tauri::command]
async fn client_links() -> Result<Value, String> {
    tauri::async_runtime::spawn_blocking(|| {
        let (sshcc, home) = client_link_paths()?;
        serde_json::to_value(links::list_links(&sshcc, &home)).map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
async fn client_connect(client: String) -> Result<Value, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let id = links::ClientId::parse(&client).map_err(|error| format!("{error:#}"))?;
        let (sshcc, home) = client_link_paths()?;
        serde_json::to_value(links::connect(id, &sshcc, &home).map_err(|error| format!("{error:#}"))?)
            .map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
async fn client_update_path(client: String) -> Result<Value, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let id = links::ClientId::parse(&client).map_err(|error| format!("{error:#}"))?;
        let (sshcc, home) = client_link_paths()?;
        serde_json::to_value(links::update_path(id, &sshcc, &home).map_err(|error| format!("{error:#}"))?)
            .map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
async fn open_shell(target_id: String, owner_session_id: Option<String>) -> Result<Value, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let targets = client::call(&Request {
            method: "targets.list".into(), params: serde_json::json!({}),
        }).map_err(|error| format!("{error:#}"))?;
        let target = targets.as_array().and_then(|rows| rows.iter().find(|row| row["id"] == target_id))
            .ok_or_else(|| "This machine is not in the Master registry".to_string())?;
        if target["runnerInstalled"] != true {
            return Err("Install the Target service on this machine before opening a shell".into());
        }
        let mut params = serde_json::json!({"targetId":target_id});
        if let Some(id) = &owner_session_id { params["ownerSessionId"] = serde_json::json!(id); }
        let check = client::call(&Request { method: "targets.doctor".into(), params })
            .map_err(|error| format!("{error:#}"))?;
        if check["ok"] != true {
            let messages: Vec<_> = check["stages"].as_array().into_iter().flatten()
                .filter(|stage| stage["ok"] == false)
                .filter_map(|stage| stage["message"].as_str()).collect();
            return Err(format!("Cannot connect to the machine: {}", messages.join("; ")));
        }
        let status = &check["result"];
        if status.get("activeTerminals").is_none() {
            return Err("The terminal needs a Target service that supports it. Upgrade this machine".into());
        }
        let executor = &status["executor"];
        let error = executor["error"].as_str().unwrap_or("Owner is not ready");
        if executor["ready"] != true {
            if owner_session_id.is_none() && error.contains("E_OWNER_SESSION_REQUIRED") {
                return Ok(serde_json::json!({"needsSession":true,"sessions":executor["sessions"]}));
            }
            return Err(error.to_string());
        }
        let executable = std::env::current_exe().map_err(|error| error.to_string())?
            .with_file_name(if cfg!(windows) { "sshcc.exe" } else { "sshcc" });
        let session = executor["selected"]["ownerSessionId"].as_str();
        let mut arguments = vec![std::ffi::OsString::from("shell")];
        if let Some(id) = session {
            arguments.extend(["--session".into(), id.into()]);
        }
        arguments.extend(["--".into(), target_id.clone().into()]);
        #[cfg(windows)]
        let pid = crate::windows_window::spawn_shell(&executable, &arguments)
            .map_err(|error| format!("Could not open a Terminal window: {error}"))?;
        #[cfg(not(windows))]
        let pid = {
            let mut command = std::process::Command::new("x-terminal-emulator");
            command.arg("-e").arg(&executable);
            command.args(&arguments);
            let child = command.spawn().map_err(|error| format!("Could not open a Terminal window: {error}"))?;
            let pid = child.id();
            std::thread::spawn(move || { let mut child = child; let _ = child.wait(); });
            pid
        };
        Ok(serde_json::json!({"opened":true,"pid":pid,"targetId":target_id,"ownerSessionId":session}))
    }).await.map_err(|error| error.to_string())?
}

// The caller cannot supply a local path. Access is limited to the user's native
// file selection, so the webview needs no general filesystem permission.
#[tauri::command]
async fn open_json(app: AppHandle, title: Option<String>) -> Result<Option<Value>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let Some(path) = app
            .dialog()
            .file()
            .set_title(title.unwrap_or_else(|| "Open registration or enrollment".into()))
            .add_filter("JSON", &["json"])
            .blocking_pick_file()
        else {
            return Ok(None);
        };
        let path = path.into_path().map_err(|error| error.to_string())?;
        let file = std::fs::File::open(path).map_err(|error| error.to_string())?;
        let mut text = String::new();
        file.take(2 * 1024 * 1024 + 1)
            .read_to_string(&mut text)
            .map_err(|error| error.to_string())?;
        if text.len() > 2 * 1024 * 1024 {
            return Err("JSON file exceeds 2 MiB".into());
        }
        serde_json::from_str(text.trim_start_matches('\u{feff}'))
            .map(Some)
            .map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
async fn save_json(
    app: AppHandle,
    document: Value,
    title: Option<String>,
) -> Result<Option<String>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let Some(path) = app
            .dialog()
            .file()
            .set_title(title.unwrap_or_else(|| "Save public enrollment bundle".into()))
            .add_filter("JSON", &["json"])
            .set_file_name("sshcc-enrollment.json")
            .blocking_save_file()
        else {
            return Ok(None);
        };
        let path = path.into_path().map_err(|error| error.to_string())?;
        let text = serde_json::to_string_pretty(&document).map_err(|error| error.to_string())?;
        std::fs::write(&path, text).map_err(|error| error.to_string())?;
        Ok(Some(path.display().to_string()))
    })
    .await
    .map_err(|error| error.to_string())?
}

// The webview names only the host and path; the bytes never pass through it and
// are written solely to the location picked in the native save dialog.
#[tauri::command]
async fn save_remote_file(
    app: AppHandle,
    target_id: String,
    path: String,
    name: String,
    title: Option<String>,
) -> Result<Option<String>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let Some(dest) = app
            .dialog()
            .file()
            .set_title(title.unwrap_or_else(|| "Download file".into()))
            .set_file_name(name)
            .blocking_save_file()
        else {
            return Ok(None);
        };
        let dest = dest.into_path().map_err(|error| error.to_string())?;
        client::download(&target_id, &path, &dest).map_err(|error| format!("{error:#}"))?;
        Ok(Some(dest.display().to_string()))
    })
    .await
    .map_err(|error| error.to_string())?
}

/// Largest backup file the window reads; the service refuses anything bigger too.
const MAX_BACKUP_BYTES: u64 = 4 * 1024 * 1024;

// Backup: the native save dialog first, then the service seals the file with the
// passphrase. The webview names no path; the file is written only where the person chose.
#[tauri::command]
async fn save_backup(
    app: AppHandle,
    passphrase: String,
    name: String,
    title: Option<String>,
) -> Result<Option<String>, String> {
    use base64::{engine::general_purpose::STANDARD, Engine};
    tauri::async_runtime::spawn_blocking(move || {
        let Some(path) = app
            .dialog()
            .file()
            .set_title(title.unwrap_or_else(|| "Save the backup of this Master".into()))
            .add_filter("SSH Control Center backup", &["sshccbak"])
            .set_file_name(name)
            .blocking_save_file()
        else {
            return Ok(None);
        };
        let path = path.into_path().map_err(|error| error.to_string())?;
        let made = client::call(&Request {
            method: "backup.create".into(),
            params: serde_json::json!({ "passphrase": passphrase }),
        })
        .map_err(|error| format!("{error:#}"))?;
        let bytes = STANDARD
            .decode(made["file"].as_str().unwrap_or_default())
            .map_err(|error| error.to_string())?;
        std::fs::write(&path, bytes).map_err(|error| error.to_string())?;
        Ok(Some(path.display().to_string()))
    })
    .await
    .map_err(|error| error.to_string())?
}

// Update fallback (entry 8): the installer that updates one host, saved where the person
// chooses, to run on that host by hand. The Ubuntu file is the service package itself.
#[tauri::command]
async fn save_upgrade(app: AppHandle, target_id: String, os: String, title: Option<String>) -> Result<Option<String>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let templates = sshcc_core::setup_package::default_templates();
        let (bytes, _) = sshcc_core::upgrade::package(&templates, &os, &target_id).map_err(|error| format!("{error:#}"))?;
        let name = if os == "windows" { "sshcc-update.exe".to_owned() } else { format!("sshcc_{}.deb", sshcc_core::upgrade::ours()) };
        let Some(path) = app.dialog().file().set_title(title.unwrap_or_else(|| "Save the update installer".into())).set_file_name(name).blocking_save_file() else {
            return Ok(None);
        };
        let path = path.into_path().map_err(|error| error.to_string())?;
        std::fs::write(&path, bytes).map_err(|error| error.to_string())?;
        Ok(Some(path.display().to_string()))
    })
    .await
    .map_err(|error| error.to_string())?
}

// Activity CSV: the native save dialog first, then the service builds the CSV for the
// filter on screen. The webview names no path and sends no file content.
#[tauri::command]
async fn save_csv(
    app: AppHandle,
    params: Value,
    name: String,
    title: Option<String>,
) -> Result<Option<String>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let Some(path) = app
            .dialog()
            .file()
            .set_title(title.unwrap_or_else(|| "Export CSV".into()))
            .add_filter("CSV", &["csv"])
            .set_file_name(name)
            .blocking_save_file()
        else {
            return Ok(None);
        };
        let path = path.into_path().map_err(|error| error.to_string())?;
        let made = client::call(&Request { method: "activity.csv".into(), params })
            .map_err(|error| format!("{error:#}"))?;
        std::fs::write(&path, made["csv"].as_str().unwrap_or_default()).map_err(|error| error.to_string())?;
        Ok(Some(path.display().to_string()))
    })
    .await
    .map_err(|error| error.to_string())?
}

// Restore: the person picks a .sshccbak file in the native dialog. The webview gets
// only its name and its (encrypted) bytes, never a path it could choose itself.
#[tauri::command]
async fn open_backup(app: AppHandle, title: Option<String>) -> Result<Option<Value>, String> {
    use base64::{engine::general_purpose::STANDARD, Engine};
    tauri::async_runtime::spawn_blocking(move || {
        let Some(path) = app
            .dialog()
            .file()
            .set_title(title.unwrap_or_else(|| "Choose a backup file".into()))
            .add_filter("SSH Control Center backup", &["sshccbak"])
            .blocking_pick_file()
        else {
            return Ok(None);
        };
        let path = path.into_path().map_err(|error| error.to_string())?;
        let mut bytes = Vec::new();
        std::fs::File::open(&path)
            .map_err(|error| error.to_string())?
            .take(MAX_BACKUP_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(|error| error.to_string())?;
        if bytes.len() as u64 > MAX_BACKUP_BYTES {
            return Err("E_BACKUP_NOT_A_BACKUP: the file is too large to be a backup".into());
        }
        let name = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        Ok(Some(serde_json::json!({ "name": name, "file": STANDARD.encode(bytes) })))
    })
    .await
    .map_err(|error| error.to_string())?
}

// Setup data is fetched again from the owner-authorized service. The webview
// supplies only a request ID (and the dialog titles in the window's language);
// writing is limited to the native save dialog.
#[tauri::command]
async fn save_setup(
    app: AppHandle,
    request_id: String,
    title: Option<String>,
    folder_title: Option<String>,
) -> Result<Option<Value>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let status = |id: &str| {
            client::call(&Request {
                method: "pairing.status".into(),
                params: serde_json::json!({"requestId": id}),
            })
            .map_err(|error| format!("{error:#}"))
        };
        let request = status(&request_id)?;
        let templates = sshcc_core::setup_package::default_templates();
        // An unknown-OS pair: one folder, both installers (Windows .exe and Ubuntu .deb).
        if let Some(sibling) = request["sibling"].as_str() {
            let other = status(sibling)?;
            let Some(folder) = app
                .dialog()
                .file()
                .set_title(folder_title.unwrap_or_else(|| {
                    "Choose a folder for the installers (Windows and Ubuntu)".into()
                }))
                .blocking_pick_folder()
            else {
                return Ok(None);
            };
            let folder = folder.into_path().map_err(|error| error.to_string())?;
            return sshcc_core::setup_package::build_into(
                &[request["envelope"].clone(), other["envelope"].clone()],
                &templates,
                &folder,
            )
            .map(Some)
            .map_err(|error| format!("{error:#}"));
        }
        let envelope = &request["envelope"];
        let filename =
            sshcc_core::setup_package::file_name(envelope).map_err(|error| format!("{error:#}"))?;
        let extension =
            sshcc_core::setup_package::extension(envelope).map_err(|error| format!("{error:#}"))?;
        let Some(path) = app
            .dialog()
            .file()
            .set_title(title.unwrap_or_else(|| "Save installer for the new machine".into()))
            .add_filter("Installer", &[extension])
            .set_file_name(filename)
            .blocking_save_file()
        else {
            return Ok(None);
        };
        let path = path.into_path().map_err(|error| error.to_string())?;
        sshcc_core::setup_package::build(envelope, &templates, &path)
            .map(Some)
            .map_err(|error| format!("{error:#}"))
    })
    .await
    .map_err(|error| error.to_string())?
}

// Opening the other direction of a merge makes this Master a Target of the merged
// Master. The webview supplies only the merged Target ID; the pairing request comes
// from the service and the installer runs from a staged copy of this installation,
// because Setup.ps1 cannot replace the folder it runs from.
#[tauri::command]
async fn merge_two_way(app: AppHandle, target_id: String) -> Result<Value, String> {
    use tauri::Emitter;
    tauri::async_runtime::spawn_blocking(move || {
        let progress = |step: &str, message: &str| {
            let _ = app.emit("merge-two-way", serde_json::json!({"targetId":target_id,"step":step,"message":message}));
        };
        let call = |method: &str, params: Value| {
            client::call(&Request { method: method.into(), params }).map_err(|error| format!("{error:#}"))
        };
        progress("request", "Requesting pairing from the remote Master");
        let reply = call("merges.pair_request", serde_json::json!({"targetId":target_id}))?;
        let code = if reply.get("existingTargetId").is_some() {
            None
        } else {
            progress("install", "Installing the SSH server and the remote Master’s access on this machine");
            Some(run_pairing_setup(&reply["envelope"], &|message| progress("install", message))?)
        };
        progress("confirm", "Remote Master is checking this machine and merging back");
        let mut params = serde_json::json!({"targetId":target_id});
        if let Some(code) = code { params["code"] = serde_json::json!(code); }
        let result = call("merges.pair_complete", params)?;
        progress("done", "");
        Ok(result)
    })
    .await
    .map_err(|error| error.to_string())?
}

#[cfg(windows)]
fn run_pairing_setup(envelope: &Value, progress: &dyn Fn(&str)) -> Result<String, String> {
    use std::os::windows::process::CommandExt;
    let text = |error: std::io::Error| error.to_string();
    let install = std::env::current_exe().map_err(text)?
        .parent().ok_or("Installation folder not found")?.to_path_buf();
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos();
    let hex = format!("{:032x}", nanos ^ ((std::process::id() as u128) << 96));
    let run_id = format!("{}-{}-{}-{}-{}", &hex[0..8], &hex[8..12], &hex[12..16], &hex[16..20], &hex[20..32]);
    let staging = std::env::temp_dir().join(format!("sshcc-merge-{run_id}"));
    std::fs::create_dir(&staging).map_err(text)?;
    let result = (|| -> Result<String, String> {
        for name in ["sshcc.exe", "Setup.ps1", "Uninstall.ps1", "PairingBootstrap.ps1", "Configure-Codex.ps1"] {
            let source = install.join(name);
            if source.is_file() {
                std::fs::copy(&source, staging.join(name)).map_err(text)?;
            }
        }
        let request = staging.join("pairing-request.json");
        std::fs::write(&request, serde_json::to_vec(envelope).map_err(|e| e.to_string())?).map_err(text)?;
        // Setup.ps1 asks for UAC itself and writes results to a system-owned stage.
        let mut child = std::process::Command::new("powershell.exe")
            .args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-File"])
            .arg(staging.join("Setup.ps1"))
            .arg("-PairingRequest").arg(&request)
            .args(["-SetupRunId", &run_id])
            .creation_flags(0x08000000)
            .spawn().map_err(text)?;
        let stage = std::path::PathBuf::from(std::env::var_os("ProgramData").unwrap_or_else(|| "C:\\ProgramData".into()))
            .join("SSHControlCenterSetup").join(&run_id);
        let mut last = String::new();
        let status = loop {
            if let Some(status) = child.try_wait().map_err(text)? { break status; }
            if let Some(message) = std::fs::read_to_string(stage.join("progress.json")).ok()
                .and_then(|t| serde_json::from_str::<Value>(t.trim_start_matches('\u{feff}')).ok())
                .and_then(|p| p["message"].as_str().map(str::to_owned)) {
                if message != last { progress(&message); last = message; }
            }
            std::thread::sleep(std::time::Duration::from_millis(700));
        };
        if let Some(code) = std::fs::read_to_string(stage.join("pairing-code.txt")).ok()
            .map(|code| code.trim().to_owned()).filter(|code| !code.is_empty()) {
            return Ok(code);
        }
        let error = std::fs::read_to_string(stage.join("result.json")).ok()
            .and_then(|t| serde_json::from_str::<Value>(t.trim_start_matches('\u{feff}')).ok())
            .and_then(|r| r["error"].as_str().filter(|e| !e.is_empty()).map(str::to_owned))
            .unwrap_or_else(|| format!("The installer exited without a pairing code ({status}); the admin prompt may have been cancelled"));
        Err(error)
    })();
    let _ = std::fs::remove_dir_all(&staging);
    result
}

#[cfg(not(windows))]
fn run_pairing_setup(_envelope: &Value, _progress: &dyn Fn(&str)) -> Result<String, String> {
    Err("Automatic two-way setup currently supports Windows Masters only".into())
}

fn open_management_window(app: &AppHandle, isolated_window: bool) {
    if isolated_window {
        show_window(app);
    } else {
        #[cfg(windows)]
        crate::windows_window::open();
        #[cfg(not(windows))]
        show_window(app);
    }
}

fn show_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        use tauri::Emitter;
        HIDE_GENERATION.fetch_add(1, Ordering::AcqRel);
        let reopened = !window.is_visible().unwrap_or(true);
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
        if reopened {
            let _ = window.emit("management-window-shown", ());
        }
        return;
    }
    if OPENING.swap(true, Ordering::AcqRel) {
        return;
    }
    let app = app.clone();
    // Creating a WebView from a synchronous Windows event callback can deadlock.
    // Build from a worker; Tauri marshals native window work to its event loop.
    tauri::async_runtime::spawn_blocking(move || {
        let result = WebviewWindowBuilder::new(&app, "main", WebviewUrl::App("index.html".into()))
            .title("SSH Control Center")
            .inner_size(1180.0, 780.0)
            .min_inner_size(820.0, 600.0)
            // Let the page receive files dropped from the desktop (Files tab upload).
            .disable_drag_drop_handler()
            .build();
        OPENING.store(false, Ordering::Release);
        if let Err(error) = result {
            let text = crate::options::tray_text(crate::options::os_is_thai());
            app.dialog()
                .message(format!("{}: {error}", text.cannot_open))
                .title("SSH Control Center")
                .show(|_| {});
        }
    });
}

// Opt-in acceptance hook. Uses the same creation and close lifecycle as tray
// clicks, and leaves the process in tray-only mode for external RAM/CPU sampling.
// It neither starts a service nor opens a network/debug endpoint.
fn run_benchmark(app: AppHandle, cycles: u8, report: std::path::PathBuf) {
    tauri::async_runtime::spawn_blocking(move || {
        use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
        let mut completed = 0_u8;
        let outcome = (|| -> Result<(), String> {
            for _ in 0..cycles {
                show_window(&app);
                let deadline = Instant::now() + Duration::from_secs(30);
                let window = loop {
                    if let Some(window) = app.get_webview_window("main") {
                        break window;
                    }
                    if Instant::now() >= deadline {
                        return Err("management window did not appear within 30 seconds".into());
                    }
                    std::thread::sleep(Duration::from_millis(100));
                };
                std::thread::sleep(Duration::from_secs(5));
                window.close().map_err(|error| error.to_string())?;
                drop(window);
                let deadline = Instant::now() + Duration::from_secs(15);
                while app.get_webview_window("main").is_some() {
                    if Instant::now() >= deadline {
                        return Err("management window did not close within 15 seconds".into());
                    }
                    std::thread::sleep(Duration::from_millis(100));
                }
                completed += 1;
                std::thread::sleep(Duration::from_secs(5));
            }
            Ok(())
        })();
        let document = serde_json::json!({
            "schemaVersion": 1,
            "pid": std::process::id(),
            "ok": outcome.is_ok(),
            "cyclesRequested": cycles,
            "cyclesCompleted": completed,
            "windowAbsent": app.get_webview_window("main").is_none(),
            "completedAtUnixSeconds": SystemTime::now().duration_since(UNIX_EPOCH).map(|time| time.as_secs()).unwrap_or(0),
            "error": outcome.as_ref().err()
        });
        let write = serde_json::to_vec_pretty(&document)
            .map_err(|error| error.to_string())
            .and_then(|bytes| std::fs::write(&report, bytes).map_err(|error| error.to_string()));
        let report_ok = write.is_ok();
        if let Err(error) = write {
            eprintln!("cannot save benchmark report {}: {error}", report.display());
        }
        app.exit(if outcome.is_ok() && report_ok { 0 } else { 1 });
    });
}
pub fn run_tauri(isolated_window: bool) {
    client::set_channel("desktop");
    let args: Vec<String> = std::env::args().skip(1).collect();
    let benchmark = match benchmark_options(&args) {
        Ok(options) => options,
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(2);
        }
    };
    let benchmark_window = isolated_window && benchmark.is_some();
    let idle_lifetime = if isolated_window && !benchmark_window {
        idle_window_lifetime()
    } else {
        Duration::ZERO
    };
    #[cfg_attr(not(windows), allow(unused_mut))]
    let mut context = tauri::generate_context!();
    // Separate instance identity and WebView2 data directory from the resident
    // tray and any browser processes created by an earlier tray version.
    #[cfg(windows)]
    if isolated_window {
        context.config_mut().identifier.push_str(".window");
    }
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(move |app, _, _| {
            open_management_window(app, isolated_window)
        }))
        .plugin(tauri_plugin_dialog::init())
        .on_window_event(move |window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                if !idle_lifetime.is_zero() {
                    api.prevent_close();
                    hide_until_idle(window, idle_lifetime);
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            rpc,
            open_shell,
            open_json,
            save_json,
            save_setup,
            save_remote_file,
            save_backup,
            open_backup,
            save_csv,
            save_upgrade,
            merge_two_way,
            client_links,
            client_connect,
            client_update_path
        ])
        .setup(move |app| {
            if isolated_window {
                if let Some((cycles, report)) = benchmark.clone() {
                    run_benchmark(app.handle().clone(), cycles, report);
                } else {
                    show_window(app.handle());
                }
                return Ok(());
            }
            // The tray follows the OS language (see `options::tray_text`).
            let text = crate::options::tray_text(crate::options::os_is_thai());
            let open = MenuItem::with_id(app, "open", text.open, true, None::<&str>)?;
            let separator = PredefinedMenuItem::separator(app)?;
            let quit = MenuItem::with_id(app, "quit", text.quit, true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&open, &separator, &quit])?;
            TrayIconBuilder::new()
                .icon(Image::from_bytes(include_bytes!("../icons/icon.png"))?)
                .tooltip("SSH Control Center")
                .menu(&menu)
                .show_menu_on_left_click(false)
                .on_menu_event(move |app, event| match event.id.as_ref() {
                    "open" => open_management_window(app, isolated_window),
                    "quit" => app.exit(0),
                    _ => {}
                })
                .on_tray_icon_event(move |tray, event| {
                    if matches!(
                        event,
                        TrayIconEvent::Click {
                            button: MouseButton::Left,
                            button_state: MouseButtonState::Up,
                            ..
                        }
                    ) {
                        open_management_window(tray.app_handle(), isolated_window);
                    }
                })
                .build(app)?;
            if let Some((cycles, report)) = benchmark.clone() {
                #[cfg(windows)]
                crate::windows_window::benchmark(cycles, report);
                #[cfg(not(windows))]
                run_benchmark(app.handle().clone(), cycles, report);
            } else if args.iter().any(|arg| arg == "--show") {
                open_management_window(app.handle(), isolated_window);
            }
            Ok(())
        })
        .build(context)
        .expect("could not initialize SSH Control Center tray");
    app.run(move |_, event| {
        // Normal close destroys the window/WebView. With no remaining windows,
        // retain only the native tray. Explicit Quit supplies an exit code.
        if let tauri::RunEvent::ExitRequested {
            code: None, api, ..
        } = event
        {
            if !isolated_window || benchmark_window {
                api.prevent_exit();
            }
        }
    });
}
