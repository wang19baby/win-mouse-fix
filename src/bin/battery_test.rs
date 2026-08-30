use tungstenite::{connect, Message};
use tungstenite::client::IntoClientRequest;

fn main() {
    println!("=== Battery Auto-Start + WebSocket Test ===\n");

    ensure_lghub();

    let mut request = "ws://localhost:9010".into_client_request().unwrap();
    request.headers_mut().insert(
        "Sec-WebSocket-Protocol",
        "json".parse().unwrap(),
    );

    let (mut ws, _) = match connect(request) {
        Ok(c) => c,
        Err(e) => { println!("Failed: {}", e); return; }
    };

    let _welcome = ws.read_message().unwrap();

    ws.write_message(Message::binary(
        serde_json::json!({
            "path": "/devices/list",
            "verb": "GET"
        }).to_string(),
    )).unwrap();

    let devices_msg = ws.read_message().unwrap();
    let devices_text = match devices_msg {
        Message::Text(t) => t,
        Message::Binary(b) => String::from_utf8_lossy(&b).to_string(),
        _ => { println!("Unexpected"); return; }
    };

    let json: serde_json::Value = serde_json::from_str(&devices_text).unwrap();
    let device_infos = json["payload"]["deviceInfos"].as_array().unwrap();
    let wireless: Vec<_> = device_infos.iter()
        .filter(|d| d["connectionType"] == "WIRELESS")
        .collect();

    println!("Found {} wireless device(s)\n", wireless.len());

    for device in &wireless {
        let id = device["id"].as_str().unwrap();
        let name = device["displayName"].as_str().unwrap();
        println!("Device: {} (id={})", name, id);

        ws.write_message(Message::binary(
            serde_json::json!({
                "path": format!("/battery/{}/state", id),
                "verb": "GET"
            }).to_string(),
        )).unwrap();

        let bat_msg = ws.read_message().unwrap();
        let bat_text = match bat_msg {
            Message::Text(t) => t,
            Message::Binary(b) => String::from_utf8_lossy(&b).to_string(),
            _ => { println!("  Unexpected"); continue; }
        };

        let bat_json: serde_json::Value = serde_json::from_str(&bat_text).unwrap();
        let percentage = bat_json["payload"]["percentage"].as_u64().unwrap_or(0);
        let charging = bat_json["payload"]["charging"].as_bool().unwrap_or(false);
        println!("  Battery: {}%{}\n", percentage, if charging { " (charging)" } else { "" });
    }

    ws.close(None).unwrap();
    println!("Done!");
}

fn ensure_lghub() {
    let running = std::process::Command::new("tasklist")
        .args(["/FI", "IMAGENAME eq lghub_agent.exe", "/NH"])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).contains("lghub_agent"))
        .unwrap_or(false);

    if running {
        println!("G Hub already running\n");
        return;
    }

    println!("G Hub not running, starting...");
    let paths = [
        "C:\\Program Files\\LGHUB\\system_tray\\lghub_system_tray.exe",
        "C:\\Program Files (x86)\\LGHUB\\system_tray\\lghub_system_tray.exe",
    ];
    for p in &paths {
        if std::path::Path::new(p).exists() {
            println!("Starting: {}", p);
            let _ = std::process::Command::new(p).arg("--minimized").spawn();
            println!("Waiting 8s for G Hub to initialize...");
            std::thread::sleep(std::time::Duration::from_secs(8));
            return;
        }
    }
    println!("G Hub not found in standard paths!");
}
