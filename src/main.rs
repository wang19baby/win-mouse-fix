mod config;
mod log;
mod win;
mod scroll;
mod remap;
mod modifiers;
mod gesture;

use parking_lot::RwLock;
use config::Config;
use std::sync::LazyLock;

/// Runtime configuration: read by the hook layer, hot-swapped by `apply_config`.
/// `LazyLock` defers the (non-const) `Config::default()` until first access.
pub static CONFIG: LazyLock<RwLock<Config>> = LazyLock::new(|| RwLock::new(Config::default()));

fn main() {
    let cfg = Config::load_or_default();

    log::init(cfg.general.log_path.as_deref());
    log::write("Win Mouse Fix starting...");

    if let Err(e) = win::tray::create() {
        log::write(&format!("tray init failed: {e}"));
    }

    // Bring up hooks + injector (or remap table) per the loaded config, and
    // reinstall on every later toggle from the tray menu.
    win::hooks::apply_config(cfg);

    // Blocks until WM_QUIT (tray "Exit" or window destroy).
    win::message_loop::run();

    win::hooks::uninstall();
    win::hooks::stop_scroll();
    log::write("Win Mouse Fix exited.");
}
