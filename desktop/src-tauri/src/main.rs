#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod options;
#[cfg(windows)]
mod windows_window;

#[cfg(target_os = "linux")]
mod linux_tray;
#[cfg(not(target_os = "linux"))]
mod tauri_app;

fn main() {
    #[cfg(target_os = "linux")]
    linux_tray::run();
    #[cfg(not(target_os = "linux"))]
    tauri_app::run_tauri(false);
}
