//! Logitech device layer (Phase 8).
//!
//! Battery reading uses G Hub WebSocket API (auto-starts G Hub if needed).
pub mod hidpp;
pub mod enumerate;
pub mod battery;
pub mod cache;
pub mod dpi;

use windows_sys::Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_OK, MB_ICONINFORMATION};

/// Read battery via G Hub WebSocket and show a MessageBox summary.
pub fn log_battery_status() {
    match crate::device::battery::read_first_battery() {
        Some(info) => {
            let msg = format!(
                "鼠标电量: {}%{}",
                info.percent,
                if info.charging { "（充电中）" } else { "" }
            );
            show_battery_msg(&msg);
        }
        None => {
            show_battery_msg("无法读取鼠标电量\n请确保 G Hub 已安装并正在运行");
        }
    }
}

fn show_battery_msg(text: &str) {
    use crate::win::tray::to_wide;
    let title = to_wide("鼠标电池状态");
    let msg = to_wide(text);
    unsafe {
        MessageBoxW(0, msg.as_ptr(), title.as_ptr(), MB_OK | MB_ICONINFORMATION);
    }
}
