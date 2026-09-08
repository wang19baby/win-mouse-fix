use tungstenite::client::IntoClientRequest;
use tungstenite::{connect, Message};
use windows_sys::Win32::Devices::DeviceAndDriverInstallation::{
    SetupDiDestroyDeviceInfoList, SetupDiEnumDeviceInterfaces, SetupDiGetClassDevsW,
    SetupDiGetDeviceInterfaceDetailW, DIGCF_DEVICEINTERFACE, DIGCF_PRESENT,
    SP_DEVICE_INTERFACE_DATA, SP_DEVICE_INTERFACE_DETAIL_DATA_W,
};
use windows_sys::Win32::Devices::HumanInterfaceDevice::{
    HidD_FreePreparsedData, HidD_GetAttributes, HidD_GetPreparsedData, HidP_GetCaps,
    GUID_DEVINTERFACE_HID, HIDD_ATTRIBUTES, HIDP_CAPS,
};
use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
};

const LOGITECH_VID: u16 = 0x046D;

fn main() {
    println!("=== G502 LIGHTSPEED: HID Buttons + G Hub Slots ===\n");

    // 1. Read HID capabilities
    let devices = enumerate_logitech();
    println!("Found {} Logitech HID device(s)\n", devices.len());
    for d in &devices {
        if d.pid == 0xC539 {
            println!("HID path: {}", d.path);
            if let Some(caps) = check_hid_caps(&d.path) {
                println!("  Usage: 0x{:04X} (UsagePage: 0x{:04X})", caps.0, caps.1);
                println!("  Input buttons: {}, Input values: {}", caps.2, caps.3);
                println!("  Feature report len: {}", caps.4);
            }
        }
    }

    // 2. Get G Hub slots
    println!("\n=== G Hub Button Slots ===\n");
    let mut request = "ws://localhost:9010".into_client_request().unwrap();
    request
        .headers_mut()
        .insert("Sec-WebSocket-Protocol", "json".parse().unwrap());
    let (mut ws, _) = match connect(request) {
        Ok(c) => c,
        Err(e) => {
            println!("G Hub not available: {}", e);
            return;
        }
    };
    let _welcome = ws.read_message().unwrap();

    ws.write_message(Message::binary(
        serde_json::json!({"path": "/devices/list", "verb": "GET"}).to_string(),
    ))
    .unwrap();
    let resp = read_response(&mut ws);
    let json: serde_json::Value = serde_json::from_str(&resp).unwrap();
    let device = json["payload"]["deviceInfos"]
        .as_array()
        .unwrap()
        .iter()
        .find(|d| d["connectionType"] == "WIRELESS")
        .unwrap();

    let slots = device["slots"].as_object().unwrap();

    // Physical button mapping for G502 LIGHTSPEED
    let physical: Vec<(&str, &str, &str)> = vec![
        ("g502wireless_g1_m1", "G1", "左键上方 (DPI+)"),
        ("g502wireless_g2_m1", "G2", "左键下方 (DPI-)"),
        ("g502wireless_g3_m1", "G3", "滚轮按下 (中键)"),
        ("g502wireless_g4_m1", "G4", "左键侧面 (DPI Shift)"),
        ("g502wireless_g5_m1", "G5", "右键上方 (后退)"),
        ("g502wireless_g6_m1", "G6", "右键下方 (前进)"),
        ("g502wireless_g7_m1", "G7", "侧键1 (拇指)"),
        ("g502wireless_g8_m1", "G8", "侧键2 (拇指)"),
        ("g502wireless_g9_m1", "G9", "滚轮左倾"),
        ("g502wireless_g10_m1", "G10", "滚轮右倾"),
        ("g502wireless_g11_m1", "G11", "G-Shift (底部)"),
    ];

    println!(
        "{:<5} {:<10} {:<25} {:<18} G-Shift?",
        "Slot", "Button", "Physical", "Attribute"
    );
    println!("{}", "-".repeat(80));

    for (slot_id, gname, pos) in &physical {
        let slot = slots.get(*slot_id);
        let attr = slot
            .map(|s| s["attribute"].as_str().unwrap_or("?"))
            .unwrap_or("NOT FOUND");
        let shifted_slot = format!("{}_shifted", slot_id);
        let has_shift = slots.contains_key(&shifted_slot);
        println!(
            "{:<5} {:<10} {:<25} {:<18} {}",
            slot_id,
            gname,
            pos,
            attr,
            if has_shift { "yes" } else { "no" }
        );
    }

    // Also show non-button slots
    println!("\n=== Settings Slots ===");
    for (slot_id, slot_info) in slots {
        if slot_id.contains("setting") || slot_id.contains("lighting") {
            let attr = slot_info["attribute"].as_str().unwrap_or("?");
            println!("  {} → {}", slot_id, attr);
        }
    }

    ws.close(None).unwrap();
    println!("\nDone!");
}

// --- HID helpers ---

struct DevInfo {
    path: String,
    pid: u16,
}

fn check_hid_caps(path: &str) -> Option<(u16, u16, u16, u16, u16)> {
    let path = path.to_owned();
    let child = std::thread::spawn(move || unsafe {
        let wide: Vec<u16> = path.encode_utf16().chain(std::iter::once(0)).collect();
        let handle = CreateFileW(
            wide.as_ptr(),
            0x80000000 | 0x40000000,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            std::ptr::null_mut(),
            OPEN_EXISTING,
            0,
            0,
        );
        if handle == INVALID_HANDLE_VALUE {
            return None;
        }

        let mut preparsed: isize = 0;
        if HidD_GetPreparsedData(handle, &mut preparsed) == 0 {
            CloseHandle(handle);
            return None;
        }
        let mut caps: HIDP_CAPS = std::mem::zeroed();
        let status = HidP_GetCaps(preparsed, &mut caps);
        HidD_FreePreparsedData(preparsed);
        CloseHandle(handle);
        if status != 1 {
            return None;
        }

        Some((
            caps.UsagePage,
            caps.Usage,
            caps.NumberInputButtonCaps,
            caps.NumberInputValueCaps,
            caps.FeatureReportByteLength,
        ))
    });
    child.join().ok().flatten()
}

fn enumerate_logitech() -> Vec<DevInfo> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    let guid = GUID_DEVINTERFACE_HID;
    unsafe {
        let dev_info = SetupDiGetClassDevsW(
            &guid,
            std::ptr::null(),
            0,
            DIGCF_PRESENT | DIGCF_DEVICEINTERFACE,
        );
        if dev_info == INVALID_HANDLE_VALUE {
            return out;
        }
        let mut index = 0u32;
        loop {
            let mut iface: SP_DEVICE_INTERFACE_DATA = std::mem::zeroed();
            iface.cbSize = std::mem::size_of::<SP_DEVICE_INTERFACE_DATA>() as u32;
            if SetupDiEnumDeviceInterfaces(dev_info, std::ptr::null(), &guid, index, &mut iface)
                == 0
            {
                break;
            }
            index += 1;
            let mut needed: u32 = 0;
            SetupDiGetDeviceInterfaceDetailW(
                dev_info,
                &iface,
                std::ptr::null_mut(),
                0,
                &mut needed,
                std::ptr::null_mut(),
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
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            ) == 0
            {
                continue;
            }
            let path = wide_to_string((*detail).DevicePath.as_ptr());
            let wide: Vec<u16> = path.encode_utf16().chain(std::iter::once(0)).collect();
            let h = CreateFileW(
                wide.as_ptr(),
                0,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                std::ptr::null_mut(),
                OPEN_EXISTING,
                0,
                0,
            );
            if h == INVALID_HANDLE_VALUE {
                continue;
            }
            let mut attr: HIDD_ATTRIBUTES = std::mem::zeroed();
            attr.Size = std::mem::size_of::<HIDD_ATTRIBUTES>() as u32;
            if HidD_GetAttributes(h, &mut attr) != 0
                && attr.VendorID == LOGITECH_VID
                && seen.insert(path.clone())
            {
                out.push(DevInfo {
                    path,
                    pid: attr.ProductID,
                });
            }
            CloseHandle(h);
        }
        SetupDiDestroyDeviceInfoList(dev_info);
    }
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

fn read_response(
    ws: &mut tungstenite::WebSocket<tungstenite::stream::MaybeTlsStream<std::net::TcpStream>>,
) -> String {
    let msg = ws.read_message().unwrap();
    match msg {
        Message::Text(t) => t,
        Message::Binary(b) => String::from_utf8_lossy(&b).to_string(),
        _ => "{}".into(),
    }
}
