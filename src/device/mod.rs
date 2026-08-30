//! Logitech HID++ device layer (Phase 8).
//!
//! Minimal closed loop: enumerate Logitech HID devices → resolve the battery
//! feature index via the ROOT feature → read battery level. Live I/O requires
//! a connected device; the protocol module (`hidpp`) is unit-tested headlessly.
pub mod hidpp;
pub mod enumerate;
pub mod battery;
pub mod cache;
pub mod dpi;

use windows_sys::Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_OK, MB_ICONINFORMATION};

/// Enumerate connected Logitech devices and log each one's battery level.
/// Also shows a MessageBox with a summary for user-visible feedback.
pub fn log_battery_status() {
    let devices = enumerate::enumerate_logitech();
    if devices.is_empty() {
        crate::log::write("battery: no Logitech HID devices found");
        show_battery_msg("未检测到罗技设备");
        return;
    }
    let mut lines = Vec::new();
    for d in &devices {
        // Try both device indices for robustness.
        let result = battery::read_battery(&d.path)
            .or_else(|| battery::read_battery_with_index(&d.path, 0xFF));
        let line = match result {
            Some(s) if !s.level_invalid => {
                format!(
                    "{} (pid={:04X}): {}%{}",
                    d.path, d.pid, s.level,
                    if s.charging { " 充电中" } else { "" },
                )
            }
            _ => format!("{} (pid={:04X}): 无法读取", d.path, d.pid),
        };
        crate::log::write(&format!("battery: {line}"));
        lines.push(line);
    }
    show_battery_msg(&lines.join("\n"));
}

fn show_battery_msg(text: &str) {
    use crate::win::tray::to_wide;
    let title = to_wide("鼠标电池状态");
    let msg = to_wide(text);
    unsafe {
        MessageBoxW(0, msg.as_ptr(), title.as_ptr(), MB_OK | MB_ICONINFORMATION);
    }
}
