#![windows_subsystem = "windows"]
mod accel;
mod add_mode;
mod config;
mod device;
mod gesture;
mod gui;
mod log;
mod modifiers;
mod remap;
mod remote;
mod scroll;
#[cfg(test)]
mod trackpad_gesture;
mod win;

use config::Config;
use parking_lot::RwLock;
use std::sync::LazyLock;
use windows_sys::Win32::UI::HiDpi::{
    SetProcessDpiAwareness, SetProcessDpiAwarenessContext,
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, PROCESS_PER_MONITOR_DPI_AWARE,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_ICONERROR, MB_OK};

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
        let location = info
            .location()
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

    log::init(
        cfg.general
            .log_path
            .as_deref()
            .filter(|path| !path.trim().is_empty()),
    );
    log::write("Win Mouse Fix starting...");

    if let Err(e) = win::tray::create() {
        log::write(&format!("tray init failed: {e}"));
        let message = win::tray::to_wide(&format!(
            "Win Mouse Fix 无法创建系统托盘窗口,程序将退出。\n\n{e}"
        ));
        let title = win::tray::to_wide("Win Mouse Fix 启动失败");
        unsafe {
            MessageBoxW(0, message.as_ptr(), title.as_ptr(), MB_OK | MB_ICONERROR);
        }
        return;
    }

    // Bring up hooks + injector (or remap table) per the loaded config, and
    // reinstall on every later toggle from the tray menu.
    if let Err(e) = win::hooks::apply_config(cfg.clone()) {
        log::write(&format!("hook initialization failed: {e}"));
        let message = win::tray::to_wide(&format!(
            "Win Mouse Fix 无法安装全局输入钩子,程序将退出。\n\n{e}"
        ));
        let title = win::tray::to_wide("Win Mouse Fix 启动失败");
        unsafe {
            MessageBoxW(0, message.as_ptr(), title.as_ptr(), MB_OK | MB_ICONERROR);
        }
        return;
    }

    // Phase 11: start the phone-trackpad LAN server only when explicitly enabled
    // (it opens a LAN listener + firewall rule; off by default).
    if cfg.remote.enabled {
        if let Err(e) = remote::start_server() {
            log::write(&format!("remote: startup failed: {e}"));
            let message = win::tray::to_wide(&format!(
                "手机妙控板服务启动失败,本地鼠标功能仍可使用。\n\n{e}"
            ));
            let title = win::tray::to_wide("Win Mouse Fix");
            unsafe {
                MessageBoxW(0, message.as_ptr(), title.as_ptr(), MB_OK | MB_ICONERROR);
            }
        }
    }

    // Populate the battery cache off-thread immediately, then refresh every 20s.
    device::battery::start_bg_poll(20);

    // Blocks until WM_QUIT (tray "Exit" or window destroy).
    win::message_loop::run();

    if remote::info().is_some() {
        remote::stop_server();
    }
    win::hooks::uninstall();
    win::hooks::stop_scroll();
    log::write("Win Mouse Fix exited.");
}
