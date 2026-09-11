//! Phase 11 — phone-trackpad local server (Web PWA ↔ tray).
//!
//! Serves a fullscreen trackpad web page over the LAN and accepts WebSocket
//! input frames, dispatching them into the existing `SendInput` pipeline so the
//! phone feels identical to a real mouse. Single handshake-only WS server,
//! no SSE (status rides the same socket). LAN-only; bound to a specific NIC IP.

use std::io::{Read, Write};
use std::net::{IpAddr, Shutdown, SocketAddr, TcpListener, TcpStream};
use std::os::windows::process::CommandExt;
use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    mpsc::{Receiver, SyncSender},
    Arc, LazyLock,
};
use std::time::{Duration, Instant};

use parking_lot::{Mutex, RwLock};
use qrcode::{Color, QrCode};
use windows_sys::Win32::Security::Cryptography::{
    BCryptGenRandom, BCRYPT_USE_SYSTEM_PREFERRED_RNG,
};

const CREATE_NO_WINDOW: u32 = 0x08000000;
#[derive(Clone)]
pub struct RemoteInfo {
    pub url: String,
    pub pad_url: String,
}

static REMOTE: LazyLock<RwLock<Option<RemoteInfo>>> = LazyLock::new(|| RwLock::new(None));

/// Read the current connect URL + token (for the tray QR popup).
pub fn info() -> Option<RemoteInfo> {
    (*REMOTE.read()).clone()
}

/// Authenticated, currently-connected WS streams. The tray broadcasts status
/// updates (PC battery, etc.) by walking this list. A per-connection guard
/// removes each stream as soon as its reader exits.
static ACTIVE: LazyLock<Mutex<Vec<Arc<Mutex<TcpStream>>>>> =
    LazyLock::new(|| Mutex::new(Vec::new()));
/// Cancellation token for the current listener generation. A fresh token per
/// start prevents a rapid stop/start from reviving the previous accept loop.
static SERVER_STOP: LazyLock<Mutex<Option<Arc<AtomicBool>>>> = LazyLock::new(|| Mutex::new(None));
/// Serializes start/stop transitions and keeps published state coherent.
static SERVER_TRANSITION: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

const MAX_OPEN_CONNECTIONS: usize = 32;
static OPEN_CONNECTIONS: AtomicUsize = AtomicUsize::new(0);
static OPEN_STREAMS: LazyLock<Mutex<Vec<Arc<TcpStream>>>> =
    LazyLock::new(|| Mutex::new(Vec::new()));

const MAX_PENDING_BROADCASTS: usize = 256;
enum Outbound {
    Status,
    Message(String),
}
static BROADCAST_TX: LazyLock<Mutex<Option<SyncSender<Outbound>>>> =
    LazyLock::new(|| Mutex::new(None));

fn enqueue_outbound_on(sender: Option<&SyncSender<Outbound>>, message: Outbound) -> bool {
    sender.is_some_and(|tx| tx.try_send(message).is_ok())
}

fn enqueue_outbound(message: Outbound) {
    let _ = enqueue_outbound_on(BROADCAST_TX.lock().as_ref(), message);
}

fn run_broadcaster(rx: Receiver<Outbound>, stop: Arc<AtomicBool>) {
    while let Ok(message) = rx.recv() {
        if stop.load(Ordering::Acquire) {
            return;
        }
        let payload = match message {
            Outbound::Status => build_status_json(),
            Outbound::Message(payload) => payload,
        };
        broadcast_now(&payload);
    }
}

fn broadcast_now(payload: &str) {
    let clients = ACTIVE.lock().clone();
    let mut failed = Vec::new();
    for client in clients {
        let result = {
            let mut stream = client.lock();
            write_frame(&mut stream, 0x1, payload.as_bytes())
        };
        if result.is_err() {
            failed.push(client);
        }
    }
    if !failed.is_empty() {
        ACTIVE.lock().retain(|candidate| {
            !failed
                .iter()
                .any(|failed_stream| Arc::ptr_eq(candidate, failed_stream))
        });
    }
}

fn reserve_bounded_slot(counter: &AtomicUsize, limit: usize) -> bool {
    counter
        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
            (current < limit).then_some(current + 1)
        })
        .is_ok()
}

struct ConnectionSlot {
    stream: Arc<TcpStream>,
}

impl ConnectionSlot {
    fn acquire(stream: &TcpStream) -> std::io::Result<Option<Self>> {
        if !reserve_bounded_slot(&OPEN_CONNECTIONS, MAX_OPEN_CONNECTIONS) {
            return Ok(None);
        }
        let clone = match stream.try_clone() {
            Ok(clone) => clone,
            Err(e) => {
                OPEN_CONNECTIONS.fetch_sub(1, Ordering::Relaxed);
                return Err(e);
            }
        };
        let stream = Arc::new(clone);
        OPEN_STREAMS.lock().push(Arc::clone(&stream));
        Ok(Some(Self { stream }))
    }
}

impl Drop for ConnectionSlot {
    fn drop(&mut self) {
        OPEN_STREAMS
            .lock()
            .retain(|candidate| !Arc::ptr_eq(candidate, &self.stream));
        OPEN_CONNECTIONS.fetch_sub(1, Ordering::Relaxed);
    }
}

const MAX_CONCURRENT_THUMBNAILS: usize = 2;
const MAX_THUMBNAIL_WIDTH: u64 = 1920;
const MAX_THUMBNAIL_HEIGHT: u64 = 1080;

fn bounded_thumbnail_dimension(requested: Option<u64>, default: u64, maximum: u64) -> u32 {
    requested.unwrap_or(default).clamp(1, maximum) as u32
}
static THUMBNAIL_JOBS: AtomicUsize = AtomicUsize::new(0);

struct ThumbnailSlot;

impl ThumbnailSlot {
    fn acquire() -> Option<Self> {
        reserve_bounded_slot(&THUMBNAIL_JOBS, MAX_CONCURRENT_THUMBNAILS).then(|| Self)
    }
}

impl Drop for ThumbnailSlot {
    fn drop(&mut self) {
        THUMBNAIL_JOBS.fetch_sub(1, Ordering::Relaxed);
    }
}

fn shutdown_connections() {
    for stream in std::mem::take(&mut *OPEN_STREAMS.lock()) {
        let _ = stream.shutdown(Shutdown::Both);
    }
    for stream in std::mem::take(&mut *ACTIVE.lock()) {
        let _ = stream.lock().shutdown(Shutdown::Both);
    }
}

struct ActiveConnectionGuard {
    stream: Arc<Mutex<TcpStream>>,
}

impl ActiveConnectionGuard {
    fn register(stream: &TcpStream) -> std::io::Result<Self> {
        let clone = stream.try_clone()?;
        clone.set_write_timeout(Some(Duration::from_millis(250)))?;
        let stream = Arc::new(Mutex::new(clone));
        {
            let mut active = ACTIVE.lock();
            active.push(Arc::clone(&stream));
            crate::win::window_list::REMOTE_ACTIVE.store(true, Ordering::Release);
        }
        crate::win::message_loop::request_hook_install();
        Ok(Self { stream })
    }

    fn write_frame(&self, opcode: u8, payload: &[u8]) -> std::io::Result<()> {
        let mut stream = self.stream.lock();
        write_frame(&mut stream, opcode, payload)
    }
}

impl Drop for ActiveConnectionGuard {
    fn drop(&mut self) {
        let no_clients = {
            let mut active = ACTIVE.lock();
            active.retain(|candidate| !Arc::ptr_eq(candidate, &self.stream));
            active.is_empty()
        };
        if no_clients {
            crate::win::window_list::REMOTE_ACTIVE.store(false, Ordering::Release);
            crate::win::message_loop::request_hook_uninstall();
        }
    }
}
/// Bind and start the trackpad server. Binding is completed before return so a
/// tray click can open the QR page immediately without racing a setup thread.
pub fn start_server() -> Result<RemoteInfo, String> {
    start_server_inner(true, None)
}

fn start_server_inner(
    configure_firewall: bool,
    token_override: Option<String>,
) -> Result<RemoteInfo, String> {
    let _transition = SERVER_TRANSITION.lock();
    if let Some(existing) = info() {
        return Ok(existing);
    }

    let ip = local_ip().ok_or_else(|| "no usable LAN IP found".to_string())?;
    let preferred_port = crate::CONFIG.read().remote.port;
    let (listener, port) = bind_listener(ip, preferred_port)
        .map_err(|e| format!("cannot bind LAN listener near port {preferred_port}: {e}"))?;
    listener
        .set_nonblocking(true)
        .map_err(|e| format!("cannot configure LAN listener: {e}"))?;
    let token = match token_override {
        Some(token) => token,
        None => load_or_create_token()?,
    };
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let url = format!("http://{ip}:{port}/?t={token}&v={nonce}");
    let pad_url = format!("http://{ip}:{port}/pad.html?t={token}&v={nonce}");
    let remote_info = RemoteInfo {
        url: url.clone(),
        pad_url,
    };
    let stop = Arc::new(AtomicBool::new(false));
    let (broadcast_tx, broadcast_rx) = std::sync::mpsc::sync_channel(MAX_PENDING_BROADCASTS);
    let broadcast_stop = Arc::clone(&stop);
    if let Err(e) = std::thread::Builder::new()
        .name("remote-broadcast".to_string())
        .spawn(move || run_broadcaster(broadcast_rx, broadcast_stop))
    {
        return Err(format!("cannot start LAN broadcast thread: {e}"));
    }
    *BROADCAST_TX.lock() = Some(broadcast_tx);
    *SERVER_STOP.lock() = Some(Arc::clone(&stop));
    *REMOTE.write() = Some(remote_info.clone());

    let worker_stop = Arc::clone(&stop);
    if let Err(e) = std::thread::Builder::new()
        .name("remote-listener".to_string())
        .spawn(move || run_listener(listener, token, worker_stop, configure_firewall))
    {
        stop.store(true, Ordering::Release);
        *BROADCAST_TX.lock() = None;
        *SERVER_STOP.lock() = None;
        *REMOTE.write() = None;
        return Err(format!("cannot start LAN listener thread: {e}"));
    }

    crate::log::write(&format!("remote: trackpad ready on {ip}:{port}"));
    Ok(remote_info)
}

/// Stop the current listener generation and every accepted connection.
pub fn stop_server() {
    let _transition = SERVER_TRANSITION.lock();
    if let Some(stop) = SERVER_STOP.lock().take() {
        stop.store(true, Ordering::Release);
    }
    *BROADCAST_TX.lock() = None;
    shutdown_connections();
    *REMOTE.write() = None;
    crate::win::window_list::REMOTE_ACTIVE.store(false, Ordering::Release);
    crate::win::message_loop::request_hook_uninstall();
    crate::log::write("remote: trackpad server stopped");
}

fn bind_listener(ip: IpAddr, preferred_port: u16) -> std::io::Result<(TcpListener, u16)> {
    let mut last_error = None;
    for offset in 0..=10u16 {
        let Some(port) = preferred_port.checked_add(offset) else {
            break;
        };
        match TcpListener::bind(SocketAddr::new(ip, port)) {
            Ok(listener) => {
                let actual_port = listener.local_addr()?.port();
                return Ok((listener, actual_port));
            }
            Err(e) => last_error = Some(e),
        }
    }
    Err(last_error.unwrap_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::AddrNotAvailable, "no candidate ports")
    }))
}

fn run_listener(
    listener: TcpListener,
    token: String,
    stop: Arc<AtomicBool>,
    configure_firewall: bool,
) {
    if stop.load(Ordering::Acquire) {
        finish_listener(&stop);
        return;
    }

    let addr = listener.local_addr().ok();
    if let Some(addr) = addr {
        if configure_firewall && !stop.load(Ordering::Acquire) {
            let port = addr.port();
            if let Err(e) = std::thread::Builder::new()
                .name("remote-firewall".to_string())
                .spawn(move || add_firewall_rule(port))
            {
                crate::log::write(&format!("remote: firewall worker failed to start: {e}"));
            }
        }
    }

    while !stop.load(Ordering::Acquire) {
        match listener.accept() {
            Ok((stream, _)) => {
                // The listener is nonblocking so stop is observed promptly. On
                // Windows, accepted sockets can inherit that mode; restore
                // blocking I/O so per-client read/write timeouts work as intended.
                if let Err(e) = stream.set_nonblocking(false) {
                    crate::log::write(&format!(
                        "remote: cannot configure accepted connection: {e}"
                    ));
                    let _ = stream.shutdown(Shutdown::Both);
                    continue;
                }
                let slot = match ConnectionSlot::acquire(&stream) {
                    Ok(Some(slot)) => slot,
                    Ok(None) | Err(_) => {
                        let _ = stream.shutdown(Shutdown::Both);
                        continue;
                    }
                };
                if stop.load(Ordering::Acquire) {
                    let _ = stream.shutdown(Shutdown::Both);
                    continue;
                }
                let connection_token = token.clone();
                let connection_stop = Arc::clone(&stop);
                if let Err(e) = std::thread::Builder::new()
                    .name("remote-connection".to_string())
                    .spawn(move || {
                        let _slot = slot;
                        handle_conn(stream, connection_token, connection_stop);
                    })
                {
                    crate::log::write(&format!("remote: connection worker failed to start: {e}"));
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(e) => {
                crate::log::write(&format!("remote: listener failed: {e}"));
                break;
            }
        }
    }
    finish_listener(&stop);
}

fn finish_listener(stop: &Arc<AtomicBool>) {
    let mut current = SERVER_STOP.lock();
    if current
        .as_ref()
        .is_some_and(|active| Arc::ptr_eq(active, stop))
    {
        *current = None;
        drop(current);
        stop.store(true, Ordering::Release);
        *BROADCAST_TX.lock() = None;
        *REMOTE.write() = None;
        shutdown_connections();
        crate::win::window_list::REMOTE_ACTIVE.store(false, Ordering::Release);
        crate::win::message_loop::request_hook_uninstall();
    }
}

#[cfg(test)]
fn listen(ip: IpAddr, port: u16, token: String, _listen_tx: std::sync::mpsc::Sender<TcpListener>) {
    let listener = match TcpListener::bind(SocketAddr::new(ip, port)) {
        Ok(listener) => listener,
        Err(_) => return,
    };
    let stop = Arc::new(AtomicBool::new(false));
    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                let connection_token = token.clone();
                let connection_stop = Arc::clone(&stop);
                std::thread::spawn(move || handle_conn(stream, connection_token, connection_stop));
            }
            Err(_) => return,
        }
    }
}

fn same_host_ip(peer: IpAddr, local: IpAddr) -> bool {
    peer.is_loopback() || peer == local
}

fn same_host_request(stream: &TcpStream) -> bool {
    match (stream.peer_addr(), stream.local_addr()) {
        (Ok(peer), Ok(local)) => same_host_ip(peer.ip(), local.ip()),
        _ => false,
    }
}

fn reject_private_page(stream: &mut TcpStream) {
    let _ = stream.write_all(
        b"HTTP/1.1 403 Forbidden\r\nContent-Length: 9\r\nConnection: close\r\n\r\nForbidden",
    );
}

fn handle_conn(mut stream: TcpStream, token: String, stop: Arc<AtomicBool>) {
    let ip = stream
        .peer_addr()
        .map(|a| a.ip().to_string())
        .unwrap_or_default();
    if let Err(e) = stream.set_read_timeout(Some(Duration::from_secs(1))) {
        crate::log::write(&format!("remote: cannot set connection read timeout: {e}"));
        return;
    }
    if stop.load(Ordering::Acquire) {
        return;
    }
    let header_deadline = Instant::now() + Duration::from_secs(10);

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
    let _is_sw;
    let is_winlist;
    let is_pad;
    loop {
        match stream.read(&mut tmp) {
            Ok(0) => return,
            Ok(n) => {
                buf.extend_from_slice(&tmp[..n]);
                if let Some(pos) = find_sub(&buf, b"\r\n\r\n") {
                    let header = String::from_utf8_lossy(&buf[..pos]);
                    if let Some(fl) = header.split("\r\n").next() {
                        first_line = fl.to_string();
                    }

                    for line in header.split("\r\n") {
                        let low = line.to_ascii_lowercase();
                        if let Some(pos) = low.find("sec-websocket-key:") {
                            ws_key = line[pos + "sec-websocket-key:".len()..].trim().to_string();
                        } else if let Some(pos) = low.find("sec-websocket-extensions:") {
                            ws_ext = line[pos + "sec-websocket-extensions:".len()..]
                                .trim()
                                .to_string();
                        }
                    }

                    is_ws = first_line.contains(" /ws");
                    let req_path = first_line.split_whitespace().nth(1).unwrap_or("");
                    is_qr = req_path == "/qr" || req_path == "//qr";
                    is_diag = req_path == "/diag";
                    is_report = req_path.starts_with("/report");
                    is_manifest = req_path == "/manifest.json";
                    _is_sw = req_path == "/sw.js" || req_path.starts_with("/sw.js?");
                    is_winlist =
                        req_path == "/winlist.html" || req_path.starts_with("/winlist.html?");
                    is_pad = req_path == "/pad.html" || req_path.starts_with("/pad.html?");
                    break;
                }
                if buf.len() > 16384 {
                    return;
                }
            }
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
                ) =>
            {
                if stop.load(Ordering::Acquire) || Instant::now() >= header_deadline {
                    return;
                }
            }
            Err(_) => return,
        }
    }

    if stop.load(Ordering::Acquire) {
        return;
    }

    dbg_log(&format!(
        "remote: connection from {ip}, websocket={is_ws}, qr={is_qr}"
    ));

    if is_ws {
        if ws_key.is_empty() || ws_handshake(&mut stream, &ws_key, &ws_ext).is_err() {
            dbg_log(&format!("remote: ws handshake FAIL from {ip}"));
            return;
        }

        dbg_log(&format!("remote: ws handshake OK from {ip}"));
        ws_loop(stream, &token, stop);
    } else if is_qr {
        // The QR page embeds the bearer token and is an administrative surface.
        // Only the desktop hosting this listener may retrieve it.
        if same_host_request(&stream) {
            serve_qr_page(&mut stream);
        } else {
            reject_private_page(&mut stream);
        }
    } else if is_diag {
        serve_diag(&mut stream);
    } else if is_report {
        dbg_log(&format!("remote: client report received from {ip}"));
        let _ = stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nOK");
    } else if is_manifest {
        serve_manifest(&mut stream);
    } else if is_winlist {
        serve_winlist(&mut stream);
    } else if is_pad {
        serve_pad(&mut stream);
    } else {
        serve_page(&mut stream);
    }
}
/// Optional lifecycle diagnostics use the configured application log in debug
/// builds. Raw headers, bearer tokens, and input payloads are never recorded.
#[inline]
fn dbg_log(msg: &str) {
    #[cfg(debug_assertions)]
    crate::log::file_only(msg);
    #[cfg(not(debug_assertions))]
    let _ = msg;
}
fn ws_handshake(stream: &mut TcpStream, key: &str, ext: &str) -> std::io::Result<()> {
    // Do NOT echo `permessage-deflate`. We never compress/decompress frames, so
    // accepting the extension would make the client (esp. iOS Safari) expect
    // compressed server->client frames; our uncompressed `status`/`pong` frames
    // then trip a protocol error and Safari closes with code 1006. Declining the
    // extension (per RFC 6455: a server MUST NOT include an extension it can't
    // honor) keeps both sides on uncompressed JSON, which our frame code handles.
    if ext.to_ascii_lowercase().contains("permessage-deflate") {
        dbg_log("remote: declining unsupported permessage-deflate extension");
    }
    let accept = ws_accept(key);
    let resp = format!(
        "HTTP/1.1 101 Switching Protocols\r\n\
         Upgrade: websocket\r\n\
         Connection: Upgrade\r\n\
         Sec-WebSocket-Accept: {accept}\r\n\
         \r\n"
    );
    stream.write_all(resp.as_bytes())
}

fn ws_loop(mut stream: TcpStream, token: &str, stop: Arc<AtomicBool>) {
    let ip = stream
        .peer_addr()
        .map(|a| a.ip().to_string())
        .unwrap_or_default();
    // Poll the generation token once per second so stop/restart is prompt even
    // on Windows, where shutting down a duplicated socket does not reliably
    // interrupt another thread's synchronous recv. Liveness pings remain at
    // the original 30-second cadence.
    dbg_log(&format!("remote: ws loop start {ip}"));
    if let Err(e) = stream.set_read_timeout(Some(Duration::from_secs(1))) {
        crate::log::write(&format!("remote: cannot set WebSocket read timeout: {e}"));
        return;
    }
    let mut authed = false;
    let mut failed_probes = 0;
    let mut last_probe = Instant::now();
    let mut active_connection: Option<ActiveConnectionGuard> = None;
    while !stop.load(Ordering::Acquire) {
        match read_frame(&mut stream) {
            Ok(Some((opcode, payload))) => {
                if stop.load(Ordering::Acquire) {
                    return;
                }
                failed_probes = 0;
                match opcode {
                    0x1 => {
                        // text frame
                        let was = authed;
                        let result = if let Some(connection) = active_connection.as_ref() {
                            let mut writer = connection.stream.lock();
                            dispatch(&payload, token, &mut authed, &mut writer, &stop)
                        } else {
                            dispatch(&payload, token, &mut authed, &mut stream, &stop)
                        };
                        if result.is_err() {
                            dbg_log(&format!("remote: ws {ip} dispatch err -> drop"));
                            return;
                        }
                        if !was && authed {
                            match ActiveConnectionGuard::register(&stream) {
                                Ok(connection) => {
                                    active_connection = Some(connection);
                                    dbg_log(&format!("remote: ws {ip} authenticated"));
                                }
                                Err(e) => {
                                    crate::log::write(&format!(
                                        "remote: cannot register authenticated client {ip}: {e}"
                                    ));
                                    return;
                                }
                            }
                        }
                    }
                    0x8 => {
                        // close
                        dbg_log(&format!("remote: ws {ip} close frame from peer"));
                        let _ =
                            write_client_frame(&mut stream, active_connection.as_ref(), 0x8, &[]);
                        return;
                    }
                    0x9 => {
                        // ping -> pong
                        let _ = write_client_frame(
                            &mut stream,
                            active_connection.as_ref(),
                            0xA,
                            &payload,
                        );
                    }
                    _ => {} // pong / other: ignore
                }
            }
            Ok(None) => {
                dbg_log(&format!("remote: ws {ip} EOF (peer gone)"));
                return;
            }
            Err(_) => {
                if stop.load(Ordering::Acquire) {
                    return;
                }
                if last_probe.elapsed() < Duration::from_secs(30) {
                    continue;
                }
                last_probe = Instant::now();
                // Read error (idle timeout surfaces here from read_frame).
                // Probe with a WebSocket ping; if we can't write it the peer is
                // dead, otherwise keep the channel alive — browsers auto-pong.
                if write_client_frame(&mut stream, active_connection.as_ref(), 0x9, &[]).is_err() {
                    dbg_log(&format!(
                        "remote: ws {ip} read-err + probe write fail -> drop"
                    ));
                    return;
                }
                failed_probes += 1;
                if failed_probes > 3 {
                    dbg_log(&format!(
                        "remote: ws {ip} {failed_probes} probes unanswered -> drop"
                    ));
                    return;
                }
                dbg_log(&format!(
                    "remote: ws {ip} read-err -> ping probe sent ({failed_probes})"
                ));
            }
        }
    }
}

fn write_client_frame(
    read_stream: &mut TcpStream,
    active: Option<&ActiveConnectionGuard>,
    opcode: u8,
    payload: &[u8],
) -> std::io::Result<()> {
    match active {
        Some(connection) => connection.write_frame(opcode, payload),
        None => write_frame(read_stream, opcode, payload),
    }
}

/// Read exactly `buf` bytes. Returns:
/// - `Ok(true)` on success,
/// - `Ok(false)` on a clean EOF / non-timeout read error (treat as peer gone),
/// - `Err` on a timeout or transient would-block result.
fn recv_exact(stream: &mut TcpStream, buf: &mut [u8]) -> std::io::Result<bool> {
    match stream.read_exact(buf) {
        Ok(()) => Ok(true),
        Err(e)
            if matches!(
                e.kind(),
                std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
            ) =>
        {
            Err(e)
        }
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
    // Include live window count so the winlist page can skip expensive full diffs
    let windows = crate::win::window_list::enumerate_windows();
    let win_count = windows.len();
    let remote = &cfg.remote;
    let json = serde_json::json!({
        "t": "status",
        "conn": true,
        "pc_battery": pc_battery,
        "win_count": win_count,
        "large_screen_split": remote.large_screen_split,
        "split_ratio": remote.split_ratio,
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
/// Queue the latest status for authenticated WebSocket clients. The caller
/// never performs network I/O; a bounded generation-owned worker serializes
/// best-effort broadcasts.
pub fn broadcast_status() {
    enqueue_outbound(Outbound::Status);
}

pub fn broadcast_message(msg: &str) {
    enqueue_outbound(Outbound::Message(msg.to_owned()));
}
fn broadcast_thumbnail(stop: &Arc<AtomicBool>, hwnd: isize, thumb: Option<Vec<u8>>) {
    use base64::Engine;
    let data_uri = thumb.map(|bytes| {
        let encoded = base64::engine::general_purpose::STANDARD.encode(bytes);
        format!("data:image/jpeg;base64,{encoded}")
    });
    let json = serde_json::json!({
        "t": "thumb_update",
        "hwnd": hwnd,
        "thumb": data_uri,
    });

    let _transition = SERVER_TRANSITION.lock();
    let is_current = SERVER_STOP
        .lock()
        .as_ref()
        .is_some_and(|current| Arc::ptr_eq(current, stop));
    if !stop.load(Ordering::Acquire) && is_current {
        enqueue_outbound(Outbound::Message(json.to_string()));
    }
}

/// Push a `win_event` message to all authenticated WS clients.
/// Called by the WinEvent hook whenever a window is created, destroyed,
/// moved, or activated so the winlist page can update incrementally.
pub fn broadcast_win_event(event: &str, hwnd: isize, title: Option<&str>) {
    let json = serde_json::json!({
        "t": "win_event",
        "event": event,
        "hwnd": hwnd,
        "title": title,
    });
    broadcast_message(&json.to_string());
}

/// Dispatch a text frame. Returns `Err` to close the socket (auth failure).
fn dispatch(
    payload: &[u8],
    token: &str,
    authed: &mut bool,
    stream: &mut TcpStream,
    stop: &Arc<AtomicBool>,
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
                // Push window_list immediately after auth so winlist.html doesn't need to request it
                let windows = crate::win::window_list::enumerate_windows();
                let desktops = crate::win::window_list::enumerate_desktops();
                let json = serde_json::json!({
                    "t": "window_list",
                    "windows": windows,
                    "desktops": desktops,
                });
                dbg_log(&format!(
                    "dispatch: window_list pushed to client ({} windows)",
                    windows.len()
                ));
                let _ = write_frame(stream, 0x1, json.to_string().as_bytes());
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
        "get_windows" | "window_list" => {
            crate::log::write(&format!("dispatch: received {} request", t));
            let windows = crate::win::window_list::enumerate_windows();
            let desktops = crate::win::window_list::enumerate_desktops();
            let count = windows.len();
            let json = serde_json::json!({
                "t": "window_list",
                "windows": windows,
                "desktops": desktops,
                "count": count,
            });
            crate::log::write(&format!(
                "dispatch: sending window_list reply windows={}",
                count
            ));
            let _ = write_frame(stream, 0x1, json.to_string().as_bytes());
        }
        "switch_window" => {
            let ok = if let Some(hwnd_val) = v.get("hwnd") {
                if let Some(hwnd) = hwnd_val.as_i64() {
                    crate::win::window_list::focus_window(hwnd as isize)
                } else {
                    false
                }
            } else {
                false
            };
            let json = serde_json::json!({"t": "switch_result", "ok": ok});
            let _ = write_frame(stream, 0x1, json.to_string().as_bytes());
        }
        "switch_desktop" => {
            if let Some(idx) = v.get("index").and_then(|x| x.as_u64()) {
                crate::win::window_list::switch_to_desktop(idx as u32);
            }
        }
        "thumb_request" => {
            let hwnd = v.get("hwnd").and_then(|x| x.as_i64()).unwrap_or(0) as isize;
            let max_w = bounded_thumbnail_dimension(
                v.get("w").and_then(|x| x.as_u64()),
                320,
                MAX_THUMBNAIL_WIDTH,
            );
            let max_h = bounded_thumbnail_dimension(
                v.get("h").and_then(|x| x.as_u64()),
                200,
                MAX_THUMBNAIL_HEIGHT,
            );
            let thumbnail_stop = Arc::clone(stop);
            if let Some(slot) = ThumbnailSlot::acquire() {
                if let Err(e) = std::thread::Builder::new()
                    .name("remote-thumbnail".to_string())
                    .spawn(move || {
                        let _slot = slot;
                        let thumb =
                            crate::win::window_list::capture_window_thumb(hwnd, max_w, max_h);
                        broadcast_thumbnail(&thumbnail_stop, hwnd, thumb);
                    })
                {
                    crate::log::write(&format!("remote: thumbnail worker failed to start: {e}"));
                    broadcast_thumbnail(stop, hwnd, None);
                }
            } else {
                broadcast_thumbnail(stop, hwnd, None);
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
    let header = format!(
        "HTTP/1.1 200 OK\r\n\
Content-Type: text/html; charset=utf-8\r\n\
Cache-Control: no-cache\r\n\
X-Build-Id: trackpad-html-bytes\r\n\
Content-Length: {}\r\n\
Connection: close\r\n\
\r\n",
        body.len()
    );
    let _ = stream.write_all(header.as_bytes());
    let _ = stream.write_all(body);
}

fn serve_winlist(stream: &mut TcpStream) {
    let body = WINLIST_HTML.as_bytes();
    let header = format!(
        "HTTP/1.1 200 OK\r\n\
Content-Type: text/html; charset=utf-8\r\n\
Cache-Control: no-cache\r\n\
Content-Length: {}\r\n\
Connection: close\r\n\
\r\n",
        body.len()
    );
    let _ = stream.write_all(header.as_bytes());
    let _ = stream.write_all(body);
}
fn serve_pad(stream: &mut TcpStream) {
    let body = PAD_HTML.as_bytes();
    let header = format!(
        "HTTP/1.1 200 OK\r\n\
Content-Type: text/html; charset=utf-8\r\n\
Cache-Control: no-cache\r\n\
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

#[allow(dead_code)]
static SW_JS: &str = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/assets/sw.js"));

#[allow(dead_code)]
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
                let data = serde_json::json!({ "url": &info.url, "pad_url": &info.pad_url, "size": n, "modules": modules });
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
        .creation_flags(CREATE_NO_WINDOW)
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
/// Versioned storage rotates the pre-0.1.1 time-seeded token format once,
/// while the bearer token exposed on the network remains 64 lowercase hex
/// characters.
fn load_or_create_token() -> Result<String, String> {
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let path = dir.join("token.txt");
            if let Ok(s) = std::fs::read_to_string(&path) {
                if let Some(token) = parse_persisted_token(&s) {
                    return Ok(token);
                }
            }
            let token = random_token()?;
            if let Err(e) = std::fs::write(&path, format!("v2:{token}")) {
                crate::log::write(&format!(
                    "remote: pairing token could not be persisted at {}: {e}",
                    path.display()
                ));
            }
            return Ok(token);
        }
    }
    random_token()
}

fn parse_persisted_token(value: &str) -> Option<String> {
    let token = value.trim().strip_prefix("v2:")?;
    (token.len() == 64 && token.chars().all(|c| c.is_ascii_hexdigit()))
        .then(|| token.to_ascii_lowercase())
}

/// Generate a 256-bit bearer token with the Windows system-preferred CSPRNG.
/// Failure is fatal to server startup: predictable input authorization is not
/// an acceptable fallback.
fn random_token() -> Result<String, String> {
    let mut bytes = [0u8; 32];
    let status = unsafe {
        BCryptGenRandom(
            std::ptr::null_mut(),
            bytes.as_mut_ptr(),
            bytes.len() as u32,
            BCRYPT_USE_SYSTEM_PREFERRED_RNG,
        )
    };
    if status != 0 {
        return Err(format!(
            "Windows secure random generator failed with NTSTATUS {status:#x}"
        ));
    }

    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut token = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        token.push(HEX[(byte >> 4) as usize] as char);
        token.push(HEX[(byte & 0x0f) as usize] as char);
    }
    Ok(token)
}

// ─── WebSocket accept: SHA1 + base64 ────────────────────────────────────────

fn ws_accept(key: &str) -> String {
    const GUID: &str = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";
    let concat = format!("{key}{GUID}");
    base64(&sha1(concat.as_bytes()))
}

fn sha1(data: &[u8]) -> [u8; 20] {
    fn rol(v: u32, n: u32) -> u32 {
        v.rotate_left(n)
    }
    let mut h: [u32; 5] = [
        0x6745_2301,
        0xEFCD_AB89,
        0x98BA_DCFE,
        0x1032_5476,
        0xC3D2_E1F0,
    ];
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
        #[allow(clippy::needless_range_loop)]
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
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
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
        s.push(if chunk.len() > 1 {
            T[((n >> 6) & 63) as usize] as char
        } else {
            '='
        });
        s.push(if chunk.len() > 2 {
            T[(n & 63) as usize] as char
        } else {
            '='
        });
    }
    s
}

// ─── embedded trackpad page ─────────────────────────────────────────────────

const TRACKPAD_HTML: &str =
    include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/assets/trackpad.html"));
const QR_HTML: &str = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/assets/qr.html"));
const MANIFEST_JSON: &str =
    include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/assets/manifest.json"));
const WINLIST_HTML: &str =
    include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/assets/winlist.html"));
const PAD_HTML: &str = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/assets/pad.html"));

#[cfg(test)]
mod tests {
    use super::*;
    static LIFECYCLE_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

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

    /// Regression: start_server() must bind and publish state promptly so the
    /// tray can open the QR page without blocking or racing initialization.
    #[test]
    fn start_server_returns_ready_state_immediately() {
        use std::time::Instant;
        let _guard = LIFECYCLE_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let t0 = Instant::now();
        let started = start_server_inner(false, Some("00".repeat(32)));
        let elapsed = t0.elapsed();
        assert!(
            elapsed.as_millis() < 50,
            "start_server blocked for {elapsed:?}"
        );
        let started = started.expect("test host must expose a usable LAN address");
        let published = info().expect("successful start must publish connection info");
        assert_eq!(published.url, started.url);
        assert!(BROADCAST_TX.lock().is_some());
        stop_server();
        assert!(BROADCAST_TX.lock().is_none());
    }

    #[test]
    fn stop_server_closes_unauthenticated_connections() {
        let _guard = LIFECYCLE_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let started = start_server_inner(false, Some("00".repeat(32)))
            .expect("test host must expose a usable LAN address");
        let authority = started
            .url
            .strip_prefix("http://")
            .and_then(|url| url.split('/').next())
            .unwrap();
        let mut client = TcpStream::connect(authority).unwrap();
        client.write_all(b"GET / HTTP/1.1\r\n").unwrap();

        let accepted_deadline = std::time::Instant::now() + Duration::from_millis(500);
        while OPEN_CONNECTIONS.load(Ordering::Relaxed) == 0
            && std::time::Instant::now() < accepted_deadline
        {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(OPEN_CONNECTIONS.load(Ordering::Relaxed), 1);

        stop_server();
        let closed_deadline = std::time::Instant::now() + Duration::from_secs(2);
        while OPEN_CONNECTIONS.load(Ordering::Relaxed) != 0
            && std::time::Instant::now() < closed_deadline
        {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(OPEN_CONNECTIONS.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn server_caps_and_closes_an_unauthenticated_connection_flood() {
        let _guard = LIFECYCLE_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let started = start_server_inner(false, Some("00".repeat(32)))
            .expect("test host must expose a usable LAN address");
        let address: SocketAddr = started
            .url
            .strip_prefix("http://")
            .and_then(|url| url.split('/').next())
            .unwrap()
            .parse()
            .unwrap();

        let mut clients = Vec::new();
        for index in 0..(MAX_OPEN_CONNECTIONS + 8) {
            let mut client = TcpStream::connect_timeout(&address, Duration::from_millis(250))
                .expect("loopback-equivalent LAN connection must succeed");
            client
                .write_all(
                    b"GET /ws HTTP/1.1\r\nHost: test\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\r\n",
                )
                .unwrap();
            clients.push(client);

            if index < MAX_OPEN_CONNECTIONS {
                let accepted_deadline = std::time::Instant::now() + Duration::from_millis(250);
                while OPEN_CONNECTIONS.load(Ordering::Relaxed) <= index
                    && std::time::Instant::now() < accepted_deadline
                {
                    std::thread::sleep(Duration::from_millis(2));
                }
                assert_eq!(OPEN_CONNECTIONS.load(Ordering::Relaxed), index + 1);
            }
        }
        assert_eq!(
            OPEN_CONNECTIONS.load(Ordering::Relaxed),
            MAX_OPEN_CONNECTIONS
        );

        stop_server();
        let closed_deadline = std::time::Instant::now() + Duration::from_secs(2);
        while OPEN_CONNECTIONS.load(Ordering::Relaxed) != 0
            && std::time::Instant::now() < closed_deadline
        {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(OPEN_CONNECTIONS.load(Ordering::Relaxed), 0);
        drop(clients);
    }

    #[test]
    fn listener_stop_releases_bound_port() {
        let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        listener.set_nonblocking(true).unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let worker_stop = Arc::clone(&stop);
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            run_listener(listener, "test-token".to_string(), worker_stop, false);
            let _ = done_tx.send(());
        });

        stop.store(true, Ordering::Release);
        assert!(
            done_rx.recv_timeout(Duration::from_millis(250)).is_ok(),
            "listener must observe cancellation promptly"
        );
        TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, port))
            .expect("listener port must be reusable after stop");
    }

    #[test]
    fn secure_token_has_expected_wire_format() {
        let token = random_token().expect("Windows system RNG must be available");
        assert_eq!(token.len(), 64);
        assert!(token.bytes().all(|b| b.is_ascii_hexdigit()));
        assert_eq!(token, token.to_ascii_lowercase());
    }

    #[test]
    fn persisted_token_requires_secure_format_version() {
        let token = "AB".repeat(32);
        assert_eq!(
            parse_persisted_token(&format!("v2:{token}")),
            Some(token.to_ascii_lowercase())
        );
        assert_eq!(parse_persisted_token(&token), None);
        assert_eq!(parse_persisted_token("v2:not-a-token"), None);
    }

    #[test]
    fn unauthenticated_connection_reservations_are_bounded() {
        let counter = AtomicUsize::new(0);
        for _ in 0..MAX_OPEN_CONNECTIONS {
            assert!(reserve_bounded_slot(&counter, MAX_OPEN_CONNECTIONS));
        }
        assert!(!reserve_bounded_slot(&counter, MAX_OPEN_CONNECTIONS));
        counter.fetch_sub(1, Ordering::Relaxed);
        assert!(reserve_bounded_slot(&counter, MAX_OPEN_CONNECTIONS));
    }

    #[test]
    fn thumbnail_jobs_have_a_strict_concurrency_cap() {
        assert_eq!(THUMBNAIL_JOBS.load(Ordering::Relaxed), 0);
        let first = ThumbnailSlot::acquire().unwrap();
        let second = ThumbnailSlot::acquire().unwrap();
        assert!(ThumbnailSlot::acquire().is_none());
        drop(first);
        let replacement = ThumbnailSlot::acquire().unwrap();
        drop(second);
        drop(replacement);
        assert_eq!(THUMBNAIL_JOBS.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn thumbnail_dimensions_are_clamped_before_capture() {
        assert_eq!(bounded_thumbnail_dimension(None, 320, 1920), 320);
        assert_eq!(bounded_thumbnail_dimension(Some(0), 320, 1920), 1);
        assert_eq!(bounded_thumbnail_dimension(Some(u64::MAX), 320, 1920), 1920);
    }

    #[test]
    fn outbound_broadcast_queue_is_bounded_and_fail_open() {
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        assert!(enqueue_outbound_on(Some(&tx), Outbound::Status));
        assert!(!enqueue_outbound_on(
            Some(&tx),
            Outbound::Message("overflow".to_string())
        ));
        assert!(matches!(rx.recv().unwrap(), Outbound::Status));
        drop(rx);
        assert!(!enqueue_outbound_on(Some(&tx), Outbound::Status));
        assert!(!enqueue_outbound_on(None, Outbound::Status));
    }

    #[test]
    fn bearer_qr_is_only_available_to_the_host_machine() {
        let local: IpAddr = "192.168.1.10".parse().unwrap();
        assert!(same_host_ip("127.0.0.1".parse().unwrap(), local));
        assert!(same_host_ip(local, local));
        assert!(!same_host_ip(
            "192.168.1.11".parse().unwrap(),
            "192.168.1.10".parse().unwrap()
        ));
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
            let mut counts: std::collections::HashMap<&str, u32> = std::collections::HashMap::new();
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

    /// The previous four hand-edits shipped trackpad.html with JS that
    /// crashed at runtime — duplicate var params, literal '+' prefix on
    /// declarations, unclosed pinchMetric() brace, missing LONGPRESS_MS
    /// const. Each was caught only by the user opening devtools in their
    /// phone browser. We don't have a JS parser in Rust, but we can at
    /// least verify the braces balance and every `name` referenced as
    /// identifier is declared somewhere in the same script block.
    #[test]
    fn trackpad_html_is_syntactically_valid() {
        let html = TRACKPAD_HTML;
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
            let bytes = body.as_bytes();

            // 1) Brace / paren / bracket balance.
            let mut depth: [i32; 3] = [0, 0, 0];
            let mut in_str: Option<u8> = None;
            let mut in_line_com = false;
            let mut in_block_com = false;
            let mut i = 0;
            while i < bytes.len() {
                let c = bytes[i];
                let next = bytes.get(i + 1).copied();
                if in_line_com {
                    if c == b'\n' {
                        in_line_com = false;
                    }
                } else if in_block_com {
                    if c == b'*' && next == Some(b'/') {
                        in_block_com = false;
                        i += 1;
                    }
                } else if let Some(q) = in_str {
                    if c == b'\\' {
                        i += 1;
                    } else if c == q {
                        in_str = None;
                    }
                } else {
                    match c {
                        b'/' if next == Some(b'/') => {
                            in_line_com = true;
                            i += 1;
                        }
                        b'/' if next == Some(b'*') => {
                            in_block_com = true;
                            i += 1;
                        }
                        b'"' | b'\'' | b'`' => {
                            in_str = Some(c);
                        }
                        b'{' => depth[0] += 1,
                        b'}' => depth[0] -= 1,
                        b'(' => depth[1] += 1,
                        b')' => depth[1] -= 1,
                        b'[' => depth[2] += 1,
                        b']' => depth[2] -= 1,
                        _ => {}
                    }
                }
                i += 1;
            }
            assert_eq!(
                depth, [0, 0, 0],
                "trackpad.html: unbalanced braces/parens/brackets in <script> at byte {block_start}: depth={depth:?}"
            );

            // 2) Every config constant the touch payload overrides must be
            //    declared as a `var NAME = ...` (or `var A = ..., NAME = ...`)
            //    somewhere in this script block. Catches the LONGPRESS_MS bug.
            let required = [
                "SENS",
                "ACCEL_REF",
                "ACCEL_SLOPE",
                "ACCEL_MAX_MULT",
                "MOVE_EMA",
                "SCROLL_GAIN",
                "TAP_MS",
                "TAP_PX",
                "SWIPE_PX",
                "DECIDE_PX",
                "PINCH_BIAS",
                "DIAG_MIN",
                "DIAG_RATIO",
                "LONGPRESS_MS",
            ];
            // Manual word-boundary check: find every occurrence of `name`
            // and ensure neither side is an identifier character.
            let is_ident = |b: u8| b.is_ascii_alphanumeric() || b == b'_';
            for name in required {
                let needle = name.as_bytes();
                let mut j = 0usize;
                let mut hit = false;
                while j + needle.len() <= bytes.len() {
                    if &bytes[j..j + needle.len()] == needle {
                        let prev_ok = j == 0 || !is_ident(bytes[j - 1]);
                        let next_ok =
                            j + needle.len() == bytes.len() || !is_ident(bytes[j + needle.len()]);
                        if prev_ok && next_ok {
                            hit = true;
                            break;
                        }
                    }
                    j += 1;
                }
                assert!(
                    hit,
                    "trackpad.html: config var `{name}` referenced in tc.* handler is missing a declaration in this <script> block"
                );
            }
            start = close + "</script>".len();
        }
    }

    /// Required DOM ids that the JS expects to bind to.
    /// regression where the `<div id="banner">` was accidentally
    /// deleted during a hand-edit, leaving bannerText === null and
    /// crashing on the first ws.onopen message.
    #[test]
    fn trackpad_html_has_required_dom_ids() {
        let html = TRACKPAD_HTML;
        for id in &[
            "pad",
            "status",
            "batteries",
            "fingers",
            "banner",
            "bannerText",
            "inputToggle",
            "inputBar",
            "textField",
            "charCount",
            "collapseBtn",
            "sendBtn",
        ] {
            let needle = format!("id=\"{id}\"");
            assert!(
                html.contains(&needle),
                "trackpad.html: required DOM id `{id}` is missing. JS will throw 'Cannot set properties of null' when the page loads."
            );
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
            if n == 0 {
                break;
            }
            total += n;
            if buf[..total].windows(4).any(|w| w == b"\r\n\r\n") {
                break;
            }
        }
        let resp = String::from_utf8_lossy(&buf[..total]);
        assert!(
            resp.contains("101"),
            "expected 101 switching protocols, got: {resp}"
        );
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
        s2.write_all(&masked_text_frame(b"{\"t\":\"auth\",\"token\":\"wrong\"}"))
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
        vec![0x89, 0x80, mask[0], mask[1], mask[2], mask[3]]
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
            .map(|l| {
                l.split_once(':')
                    .map(|x| x.1)
                    .unwrap_or("")
                    .trim()
                    .to_string()
            })
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
        assert_eq!(
            op, 0xA,
            "expected pong (0xA) after dispatching all commands"
        );

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
        assert_eq!(
            op2, 0x8,
            "expected a clean close frame, got opcode {op2} (a bare 1006 would loop the client)"
        );
        let code = ((p2[0] as u16) << 8) | (p2[1] as u16);
        assert_eq!(code, 1008, "expected close code 1008, got {code}");
    }
}
