use std::fs::OpenOptions;
use std::io::Write;
use std::sync::{Mutex, OnceLock};

static LOG: OnceLock<Mutex<Option<std::fs::File>>> = OnceLock::new();

/// Open an optional rolling log file. Safe to call once at startup.
pub fn init(path: Option<&str>) {
    let file = path.and_then(|p| OpenOptions::new().create(true).append(true).open(p).ok());
    let _ = LOG.set(Mutex::new(file));
}

/// Write a line to stderr and, if initialized, to the log file.
pub fn write(msg: &str) {
    let line = format!("[{}] {msg}", now_secs());
    eprintln!("{line}");
    if let Some(g) = LOG.get() {
        if let Ok(mut g) = g.lock() {

            if let Some(f) = g.as_mut() {
                let _ = writeln!(f, "{line}");
                let _ = f.flush();
            }
        }
    }
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}
