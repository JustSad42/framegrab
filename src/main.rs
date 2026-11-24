#![windows_subsystem = "windows"]
//! Framegrab: a fast screenshot, text-grab, scrolling-capture and screen-recording tool.

mod app;
mod autostart;
mod bar;
mod capture;
mod config;
mod dxgi;
mod dragout;
mod editor;
mod feedback;
mod gfx;
mod hook;
mod ocr;
mod output;
mod overlay;
mod process;
mod upload;
mod record;
mod scroll;
mod settings;
mod toast;
mod translate;
mod ui;
mod updater;
mod winfo;
mod win;

use windows::Win32::Foundation::{ERROR_ALREADY_EXISTS, GetLastError};
use windows::Win32::System::Threading::CreateMutexW;
use windows_core::PCWSTR;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let has = |f: &str| args.iter().any(|a| a == f);
    let after = |f: &str| args.iter().position(|a| a == f).and_then(|i| args.get(i + 1)).cloned();

    if let Some(p) = after("--edit") {
        editor::run(std::path::PathBuf::from(p), after("--out").map(std::path::PathBuf::from));
        return;
    }
    if has("--settings") {
        settings::run();
        return;
    }
    if let Some(i) = args.iter().position(|a| a == "--trigger") {
        let cmd = args[i + 1..].join(" ");
        std::process::exit(if app::send_to_running(&cmd) { 0 } else { 1 });
    }

    let name = win::wide(if config::test_mode() { "Local\\Framegrab-test" } else { "Local\\Framegrab" });
    let _mutex = unsafe { CreateMutexW(None, false, PCWSTR(name.as_ptr())) };
    if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
        // Already running: a plain launch opens Settings, background launches do nothing.
        if !has("--background") {
            app::spawn_self(&["--settings"]);
        }
        return;
    }
    updater::cleanup();
    app::run(has("--background"));
}
