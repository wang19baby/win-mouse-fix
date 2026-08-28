//! G502 HERO DPI diagnostic — probes HID++ 0x2201 AdjustableDpi
//! function codes against a real device and prints raw bytes.

use std::ptr::null_mut;

use windows_sys::Win32::Devices::DeviceAndDriverInstallation::{
    SetupDiDestroyDeviceInfoList, SetupDiEnumDeviceInfo, SetupDiGetClassDevsW,
    SetupDiEnumDeviceInterfaces, SetupDiGetDeviceInterfaceDetailW, DIGCF_DEVICEINTERFACE, DIGCF_PRESENT,
    SP_DEVICE_INTERFACE_DATA, SP_DEVICE_INTERFACE_DETAIL_DATA_W, SP_DEVINFO_DATA,
};
use windows_sys::Win32::Devices::HumanInterfaceDevice::{
    HIDD_ATTRIBUTES, HidD_GetAttributes, HidD_GetSerialNumberString,
};
use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
};
use windows_sys::Win32::System::Registry::{
    RegCloseKey, RegEnumKeyExW, RegOpenKeyExW, RegQueryValueExW,
    HKEY_LOCAL_MACHINE, KEY_READ, KEY_ENUMERATE_SUB_KEYS,
};
use windows_sys::Win32::System::IO::DeviceIoControl;

const LOGITECH_VID: u16 = 0x046D;
const RID_SHORT: u8 = 0x10;

// Hardcoded GUID_DEVINTERFACE_HID {4D36E96B-E325-11CE-BFC1-08002BE10318}
const HID_INTERFACE_GUID: windows_sys::core::GUID = windows_sys::core::GUID {
    data1: 0x4D36E96B,
    data2: 0xE325,
    data3: 0x11CE,
    data4: [0xBF, 0xC1, 0x08, 0x00, 0x2B, 0xE1, 0x03, 0x18],
};

// Keyboard Device Interface GUID — Logitech receivers register here on some systems
const KEYBOARD_INTERFACE_GUID: windows_sys::core::GUID = windows_sys::core::GUID {
    data1: 0x4D1E55B2,
    data2: 0xF16F,
    data3: 0x11CF,
    data4: [0x88, 0xCB, 0x00, 0x11, 0x11, 0x00, 0x03, 0x00],
};

// Mouse Device Interface GUID — Logitech receivers register here on most systems
const MOUSE_INTERFACE_GUID: windows_sys::core::GUID = windows_sys::core::GUID {
    data1: 0x745A17A0,
    data2: 0x74D3,
    data3: 0x11D0,
    data4: [0xB6, 0xFE, 0x00, 0xA0, 0xC9, 0x0F, 0x57, 0xDA],
};

#[derive(Debug, Clone, Copy)]
struct ShortMsg {
    device_idx: u8,
    feature_index: u8,
    fn_id: u8,
    p1: u8,
    p2: u8,
    p3: u8,
}

impl ShortMsg {
    fn encode(self) -> [u8; 7] {
        let byte3 = (self.fn_id << 4) | 0x00;
        let mut b = [RID_SHORT, self.device_idx, self.feature_index, byte3,
                     self.p1, self.p2, 0u8];
        b[6] = b[0..6].iter().fold(0u8, |a, &v| a.wrapping_add(v));
        b
    }
}

fn parse_response(raw: [u8; 7]) -> [u8; 6] {
    [raw[1], raw[2], raw[3], raw[4], raw[5], raw[6]]
}

fn hex(b: &[u8]) -> String {
    let mut s = String::new();
    for (i, &v) in b.iter().enumerate() {
        if i > 0 { s.push(' '); }
        use std::fmt::Write;
        write!(&mut s, "{:02X}", v).unwrap();
    }
    s
}

#[derive(Debug, Clone)]
struct HidDevice {
    path: String,
    vid: u16,
    pid: u16,
    serial: Option<String>,
}

unsafe fn wide_to_string(p: *const u16) -> String {
    if p.is_null() { return String::new(); }
    let mut len = 0usize;
    while *p.add(len) != 0 { len += 1; }
    String::from_utf16_lossy(std::slice::from_raw_parts(p, len))
}

/// Main entry: try SetupDi with all three relevant GUIDs, then registry fallback.
/// On most Windows systems Logitech receivers only appear in Keyboard/Mouse GUIDs,
/// not the standard HID GUID, so we try all three.
unsafe fn enumerate_all_hid() -> Vec<HidDevice> {
    // Try all three Device Interface GUIDs
    for guid in [&HID_INTERFACE_GUID, &KEYBOARD_INTERFACE_GUID, &MOUSE_INTERFACE_GUID] {
        let set = SetupDiGetClassDevsW(
            guid as *const _,
            null_mut(),
            0isize,
            DIGCF_DEVICEINTERFACE | DIGCF_PRESENT,
        );
        if set != INVALID_HANDLE_VALUE {
            let devices = enumerate_via_interface_paths(set, guid);
            SetupDiDestroyDeviceInfoList(set);
            if !devices.is_empty() {
                println!("  [DBG] SetupDi found {} devices (GUID {:08X})", devices.len(), guid.data1);
                return devices;
            }
        }
    }

    // Fallback: try USB registry for receiver SymbolicName paths
    let devices = enumerate_usb_registry(LOGITECH_VID);
    if !devices.is_empty() {
        println!("  [DBG] USB registry found {} devices", devices.len());
        return devices;
    }

    // Fallback: try Device Classes registry for HID interface symlinks
    let devices = enumerate_device_classes(LOGITECH_VID);
    if !devices.is_empty() {
        println!("  [DBG] DeviceClasses found {} devices", devices.len());
        return devices;
    }

    Vec::new()
}

/// Enumerate Device Classes registry for the 3 relevant device interface GUIDs.
/// This is a targeted scan (not all 170+ GUIDs) so it completes in milliseconds.
unsafe fn enumerate_device_classes(_target_vid: u16) -> Vec<HidDevice> {
    let mut devices = Vec::new();
    println!("  [DBG] Trying DeviceClasses (targeted 3-GUID scan)...");

    let paths = [
        (r"SYSTEM\CurrentControlSet\Control\DeviceClasses\{4D36E96B-E325-11CE-BFC1-08002BE10318}", "HID"),
        (r"SYSTEM\CurrentControlSet\Control\DeviceClasses\{4D1E55B2-F16F-11CF-88CB-001111000030}", "Keyboard"),
        (r"SYSTEM\CurrentControlSet\Control\DeviceClasses\{745A17A0-74D3-11D0-B6FE-00A0C90F57DA}", "Mouse"),
    ];

    for (path_str, name) in paths {
        let path_ws: Vec<u16> = path_str.encode_utf16().chain(std::iter::once(0)).collect();
        let mut class_key = 0isize;
        if RegOpenKeyExW(HKEY_LOCAL_MACHINE, path_ws.as_ptr(), 0, KEY_READ | KEY_ENUMERATE_SUB_KEYS, &mut class_key) != 0 || class_key == 0 {
            println!("  [DBG] Could not open DeviceClasses key for {}", name);
            continue;
        }
        println!("  [DBG] Scanning DeviceClasses {}", name);
        enumerate_device_class_instances(class_key, &mut devices);
        RegCloseKey(class_key);

        if !devices.is_empty() {
            println!("  [DBG] DeviceClasses {} found {} devices", name, devices.len());
            break;
        }
    }

    devices
}


/// Enumerate all interface symlinks in a DeviceClass subkey.
unsafe fn enumerate_device_class_instances(class_key: isize, devices: &mut Vec<HidDevice>) {
    let mut symlink_buf: [u16; 4096] = [0; 4096];
    let mut inst_idx = 0u32;

    loop {
        let mut name_len = symlink_buf.len() as u32;
        let ret = RegEnumKeyExW(class_key, inst_idx, symlink_buf.as_mut_ptr(), &mut name_len,
            null_mut(), null_mut(), null_mut(), null_mut());
        // ERROR_MORE_DATA (0xEA) means buffer too small — skip this entry and try next
        if ret == 0xEA { inst_idx += 1; continue; }
        if ret != 0 { break; }

        let symlink = wide_to_string(symlink_buf.as_ptr());

        let path = if symlink.starts_with("##?#") {
            format!("\\\\?\\{}", &symlink[4..])
        } else {
            // HID#...#{guid} — already a device path format
            format!("\\\\?\\{}", symlink)
        };
        let wide: Vec<u16> = path.encode_utf16().chain(std::iter::once(0)).collect();

        let handle = CreateFileW(wide.as_ptr(), 0, FILE_SHARE_READ | FILE_SHARE_WRITE,
            null_mut(), OPEN_EXISTING, 0, 0);

        if handle != INVALID_HANDLE_VALUE {
            let mut attr: HIDD_ATTRIBUTES = std::mem::zeroed();
            attr.Size = std::mem::size_of::<HIDD_ATTRIBUTES>() as u32;
            if HidD_GetAttributes(handle, &mut attr) != 0 {
                println!("  [DBG] DeviceClass SUCCESS vid={:04X} pid={:04X}", attr.VendorID, attr.ProductID);
                devices.push(HidDevice {
                    path,
                    vid: attr.VendorID,
                    pid: attr.ProductID,
                    serial: None,
                });
            }
            CloseHandle(handle);
        } else {
            println!("  [DBG] DeviceClass FAILED to open: {}", &symlink[..symlink.len().min(80)]);
        }

        inst_idx += 1;
    }
}

/// Enumerate HID interface paths via SetupDi.
unsafe fn enumerate_via_interface_paths(set: isize, guid: &windows_sys::core::GUID) -> Vec<HidDevice> {
    let mut devices = Vec::new();
    let mut dev_info: SP_DEVINFO_DATA = std::mem::zeroed();
    dev_info.cbSize = std::mem::size_of::<SP_DEVINFO_DATA>() as u32;
    let mut dev_idx = 0u32;

    while SetupDiEnumDeviceInfo(set, dev_idx, &mut dev_info) != 0 {
        let mut iface: SP_DEVICE_INTERFACE_DATA = std::mem::zeroed();
        iface.cbSize = std::mem::size_of::<SP_DEVICE_INTERFACE_DATA>() as u32;
        let mut iface_idx = 0u32;

        let guid_ptr = guid as *const _;
        while SetupDiEnumDeviceInterfaces(set, &mut dev_info, guid_ptr, iface_idx, &mut iface) != 0 {
            let mut needed = 0u32;
            let _ = SetupDiGetDeviceInterfaceDetailW(
                set, &iface, null_mut(), 0, &mut needed, null_mut(),
            );

            if needed > 0 {
                let detail_bytes = needed as usize;
                let mut detail_buf: Vec<u8> = vec![0u8; detail_bytes];
                let detail_ptr = detail_buf.as_mut_ptr() as *mut SP_DEVICE_INTERFACE_DETAIL_DATA_W;
                (*detail_ptr).cbSize = std::mem::size_of::<SP_DEVICE_INTERFACE_DETAIL_DATA_W>() as u32;

                if SetupDiGetDeviceInterfaceDetailW(
                    set, &iface, detail_ptr, detail_bytes as u32, null_mut(), null_mut(),
                ) != 0 {
                    let path = wide_to_string((*detail_ptr).DevicePath.as_ptr());

                    let handle = CreateFileW(
                        path.as_ptr() as _, 0,
                        FILE_SHARE_READ | FILE_SHARE_WRITE,
                        null_mut(), OPEN_EXISTING, 0, 0,
                    );

                    if handle != INVALID_HANDLE_VALUE {
                        let mut attr: HIDD_ATTRIBUTES = std::mem::zeroed();
                        attr.Size = std::mem::size_of::<HIDD_ATTRIBUTES>() as u32;
                        if HidD_GetAttributes(handle, &mut attr) != 0 {
                            let serial = {
                                let mut buf: Vec<u16> = vec![0u16; 128];
                                if HidD_GetSerialNumberString(handle, buf.as_mut_ptr() as _, (buf.len() * 2) as u32) != 0 {
                                    Some(wide_to_string(buf.as_ptr()))
                                } else { None }
                            };
                            devices.push(HidDevice { path, vid: attr.VendorID, pid: attr.ProductID, serial });
                        }
                        CloseHandle(handle);
                    }
                }
            }
            iface_idx += 1;
        }
        dev_idx += 1;
    }

    devices
}

/// Enumerate USB registry (HKLM\SYSTEM\CurrentControlSet\Enum\USB) for devices by VID.
/// Looks for SymbolicName in Device Parameters subkey and tries to open with CreateFileW.
unsafe fn enumerate_usb_registry(target_vid: u16) -> Vec<HidDevice> {
    let mut devices = Vec::new();
    let vid_hex = format!("{:04X}", target_vid);

    let key_path = windows_sys::core::w!("SYSTEM\\CurrentControlSet\\Enum\\USB");
    let mut hkey = 0isize;
    if RegOpenKeyExW(HKEY_LOCAL_MACHINE, key_path, 0, KEY_READ | KEY_ENUMERATE_SUB_KEYS, &mut hkey) != 0 || hkey == 0 {
        return Vec::new();
    }

    let mut subkey_buf: [u16; 256] = [0; 256];
    let mut dev_idx = 0u32;

    loop {
        let mut name_len = subkey_buf.len() as u32;
        let ret = RegEnumKeyExW(hkey, dev_idx, subkey_buf.as_mut_ptr(), &mut name_len,
            null_mut(), null_mut(), null_mut(), null_mut());
        // ERROR_MORE_DATA (0xEA) means buffer too small — skip this entry and try next
        if ret == 0xEA { dev_idx += 1; continue; }
        if ret != 0 { break; }

        let vid_pid = wide_to_string(subkey_buf.as_ptr());
        let vid_pid_upper = vid_pid.to_uppercase();

        // Match VID_046D or VID_xxxx pattern
        if !vid_pid_upper.starts_with(&format!("VID_{}", vid_hex)) {
            dev_idx += 1;
            continue;
        }

        // Enumerate instances under this VID\PID
        let full_path = format!("SYSTEM\\CurrentControlSet\\Enum\\USB\\{}", vid_pid);
        let full_wide: Vec<u16> = full_path.encode_utf16().chain(std::iter::once(0)).collect();

        let mut instance_key = 0isize;
        if RegOpenKeyExW(HKEY_LOCAL_MACHINE, full_wide.as_ptr(), 0, KEY_READ | KEY_ENUMERATE_SUB_KEYS, &mut instance_key) != 0 || instance_key == 0 {
            dev_idx += 1;
            continue;
        }

        let mut inst_buf: [u16; 256] = [0; 256];
        let mut inst_idx = 0u32;
        loop {
            let mut inst_len = inst_buf.len() as u32;
            let ret = RegEnumKeyExW(instance_key, inst_idx, inst_buf.as_mut_ptr(), &mut inst_len,
                null_mut(), null_mut(), null_mut(), null_mut());
            if ret != 0 { break; }

            let instance = wide_to_string(inst_buf.as_ptr());

            // Get SymbolicName from Device Parameters
            let dp_path = format!("{}\\Device Parameters", full_path);
            let instance_dp = format!("{}\\{}", dp_path, instance);
            let dp_wide: Vec<u16> = instance_dp.encode_utf16().chain(std::iter::once(0)).collect();

            let mut dp_key = 0isize;
            if RegOpenKeyExW(HKEY_LOCAL_MACHINE, dp_wide.as_ptr(), 0, KEY_READ, &mut dp_key) == 0 && dp_key != 0 {
                if let Some(sym) = get_reg_string(dp_key, windows_sys::core::w!("SymbolicName")) {
                    if sym.starts_with("\\??\\") {
                        let path = format!("\\\\?\\{}", &sym[4..]);
                        try_open_device(&path, &mut devices);
                    }
                }
                RegCloseKey(dp_key);
            }

            inst_idx += 1;
        }

        RegCloseKey(instance_key);
        dev_idx += 1;
    }

    RegCloseKey(hkey);
    devices
}

/// Enumerate HID registry (HKLM\SYSTEM\CurrentControlSet\Enum\HID) for devices by VID.
unsafe fn enumerate_hid_registry(target_vid: u16) -> Vec<HidDevice> {
    let devices = Vec::new();
    let vid_hex = format!("{:04X}", target_vid);

    let key_path = windows_sys::core::w!("SYSTEM\\CurrentControlSet\\Enum\\HID");
    let mut hkey = 0isize;
    if RegOpenKeyExW(HKEY_LOCAL_MACHINE, key_path, 0, KEY_READ | KEY_ENUMERATE_SUB_KEYS, &mut hkey) != 0 || hkey == 0 {
        return Vec::new();
    }

    let mut subkey_buf: [u16; 256] = [0; 256];
    let mut dev_idx = 0u32;

    loop {
        let mut name_len = subkey_buf.len() as u32;
        let ret = RegEnumKeyExW(hkey, dev_idx, subkey_buf.as_mut_ptr(), &mut name_len,
            null_mut(), null_mut(), null_mut(), null_mut());
        // ERROR_MORE_DATA (0xEA) means buffer too small — skip this entry and try next
        if ret == 0xEA { dev_idx += 1; continue; }
        if ret != 0 { break; }

        let vid_subkey = wide_to_string(subkey_buf.as_ptr());
        let vid_upper = vid_subkey.to_uppercase();

        if !vid_upper.starts_with(&format!("VID_{}", vid_hex)) {
            dev_idx += 1;
            continue;
        }

        // Enumerate PID subkeys
        let full_path = format!("SYSTEM\\CurrentControlSet\\Enum\\HID\\{}", vid_subkey);
        let full_wide: Vec<u16> = full_path.encode_utf16().chain(std::iter::once(0)).collect();

        let mut pid_key = 0isize;
        if RegOpenKeyExW(HKEY_LOCAL_MACHINE, full_wide.as_ptr(), 0, KEY_READ | KEY_ENUMERATE_SUB_KEYS, &mut pid_key) != 0 || pid_key == 0 {
            dev_idx += 1;
            continue;
        }

        let mut inst_buf: [u16; 256] = [0; 256];
        let mut inst_idx = 0u32;
        loop {
            let mut inst_len = inst_buf.len() as u32;
            let ret = RegEnumKeyExW(pid_key, inst_idx, inst_buf.as_mut_ptr(), &mut inst_len,
                null_mut(), null_mut(), null_mut(), null_mut());
            if ret != 0 { break; }

            let instance = wide_to_string(inst_buf.as_ptr());

            // Get HardwareID from the instance
            let inst_path = format!("{}\\{}", full_path, instance);
            let inst_wide: Vec<u16> = inst_path.encode_utf16().chain(std::iter::once(0)).collect();

            let mut dev_key = 0isize;
            if RegOpenKeyExW(HKEY_LOCAL_MACHINE, inst_wide.as_ptr(), 0, KEY_READ, &mut dev_key) != 0 || dev_key == 0 {
                inst_idx += 1;
                continue;
            }

            if let Some(hw_id) = get_reg_multi_string(dev_key, windows_sys::core::w!("HardwareID")) {
                let hw_upper = hw_id.to_uppercase();
                let (_, pid) = extract_vid_pid(&hw_upper);
                if pid != 0 {
                    // Try to find FriendlyName or DeviceDesc
                    let friendly = get_reg_string(dev_key, windows_sys::core::w!("DeviceDesc"));
                    println!("  [DBG] HID instance: {} vid=0x{:04X} pid=0x{:04X} desc={:?}",
                        instance, target_vid, pid, friendly);
                }
            }

            RegCloseKey(dev_key);
            inst_idx += 1;
        }

        RegCloseKey(pid_key);
        dev_idx += 1;
    }

    RegCloseKey(hkey);
    devices
}

/// Try to open a device path with CreateFileW and add to list if HID.
unsafe fn try_open_device(path: &str, devices: &mut Vec<HidDevice>) {
    let wide: Vec<u16> = path.encode_utf16().chain(std::iter::once(0)).collect();
    let handle = CreateFileW(wide.as_ptr(), 0, FILE_SHARE_READ | FILE_SHARE_WRITE,
        null_mut(), OPEN_EXISTING, 0, 0);

    if handle != INVALID_HANDLE_VALUE {
        let mut attr: HIDD_ATTRIBUTES = std::mem::zeroed();
        attr.Size = std::mem::size_of::<HIDD_ATTRIBUTES>() as u32;
        if HidD_GetAttributes(handle, &mut attr) != 0 {
            println!("  [DBG] SUCCESS opened vid={:04X} pid={:04X} path={}", attr.VendorID, attr.ProductID, path);
            devices.push(HidDevice {
                path: path.to_string(),
                vid: attr.VendorID,
                pid: attr.ProductID,
                serial: None,
            });
        }
        CloseHandle(handle);
    }
}

unsafe fn get_reg_string(hkey: isize, name: *const u16) -> Option<String> {
    let mut buf: Vec<u8> = vec![0u8; 512];
    let mut buf_size = buf.len() as u32;
    let mut reg_type = 0u32;
    if RegQueryValueExW(hkey, name, null_mut(), &mut reg_type, buf.as_mut_ptr(), &mut buf_size) != 0 {
        return None;
    }
    if reg_type != 1 { return None; }
    let char_len = (buf_size as usize) / 2;
    if char_len == 0 { return None; }
    let mut v: Vec<u16> = vec![0u16; char_len];
    for i in 0..char_len {
        v[i] = u16::from_le_bytes([buf[i * 2], buf[i * 2 + 1]]);
    }
    let end = v.iter().position(|&c| c == 0).unwrap_or(v.len());
    Some(String::from_utf16_lossy(&v[..end]))
}

unsafe fn get_reg_multi_string(hkey: isize, name: *const u16) -> Option<String> {
    let mut buf: Vec<u8> = vec![0u8; 2048];
    let mut buf_size = buf.len() as u32;
    let mut reg_type = 0u32;
    if RegQueryValueExW(hkey, name, null_mut(), &mut reg_type, buf.as_mut_ptr(), &mut buf_size) != 0 {
        return None;
    }
    // REG_MULTI_SZ: array of null-terminated strings, double-null at end
    let char_len = (buf_size as usize) / 2;
    if char_len == 0 { return None; }
    let mut v: Vec<u16> = vec![0u16; char_len];
    for i in 0..char_len {
        v[i] = u16::from_le_bytes([buf[i * 2], buf[i * 2 + 1]]);
    }
    // Take first string
    let end = v.iter().position(|&c| c == 0).unwrap_or(v.len());
    Some(String::from_utf16_lossy(&v[..end]))
}

fn extract_vid_pid(hw_id: &str) -> (u16, u16) {
    let mut vid = 0u16;
    let mut pid = 0u16;
    for part in hw_id.split(|c| c == '\\' || c == '#') {
        if let Some(vid_start) = part.find("VID_") {
            let hex_str = &part[vid_start + 4..];
            if hex_str.len() >= 4 {
                if let Ok(v) = u16::from_str_radix(&hex_str[..4], 16) { vid = v; }
            }
        }
        if let Some(pid_start) = part.find("PID_") {
            let hex_str = &part[pid_start + 4..];
            if hex_str.len() >= 4 {
                if let Ok(p) = u16::from_str_radix(&hex_str[..4], 16) { pid = p; }
            }
        }
    }
    (vid, pid)
}

fn filter_logitech(devices: &[HidDevice]) -> Vec<HidDevice> {
    devices.iter().filter(|d| d.vid == LOGITECH_VID).cloned().collect()
}

fn hidpp_exchange(handle: isize, req: [u8; 7]) -> Option<[u8; 7]> {
    let mut resp = [0u8; 7];
    let mut written = 0u32;
    let ok = unsafe {
        DeviceIoControl(
            handle, 0x2200B4,
            req.as_ptr() as _, req.len() as _,
            resp.as_mut_ptr() as _, resp.len() as _,
            std::ptr::addr_of_mut!(written), null_mut(),
        )
    };
    if ok != 0 && written == 7 { Some(resp) } else { None }
}

fn send(path: &str, req: [u8; 7]) -> Option<[u8; 7]> {
    let wide: Vec<u16> = path.encode_utf16().chain(std::iter::once(0)).collect();
    let handle = unsafe {
        CreateFileW(wide.as_ptr(), 0, FILE_SHARE_READ | FILE_SHARE_WRITE,
            null_mut(), OPEN_EXISTING, 0, 0)
    };
    if handle == INVALID_HANDLE_VALUE { return None; }
    let result = hidpp_exchange(handle, req);
    unsafe { CloseHandle(handle); }
    result
}

fn resolve_feature(path: &str, device_idx: u8, feature_id: u16) -> Option<u8> {
    let req = ShortMsg {
        device_idx,
        feature_index: 0x00,
        fn_id: 0x00,
        p1: (feature_id >> 8) as u8,
        p2: (feature_id & 0xFF) as u8,
        p3: 0x00,
    };
    let resp = send(path, req.encode())?;
    let r = parse_response(resp);
    if r[4] == 0x00 { Some(r[5]) } else { None }
}

fn read_dpi(path: &str, device_idx: u8, feat_idx: u8) -> Option<u16> {
    let req = ShortMsg {
        device_idx, feature_index: feat_idx, fn_id: 0x00,
        p1: 0x00, p2: 0x00, p3: 0x00,
    };
    let resp = send(path, req.encode())?;
    let r = parse_response(resp);
    if r[4] == 0x00 {
        Some(r[4] as u16 | ((r[5] as u16) << 8))
    } else { None }
}

fn write_dpi(path: &str, device_idx: u8, feat_idx: u8, fn_id: u8, dpi: u16) -> Option<[u8; 6]> {
    let req = ShortMsg {
        device_idx, feature_index: feat_idx, fn_id,
        p1: 0x00,
        p2: (dpi & 0xFF) as u8,
        p3: ((dpi >> 8) & 0xFF) as u8,
    };
    send(path, req.encode()).map(parse_response)
}

fn probe_function_ids(path: &str, device_idx: u8, feat_idx: u8, base_dpi: u16) {
    let test_dpi = (base_dpi + 100).min(4000);
    let restore_dpi = base_dpi;

    println!("\n  {:>4} | {:>22} | {:>22} | {}",
        "fn", "request", "response", "verdict");
    println!("  {}", "-".repeat(72));

    for fn_id in 0u8..=0x0Fu8 {
        let resp = write_dpi(path, device_idx, feat_idx, fn_id, test_dpi);
        let (resp_hex, detail) = match resp {
            Some(r) if r[4] == 0x00 => {
                let readback = read_dpi(path, device_idx, feat_idx);
                match readback {
                    Some(v) if v == test_dpi => (hex(&r), "OK".to_string()),
                    Some(v) => (hex(&r), format!("readback={}", v)),
                    None => (hex(&r), "readback=ERR".to_string()),
                }
            }
            Some(r) => (hex(&r), format!("error={:02X}", r[4])),
            None => ("timeout".to_string(), "no_response".to_string()),
        };

        let _ = write_dpi(path, device_idx, feat_idx, fn_id, restore_dpi);

        let req = ShortMsg {
            device_idx, feature_index: feat_idx, fn_id,
            p1: 0x00,
            p2: (test_dpi & 0xFF) as u8,
            p3: ((test_dpi >> 8) & 0xFF) as u8,
        };

        println!("  0x{:02X}({}) | {} | {:>22} | {}",
            fn_id,
            if fn_id == 0x00 { "GetDpi" } else { "SetDpi" },
            hex(&req.encode()),
            resp_hex,
            detail);
    }
}

fn main() {
    println!("═══════════════════════════════════════════════════════════");
    println!("  G502 DPI Probe — HID++ 0x2201 function scanner");
    println!("═══════════════════════════════════════════════════════════\n");

    let all = unsafe { enumerate_all_hid() };

    println!("All HID devices found ({} total):\n", all.len());
    for (i, d) in all.iter().enumerate() {
        println!("  [{}] vid={:04X} pid={:04X}  {}",
            i, d.vid, d.pid,
            &d.path[..d.path.len().min(80)]
        );
    }
    println!();

    let logitech = filter_logitech(&all);
    if logitech.is_empty() {
        println!("NOTE: No Logitech devices found.");
        if all.is_empty() {
            println!("  → All enumeration methods returned 0 devices.");
            println!("  → Try running as Administrator.");
        } else {
            println!("  → VID 0x046D not found in {} device(s).", all.len());
        }
        return;
    }

    println!("Logitech device(s):");
    for (i, d) in logitech.iter().enumerate() {
        println!("  [{}] vid={:04X} pid={:04X}  {}", i, d.vid, d.pid, d.path);
    }
    println!();

    let indices = [0xFF, 0x01, 0x02, 0x03, 0x04, 0x05];

    for dev in &logitech {
        println!("\n─── Probing vid={:04X} pid={:04X} ───", dev.vid, dev.pid);
        println!("  path: {}", dev.path);

        for &di in &indices {
            print!("  device_idx=0x{:02X} ... ", di);
            match resolve_feature(&dev.path, di, 0x2201) {
                Some(feat_idx) => {
                    println!("\n  ✓ 0x2201 at feature index 0x{:02X}", feat_idx);
                    match read_dpi(&dev.path, di, feat_idx) {
                        Some(current) => {
                            println!("  Current DPI: {} (sensor 0)", current);
                            println!("\n  Scanning SetSensorDpi function ids (0x00–0x0F):");
                            probe_function_ids(&dev.path, di, feat_idx, current);
                        }
                        None => {
                            println!("  Feature found but could not read current DPI.");
                        }
                    }
                    break;
                }
                None => {
                    println!("(0x2201 not found)");
                }
            }
        }
    }

    println!("\n═══════════════════════════════════════════════════════════");
    println!("  Done. Share output above to confirm the correct function id.");
    println!("═══════════════════════════════════════════════════════════");
}
