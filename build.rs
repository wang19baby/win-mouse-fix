//! Compile `resources.rc` (which embeds assets/icon.ico) into a PE resource
//! and link it into the exe, so the binary carries a real file icon in
//! Explorer / the taskbar. Uses the Windows SDK `rc.exe` (not on PATH, so we
//! search the SDK install dirs). No-op on non-Windows targets.

use std::env;
use std::path::PathBuf;
use std::process::Command;

fn main() {
    let is_windows = env::var("CARGO_CFG_WINDOWS").is_ok()
        || env::var("TARGET").map(|t| t.contains("windows")).unwrap_or(false);
    if !is_windows {
        return;
    }

    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let rc_file = manifest_dir.join("resources.rc");
    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    let res_file = out_dir.join("win-mouse-fix.res");

    match find_rc() {
        Some(rc_exe) => {
            let status = Command::new(&rc_exe)
                .arg("/fo")
                .arg(&res_file)
                .arg(&rc_file)
                .status();
            match status {
                Ok(s) if s.success() => {
                    println!("cargo:rustc-link-arg={}", res_file.display());
                }
                Ok(s) => {
                    println!("cargo:warning=rc.exe exited with {s}; exe will have no file icon");
                }
                Err(e) => {
                    println!("cargo:warning=failed to run rc.exe: {e}");
                }
            }
        }
        None => {
            println!("cargo:warning=rc.exe not found; exe will have no file icon");
        }
    }

    println!("cargo:rerun-if-changed=resources.rc");
    println!("cargo:rerun-if-changed=assets/icon.ico");
}

/// Locate rc.exe: first on PATH, then under the Windows SDK bin dirs.
fn find_rc() -> Option<PathBuf> {
    if let Ok(path) = env::var("PATH") {
        for dir in env::split_paths(&path) {
            let candidate = dir.join("rc.exe");
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }

    let pf_x86 = env::var("ProgramFiles(x86)")
        .unwrap_or_else(|_| r"C:\Program Files (x86)".to_string());
    let sdk_bin = PathBuf::from(pf_x86)
        .join("Windows Kits")
        .join("10")
        .join("bin");

    if let Ok(entries) = std::fs::read_dir(&sdk_bin) {
        let mut found: Vec<PathBuf> = Vec::new();
        for entry in entries.flatten() {
            let candidate = entry.path().join("x64").join("rc.exe");
            if candidate.is_file() {
                found.push(candidate);
            }
        }
        found.sort();
        if let Some(last) = found.into_iter().last() {
            return Some(last);
        }
    }
    None
}
