#![windows_subsystem = "windows"]
mod config;
mod log;
mod win;
mod scroll;
mod remap;
mod modifiers;
mod gesture;
mod accel;
mod add_mode;
mod device;
mod remote;
mod trackpad_gesture;
mod gui;

use parking_lot::RwLock;
use config::Config;
use std::sync::LazyLock;
use windows_sys::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, PROCESS_PER_MONITOR_DPI_AWARE,
    SetProcessDpiAwareness, SetProcessDpiAwarenessContext,
};

/// Runtime configuration: read by the hook layer, hot-swapped by `apply_config`.
/// `LazyLock` defers the (non-const) `Config::default()` until first access.
pub static CONFIG: LazyLock<RwLock<Config>> = LazyLock::new(|| RwLock::new(Config::default()));

fn main() {
    // Install panic hook to write crash log before aborting.
    std::panic::set_hook(Box::new(|info| {
        let thread = std::thread::current();
        let thread_name = thread.name().unwrap_or("<unnamed>");
        let payload = info.payload();
        let msg = if let Some(s) = payload.downcast_ref::<&str>() {
            s.to_string()
        } else if let Some(s) = payload.downcast_ref::<String>() {
            s.clone()
        } else {
            "Box<dyn Any>".to_string()
        };
        let location = info.location()
            .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()))
            .unwrap_or_else(|| "<unknown>".to_string());
        let backtrace = std::backtrace::Backtrace::force_capture();
        let crash = format!(
            "=== CRASH ===\n\
             thread: {thread_name}\n\
             message: {msg}\n\
             location: {location}\n\
             {backtrace}\n"
        );
        // Write to stderr
        eprintln!("{crash}");
        // Write to crash.log next to the exe
        if let Ok(exe) = std::env::current_exe() {
            if let Some(dir) = exe.parent() {
                let path = dir.join("crash.log");
                let _ = std::fs::write(&path, &crash);
            }
        }
        // Also try to write to the regular log file
        crate::log::write(&format!("CRASH: {msg} at {location}"));
    }));

    // Enable per-monitor DPI awareness before any window/DPI query so density
    // detection (Phase 10) reports real per-monitor DPI instead of the 96 default.
    unsafe {
        if SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) == 0 {
            let _ = SetProcessDpiAwareness(PROCESS_PER_MONITOR_DPI_AWARE);
        }
    }

    let cfg = Config::load_or_default();

    log::init(cfg.general.log_path.as_deref());
    log::write("Win Mouse Fix starting...");

    if let Err(e) = win::tray::create() {
        log::write(&format!("tray init failed: {e}"));
    }

    // Bring up hooks + injector (or remap table) per the loaded config, and
    // reinstall on every later toggle from the tray menu.
    // Phase 11: start the phone-trackpad LAN server only when explicitly enabled
    // (it opens a LAN listener + firewall rule; off by default).
    if cfg.remote.enabled {
        remote::start_server();
    }

    win::hooks::apply_config(cfg.clone());

    // Start background battery poll thread (updates cache every 20s, zero main-thread cost).
    device::battery::start_bg_poll(20);

    // Blocks until WM_QUIT (tray "Exit" or window destroy).
    win::message_loop::run();

    win::hooks::uninstall();
    win::hooks::stop_scroll();
    log::write("Win Mouse Fix exited.");
}
