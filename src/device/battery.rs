//! Read battery level via Logitech G Hub WebSocket API.
//!
//! The BOLT/LIGHTSPEED receiver does NOT support standard HID++ 2.0, so we
//! connect to G Hub's local WebSocket (ws://localhost:9010) to read battery.
//! G Hub must be running; we auto-start it silently if needed.
use tungstenite::{connect, Message};
use tungstenite::client::IntoClientRequest;
use crate::device::cache::BatteryInfo;

/// Check if port 9010 is accepting connections (G Hub WebSocket ready).
fn is_lghub_port_ready() -> bool {
    std::net::TcpStream::connect_timeout(
        &"127.0.0.1:9010".parse().unwrap(),
        std::time::Duration::from_millis(500),
    ).is_ok()
}

/// Wait for G Hub WebSocket to be ready, with timeout.
fn wait_for_lghub_ready(timeout_ms: u64) -> bool {
    let start = std::time::Instant::now();
    let timeout = std::time::Duration::from_millis(timeout_ms);
    while start.elapsed() < timeout {
        if is_lghub_port_ready() {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
    }
    false
}

/// Ensure G Hub system tray is running and WebSocket is ready.
/// Returns true if G Hub is ready within timeout.
fn ensure_lghub_running() -> bool {
    // Fast check: is port already open?
    if is_lghub_port_ready() {
        return true;
    }

    // Port not open — check if agent process is running but WebSocket not ready
    let agent_running = std::process::Command::new("tasklist")
        .args(["/FI", "IMAGENAME eq lghub_agent.exe", "/NH"])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).contains("lghub_agent"))
        .unwrap_or(false);

    if agent_running {
        crate::log::write("battery: G Hub agent running, waiting for WebSocket...");
        return wait_for_lghub_ready(10_000);
    }

    // Not running — try to start
    crate::log::write("battery: G Hub not running, starting...");
    let paths = [
        "C:\\Program Files\\LGHUB\\system_tray\\lghub_system_tray.exe",
        "C:\\Program Files (x86)\\LGHUB\\system_tray\\lghub_system_tray.exe",
    ];
    for p in &paths {
        if std::path::Path::new(p).exists() {
            let _ = std::process::Command::new(p)
                .arg("--minimized")
                .spawn();
            crate::log::write(&format!("battery: started {}", p));
            // Wait for WebSocket to become ready (up to 15 seconds)
            return wait_for_lghub_ready(15_000);
        }
    }

    crate::log::write("battery: G Hub not found in standard paths");
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

/// Read device list from G Hub WebSocket.
/// Returns JSON array of device info objects.
#[allow(dead_code)]
pub fn read_device_list() -> Option<Vec<serde_json::Value>> {
    if !ensure_lghub_running() {
        return None;
    }

    let mut request = "ws://localhost:9010".into_client_request().ok()?;
    request.headers_mut().insert(
        "Sec-WebSocket-Protocol",
        "json".parse().ok()?,
    );

    let (mut ws, _) = connect(request).ok()?;
    let _welcome = ws.read_message().ok()?;

    ws.write_message(Message::binary(
        serde_json::json!({
            "path": "/devices/list",
            "verb": "GET"
        }).to_string(),
    )).ok()?;

    let msg = ws.read_message().ok()?;
    let text = match msg {
        Message::Text(t) => t,
        Message::Binary(b) => String::from_utf8_lossy(&b).to_string(),
        _ => return None,
    };

    let json: serde_json::Value = serde_json::from_str(&text).ok()?;
    let infos = json["payload"]["deviceInfos"].as_array()?.clone();
    Some(infos)
}
