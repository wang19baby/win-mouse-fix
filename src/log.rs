use std::fs::OpenOptions;
use std::io::Write;
use std::sync::{Mutex, OnceLock};

static LOG: OnceLock<Mutex<Option<std::fs::File>>> = OnceLock::new();
static LOG_PATH: OnceLock<Mutex<Option<String>>> = OnceLock::new();
static MAX_FILE_SIZE: u64 = 2 * 1024 * 1024; // 2 MB per log file
static MAX_FILES: u32 = 3; // keep up to 3 rotated files

/// Open an optional rolling log file. Safe to call once at startup.
pub fn init(path: Option<&str>) {
    if let Some(p) = path {
        // Rotate if existing file exceeds max size
        if let Ok(meta) = std::fs::metadata(p) {
            if meta.len() > MAX_FILE_SIZE {
                let _ = rotate_log(p);
            }
        }
        let file = OpenOptions::new().create(true).append(true).open(p).ok();
        let _ = LOG.set(Mutex::new(file));
        let _ = LOG_PATH.set(Mutex::new(Some(p.to_string())));
    } else {
        let _ = LOG.set(Mutex::new(None));
        let _ = LOG_PATH.set(Mutex::new(None));
    }
}

fn rotate_log(base: &str) -> std::io::Result<()> {
    let oldest = format!("{base}.{MAX_FILES}");
    let _ = std::fs::remove_file(&oldest);
    for i in (1..MAX_FILES).rev() {
        let src = format!("{base}.{i}");
        let dst = format!("{base}.{}", i + 1);
        let _ = std::fs::rename(&src, &dst);
    }
    std::fs::rename(base, format!("{base}.1"))
}

/// Write a line to stderr and, if initialized, to the log file.
pub fn write(msg: &str) {
    let line = format!("[{}] {msg}", now_str());
    eprintln!("{line}");
    write_to_file(&line);
}

/// Write only to log file (no stderr). Useful for high-frequency debug messages.
#[allow(dead_code)]
pub fn file_only(msg: &str) {
    let line = format!("[{}] {msg}", now_str());
    write_to_file(&line);
}

fn write_to_file(line: &str) {
    if let Some(g) = LOG.get() {
        if let Ok(mut g) = g.lock() {
            if let Some(f) = g.as_mut() {
                // Check if rotation needed
                let need_rotate = f.metadata().map(|m| m.len() > MAX_FILE_SIZE).unwrap_or(false);
                if need_rotate {
                    if let Some(path_lock) = LOG_PATH.get() {
                        if let Ok(path_guard) = path_lock.lock() {
                            if let Some(ref path) = *path_guard {
                                let _ = rotate_log(path);
                                if let Ok(new_f) = OpenOptions::new().create(true).append(true).open(path) {
                                    *f = new_f;
                                }
                            }
                        }
                    }
                }
                let _ = writeln!(f, "{line}");
                let _ = f.flush();
            }
        }
    }
}

/// Human-readable timestamp: "2026-08-30 14:32:05"
fn now_str() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let days = (now / 86400) as i64;
    let secs = (now % 86400) as u32;
    let h = secs / 3600;
    let m = (secs % 3600) / 60;
    let s = secs % 60;
    let z = days + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = (z - era * 146096) as u32;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m_idx = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m_idx <= 2 { y + 1 } else { y };
    format!("{y:04}-{m_idx:02}-{d:02} {h:02}:{m:02}:{s:02}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn now_str_format() {
        let s = now_str();
        assert_eq!(s.len(), 19);
        assert_eq!(&s[4..5], "-");
        assert_eq!(&s[7..8], "-");
        assert_eq!(&s[10..11], " ");
        assert_eq!(&s[13..14], ":");
        assert_eq!(&s[16..17], ":");
    }

    #[test]
    fn rotate_log_removes_oldest() {
        let dir = std::env::temp_dir().join("win_mouse_fix_log_test");
        let _ = std::fs::create_dir_all(&dir);
        let base = dir.join("test.log");
        std::fs::write(&base, "base\n").unwrap();
        std::fs::write(base.with_extension("log.1"), "v1\n").unwrap();
        std::fs::write(base.with_extension("log.2"), "v2\n").unwrap();
        std::fs::write(base.with_extension("log.3"), "v3_oldest\n").unwrap();

        rotate_log(base.to_str().unwrap()).unwrap();

        // .log.3 (oldest) should be gone, replaced by renamed .log.2
        assert!(base.with_extension("log.3").exists());
        assert_eq!(std::fs::read_to_string(base.with_extension("log.3")).unwrap(), "v2\n");
        assert_eq!(std::fs::read_to_string(base.with_extension("log.2")).unwrap(), "v1\n");
        assert_eq!(std::fs::read_to_string(base.with_extension("log.1")).unwrap(), "base\n");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
