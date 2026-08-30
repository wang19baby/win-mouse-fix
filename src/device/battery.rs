//! Read battery level from a Logitech HID++ device.
//!
//! Uses HidD_SetOutputReport + HidD_GetInputReport (interrupt endpoint)
//! which works with BOLT/LIGHTSPEED receivers. WriteFile/ReadFile does NOT
//! work for HID++ on these receivers.
use std::ptr::null_mut;
use windows_sys::Win32::Devices::HumanInterfaceDevice::{
    HidD_GetInputReport, HidD_SetOutputReport,
};
use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
};
use crate::device::cache::BatteryInfo;
use crate::device::enumerate;

const GENERIC_READ: u32 = 0x80000000;
const GENERIC_WRITE: u32 = 0x40000000;

/// Read battery from a device path using HID++ interrupt endpoints.
pub fn read_battery_hidpp(path: &str, device_index: u8) -> Option<(u8, bool)> {
    unsafe {
        let wide: Vec<u16> = path.encode_utf16().chain(std::iter::once(0)).collect();
        let handle = CreateFileW(
            wide.as_ptr(),
            GENERIC_READ | GENERIC_WRITE,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            null_mut(),
            OPEN_EXISTING,
            0,
            0,
        );
        if handle == INVALID_HANDLE_VALUE {
            return None;
        }
        let result = read_battery_handle_hidpp(handle, device_index);
        CloseHandle(handle);
        result
    }
}

/// HID++ 2.0 via interrupt endpoints: GetFeature for BATTERY, then read level.
unsafe fn read_battery_handle_hidpp(handle: isize, device_index: u8) -> Option<(u8, bool)> {
    // Step 1: resolve BATTERY feature index via ROOT (0x0000).
    let mut out = [0u8; 7];
    out[0] = 0x10; // report ID (short)
    out[1] = device_index;
    out[2] = 0x00; // ROOT feature high byte
    out[3] = 0x05; // fn=0, sw_id=5
    out[4] = 0x10; // BATTERY high byte
    out[5] = 0x00; // BATTERY low byte

    if HidD_SetOutputReport(handle, out.as_mut_ptr() as _, 7) == 0 {
        return None;
    }
    let mut resp = [0u8; 32];
    resp[0] = 0x10;
    if HidD_GetInputReport(handle, resp.as_mut_ptr() as _, 32) == 0 {
        return None;
    }

    let battery_feature_idx = resp[4]; // param[0]
    if battery_feature_idx == 0 {
        return None;
    }

    // Step 2: read battery level using resolved feature index.
    let mut out2 = [0u8; 7];
    out2[0] = 0x10;
    out2[1] = device_index;
    out2[2] = battery_feature_idx;
    out2[3] = 0x05;

    if HidD_SetOutputReport(handle, out2.as_mut_ptr() as _, 7) == 0 {
        return None;
    }
    let mut resp2 = [0u8; 32];
    resp2[0] = 0x10;
    if HidD_GetInputReport(handle, resp2.as_mut_ptr() as _, 32) == 0 {
        return None;
    }

    let level = resp2[4];
    let charging = (resp2[5] & 0x01) != 0;
    if level == 0xFF { None } else { Some((level, charging)) }
}

/// Enumerate Logitech devices and return the first readable battery level.
/// Used by the tray poll to refresh the global cache.
///
/// Tries device indices 0x01 (direct) and 0xFF (receiver-routed) on each
/// enumerated HID path. The BOLT/LIGHTSPEED receiver's HID++ interface is
/// typically on the mi_02&col01 collection.
pub fn read_first_battery() -> Option<BatteryInfo> {
    let devices = enumerate::enumerate_logitech();
    if devices.is_empty() {
        crate::log::write("battery: no Logitech HID devices found");
        return None;
    }
    crate::log::write(&format!("battery: found {} Logitech device(s)", devices.len()));

    for d in &devices {
        for &idx in &[0x01u8, 0xFF] {
            if let Some((level, charging)) = read_battery_hidpp(&d.path, idx) {
                crate::log::write(&format!(
                    "battery: OK pid={:04X} idx=0x{:02X} → {}%{}",
                    d.pid, idx, level, if charging { " charging" } else { "" }
                ));
                return Some(BatteryInfo::from_level(level, charging));
            }
        }
    }
    crate::log::write("battery: no device responded with a valid battery level");
    None
}
