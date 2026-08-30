//! Read battery level from a Logitech HID++ device.
use std::ptr::null_mut;
use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, OPEN_EXISTING, FILE_SHARE_READ, FILE_SHARE_WRITE, ReadFile, WriteFile,
};
use crate::device::cache::BatteryInfo;
use crate::device::enumerate;
use crate::device::hidpp::{self, feature, BatteryStatus};

/// Open a HID device by path and read its battery level via HID++ 2.0.
/// Returns `None` if the device can't be opened or doesn't respond.
///
/// Note: this targets the first paired device (device index `0x01`). A robust
/// implementation would walk device indices / the receiver (0xFF); that comes
/// with the device-layer maturity work.
pub fn read_battery(path: &str) -> Option<BatteryStatus> {
    read_battery_with_index(path, 0x01)
}

unsafe fn read_battery_handle(handle: isize, device_index: u8) -> Option<BatteryStatus> {
    // Step 1: resolve the battery feature index via the ROOT feature.
    let req = hidpp::get_feature_index_request(device_index, feature::BATTERY);
    let mut out = [0u8; 7];
    if !write_report(handle, &req.encode(), &mut out) {
        return None;
    }
    let resp = hidpp::ShortMessage::decode(&out)?;
    let idx = hidpp::feature_index_from_response(&resp)?;

    // Step 2: read battery level status.
    let req2 = hidpp::battery_level_request(device_index, idx);
    if !write_report(handle, &req2.encode(), &mut out) {
        return None;
    }
    let resp2 = hidpp::ShortMessage::decode(&out)?;
    hidpp::battery_status_from_response(&resp2)
}

/// Enumerate Logitech devices and return the first readable battery level.
/// Used by the tray poll to refresh the global cache.
///
/// Tries multiple device indices (0x01 for direct, 0xFF for receiver-routed)
/// to handle both direct connections and BOLT/Unifying receivers.
pub fn read_first_battery() -> Option<BatteryInfo> {
    let devices = enumerate::enumerate_logitech();
    if devices.is_empty() {
        crate::log::write("battery: no Logitech HID devices found");
        return None;
    }
    crate::log::write(&format!("battery: found {} Logitech device(s)", devices.len()));

    for d in &devices {
        crate::log::write(&format!(
            "battery: trying {} (vid={:04x} pid={:04x})",
            d.path, d.vid, d.pid
        ));
        // Try device index 0x01 (direct) first.
        if let Some(s) = read_battery_with_index(&d.path, 0x01) {
            if !s.level_invalid {
                crate::log::write(&format!("battery: OK via idx 0x01 → {}%{}", s.level, if s.charging { " charging" } else { "" }));
                return Some(BatteryInfo::from_level(s.level, s.charging));
            }
        }
        // Try device index 0xFF (receiver-routed, used by BOLT/Unifying).
        if let Some(s) = read_battery_with_index(&d.path, 0xFF) {
            if !s.level_invalid {
                crate::log::write(&format!("battery: OK via idx 0xFF → {}%{}", s.level, if s.charging { " charging" } else { "" }));
                return Some(BatteryInfo::from_level(s.level, s.charging));
            }
        }
    }
    crate::log::write("battery: no device responded with a valid battery level");
    None
}

/// Read battery from a specific device path with a given device index.
pub fn read_battery_with_index(path: &str, device_index: u8) -> Option<BatteryStatus> {
    unsafe {
        let wide: Vec<u16> = path.encode_utf16().chain(std::iter::once(0)).collect();
        let handle = CreateFileW(
            wide.as_ptr(),
            0,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            null_mut(),
            OPEN_EXISTING,
            0,
            0,
        );
        if handle == INVALID_HANDLE_VALUE {
            return None;
        }
        let result = read_battery_handle(handle, device_index);
        CloseHandle(handle);
        result
    }
}

/// Write a short HID++ report and read the echoed response (same length).
unsafe fn write_report(handle: isize, send: &[u8], recv: &mut [u8]) -> bool {
    let mut written: u32 = 0;
    if WriteFile(
        handle,
        send.as_ptr() as *const _,
        send.len() as u32,
        &mut written,
        null_mut(),
    ) == 0
    {
        return false;
    }
    let mut read: u32 = 0;
    if ReadFile(
        handle,
        recv.as_mut_ptr() as *mut _,
        recv.len() as u32,
        &mut read,
        null_mut(),
    ) == 0
    {
        return false;
    }
    read == recv.len() as u32
}
