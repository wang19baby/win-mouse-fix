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

/// Enumerate connected Logitech devices and log each one's battery level.
/// Best-effort: devices that don't respond are logged and skipped.
pub fn log_battery_status() {
    let devices = enumerate::enumerate_logitech();
    if devices.is_empty() {
        crate::log::write("device: no Logitech HID devices found");
        return;
    }
    for d in &devices {
        let line = match battery::read_battery(&d.path) {
            Some(s) => format!(
                "device {} (vid={:04x} pid={:04x}): battery {}%{}",
                d.path,
                d.vid,
                d.pid,
                s.level,
                if s.charging { " charging" } else { "" },
            ),
            None => format!(
                "device {} (vid={:04x} pid={:04x}): battery read failed",
                d.path, d.vid, d.pid,
            ),
        };
        crate::log::write(&line);
    }
}
