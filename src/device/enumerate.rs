//! Enumerate Logitech HID++ devices via Win32 SetupAPI + HID.
//!
//! Enumerates across three Device Interface GUIDs that Logitech USB receivers
//! register under on Windows (the standard HID GUID often has no entries):
//!   1. `GUID_DEVINTERFACE_HID`  — standard HID device interface
//!   2. Keyboard Device Interface — `{4d1e55b2-f16f-11cf-88cb-001111000030}`
//!   3. Mouse Device Interface    — `{745a17a0-74d3-11d0-b6fe-00a0c90f57da}`
//!
//! Filters by Logitech's USB vendor id (0x046D). Each entry carries the device
//! path (for `CreateFileW`), vid/pid, and serial string.

use std::ptr::{null, null_mut};
use windows_sys::Win32::Devices::DeviceAndDriverInstallation::{
    SetupDiDestroyDeviceInfoList, SetupDiEnumDeviceInterfaces, SetupDiGetClassDevsW,
    SetupDiGetDeviceInterfaceDetailW, DIGCF_DEVICEINTERFACE, DIGCF_PRESENT,
    SP_DEVICE_INTERFACE_DATA, SP_DEVICE_INTERFACE_DETAIL_DATA_W,
};
use windows_sys::Win32::Devices::HumanInterfaceDevice::{
    GUID_DEVINTERFACE_HID, HIDD_ATTRIBUTES, HidD_GetAttributes, HidD_GetSerialNumberString,
};
use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
};
use windows_sys::core::GUID;

const LOGITECH_VID: u16 = 0x046D;

/// Keyboard Device Interface GUID — `{4d1e55b2-f16f-11cf-88cb-001111000030}`.
const GUID_DEVINTERFACE_KEYBOARD: GUID = GUID {
    data1: 0x4D1E55B2,
    data2: 0xF16F,
    data3: 0x11CF,
    data4: [0x88, 0xCB, 0x00, 0x11, 0x11, 0x00, 0x03, 0x00],
};

/// Mouse Device Interface GUID — `{745a17a0-74d3-11d0-b6fe-00a0c90f57da}`.
const GUID_DEVINTERFACE_MOUSE: GUID = GUID {
    data1: 0x745A17A0,
    data2: 0x74D3,
    data3: 0x11D0,
    data4: [0xB6, 0xFE, 0x00, 0xA0, 0xC9, 0x0F, 0x57, 0xDA],
};

/// A discovered Logitech HID device.
#[derive(Debug, Clone)]
pub struct HidDeviceInfo {
    pub path: String,
    pub vid: u16,
    pub pid: u16,
    pub serial: Option<String>,
}

/// Enumerate currently-present Logitech HID devices.
///
/// Tries three Device Interface GUIDs sequentially; results are deduplicated
/// by device path so the same physical device does not appear twice.
pub fn enumerate_logitech() -> Vec<HidDeviceInfo> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();

    for guid in [
        &GUID_DEVINTERFACE_HID,
        &GUID_DEVINTERFACE_KEYBOARD,
        &GUID_DEVINTERFACE_MOUSE,
    ] {
        unsafe {
            for info in enumerate_one_guid(guid) {
                if seen.insert(info.path.clone()) {
                    out.push(info);
                }
            }
        }
    }

    out
}

unsafe fn enumerate_one_guid(guid: &GUID) -> Vec<HidDeviceInfo> {
    let dev_info = SetupDiGetClassDevsW(
        guid,
        null(),
        0,
        DIGCF_PRESENT | DIGCF_DEVICEINTERFACE,
    );
    if dev_info == INVALID_HANDLE_VALUE {
        return Vec::new();
    }

    let mut out = Vec::new();
    let mut index = 0u32;

    loop {
        let mut iface: SP_DEVICE_INTERFACE_DATA = std::mem::zeroed();
        iface.cbSize = std::mem::size_of::<SP_DEVICE_INTERFACE_DATA>() as u32;
        if SetupDiEnumDeviceInterfaces(dev_info, null(), guid, index, &mut iface) == 0 {
            break;
        }
        index += 1;

        let mut needed: u32 = 0;
        SetupDiGetDeviceInterfaceDetailW(
            dev_info,
            &iface,
            null_mut(),
            0,
            &mut needed,
            null_mut(),
        );
        if needed == 0 {
            continue;
        }

        let mut buf: Vec<u8> = vec![0u8; needed as usize];
        let detail = buf.as_mut_ptr() as *mut SP_DEVICE_INTERFACE_DETAIL_DATA_W;
        (*detail).cbSize = std::mem::size_of::<SP_DEVICE_INTERFACE_DETAIL_DATA_W>() as u32;

        if SetupDiGetDeviceInterfaceDetailW(
            dev_info,
            &iface,
            detail,
            needed,
            null_mut(),
            null_mut(),
        ) == 0
        {
            continue;
        }

        let path = wide_to_string((*detail).DevicePath.as_ptr());

        if let Some(mut info) = open_and_describe(&path) {
            if info.vid == LOGITECH_VID {
                info.path = path;
                out.push(info);
            }
        }
    }

    SetupDiDestroyDeviceInfoList(dev_info);
    out
}

unsafe fn wide_to_string(p: *const u16) -> String {
    if p.is_null() {
        return String::new();
    }
    let mut len = 0usize;
    while *p.add(len) != 0 {
        len += 1;
    }
    String::from_utf16_lossy(std::slice::from_raw_parts(p, len))
}

unsafe fn open_and_describe(path: &str) -> Option<HidDeviceInfo> {
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

    let mut attr: HIDD_ATTRIBUTES = std::mem::zeroed();
    attr.Size = std::mem::size_of::<HIDD_ATTRIBUTES>() as u32;
    if HidD_GetAttributes(handle, &mut attr) == 0 {
        CloseHandle(handle);
        return None;
    }

    let serial = {
        let mut buf: Vec<u16> = vec![0u16; 128];
        if HidD_GetSerialNumberString(
            handle,
            buf.as_mut_ptr() as *mut std::ffi::c_void,
            (buf.len() * 2) as u32,
        ) != 0
        {
            Some(wide_to_string(buf.as_ptr()))
        } else {
            None
        }
    };

    CloseHandle(handle);

    Some(HidDeviceInfo {
        path: String::new(),
        vid: attr.VendorID,
        pid: attr.ProductID,
        serial,
    })
}
