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
    // Blocks until WM_QUIT (tray "Exit" or window destroy).
    win::message_loop::run();

    win::hooks::uninstall();
    win::hooks::stop_scroll();
    log::write("Win Mouse Fix exited.");
}
