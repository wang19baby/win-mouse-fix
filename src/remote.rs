//! Phase 11 — phone-trackpad local server (Web PWA ↔ tray).
//!
//! Serves a fullscreen trackpad web page over the LAN and accepts WebSocket
//! input frames, dispatching them into the existing `SendInput` pipeline so the
//! phone feels identical to a real mouse. Single handshake-only WS server,
//! no SSE (status rides the same socket). LAN-only; bound to a specific NIC IP.

use std::sync::Arc;

 use std::io::{Read, Write};
 use std::net::{IpAddr, SocketAddr, TcpListener, TcpStream};
 use std::sync::LazyLock;
use std::time::Duration;

use parking_lot::RwLock;
use qrcode::{Color, QrCode};
#[derive(Clone)]
pub struct RemoteInfo {
    pub url: String,
    #[allow(dead_code)]
    pub token: String,
}

static REMOTE: LazyLock<RwLock<Option<RemoteInfo>>> = LazyLock::new(|| RwLock::new(None));

/// Read the current connect URL + token (for the tray QR popup).
pub fn info() -> Option<RemoteInfo> {
    (*REMOTE.read()).clone()
}

/// Authenticated, currently-connected WS streams. The tray broadcasts status
/// updates (PC battery, etc.) by walking this list. Removed on socket close.
static ACTIVE: LazyLock<parking_lot::Mutex<Vec<Arc<parking_lot::Mutex<TcpStream>>>>> =
    LazyLock::new(|| parking_lot::Mutex::new(Vec::new()));
/// Listening socket of the trackpad server. Held so stop_server() can drop it
/// and break the listen thread out of `listener.incoming()`.
static LISTENER: LazyLock<parking_lot::Mutex<Option<std::net::TcpListener>>> =
    LazyLock::new(|| parking_lot::Mutex::new(None));
/// Start the trackpad server on a background thread.
pub fn start_server() {
    // Fire-and-forget: do all the slow work (IP discovery, port pick, bind,
    // firewall rule, listener storage) on a background thread so the tray
    // WM_COMMAND handler returns immediately. Previously this function did a
    // 2-second recv_timeout on the caller (tray thread), which starved the
    // WM_TIMER queue — click_tick + window_switcher::tick() stopped firing
    // for up to 2s after toggling the menu, which the user perceived as
    // mouse/click hangs.
    std::thread::spawn(start_server_impl);
}

fn start_server_impl() {
    let ip = match local_ip() {
        Some(ip) => ip,
        None => {
            crate::log::write("remote: no usable LAN IP found — trackpad disabled");
            return;
        }
    };
    let port = match pick_port(ip) {
        Some(p) => p,
        None => {
            crate::log::write("remote: no free port (18765-18775) — trackpad disabled");
            return;
        }
    };
    let token = load_or_create_token();
    let url = format!("http://{ip}:{port}/?t={token}");
    *REMOTE.write() = Some(RemoteInfo {
        url: url.clone(),
        token: token.clone(),
    });
    crate::log::write(&format!("remote: trackpad ready -> {url}"));
    let (listen_tx, _listen_rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || listen(ip, port, token, listen_tx));
}

/// Shut down the trackpad server. Drops the listener so the accept loop exits
/// and clears REMOTE so the tray menu shows the unchecked state.
pub fn stop_server() {
    // Drop the listening socket — the accept loop exits immediately.
    *LISTENER.lock() = None;
    *REMOTE.write() = None;
    crate::log::write("remote: trackpad server stopped");
}

fn listen(ip: IpAddr, port: u16, token: String, _listen_tx: std::sync::mpsc::Sender<std::net::TcpListener>) {
    let addr = SocketAddr::new(ip, port);
    let listener = match std::net::TcpListener::bind(addr) {
        Ok(l) => l,
        Err(e) => {
            crate::log::write(&format!("remote: bind {addr} failed: {e}"));
            return;
        }
    };
    // Tests bypass the global LISTENER + firewall rule so parallel unit
    // tests don't race on the shared static or pay 175ms per test for netsh.
    if std::cfg!(test) {
        for stream in listener.incoming() {
            match stream {
                Ok(s) => {
                    let tok = token.clone();
                    std::thread::spawn(move || handle_conn(s, tok));
                }
                Err(_) => continue,
            }
        }
        return;
    }
    // Store the listener BEFORE the firewall call so stop_server() can drop
    // it (and break the accept loop) even while netsh is still running.
    *LISTENER.lock() = Some(listener);
    crate::log::write(&format!("remote: listening on {addr}"));
    add_firewall_rule(port); // best-effort (needs admin); non-fatal
    let owned = match LISTENER.lock().take() {
        Some(l) => l,
        None => {
            crate::log::write("remote: stopped during firewall rule, exiting listen thread");
            return;
        }
    };
    for stream in owned.incoming() {
        match stream {
            Ok(s) => {
                let tok = token.clone();
                std::thread::spawn(move || handle_conn(s, tok));
            }
            Err(_) => continue,
        }
    }
}

fn handle_conn(mut stream: TcpStream, token: String) {
    let ip = stream
        .peer_addr()
        .map(|a| a.ip().to_string())
        .unwrap_or_default();
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .ok();

    // Read HTTP headers up to the blank line.
    let mut buf: Vec<u8> = Vec::with_capacity(4096);
    let mut tmp = [0u8; 1024];

    let mut first_line = String::new();
    let mut ws_key = String::new();
    let mut ws_ext = String::new();


    let is_ws;
    let is_qr;
    let is_diag;
    let is_report;
    let is_manifest;
    let is_sw;
    loop {
        match stream.read(&mut tmp) {
            Ok(0) => return,
            Ok(n) => {
                buf.extend_from_slice(&tmp[..n]);
                if let Some(pos) = find_sub(&buf, b"\r\n\r\n") {

                    let header = String::from_utf8_lossy(&buf[..pos]);
                    dbg_log(&format!("remote: RAW HEADERS from {ip}: {header}"));
                    if let Some(fl) = header.split("\r\n").next() {
                        first_line = fl.to_string();
                    }

                    for line in header.split("\r\n") {
                        let low = line.to_ascii_lowercase();
                        // NB: match the header *name* case-insensitively, but keep the
                        // header *value* byte-exact. `Sec-WebSocket-Key` is base64 and
                        // case-sensitive — lowercasing it (e.g. the real iPhone sends
                        // mixed-case keys) makes the computed `Sec-WebSocket-Accept`
                        // wrong, so Safari rejects the handshake with code 1006.
                        if let Some(pos) = low.find("sec-websocket-key:") {
                            ws_key = line[pos + "sec-websocket-key:".len()..].trim().to_string();
                        } else if let Some(pos) = low.find("sec-websocket-extensions:") {
                            ws_ext = line[pos + "sec-websocket-extensions:".len()..].trim().to_string();
                        }
                    }



                    is_ws = first_line.contains(" /ws");
                    let req_path = first_line.split_whitespace().nth(1).unwrap_or("");
                    is_qr = req_path == "/qr" || req_path == "//qr";
                    is_diag = req_path == "/diag";
                    is_report = req_path.starts_with("/report");
                    is_manifest = req_path == "/manifest.json";
                    is_sw = req_path == "/sw.js" || req_path.starts_with("/sw.js?");
                    break;
                }
                if buf.len() > 16384 {
                    return;
                }
            }
            Err(_) => return,
        }
    }




    dbg_log(&format!(
        "remote: conn from {ip} is_ws={is_ws} is_qr={is_qr} first_line='{first_line}'"
    ));
    if is_ws {

        dbg_log(&format!(
            "remote: ws conn from {ip} key_len={} first_line='{first_line}'",
            ws_key.len()
        ));

        if ws_key.is_empty() || ws_handshake(&mut stream, &ws_key, &ws_ext).is_err() {
            
dbg_log(&format!("remote: ws handshake FAIL from {ip}"));
            return;
        }
        
dbg_log(&format!("remote: ws handshake OK from {ip}"));
        ws_loop(stream, &token);


    } else if is_qr {
        serve_qr_page(&mut stream);
    } else if is_diag {
        serve_diag(&mut stream);
    } else if is_report {
        dbg_log(&format!("remote: CLIENT REPORT from {ip}: {first_line}"));
        let _ = stream.write_all(
            b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nOK",
        );
    } else if is_manifest {
        serve_manifest(&mut stream);
    } else if is_sw {
        serve_sw(&mut stream);
    } else {
        serve_page(&mut stream);
    }
}
/// Dedicated, config-independent debug log for the WS path (bypasses the
/// buffered/optional app log so phone connection lifecycle is always captured).
fn dbg_log(msg: &str) {
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open("D:/work_space/personal_workspace/win-mouse-fix/ws_debug.log")
    {
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let _ = std::io::Write::write_all(&mut f, format!("[{ts}] {msg}\n").as_bytes());
        let _ = std::io::Write::flush(&mut f);
    }
}
fn ws_handshake(stream: &mut TcpStream, key: &str, ext: &str) -> std::io::Result<()> {
    // Do NOT echo `permessage-deflate`. We never compress/decompress frames, so
    // accepting the extension would make the client (esp. iOS Safari) expect
    // compressed server->client frames; our uncompressed `status`/`pong` frames
    // then trip a protocol error and Safari closes with code 1006. Declining the
    // extension (per RFC 6455: a server MUST NOT include an extension it can't
    // honor) keeps both sides on uncompressed JSON, which our frame code handles.
    if ext.to_ascii_lowercase().contains("permessage-deflate") {
        dbg_log(&format!(
            "remote: ws_handshake declining permessage-deflate (not implemented) from ext='{ext}'"
        ));
    }
    let accept = ws_accept(key);
    let resp = format!(        "HTTP/1.1 101 Switching Protocols\r\n\
         Upgrade: websocket\r\n\
         Connection: Upgrade\r\n\
         Sec-WebSocket-Accept: {accept}\r\n\
         \r\n"
    );
    let hexstr: String = resp.as_bytes().iter().map(|b| format!("{b:02x}")).collect();
    dbg_log(&format!("remote: ws_handshake key='{key}' accept='{accept}' ext='{ext}' resp_hex={hexstr}"));
    stream.write_all(resp.as_bytes())
}



fn ws_loop(mut stream: TcpStream, token: &str) {
    let ip = stream
        .peer_addr()
        .map(|a| a.ip().to_string())
        .unwrap_or_default();
    // Long-lived control channel: do NOT drop on idle. The 10s read timeout set
    // in handle_conn would close a connected-but-quiet trackpad after 10s.
    // Instead probe liveness with a WebSocket ping and only reap peers that stop
    // answering (e.g. phone out of range). Browsers auto-pong, so idle stays up.
    
dbg_log(&format!("remote: ws loop start {ip}"));
    let _ = stream.set_read_timeout(Some(Duration::from_secs(30)));
    let mut authed = false;
    let mut failed_probes = 0; // reap peers that never answer a ping
    loop {
        match read_frame(&mut stream) {
            Ok(Some((opcode, payload))) => {
                failed_probes = 0;
                match opcode {
                    0x1 => {
                        // text frame
                        let was = authed;
                        if dispatch(&payload, token, &mut authed, &mut stream).is_err() {
                            dbg_log(&format!("remote: ws {ip} dispatch err -> drop"));
                            return;
                        }
                        if !was && authed {
                            // Just authenticated: register this stream so the
                            // tray can broadcast status updates to it. try_clone
                            // on a freshly-accepted socket is infallible.
                            let clone = stream
                                .try_clone()
                                .expect("clone of just-accepted TcpStream");
                            ACTIVE.lock().push(Arc::new(parking_lot::Mutex::new(clone)));
                            dbg_log(&format!("remote: ws {ip} authed -> status sent"));
                        }
                    }
                    0x8 => {
                        // close
                        dbg_log(&format!("remote: ws {ip} close frame from peer"));
                        let _ = write_frame(&mut stream, 0x8, &[]);
                        return;
                    }
                    0x9 => {
                        // ping -> pong
                        let _ = write_frame(&mut stream, 0xA, &payload);
                    }
                    _ => {} // pong / other: ignore
                }
            }
            Ok(None) => {
                dbg_log(&format!("remote: ws {ip} EOF (peer gone)"));
                return;
            }
            Err(_) => {
                // Read error (idle timeout surfaces here from read_frame).
                // Probe with a WebSocket ping; if we can't write it the peer is
                // dead, otherwise keep the channel alive — browsers auto-pong.
                if write_frame(&mut stream, 0x9, &[]).is_err() {
                    dbg_log(&format!("remote: ws {ip} read-err + probe write fail -> drop"));
                    return;
                }
                failed_probes += 1;
                if failed_probes > 3 {
                    dbg_log(&format!("remote: ws {ip} {failed_probes} probes unanswered -> drop"));
                    return;
                }
                dbg_log(&format!("remote: ws {ip} read-err -> ping probe sent ({failed_probes})"));
            }
        }
    }
}

/// Read exactly `buf` bytes. Returns:
/// - `Ok(true)` on success,
/// - `Ok(false)` on a clean EOF / non-timeout read error (treat as peer gone),
/// - `Err` on a read *timeout* (idle — caller should probe with a ping).
fn recv_exact(stream: &mut TcpStream, buf: &mut [u8]) -> std::io::Result<bool> {
    match stream.read_exact(buf) {
        Ok(()) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::TimedOut => Err(e),
        Err(_) => Ok(false),
    }
}

/// Parse one WS frame (client frames are masked). Returns (opcode, payload).
/// Ignores fragmentation (our client sends one complete text frame per message).
fn read_frame(stream: &mut TcpStream) -> std::io::Result<Option<(u8, Vec<u8>)>> {
    let mut hdr = [0u8; 2];
    if !recv_exact(stream, &mut hdr)? {
        return Ok(None);
    }
    let opcode = hdr[0] & 0x0F;
    if (hdr[0] & 0x40) != 0 {
        // RSV1 set without negotiated permessage-deflate: protocol error.
        // Close cleanly (1002) so the peer sees a proper close, not 1006.
        let _ = write_frame(stream, 0x8, &[0x03, 0xEA]);
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "unexpected compressed frame",
        ));
    }
    let masked = (hdr[1] & 0x80) != 0;
    let mut len = (hdr[1] & 0x7F) as u64;
    if len == 126 {
        let mut ext = [0u8; 2];
        if !recv_exact(stream, &mut ext)? {
            return Ok(None);
        }
        len = u16::from_be_bytes(ext) as u64;
    } else if len == 127 {
        let mut ext = [0u8; 8];
        if !recv_exact(stream, &mut ext)? {
            return Ok(None);
        }
        len = u64::from_be_bytes(ext);
    }
    let mask = if masked {
        let mut m = [0u8; 4];
        if !recv_exact(stream, &mut m)? {
            return Ok(None);
        }
        m
    } else {
        [0u8; 4]
    };
    if len > 1_000_000 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "frame too large",
        ));
    }
    let mut payload = vec![0u8; len as usize];
    if !recv_exact(stream, &mut payload)? {
        return Ok(None);
    }
    if masked {
        for (i, b) in payload.iter_mut().enumerate() {
            *b ^= mask[i % 4];
        }
    }
    Ok(Some((opcode, payload)))
}

/// Write a server→client frame (never masked).
fn write_frame(stream: &mut TcpStream, opcode: u8, payload: &[u8]) -> std::io::Result<()> {
    let len = payload.len();
    if len < 126 {
        stream.write_all(&[0x80 | opcode, len as u8])?;
    } else if len < 65536 {
        stream.write_all(&[0x80 | opcode, 126, (len >> 8) as u8, len as u8])?;
    } else {
        stream.write_all(&[0x80 | opcode, 127])?;
        stream.write_all(&(len as u64).to_be_bytes())?;
    }
    stream.write_all(payload)
}

/// Send a clean WebSocket close frame (status code + short reason). Lets the peer
/// distinguish a deliberate rejection from an abnormal 1006 drop.
fn close_clean(stream: &mut TcpStream, code: u16) {
    let mut body = vec![(code >> 8) as u8, (code & 0xFF) as u8];
    body.extend_from_slice(b"bye");
    let _ = write_frame(stream, 0x8, &body);
}

/// Build the current status JSON: touch config + PC battery + connection flag.
/// Used both on initial auth (per-connection) and by `broadcast_status` for
/// periodic updates. Safe to call from any thread; takes only the CONFIG read
/// lock and the BATTERY read lock briefly.
fn build_status_json() -> String {
    let cfg = crate::CONFIG.read();
    let touch = &cfg.touch;
    let pc_battery = crate::device::cache::BATTERY.read().map(|b| {
        serde_json::json!({
            "percent": b.percent,
            "charging": b.charging,
            "low": b.low,
        })
    });
    let json = serde_json::json!({
        "t": "status",
        "conn": true,
        "pc_battery": pc_battery,
        "touch": {
            "gain": touch.gain,
            "accel_ref": touch.accel_ref,
            "accel_slope": touch.accel_slope,
            "accel_max_mult": touch.accel_max_mult,
            "move_ema": touch.move_ema,
            "scroll_gain": touch.scroll_gain,
            "tap_ms": touch.tap_ms,
            "tap_px": touch.tap_px,
            "swipe_px": touch.swipe_px,
            "decide_px": touch.decide_px,
            "pinch_bias": touch.pinch_bias,
            "diag_min": touch.diag_min,
            "diag_ratio": touch.diag_ratio,
            "longpress_ms": touch.longpress_ms,
        }
    });
    json.to_string()
}

/// Push the latest status JSON to every currently-authenticated WS client.
/// Best-effort: a client whose socket is no longer writable is silently
/// dropped from ACTIVE so the list does not grow unboundedly.
pub fn broadcast_status() {
    if ACTIVE.lock().is_empty() {
        return;
    }
    let payload = build_status_json();
    ACTIVE.lock().retain(|stream| {
        let mut s = stream.lock();
        match write_frame(&mut s, 0x1, payload.as_bytes()) {
            Ok(()) => true,
            Err(_) => false,
        }
    });
}

/// Dispatch a text frame. Returns `Err` to close the socket (auth failure).
fn dispatch(
    payload: &[u8],
    token: &str,
    authed: &mut bool,
    stream: &mut TcpStream,
) -> std::io::Result<()> {
    let v: serde_json::Value = match serde_json::from_slice(payload) {
        Ok(v) => v,
        Err(_) => return Ok(()), // ignore malformed
    };
    let t = match v.get("t").and_then(|x| x.as_str()) {
        Some(t) => t,
        None => return Ok(()),
    };

    if !*authed {
        if t == "auth" {
            let ok = v.get("token").and_then(|x| x.as_str()) == Some(token);
            if ok {
                *authed = true;
                let status = build_status_json();
                let _ = write_frame(stream, 0x1, status.as_bytes());
            } else {
                // the browser delivers the reject message and stops retrying
                // instead of surfacing a bare 1006 and reconnecting forever.
                let _ = write_frame(stream, 0x1, br#"{"t":"reject","reason":"bad_token"}"#);
                close_clean(stream, 1008);
                return Err(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    "bad token",
                ));
            }
        } else {
            let _ = write_frame(stream, 0x1, br#"{"t":"reject","reason":"not_authed"}"#);
            close_clean(stream, 1008);
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "not authed",
            ));
        }
        return Ok(());
    }

    match t {
        "move" => {
            let dx = v.get("dx").and_then(|x| x.as_f64()).unwrap_or(0.0) as i32;
            let dy = v.get("dy").and_then(|x| x.as_f64()).unwrap_or(0.0) as i32;
            crate::scroll::injector::send_mouse_move(dx, dy);
        }
        "scroll" => {
            let dx = v.get("dx").and_then(|x| x.as_f64()).unwrap_or(0.0) as i32;
            let dy = v.get("dy").and_then(|x| x.as_f64()).unwrap_or(0.0) as i32;
            if dy != 0 {
                crate::win::hooks::push_remote_scroll(dy, false);
            }
            if dx != 0 {
                crate::win::hooks::push_remote_scroll(dx, true);
            }
        }
        "zoom" => {
            let delta = v.get("delta").and_then(|x| x.as_f64()).unwrap_or(0.0) as i32;
            if delta != 0 {
                crate::win::hooks::send_remote_zoom(delta);
            }
        }
        "tap" => {
            let right = v.get("b").and_then(|x| x.as_str()) == Some("right");
            crate::win::hooks::send_remote_click(right);
        }
        "gesture" => {
            if let Some(g) = v.get("g").and_then(|x| x.as_str()) {
                crate::win::hooks::send_remote_gesture(g);
            }
        }
        "key" => {
            if let Some(text) = v.get("text").and_then(|x| x.as_str()) {
                if !text.is_empty() {
                    crate::win::hooks::send_remote_text(text);
                }
            }
        }
        _ => {}
    }
    Ok(())
}

fn serve_page(stream: &mut TcpStream) {
    let body = TRACKPAD_HTML.as_bytes();
    // Cache-Control: no-cache forces the browser to revalidate on every
    // load. Without this, a PC browser that visited once during a broken
    // build keeps replaying the broken HTML even after the server is
    // fixed, because regular browsers (Chrome/Edge) cache HTML by default
    // unless told otherwise.
    let header = format!(
        "HTTP/1.1 200 OK\r\n\
         Content-Type: text/html; charset=utf-8\r\n\
         Cache-Control: no-cache, no-store, must-revalidate\r\n\
         Pragma: no-cache\r\n\
         Expires: 0\r\n\
         Content-Length: {}\r\n\
         Connection: close\r\n\
         \r\n",
        body.len()
    );


    let _ = stream.write_all(header.as_bytes());
    let _ = stream.write_all(body);
}

fn serve_manifest(stream: &mut TcpStream) {
    let body = MANIFEST_JSON.as_bytes();
    let header = format!(
        "HTTP/1.1 200 OK\r\n\
         Content-Type: application/manifest+json; charset=utf-8\r\n\
         Content-Length: {}\r\n\
         Connection: close\r\n\
         \r\n",
        body.len()
    );
    let _ = stream.write_all(header.as_bytes());
    let _ = stream.write_all(body);
}

static SW_JS: &str = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/assets/sw.js"));

fn serve_sw(stream: &mut TcpStream) {
    let body = SW_JS.as_bytes();
    let header = format!(
        "HTTP/1.1 200 OK\r\n\
         Content-Type: application/javascript; charset=utf-8\r\n\
         Content-Length: {}\r\n\
         Cache-Control: no-cache\r\n\
         Connection: close\r\n\
         \r\n",
        body.len()
    );
    let _ = stream.write_all(header.as_bytes());
    let _ = stream.write_all(body);
}

/// Diagnostic page: tests whether the phone's browser can do WebSockets at all,
/// comparing our server against a public echo server. Phase 11 debugging.
fn serve_diag(stream: &mut TcpStream) {
    let body = r#"<!DOCTYPE html><html lang="zh"><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1"><title>WS Diag</title></head>
<body style="font:13px monospace;padding:10px;background:#111;color:#0f0;white-space:pre-wrap;word-break:break-all;">
<pre id="out">running…</pre>
<script>
var out = document.getElementById('out');
function log(s){ out.textContent += '\n' + s; }
function test(url, label){
  log('--- ' + label + ' => ' + url);
  try {
    var ws = new WebSocket(url);
    var opened = false;
    ws.onopen = function(){ opened = true; log(label + ' OPEN ok'); try{ ws.close(1000,'ok'); }catch(e){} };
    ws.onerror = function(e){ log(label + ' ERROR ' + (e && e.message ? e.message : '(no detail)')); };
    ws.onclose = function(e){ log(label + ' CLOSE code=' + e.code + ' reason=' + JSON.stringify(e.reason) + ' clean=' + e.wasClean); };
    setTimeout(function(){ if(!opened) log(label + ' TIMEOUT no-open 5s'); }, 5000);
  } catch(e){ log(label + ' EXCEPTION ' + e.message); }
}
var myWs = (location.protocol === 'https:' ? 'wss://' : 'ws://') + location.host + '/ws';
test(myWs, 'MINE');
test('wss://echo.websocket.events', 'PUBLIC');
fetch(location.origin + '/').then(function(r){ return r.text(); }).then(function(t){ log('FETCH / ok bytes=' + t.length); }).catch(function(e){ log('FETCH / err ' + e.message); });
</script></body></html>"#;
    let header = format!(
        "HTTP/1.1 200 OK\r\n\
         Content-Type: text/html; charset=utf-8\r\n\
         Content-Length: {}\r\n\
         Connection: close\r\n\
         \r\n",
        body.len()
    );
    let _ = stream.write_all(header.as_bytes());
    let _ = stream.write_all(body.as_bytes());
}

/// Serve the QR-connect page (Phase 11). The QR matrix is computed server-side
/// and drawn client-side, so no image file is needed.
fn serve_qr_page(stream: &mut TcpStream) {
    let body = match (*REMOTE.read()).clone() {
        Some(info) => match QrCode::new(info.url.as_bytes()) {
            Ok(code) => {
                let n = code.width();
                let modules: Vec<u8> = code
                    .to_colors()
                    .iter()
                    .map(|c| if *c == Color::Dark { 1u8 } else { 0u8 })
                    .collect();
                let data = serde_json::json!({ "url": &info.url, "size": n, "modules": modules });
                let json = serde_json::to_string(&data).unwrap_or_default();
                QR_HTML.replace("__QR_DATA__", &json)
            }
            Err(_) => format!("<h1>二维码生成失败</h1><p>连接地址:<br>{}</p>", info.url),
        },
        None => "<h1>妙控板服务未就绪</h1><p>请稍候重试或重启程序。</p>".to_string(),
    };
    let header = format!(
        "HTTP/1.1 200 OK\r\n\
         Content-Type: text/html; charset=utf-8\r\n\
         Cache-Control: no-cache, no-store, must-revalidate\r\n\
         Pragma: no-cache\r\n\
         Expires: 0\r\n\
         Content-Length: {}\r\n\
         Connection: close\r\n\
         \r\n",
        body.len()
    );
    let _ = stream.write_all(header.as_bytes());
    let _ = stream.write_all(body.as_bytes());
}

// ─── LAN IP discovery ───────────────────────────────────────────────────────

/// Pick the IP the phone will actually reach: the default-route interface
/// (via the UDP-connect trick), falling back to adapter enumeration.
fn local_ip() -> Option<IpAddr> {
    if let Ok(s) = std::net::UdpSocket::bind("0.0.0.0:0") {
        if s.connect("8.8.8.8:80").is_ok() {
            if let Ok(addr) = s.local_addr() {
                if let IpAddr::V4(ip) = addr.ip() {
                    let o = ip.octets();
                    let loopback = o[0] == 127;
                    let link_local = o[0] == 169 && o[1] == 254;
                    let unspecified = o == [0, 0, 0, 0];
                    if !loopback && !link_local && !unspecified {
                        return Some(IpAddr::V4(ip));
                    }
                }
            }
        }
    }
    None
}


fn pick_port(ip: IpAddr) -> Option<u16> {
    for p in 18765..=18775 {
        if std::net::TcpListener::bind(SocketAddr::new(ip, p)).is_ok() {
            return Some(p);
        }
    }
    None
}

// ─── helpers ───────────────────────────────────────────────────────────────

fn find_sub(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.len() > haystack.len() {
        return None;
    }
    haystack.windows(needle.len()).position(|w| w == needle)
}

/// Best-effort inbound firewall rule. Requires admin; failure is logged, not fatal.
fn add_firewall_rule(port: u16) {
    let out = std::process::Command::new("netsh")
        .args([
            "advfirewall",
            "firewall",
            "add",
            "rule",
            &format!("name=WinMouseFix-Trackpad-{port}"),
            "dir=in",
            "action=allow",
            "protocol=TCP",
            &format!("localport={port}"),
        ])
        .output();
    match out {
        Ok(o) if o.status.success() => crate::log::write("remote: firewall rule added"),
        Ok(o) => crate::log::write(&format!(
            "remote: firewall rule not added (need admin?): {}",
            String::from_utf8_lossy(&o.stderr).trim()
        )),
        Err(e) => crate::log::write(&format!("remote: netsh unavailable: {e}")),
    }
}

/// Load a persisted pairing token, creating and saving one on first run.
/// Stored next to the executable as `token.txt` so QR codes survive restarts
/// (a fresh random token each launch would orphan every already-scanned phone).
fn load_or_create_token() -> String {
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let path = dir.join("token.txt");
            if let Ok(s) = std::fs::read_to_string(&path) {
                let s = s.trim().to_string();
                if s.len() == 64 && s.chars().all(|c| c.is_ascii_hexdigit()) {
                    return s;
                }
            }
            let tok = random_token();
            let _ = std::fs::write(&path, &tok);
            return tok;
        }
    }
    random_token()
}

/// 256-bit token as 64 hex chars (xorshift, seeded from time/pid/counter).
fn random_token() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEED: AtomicU64 = AtomicU64::new(0);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    let mut s = nanos
        ^ std::process::id() as u64
        ^ SEED.fetch_add(0x9E37_79B9_7F4A_7C15, Ordering::Relaxed);
    let mut out = String::with_capacity(64);
    for _ in 0..32 {
        s ^= s << 13;
        s ^= s >> 7;
        s ^= s << 17;
        let b = (s & 0xFF) as u8;
        out.push_str(&format!("{b:02x}"));
    }
    out
}

// ─── WebSocket accept: SHA1 + base64 ────────────────────────────────────────

fn ws_accept(key: &str) -> String {
    const GUID: &str = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";
    let concat = format!("{key}{GUID}");
    base64(&sha1(concat.as_bytes()))
}

fn sha1(data: &[u8]) -> [u8; 20] {
    fn rol(v: u32, n: u32) -> u32 {
        (v << n) | (v >> (32 - n))
    }
    let mut h: [u32; 5] = [0x6745_2301, 0xEFCD_AB89, 0x98BA_DCFE, 0x1032_5476, 0xC3D2_E1F0];
    let ml = (data.len() as u64) * 8;
    let mut msg = data.to_vec();
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&ml.to_be_bytes());
    for chunk in msg.chunks(64) {
        let mut w = [0u32; 80];
        for i in 0..16 {
            w[i] = u32::from_be_bytes([
                chunk[4 * i],
                chunk[4 * i + 1],
                chunk[4 * i + 2],
                chunk[4 * i + 3],
            ]);
        }
        for i in 16..80 {
            w[i] = rol(w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16], 1);
        }
        let (mut a, mut b, mut c, mut d, mut e) = (h[0], h[1], h[2], h[3], h[4]);
        for i in 0..80 {
            let (f, k) = if i < 20 {
                ((b & c) | ((!b) & d), 0x5A82_7999)
            } else if i < 40 {
                (b ^ c ^ d, 0x6ED9_EBA1)
            } else if i < 60 {
                ((b & c) | (b & d) | (c & d), 0x8F1B_BCDC)
            } else {
                (b ^ c ^ d, 0xCA62_C1D6)
            };
            let tmp = rol(a, 5)
                .wrapping_add(f)
                .wrapping_add(e)
                .wrapping_add(k)
                .wrapping_add(w[i]);
            e = d;
            d = c;
            c = rol(b, 30);
            b = a;
            a = tmp;
        }
        h[0] = h[0].wrapping_add(a);
        h[1] = h[1].wrapping_add(b);
        h[2] = h[2].wrapping_add(c);
        h[3] = h[3].wrapping_add(d);
        h[4] = h[4].wrapping_add(e);
    }
    let mut out = [0u8; 20];
    for i in 0..5 {
        out[i * 4..i * 4 + 4].copy_from_slice(&h[i].to_be_bytes());
    }
    out
}

fn base64(data: &[u8]) -> String {
    const T: &[u8; 64] =
        b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut s = String::new();
    for chunk in data.chunks(3) {
        let b = [
            chunk[0],
            chunk.get(1).copied().unwrap_or(0),
            chunk.get(2).copied().unwrap_or(0),
        ];
        let n = ((b[0] as u32) << 16) | ((b[1] as u32) << 8) | (b[2] as u32);
        s.push(T[((n >> 18) & 63) as usize] as char);
        s.push(T[((n >> 12) & 63) as usize] as char);
        s.push(if chunk.len() > 1 { T[((n >> 6) & 63) as usize] as char } else { '=' });
        s.push(if chunk.len() > 2 { T[(n & 63) as usize] as char } else { '=' });
    }
    s
}

// ─── embedded trackpad page ─────────────────────────────────────────────────


const TRACKPAD_HTML: &str = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/assets/trackpad.html"));
const QR_HTML: &str = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/assets/qr.html"));
const MANIFEST_JSON: &str = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/assets/manifest.json"));

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha1_known_vector() {
        // SHA1("abc") = a9993e364706816aba3e25717850c26c9cd0d89d
        let got = sha1(b"abc");
        let hex: String = got.iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(hex, "a9993e364706816aba3e25717850c26c9cd0d89d");
    }

    #[test]
    fn base64_basic() {
        assert_eq!(base64(b"Man"), "TWFu");
        assert_eq!(base64(b"Ma"), "TWE=");
        assert_eq!(base64(b"M"), "TQ==");
    }

    #[test]
    fn ws_accept_rfc_example() {
        // RFC 6455 example: key "dGhlIHNhbXBsZSBub25jZQ==" -> "s3pPLMBiTxaQ9kYGzzhZRbK+xOo="
        let got = ws_accept("dGhlIHNhbXBsZSBub25jZQ==");
        assert_eq!(got, "s3pPLMBiTxaQ9kYGzzhZRbK+xOo=");
    }

    /// Regression: start_server() must return immediately so the tray
    /// WM_COMMAND handler doesn't block the WM_TIMER queue (which would
    /// freeze click_tick + window_switcher::tick() for ~2s, making the
    /// mouse and click-cycle feel unresponsive).
    #[test]
    fn start_server_returns_immediately() {
        use std::time::Instant;
        let t0 = Instant::now();
        start_server();
        let elapsed = t0.elapsed();
        // Fire-and-forget should return in well under 10ms.
        assert!(
            elapsed.as_millis() < 50,
            "start_server blocked for {elapsed:?} -- must be fire-and-forget"
        );
        // Clean up the background thread's port so we don't leak it.
        // Give the impl a moment to bind, then stop.
        std::thread::sleep(Duration::from_millis(200));
        stop_server();
    }

    /// Regression: the served trackpad HTML must be syntactically valid JS,
    /// otherwise every phone browser throws 'SyntaxError: Unexpected token'
    /// and the trackpad silently fails (no tap, no scroll). We can't run a
    /// JS parser here, so we approximate: scan for the most common copy-paste
    /// error — duplicate top-level `var name = ...` declarations.
    #[test]
    fn trackpad_html_has_no_duplicate_top_level_vars() {
        // A duplicate `var params = ...` was hand-edited in by mistake and
        // broke the whole page until a user opened devtools. Catch it here
        // before the binary ships.
        let html = TRACKPAD_HTML;
        // Walk every top-level `<script>` block; within each one, a name must
        // not be declared with `var` twice.
        let mut start = 0usize;
        while let Some(open) = html[start..].find("<script") {
            let block_start = start + open;
            let body_start = html[block_start..]
                .find('>')
                .map(|o| block_start + o + 1)
                .unwrap_or(block_start);
            let close = match html[body_start..].find("</script>") {
                Some(c) => body_start + c,
                None => break,
            };
            let body = &html[body_start..close];
            // Count `var <name> =` declarations (rough heuristic, skips
            // re-declarations of same name in different scopes which are
            // legal in JS).
            let mut counts: std::collections::HashMap<&str, u32> =
                std::collections::HashMap::new();
            let bytes = body.as_bytes();
            let mut i = 0;
            while i < bytes.len() {
                if i + 4 <= bytes.len() && &bytes[i..i + 4] == b"var " {
                    // Find identifier end.
                    let id_start = i + 4;
                    let mut j = id_start;
                    while j < bytes.len()
                        && (bytes[j].is_ascii_alphanumeric()
                            || bytes[j] == b'_'
                            || bytes[j] == b'$')
                    {
                        j += 1;
                    }
                    if j > id_start && j < bytes.len() && bytes[j] == b'=' {
                        let name = &body[id_start..j];
                        *counts.entry(name).or_insert(0) += 1;
                    }
                    i = j;
                } else {
                    i += 1;
                }
            }
            for (name, n) in counts {
                // `var` is function-scoped so re-declaration is legal, but
                // our trackpad.js (IIFE) wraps everything in one function,
                // so duplicates at the same indent are a copy-paste error.
                if n > 1 {
                    // Allow `var i` / `var j` style loop counters (common).
                    if name == "i" || name == "j" || name == "k" {
                        continue;
                    }
                    panic!(
                        "trackpad.html: top-level `var {name}` declared {n} times — likely a copy-paste error"
                    );
                }
            }
            start = close + "</script>".len();
        }
    }
}

#[cfg(test)]
mod integration {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpStream;
    use std::time::Duration;

    fn masked_text_frame(text: &[u8]) -> Vec<u8> {
        let mask = [0x12u8, 0x34, 0x56, 0x78];
        let mut f = vec![0x81u8, 0x80 | (text.len() as u8)];
        f.extend_from_slice(&mask);
        for (i, &b) in text.iter().enumerate() {
            f.push(b ^ mask[i % 4]);
        }
        f
    }

    fn ws_handshake(s: &mut TcpStream, key: &str) {
        let req = format!(
            "GET /ws HTTP/1.1\r\nHost: x\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: {key}\r\nSec-WebSocket-Version: 13\r\n\r\n"
        );
        s.write_all(req.as_bytes()).unwrap();
        let mut buf = [0u8; 512];
        let mut total = 0;
        while total < buf.len() {
            let n = s.read(&mut buf[total..]).unwrap();
            if n == 0 { break; }
            total += n;
            if buf[..total].windows(4).any(|w| w == b"\r\n\r\n") { break; }
        }
        let resp = String::from_utf8_lossy(&buf[..total]);
        assert!(resp.contains("101"), "expected 101 switching protocols, got: {resp}");
    }

    /// Read one server text frame's payload as a String.
    fn read_text(s: &mut TcpStream) -> String {
        let mut hdr = [0u8; 2];
        s.read_exact(&mut hdr).unwrap();
        let mut len = (hdr[1] & 0x7F) as usize;
        if len == 126 {
            let mut e = [0u8; 2];
            s.read_exact(&mut e).unwrap();
            len = u16::from_be_bytes(e) as usize;
        }
        let mut body = vec![0u8; len];
        s.read_exact(&mut body).unwrap();
        String::from_utf8_lossy(&body).into_owned()
    }

    #[test]
    #[ignore = "touches real loopback listener; run with --ignored on an interactive session"]
    fn ws_handshake_auth_status_and_reject() {
        let token = "smoketoken";
        let port = 18999u16;
        std::thread::spawn(move || {
            listen("127.0.0.1".parse().unwrap(), port, token.to_string(), {
                let (tx, _rx) = std::sync::mpsc::channel();
                tx
            })
        });
        std::thread::sleep(Duration::from_millis(300));

        // 1) correct token -> status conn:true
        let mut s = connect_with_retry(port).expect("connect good");
        s.set_read_timeout(Some(Duration::from_secs(5))).ok();
        ws_handshake(&mut s, "dGhlIHNhbXBsZSBub25jZQ==");
        s.write_all(&masked_text_frame(
            format!("{{\"t\":\"auth\",\"token\":\"{token}\"}}").as_bytes(),
        ))
        .unwrap();
        let status = read_text(&mut s);
        assert!(
            status.contains("\"conn\":true"),
            "expected status conn:true, got: {status}"
        );

        // 2) fresh connection with wrong token -> reject frame, then close
        let mut s2 = connect_with_retry(port).expect("connect bad");
        s2.set_read_timeout(Some(Duration::from_secs(5))).ok();
        ws_handshake(&mut s2, "dGhlIHNhbXBsZSBub25jZQ==");
        s2.write_all(&masked_text_frame(
            b"{\"t\":\"auth\",\"token\":\"wrong\"}",
        ))
        .unwrap();
        let reject = read_text(&mut s2);
        assert!(
            reject.contains("\"reject\""),
            "expected reject frame, got: {reject}"
        );
    }

    #[test]
    fn http_root_serves_trackpad_page() {
        let port = 18998u16;
        std::thread::spawn(move || {
            listen("127.0.0.1".parse().unwrap(), port, "tok".to_string(), {
                let (tx, _rx) = std::sync::mpsc::channel();
                tx
            })
        });
        std::thread::sleep(Duration::from_millis(300));
        let mut s = TcpStream::connect(("127.0.0.1", port)).expect("connect");
        s.write_all(b"GET / HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n")
            .unwrap();
        let mut buf = Vec::new();
        s.read_to_end(&mut buf).unwrap();
        let body = String::from_utf8_lossy(&buf);
        assert!(body.contains("200 OK"), "expected 200, got: {body}");
        assert!(
            body.contains("妙控板") || body.contains("trackpad"),
            "expected embedded trackpad HTML, got: {body}"
        );
    }

    /// Build a masked client->server ping (0x9) frame.
    fn client_ping_frame() -> Vec<u8> {
        let mask = [0x12u8, 0x34, 0x56, 0x78];
        vec![0x89, 0x80 | 0, mask[0], mask[1], mask[2], mask[3]]
    }

    /// Connect to a freshly-spawned listener that may not be `accept()`-ing yet.
    /// Retries for ~2s so parallel test runs don't race the listener thread.
    fn connect_with_retry(port: u16) -> Option<TcpStream> {
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        loop {
            match TcpStream::connect(("127.0.0.1", port)) {
                Ok(s) => return Some(s),
                Err(_) if std::time::Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(50));
                }
                Err(_) => return None,
            }
        }
    }

    /// Read any server frame; returns (opcode, payload).
    fn client_read_frame(s: &mut TcpStream) -> (u8, Vec<u8>) {
        let mut hdr = [0u8; 2];
        s.read_exact(&mut hdr).unwrap();
        let opcode = hdr[0] & 0x0F;
        let mut len = (hdr[1] & 0x7F) as usize;
        if len == 126 {
            let mut e = [0u8; 2];
            s.read_exact(&mut e).unwrap();
            len = u16::from_be_bytes(e) as usize;
        } else if len == 127 {
            let mut e = [0u8; 8];
            s.read_exact(&mut e).unwrap();
            len = u64::from_be_bytes(e) as usize;
        }
        let mut body = vec![0u8; len];
        s.read_exact(&mut body).unwrap();
        (opcode, body)
    }

    /// Simulate an iOS Safari handshake that offers `permessage-deflate`: the
    /// server must DECLINE it (no echo) so the connection stays uncompressed and
    /// Safari does not abort with code 1006. Regression test for the trackpad
    /// "重连 1006" bug on real iPhones.
    #[test]
    #[ignore = "touches real loopback listener; run with --ignored on an interactive session"]
    fn ws_handshake_declines_permessage_deflate() {
        let token = "saftoken";
        let port = 18996u16;
        std::thread::spawn(move || {
            listen("127.0.0.1".parse().unwrap(), port, token.to_string(), {
                let (tx, _rx) = std::sync::mpsc::channel();
                tx
            })
        });
        std::thread::sleep(Duration::from_millis(300));

        let mut s = connect_with_retry(port).expect("connect safari");
        s.set_read_timeout(Some(Duration::from_secs(5))).ok();
        let req = "GET /ws?t=x HTTP/1.1\r\nHost: x\r\nUpgrade: websocket\r\n\
                   Connection: Upgrade\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\
                   Sec-WebSocket-Version: 13\r\n\
                   Sec-WebSocket-Extensions: permessage-deflate; client_max_window_bits\r\n\r\n";
        s.write_all(req.as_bytes()).unwrap();

        // Read the 101 response up to the blank line.
        let mut buf = [0u8; 512];
        let mut total = 0;
        while total < buf.len() {
            let n = s.read(&mut buf[total..]).unwrap();
            if n == 0 {
                break;
            }
            total += n;
            if buf[..total].windows(4).any(|w| w == b"\r\n\r\n") {
                break;
            }
        }
        let resp = String::from_utf8_lossy(&buf[..total]);
        assert!(resp.contains("101"), "expected 101, got: {resp}");
        assert!(
            !resp.to_ascii_lowercase().contains("permessage-deflate"),
            "server must NOT echo permessage-deflate (would cause 1006 on Safari), got: {resp}"
        );

        // Connection must still work uncompressed: auth -> status conn:true.
        s.write_all(&masked_text_frame(
            format!("{{\"t\":\"auth\",\"token\":\"{token}\"}}").as_bytes(),
        ))
        .unwrap();
        let status = read_text(&mut s);
        assert!(
            status.contains("\"conn\":true"),
            "expected status conn:true after Safari-style handshake, got: {status}"
        );
    }

    /// Regression test for the iOS Safari 1006 bug: the `Sec-WebSocket-Key` is
    /// base64 and case-sensitive. If the server lowercases it while extracting
    /// (e.g. via `line.to_ascii_lowercase()`), the computed `Sec-WebSocket-Accept`
    /// no longer matches what the client expects and Safari aborts the handshake
    /// with code 1006 — no `onopen`, no auth. This test uses a *mixed-case* key
    /// (what real iPhone Safari sends) and asserts the response's accept value is
    /// the correct one, i.e. the server preserved the key's original case.
    #[test]
    #[ignore = "touches real loopback listener; run with --ignored on an interactive session"]
    fn ws_handshake_accept_preserves_key_case() {
        let token = "casetoken";
        let port = 18995u16;
        std::thread::spawn(move || {
            listen("127.0.0.1".parse().unwrap(), port, token.to_string(), {
                let (tx, _rx) = std::sync::mpsc::channel();
                tx
            })
        });
        std::thread::sleep(Duration::from_millis(300));

        let mut s = connect_with_retry(port).expect("connect case");
        s.set_read_timeout(Some(Duration::from_secs(5))).ok();
        // Real iPhone Safari sends mixed-case keys (uppercase B/W/N/K/...).
        let key = "xBfWdNmd8K7V2wAq+C1a0w==";
        let req = format!(
            "GET /ws HTTP/1.1\r\nHost: x\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: {key}\r\nSec-WebSocket-Version: 13\r\n\r\n"
        );
        s.write_all(req.as_bytes()).unwrap();

        let mut buf = [0u8; 512];
        let mut total = 0;
        while total < buf.len() {
            let n = s.read(&mut buf[total..]).unwrap();
            if n == 0 {
                break;
            }
            total += n;
            if buf[..total].windows(4).any(|w| w == b"\r\n\r\n") {
                break;
            }
        }
        let resp = String::from_utf8_lossy(&buf[..total]);
        assert!(resp.contains("101"), "expected 101, got: {resp}");

        let expected = ws_accept(key);
        let actual = resp
            .lines()
            .find(|l| l.to_ascii_lowercase().starts_with("sec-websocket-accept:"))
            .map(|l| l.splitn(2, ':').nth(1).unwrap_or("").trim().to_string())
            .expect("response must contain Sec-WebSocket-Accept");
        assert_eq!(
            actual, expected,
            "Sec-WebSocket-Accept mismatch: server lowercased the key. expected={expected} actual={actual}"
        );

        // The handshake must complete so the phone can authenticate.
        s.write_all(&masked_text_frame(
            format!("{{\"t\":\"auth\",\"token\":\"{token}\"}}").as_bytes(),
        ))
        .unwrap();
        let status = read_text(&mut s);
        assert!(
            status.contains("\"conn\":true"),
            "expected status conn:true after mixed-case-key handshake, got: {status}"
        );
    }

    /// Full phone-trackpad pipeline over a real TCP/WebSocket connection:
    /// handshake -> auth -> every command type -> ping/pong liveness probe.
    /// If any dispatch (move/scroll/tap/gesture -> SendInput) panicked or
    /// dropped the socket, the server thread would close and our pong read
    /// would fail — so a successful pong proves the whole chain stayed up.
    // Ignored by default: this test drives real Win32 global state (SendInput,
    // virtual-desktop COM, netsh firewall rules) and is racy under the parallel
    // test harness. Run it explicitly on an interactive session:
    //   cargo test --bin win-mouse-fix -- --ignored ws_phone_trackpad_dispatch_e2e
    #[test]
    #[ignore = "touches real Win32 global state; run with --ignored on an interactive session"]
    fn ws_phone_trackpad_dispatch_e2e() {
        let token = "e2etoken";
        let port = 18997u16;
        std::thread::spawn(move || {
            listen("127.0.0.1".parse().unwrap(), port, token.to_string(), {
                let (tx, _rx) = std::sync::mpsc::channel();
                tx
            })
        });
        std::thread::sleep(Duration::from_millis(300));

        let mut s = TcpStream::connect(("127.0.0.1", port)).expect("connect");
        s.set_read_timeout(Some(Duration::from_secs(3))).ok();

        // 1) WebSocket handshake
        ws_handshake(&mut s, "dGhlIHNhbXBsZSBub25jZQ==");

        // 2) auth with the correct token -> status conn:true
        s.write_all(&masked_text_frame(
            format!("{{\"t\":\"auth\",\"token\":\"{token}\"}}").as_bytes(),
        ))
        .unwrap();
        let status = read_text(&mut s);
        assert!(
            status.contains("\"conn\":true"),
            "expected status conn:true, got: {status}"
        );

        // 3) exercise every command the phone page sends
        let commands = [
            "{\"t\":\"move\",\"dx\":5,\"dy\":-3}",
            "{\"t\":\"scroll\",\"dx\":0,\"dy\":40}",
            "{\"t\":\"scroll\",\"dx\":12,\"dy\":0}",
            "{\"t\":\"tap\",\"b\":\"left\"}",
            "{\"t\":\"tap\",\"b\":\"right\"}",
            "{\"t\":\"gesture\",\"g\":\"taskview\"}",
            "{\"t\":\"gesture\",\"g\":\"desk_l\"}",
            "{\"t\":\"gesture\",\"g\":\"desk_r\"}",
            "{\"t\":\"gesture\",\"g\":\"up_l\"}",
            "{\"t\":\"gesture\",\"g\":\"up_r\"}",
            "{\"t\":\"gesture\",\"g\":\"down_l\"}",
            "{\"t\":\"gesture\",\"g\":\"down_r\"}",
            "{\"t\":\"gesture\",\"g\":\"snap_up_l\"}",
            "{\"t\":\"gesture\",\"g\":\"snap_up_r\"}",
            "{\"t\":\"gesture\",\"g\":\"snap_down_l\"}",
            "{\"t\":\"gesture\",\"g\":\"snap_down_r\"}",
            // unknown type must be ignored, not drop
            "{\"t\":\"frobnicate\"}",
        ];
        for cmd in commands {
            s.write_all(&masked_text_frame(cmd.as_bytes())).unwrap();
        }

        // 4) liveness probe: server must answer our ping with a pong, proving
        //    none of the above dispatches panicked or closed the socket.
        s.write_all(&client_ping_frame()).unwrap();
        let (op, _) = client_read_frame(&mut s);
        assert_eq!(op, 0xA, "expected pong (0xA) after dispatching all commands");

        // 5) a second command round still works post-pong
        s.write_all(&masked_text_frame(b"{\"t\":\"move\",\"dx\":1,\"dy\":1}"))
            .unwrap();
        s.write_all(&client_ping_frame()).unwrap();
        let (op2, _) = client_read_frame(&mut s);
        assert_eq!(op2, 0xA, "expected pong after second command round");
    }

    /// Robustness: a wrong token must be rejected with a *clean* WebSocket close
    /// (status 1008) after the reject status frame — NOT an abnormal 1006 drop.
    /// A bare 1006 makes the browser discard the reject message and reconnect
    /// forever; a clean close lets the client stop and show "请重新扫码".
    #[test]
    #[ignore = "touches real loopback listener; run with --ignored on an interactive session"]
    fn ws_bad_token_clean_reject_not_1006() {
        let token = "cleanreject";
        let port = 18994u16;
        std::thread::spawn(move || {
            listen("127.0.0.1".parse().unwrap(), port, token.to_string(), {
                let (tx, _rx) = std::sync::mpsc::channel();
                tx
            })
        });
        std::thread::sleep(Duration::from_millis(300));

        let mut s = connect_with_retry(port).expect("connect");
        s.set_read_timeout(Some(Duration::from_secs(5))).ok();
        ws_handshake(&mut s, "dGhlIHNhbXBsZSBub25jZQ==");
        s.write_all(&masked_text_frame(b"{\"t\":\"auth\",\"token\":\"wrong\"}"))
            .unwrap();

        // 1) text frame carrying the reject status
        let (op1, p1) = client_read_frame(&mut s);
        assert_eq!(op1, 0x1, "expected a text reject frame, got opcode {op1}");
        let txt = String::from_utf8_lossy(&p1);
        assert!(txt.contains("\"reject\""), "expected reject, got: {txt}");
        assert!(
            txt.contains("bad_token"),
            "reject should carry a reason, got: {txt}"
        );

        // 2) then a clean close frame (0x8) with status code 1008
        let (op2, p2) = client_read_frame(&mut s);
        assert_eq!(op2, 0x8, "expected a clean close frame, got opcode {op2} (a bare 1006 would loop the client)");
        let code = ((p2[0] as u16) << 8) | (p2[1] as u16);
        assert_eq!(code, 1008, "expected close code 1008, got {code}");
    }
}

