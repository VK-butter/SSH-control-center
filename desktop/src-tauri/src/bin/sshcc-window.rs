#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

#[path = "../options.rs"]
mod options;
#[path = "../tauri_app.rs"]
mod tauri_app;
#[cfg(windows)]
#[path = "../windows_window.rs"]
mod windows_window;

fn main() {
    tauri_app::run_tauri(true);
}
