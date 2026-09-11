//! Read battery level via Logitech G Hub WebSocket API.
//!
//! Battery telemetry connects to G Hub's local WebSocket (ws://localhost:9010).
//! G Hub must already be running; polling never launches the heavyweight
//! companion app behind the user's back.
use crate::device::cache::BatteryInfo;
use tungstenite::client::IntoClientRequest;
use tungstenite::{connect, Message};

/// Check if port 9010 is accepting connections (G Hub WebSocket ready).
fn is_lghub_port_ready() -> bool {
    std::net::TcpStream::connect_timeout(
        &"127.0.0.1:9010".parse().unwrap(),
        std::time::Duration::from_millis(500),
    )
    .is_ok()
}

/// Verify G Hub's WebSocket is reachable, without ever starting G Hub itself.
/// Returns true only when G Hub is already running and accepting connections.
///
/// Rationale: G Hub (Logitech's control panel) is a heavyweight companion app
/// that the user runs intentionally. We must NOT spawn it on every poll cycle
/// when it's not running — that surprises the user (they closed G Hub for a
/// reason) and pollutes the system tray. If the user wants battery telemetry
/// they should launch G Hub themselves; we just consume its API.
fn ensure_lghub_running() -> bool {
    if is_lghub_port_ready() {
        return true;
    }
    // Port not open: G Hub is not running. Do nothing.
    false
}

/// Read battery from G Hub WebSocket. Returns (percentage, charging).
fn read_battery_ws() -> Option<(u8, bool)> {
    let mut request = "ws://localhost:9010".into_client_request().ok()?;
    request
        .headers_mut()
        .insert("Sec-WebSocket-Protocol", "json".parse().ok()?);

    let (mut ws, _) = connect(request).ok()?;

    // Read welcome OPTIONS message
    let _welcome = ws.read_message().ok()?;

    // Get device list
    ws.write_message(Message::binary(
        serde_json::json!({
            "path": "/devices/list",
            "verb": "GET"
        })
        .to_string(),
    ))
    .ok()?;

    let devices_msg = ws.read_message().ok()?;
    let devices_text = match devices_msg {
        Message::Text(t) => t,
        Message::Binary(b) => String::from_utf8_lossy(&b).to_string(),
        _ => return None,
    };

    let json: serde_json::Value = serde_json::from_str(&devices_text).ok()?;
    let device_infos = json["payload"]["deviceInfos"].as_array()?;

    // Find first wireless device
    let wireless = device_infos
        .iter()
        .find(|d| d["connectionType"] == "WIRELESS")?;
    let id = wireless["id"].as_str()?;

    // Read battery
    ws.write_message(Message::binary(
        serde_json::json!({
            "path": format!("/battery/{}/state", id),
            "verb": "GET"
        })
        .to_string(),
    ))
    .ok()?;

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
    if !ensure_lghub_running() {
        return None;
    }
    read_battery_ws().map(|(level, charging)| BatteryInfo::from_level(level, charging))
}

/// Start a process-lifetime worker that refreshes the battery cache immediately,
/// then every `interval_secs`. Call once at startup.
pub fn start_bg_poll(interval_secs: u64) {
    let interval = std::time::Duration::from_secs(interval_secs.max(1));
    if let Err(e) = std::thread::Builder::new()
        .name("battery-poll".to_string())
        .spawn(move || loop {
            let info = read_first_battery();
            let previous = *crate::device::cache::BATTERY.read();
            if previous != info {
                match info {
                    Some(value) => crate::log::write(&format!(
                        "battery: {}%{}",
                        value.percent,
                        if value.charging { " charging" } else { "" }
                    )),
                    None => crate::log::write("battery: device became unavailable"),
                }
                *crate::device::cache::BATTERY.write() = info;
            }
            std::thread::sleep(interval);
        })
    {
        crate::log::write(&format!("battery poll thread failed to start: {e}"));
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
    request
        .headers_mut()
        .insert("Sec-WebSocket-Protocol", "json".parse().ok()?);

    let (mut ws, _) = connect(request).ok()?;
    let _welcome = ws.read_message().ok()?;

    ws.write_message(Message::binary(
        serde_json::json!({
            "path": "/devices/list",
            "verb": "GET"
        })
        .to_string(),
    ))
    .ok()?;

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
