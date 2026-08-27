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
        let result = read_battery_handle(handle);
        CloseHandle(handle);
        result
    }
}

unsafe fn read_battery_handle(handle: isize) -> Option<BatteryStatus> {
    // Step 1: resolve the battery feature index via the ROOT feature.
    let req = hidpp::get_feature_index_request(0x01, feature::BATTERY);
    let mut out = [0u8; 7];
    if !write_report(handle, &req.encode(), &mut out) {
        return None;
    }
    let resp = hidpp::ShortMessage::decode(&out)?;
    let idx = hidpp::feature_index_from_response(&resp)?;

    // Step 2: read battery level status.
    let req2 = hidpp::battery_level_request(0x01, idx);
    if !write_report(handle, &req2.encode(), &mut out) {
        return None;
    }
    let resp2 = hidpp::ShortMessage::decode(&out)?;
    hidpp::battery_status_from_response(&resp2)
}

/// Enumerate Logitech devices and return the first readable battery level.
/// Used by the tray poll to refresh the global cache.
pub fn read_first_battery() -> Option<BatteryInfo> {
    for d in enumerate::enumerate_logitech() {
        if let Some(s) = read_battery(&d.path) {
            if !s.level_invalid {
                return Some(BatteryInfo::from_level(s.level, s.charging));
            }
        }
    }
    None
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
