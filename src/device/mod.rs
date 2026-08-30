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
        let mut found = false;
        for &idx in &[0x01u8, 0xFF] {
            if let Some((level, charging)) = battery::read_battery_hidpp(&d.path, idx) {
                lines.push(format!(
                    "{} (pid={:04X}): {}%{}",
                    d.path, d.pid, level,
                    if charging { " 充电中" } else { "" },
                ));
                crate::log::write(&format!("battery: pid={:04X} idx=0x{:02X} → {}%{}", d.pid, idx, level, if charging { " charging" } else { "" }));
                found = true;
                break;
            }
        }
        if !found {
            lines.push(format!("{} (pid={:04X}): 无法读取", d.path, d.pid));
        }
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
