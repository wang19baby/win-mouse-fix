//! Enumerate Logitech HID++ devices via Win32 SetupAPI + HID.
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
    GUID_DEVINTERFACE_HID, HIDD_ATTRIBUTES,     HidD_GetAttributes, HidD_GetSerialNumberString,
};
use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
};

const LOGITECH_VID: u16 = 0x046D;

/// A discovered Logitech HID device.
#[derive(Debug, Clone)]
pub struct HidDeviceInfo {
    pub path: String,
    pub vid: u16,
    pub pid: u16,
    pub serial: Option<String>,
}

/// Enumerate currently-present Logitech HID devices.
pub fn enumerate_logitech() -> Vec<HidDeviceInfo> {
    unsafe {
        let guid = GUID_DEVINTERFACE_HID;
        let dev_info = SetupDiGetClassDevsW(
            &guid,
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
            if SetupDiEnumDeviceInterfaces(dev_info, null(), &guid, index, &mut iface) == 0 {
                break;
            }
            index += 1;

            // Probe required detail size, then fetch the device path.
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
                    // Keep only the Logitech HID++-capable entries we can open.
                    info.path = path;
                    out.push(info);
                }
            }
        }
        SetupDiDestroyDeviceInfoList(dev_info);
        out
    }
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
    let mut info = HidDeviceInfo {
        path: String::new(),
        vid: 0,
        pid: 0,
        serial: None,
    };
    let mut attr: HIDD_ATTRIBUTES = std::mem::zeroed();
    attr.Size = std::mem::size_of::<HIDD_ATTRIBUTES>() as u32;
    if HidD_GetAttributes(handle, &mut attr) != 0 {
        info.vid = attr.VendorID;
        info.pid = attr.ProductID;
    }
    let mut serial: Vec<u16> = vec![0u16; 128];
    if HidD_GetSerialNumberString(
        handle,
        serial.as_mut_ptr() as *mut std::ffi::c_void,
        (serial.len() * 2) as u32,
    ) != 0 {
        info.serial = Some(wide_to_string(serial.as_ptr()));
    }
    CloseHandle(handle);
    Some(info)
}
