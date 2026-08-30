//! Read battery level via Logitech G Hub WebSocket API.
//!
//! The BOLT/LIGHTSPEED receiver does NOT support standard HID++ 2.0, so we
//! connect to G Hub's local WebSocket (ws://localhost:9010) to read battery.
//! G Hub must be running; we auto-start it silently if needed.
use tungstenite::{connect, Message};
use tungstenite::client::IntoClientRequest;
use crate::device::cache::BatteryInfo;

/// Ensure G Hub system tray is running. Returns true if running or started.
fn ensure_lghub_running() -> bool {
    // Check if already running
    if std::process::Command::new("tasklist")
        .args(["/FI", "IMAGENAME eq lghub_agent.exe", "/NH"])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).contains("lghub_agent"))
        .unwrap_or(false)
    {
        return true;
    }

    // Try to start G Hub system tray
    let paths = [
        "C:\\Program Files\\LGHUB\\system_tray\\lghub_system_tray.exe",
        "C:\\Program Files (x86)\\LGHUB\\system_tray\\lghub_system_tray.exe",
    ];
    for p in &paths {
        if std::path::Path::new(p).exists() {
            let _ = std::process::Command::new(p)
                .arg("--minimized")
                .spawn();
            // Give it a few seconds to start
            std::thread::sleep(std::time::Duration::from_secs(3));
            return true;
        }
    }
    false
}

/// Read battery from G Hub WebSocket. Returns (percentage, charging).
fn read_battery_ws() -> Option<(u8, bool)> {
    let mut request = "ws://localhost:9010".into_client_request().ok()?;
    request.headers_mut().insert(
        "Sec-WebSocket-Protocol",
        "json".parse().ok()?,
    );

    let (mut ws, _) = connect(request).ok()?;

    // Read welcome OPTIONS message
    let _welcome = ws.read_message().ok()?;

    // Get device list
    ws.write_message(Message::binary(
        serde_json::json!({
            "path": "/devices/list",
            "verb": "GET"
        }).to_string(),
    )).ok()?;

    let devices_msg = ws.read_message().ok()?;
    let devices_text = match devices_msg {
        Message::Text(t) => t,
        Message::Binary(b) => String::from_utf8_lossy(&b).to_string(),
        _ => return None,
    };

    let json: serde_json::Value = serde_json::from_str(&devices_text).ok()?;
    let device_infos = json["payload"]["deviceInfos"].as_array()?;

    // Find first wireless device
    let wireless = device_infos.iter()
        .find(|d| d["connectionType"] == "WIRELESS")?;
    let id = wireless["id"].as_str()?;

    // Read battery
    ws.write_message(Message::binary(
        serde_json::json!({
            "path": format!("/battery/{}/state", id),
            "verb": "GET"
        }).to_string(),
    )).ok()?;

    let bat_msg = ws.read_message().ok()?;
    let bat_text = match bat_msg {
        Message::Text(t) => t,
        Message::Binary(b) => String::from_utf8_lossy(&b).to_string(),
        _ => return None,
    };

    let bat_json: serde_json::Value = serde_json::from_str(&bat_text).ok()?;
    let percentage = bat_json["payload"]["percentage"].as_u64()? as u8;
    let charging = bat_json["payload"]["charging"].as_bool().unwrap_or(false);

    Some((percentage, charging))
}

/// Enumerate Logitech devices and return the first readable battery level.
/// Used by the tray poll to refresh the global cache.
pub fn read_first_battery() -> Option<BatteryInfo> {
    crate::log::write("battery: reading via G Hub WebSocket");

    if !ensure_lghub_running() {
        crate::log::write("battery: G Hub not found or failed to start");
        return None;
    }

    match read_battery_ws() {
        Some((level, charging)) => {
            crate::log::write(&format!(
                "battery: OK → {}%{}",
                level,
                if charging { " charging" } else { "" }
            ));
            Some(BatteryInfo::from_level(level, charging))
        }
        None => {
            crate::log::write("battery: G Hub WebSocket read failed");
            None
        }
    }
}
