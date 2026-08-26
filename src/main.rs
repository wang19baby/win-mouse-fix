mod config;
mod log;
mod win;

use std::sync::OnceLock;

use config::Config;

/// Runtime configuration, set once at startup and read by the hook layer.
pub static CONFIG: OnceLock<Config> = OnceLock::new();

fn main() {
    let cfg = Config::load_or_default();
    let _ = CONFIG.set(cfg);
    let cfg = CONFIG.get().expect("config initialized");

    log::init(cfg.general.log_path.as_deref());
    log::write("Win Mouse Fix starting...");

    if let Err(e) = win::tray::create() {
        log::write(&format!("tray init failed: {e}"));
    }
    if let Err(e) = win::hooks::install() {
        log::write(&format!("hook install failed: {e}"));
    }

    // Blocks until WM_QUIT (tray "Exit" or window destroy).
    win::message_loop::run();

    win::hooks::uninstall();
    log::write("Win Mouse Fix exited.");
}
