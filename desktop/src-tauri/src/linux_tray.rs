//! Linux keeps GTK/AppIndicator resident and creates Wry/WebKit only in a child.
//! WebKit retains a network process after destroying a WebView; process isolation
//! releases that state when the management window closes.
use gtk::prelude::*;
use serde_json::json;
use std::{
    cell::{Cell, RefCell},
    path::PathBuf,
    process::Command,
    rc::Rc,
    time::{SystemTime, UNIX_EPOCH},
};
use tray_icon::{
    menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem},
    Icon, TrayIconBuilder,
};

fn child_command() -> std::io::Result<Command> {
    let mut command = Command::new(std::env::current_exe()?.with_file_name("sshcc-window"));
    command.arg("--management-window");
    Ok(command)
}

fn open_window() {
    // The child uses Tauri's single-instance plugin to focus any existing window.
    // Reap every invocation, including the short-lived duplicate activation.
    std::thread::spawn(|| {
        let result = child_command().and_then(|mut command| command.spawn()?.wait());
        if let Err(error) = result {
            eprintln!("cannot open management window: {error}");
        }
    });
}

fn benchmark(cycles: u8, report: PathBuf) {
    std::thread::spawn(move || {
        let mut completed = 0_u8;
        let cycle_report = report.with_extension(format!("cycle-{}.json", std::process::id()));
        let outcome = (|| -> Result<(), String> {
            if cycle_report.exists() {
                return Err(
                    "temporary cycle report already exists; choose a fresh report path".into(),
                );
            }
            for _ in 0..cycles {
                let status = child_command()
                    .map_err(|error| error.to_string())?
                    .arg("--benchmark-cycles")
                    .arg("1")
                    .arg("--benchmark-report")
                    .arg(&cycle_report)
                    .status()
                    .map_err(|error| error.to_string())?;
                let document: serde_json::Value = serde_json::from_slice(
                    &std::fs::read(&cycle_report).map_err(|error| error.to_string())?,
                )
                .map_err(|error| error.to_string())?;
                // status() waits for the native window process to exit. The
                // collector independently retains every observed descendant.
                if !status.success()
                    || document["ok"] != true
                    || document["windowAbsent"] != true
                    || document["cyclesCompleted"] != 1
                {
                    return Err(format!(
                        "management child failed lifecycle validation: {document}"
                    ));
                }
                std::fs::remove_file(&cycle_report).map_err(|error| error.to_string())?;
                completed += 1;
            }
            Ok(())
        })();
        let document = json!({
            "schemaVersion": 1,
            "pid": std::process::id(),
            "ok": outcome.is_ok(),
            "cyclesRequested": cycles,
            "cyclesCompleted": completed,
            "windowAbsent": outcome.is_ok(),
            "windowProcessIsolation": true,
            "completedAtUnixSeconds": SystemTime::now().duration_since(UNIX_EPOCH)
                .map(|time| time.as_secs()).unwrap_or(0),
            "error": outcome.err()
        });
        match serde_json::to_vec_pretty(&document) {
            Ok(bytes) => {
                if let Err(error) = std::fs::write(&report, bytes) {
                    eprintln!("cannot save benchmark report {}: {error}", report.display());
                }
            }
            Err(error) => eprintln!("cannot serialize benchmark report: {error}"),
        }
    });
}

pub fn run() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let options = match crate::options::benchmark_options(&args) {
        Ok(options) => options,
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(2);
        }
    };
    let show = args.iter().any(|arg| arg == "--show");
    let application = gtk::Application::new(
        Some("dev.sshcc.control-center.tray"),
        gtk::gio::ApplicationFlags::empty(),
    );
    application.connect_startup(|app| {
        // Hold the tray application even when there are no GTK windows.
        std::mem::forget(app.hold());
    });
    let first_activation = Rc::new(Cell::new(true));
    let tray = Rc::new(RefCell::new(None));
    let resident_tray = tray.clone();
    application.connect_activate(move |app| {
        if !first_activation.replace(false) {
            open_window();
            return;
        }
        let setup = (|| -> Result<_, Box<dyn std::error::Error>> {
            let open = MenuItem::new("เปิด SSH Control Center", true, None);
            let separator = PredefinedMenuItem::separator();
            let quit = MenuItem::new("ปิดไอคอน Tray (Service ยังทำงานต่อ)", true, None);
            let open_id = open.id().clone();
            let quit_id = quit.id().clone();
            let menu = Menu::new();
            menu.append_items(&[&open, &separator, &quit])?;
            MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
                if event.id == open_id {
                    open_window();
                } else if event.id == quit_id {
                    gtk::glib::idle_add_once(|| {
                        if let Some(app) = gtk::gio::Application::default() {
                            app.quit();
                        }
                    });
                }
            }));
            let icon = Icon::from_rgba(include_bytes!("../icons/icon.rgba").to_vec(), 32, 32)?;
            Ok(TrayIconBuilder::new()
                .with_icon(icon)
                .with_tooltip("SSH Control Center")
                .with_menu(Box::new(menu))
                .with_menu_on_left_click(true)
                .build()?)
        })();
        match setup {
            Ok(icon) => *resident_tray.borrow_mut() = Some(icon),
            Err(error) => {
                eprintln!("cannot initialize SSH Control Center tray: {error}");
                app.quit();
                return;
            }
        }
        if let Some((cycles, report)) = options.clone() {
            benchmark(cycles, report);
        } else if show {
            open_window();
        }
    });
    // Custom flags are handled above; GApplication only receives its program
    // name. A second invocation activates this user's existing tray over DBus.
    application.run_with_args(&["sshcc-desktop"]);
    drop(tray);
}
