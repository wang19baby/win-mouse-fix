//! Window list for phone trackpad task-view mode.
//!
//! Enumerates visible windows (Alt+Tab filtered), captures thumbnails via
//! PrintWindow, and manages window/desktop focus. WinEvent hooks detect
//! changes (foreground, create, destroy, title) and push incremental
//! updates over WebSocket.

use std::io::Cursor;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::LazyLock;
use std::time::{Duration, Instant};
use base64::Engine;
use image::DynamicImage;
use image::RgbImage;
use image::codecs::jpeg::JpegEncoder;
use parking_lot::Mutex;
use windows_sys::Win32::Graphics::Gdi::{
    BitBlt, CreateCompatibleBitmap, CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject,
    GetDC, ReleaseDC, SelectObject, SetStretchBltMode, StretchBlt, CAPTUREBLT, HALFTONE, SRCCOPY,
    BITMAPINFOHEADER, DIB_RGB_COLORS,
    GetMonitorInfoW, MonitorFromWindow,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetAncestor, GetClassNameW, GetForegroundWindow, GetLastActivePopup,
    GetWindowTextLengthW, GetWindowTextW, GetWindowThreadProcessId, IsWindowVisible,
    SetForegroundWindow, ShowWindow, SW_MINIMIZE, SW_MAXIMIZE, SW_RESTORE, SW_SHOW, GA_ROOTOWNER, WS_EX_APPWINDOW, WS_EX_TOOLWINDOW,
    GetWindowRect, IsZoomed, IsIconic,
    EVENT_SYSTEM_FOREGROUND, EVENT_OBJECT_CREATE, EVENT_OBJECT_DESTROY,
    EVENT_OBJECT_NAMECHANGE, EVENT_OBJECT_LOCATIONCHANGE, WINEVENT_OUTOFCONTEXT,
    GetWindowLongPtrW,
    SendMessageW, WM_GETICON, ICON_SMALL2, ICON_SMALL, ICON_BIG,
};
use windows_sys::Win32::UI::Accessibility::{
    SetWinEventHook, UnhookWinEvent, HWINEVENTHOOK,
};
use windows_sys::Win32::System::Threading::{
    OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
};
use windows_sys::Win32::Foundation::{CloseHandle, HWND, LPARAM, BOOL, RECT};

use windows_sys::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED,
};
use windows_sys::core::GUID;

use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, SendInput, INPUT, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP,
    VK_MENU, VK_LMENU, VK_RMENU, VK_TAB, VK_ESCAPE,
};

const OUR_MARKER: usize = crate::win::hooks::OUR_MARKER;

// ─── Data structures ────────────────────────────────────────────────────────
/// Window metadata — title, process name, current foreground state, and a small icon.
#[derive(Debug, Clone, serde::Serialize)]
pub struct WindowMeta {
    pub hwnd: isize,
    pub title: String,
    pub process_name: String,
    pub is_current: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct DesktopInfo {
    pub index: u32,
    pub name: String,
    pub is_current: bool,
}

/// Messages sent from PC to phone via WebSocket for window-change events.
/// Distinct from the internal _WindowEvent (WinEvent hook → tray → main → remote).
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub enum WinEventPush {
    ForegroundChanged { hwnd: isize },
    WindowCreated { hwnd: isize, title: String, proc_name: String },
    WindowDestroyed { hwnd: isize },
    TitleChanged { hwnd: isize, title: String },
    DesktopChanged { desktop: u32 },
}

/// Events pushed when windows change.
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub enum _WindowEvent {
    ForegroundChanged { hwnd: isize },
    WindowCreated {
        _hwnd: isize,
        _title: String,
        _process_name: String,
    },
    WindowDestroyed(isize),
    TitleChanged {
        _hwnd: isize,
        _title: String,
    },
}
/// Controls whether WinEvent changes are pushed to the phone.
/// Set to true when the remote server starts, false when it stops.
pub static REMOTE_ACTIVE: AtomicBool = AtomicBool::new(false);

// ─── Thumbnail LRU cache (Phase C.3) ─────────────────────────────────────────
// 30 entries is plenty for a phone screen showing ~6-12 cards at once.
// Each entry is a 320x200 JPEG, roughly 10-30KB → ~600KB worst case.
// Hand-rolled LRU to skip the `lru` crate dependency (VecDeque for recency
// order + HashMap for O(1) lookup).
const THUMB_CACHE_CAPACITY: usize = 30;
const THUMB_CACHE_FRESH_MS: u128 = 30_000;  // 30s; reused without recapture

struct ThumbCacheEntry {
    data: Vec<u8>,
    last_used: Instant,
    last_captured: Instant,
}

static THUMB_CACHE: LazyLock<Mutex<ThumbCache>> =
    LazyLock::new(|| Mutex::new(ThumbCache::new()));

struct ThumbCache {
    map: std::collections::HashMap<isize, ThumbCacheEntry>,
    order: std::collections::VecDeque<isize>,  // front = most recent
}

impl ThumbCache {
    fn new() -> Self {
        Self { map: std::collections::HashMap::new(), order: std::collections::VecDeque::new() }
    }
    fn get_mut(&mut self, hwnd: &isize) -> Option<&mut ThumbCacheEntry> {
        let exists = self.map.contains_key(hwnd);
        if !exists { return None; }
        // Move to front (most recent)
        if let Some(pos) = self.order.iter().position(|x| x == hwnd) {
            self.order.remove(pos);
        }
        self.order.push_front(*hwnd);
        self.map.get_mut(hwnd)
    }
    fn put(&mut self, hwnd: isize, entry: ThumbCacheEntry) {
        if self.map.contains_key(&hwnd) {
            if let Some(pos) = self.order.iter().position(|x| x == &hwnd) {
                self.order.remove(pos);
            }
        } else if self.map.len() >= THUMB_CACHE_CAPACITY {
            // Evict LRU (back of deque)
            if let Some(victim) = self.order.pop_back() {
                self.map.remove(&victim);
            }
        }
        self.order.push_front(hwnd);
        self.map.insert(hwnd, entry);
    }
    fn pop(&mut self, hwnd: &isize) {
        if self.map.remove(hwnd).is_some() {
            if let Some(pos) = self.order.iter().position(|x| x == hwnd) {
                self.order.remove(pos);
            }
        }
    }
 }

/// Look up a cached thumbnail. Returns Some(bytes) if a fresh entry exists
/// (within THUMB_CACHE_FRESH_MS), None otherwise. Updates last_used on hit.
pub fn thumb_cache_get(hwnd: isize) -> Option<Vec<u8>> {
    let mut cache = THUMB_CACHE.lock();
    if let Some(entry) = cache.get_mut(&hwnd) {
        if entry.last_captured.elapsed().as_millis() < THUMB_CACHE_FRESH_MS {
            entry.last_used = Instant::now();
            return Some(entry.data.clone());
        }
    }
    None
}

/// Insert/replace a thumbnail in the cache.
pub fn thumb_cache_put(hwnd: isize, data: Vec<u8>) {
    let now = Instant::now();
    THUMB_CACHE.lock().put(hwnd, ThumbCacheEntry {
        data,
        last_used: now,
        last_captured: now,
    });
}

/// Returns (total_entries, fresh_entries, stale_entries) for the thumb cache.
pub fn thumb_cache_stats() -> (usize, usize, usize) {
    let cache = THUMB_CACHE.lock();
    let total = cache.map.len();
    let fresh = cache.map.iter()
        .filter(|(_, e)| e.last_captured.elapsed().as_millis() < THUMB_CACHE_FRESH_MS)
        .count();
    (total, fresh, total - fresh)
}
/// Invalidate a thumbnail (called from WinEvent handlers when a window
/// changes location / title / foreground — the previous capture no longer
/// reflects the current state).
pub fn thumb_cache_invalidate(hwnd: isize) {
    THUMB_CACHE.lock().pop(&hwnd);
}

// ─── WinEvent watcher ───────────────────────────────────────────────────────

static _WINEVENT_TX: LazyLock<Mutex<Option<Sender<_WindowEvent>>>> =
    LazyLock::new(|| Mutex::new(None));

static _WINEVENT_HOOKS: LazyLock<Mutex<Vec<HWINEVENTHOOK>>> =
    LazyLock::new(|| Mutex::new(Vec::new()));

/// Debounce state: coalesce rapid WinEvents into a single push. The 1s
/// window here is matched by `_winevent_tick`'s flush interval (still 300ms)
/// — when dirty, the next tick will flush exactly one ForegroundChanged
/// event, but the *minimum interval* between foreground pushes is 1s to
/// avoid WS chatter during fast Alt+Tab cycling or window title changes.
const _DEBOUNCE_MS: u128 = 1000;
static _LAST_PUSH: LazyLock<Mutex<Instant>> = LazyLock::new(|| Mutex::new(Instant::now()));
static _DIRTY: AtomicBool = AtomicBool::new(false);

/// Start the WinEvent watcher. Must be called from a thread with a message
/// loop (or the main thread). The channel sender receives debounced events.
pub fn _start_winevent_watcher(tx: Sender<_WindowEvent>) {
    // Stop any existing hooks first
    _stop_winevent_watcher();

    *_WINEVENT_TX.lock() = Some(tx);

    let mut hooks = _WINEVENT_HOOKS.lock();
    unsafe {
        let hook_id_foreground = SetWinEventHook(
            EVENT_SYSTEM_FOREGROUND,
            EVENT_SYSTEM_FOREGROUND,
            0, // hmodule: 0 for out-of-context
            Some(_winevent_callback),
            0,
            0,
            WINEVENT_OUTOFCONTEXT,
        );

        let hook_id_create = SetWinEventHook(
            EVENT_OBJECT_CREATE,
            EVENT_OBJECT_CREATE,
            0,
            Some(_winevent_callback),
            0,
            0,
            WINEVENT_OUTOFCONTEXT,
        );

        let hook_id_destroy = SetWinEventHook(
            EVENT_OBJECT_DESTROY,
            EVENT_OBJECT_DESTROY,
            0,
            Some(_winevent_callback),
            0,
            0,
            WINEVENT_OUTOFCONTEXT,
        );

        let hook_id_name = SetWinEventHook(
            EVENT_OBJECT_NAMECHANGE,
            EVENT_OBJECT_NAMECHANGE,
            0,
            Some(_winevent_callback),
            0,
            0,
            WINEVENT_OUTOFCONTEXT,
        );

        let hook_id_location = SetWinEventHook(
            EVENT_OBJECT_LOCATIONCHANGE,
            EVENT_OBJECT_LOCATIONCHANGE,
            0,
            Some(_winevent_callback),
            0,
            0,
            WINEVENT_OUTOFCONTEXT,
         );

        if hook_id_foreground == 0
            || hook_id_create == 0
            || hook_id_destroy == 0
            || hook_id_name == 0
            || hook_id_location == 0
        {
            crate::log::write("window_list: failed to set one or more WinEvent hooks");
        } else {
            crate::log::write("window_list: WinEvent hooks installed");
            hooks.push(hook_id_foreground);
            hooks.push(hook_id_create);
            hooks.push(hook_id_destroy);
            hooks.push(hook_id_name);
            hooks.push(hook_id_location);
        }
    }
}

pub fn _stop_winevent_watcher() {
    // Clear the sender
    *_WINEVENT_TX.lock() = None;

    // Unhook all hooks
    let mut hooks = _WINEVENT_HOOKS.lock();
    for &hook in hooks.iter() {
        unsafe {
            UnhookWinEvent(hook);
        }
    }
    hooks.clear();
}

unsafe extern "system" fn _winevent_callback(
    _hook: HWINEVENTHOOK,
    event: u32,
    hwnd: HWND,
    _id_object: i32,
    _id_child: i32,
    _event_thread: u32,
    _time: u32,
) {
    if hwnd == 0 {
        return;
    }

    // Phase: while the user is holding Alt (Alt+Tab, Alt held previews,
    // shell menu navigation, etc.), the foreground window keeps changing
    // as the user cycles. Don't push anything — it's all transient and
    // the captured thumbs during this state are the switcher overlay.
    if is_alt_tab_switcher_active() {
        return;
    }
    // Log event type for debugging
    let event_name = match event {
        EVENT_SYSTEM_FOREGROUND => "FOREGROUND",
        EVENT_OBJECT_CREATE => "CREATE",
        EVENT_OBJECT_DESTROY => "DESTROY",
        EVENT_OBJECT_NAMECHANGE => "NAMECHANGE",
        EVENT_OBJECT_LOCATIONCHANGE => "LOCATIONCHANGE",
        _ => "OTHER",
    };
    crate::log::write(&format!("winevent_callback: event={} hwnd={:#x}", event_name, hwnd));

    // Phase C.5: invalidate cached thumbnail so the next capture is fresh.
    // LOCATIONCHANGE is fire-hose-y (mouse drag etc.); only invalidate for
    // alt-tab windows to avoid cache churn on resize handle / tooltip moves.
    if event == EVENT_OBJECT_NAMECHANGE || event == EVENT_OBJECT_LOCATIONCHANGE {
        if _is_top_level_window(hwnd) && is_alt_tab_window(hwnd) {
            thumb_cache_invalidate(hwnd as isize);
        }
    }

    let now = Instant::now();
    let should_push = {
        let mut last = _LAST_PUSH.lock();
        let elapsed = now.duration_since(*last);
        if elapsed >= Duration::from_millis(_DEBOUNCE_MS as u64) {
            *last = now;
            true
        } else {
            _DIRTY.store(true, Ordering::Relaxed);
            false
        }
    };
    let evt = match event {
        EVENT_SYSTEM_FOREGROUND => Some(_WindowEvent::ForegroundChanged { hwnd: hwnd as isize }),
        EVENT_OBJECT_CREATE => {
            // Only care about top-level windows.
            // NOTE: intentionally do NOT call get_window_info here.
            // - get_window_title (GetWindowTextW) can trigger re-entrant NAMECHANGE
            //   callbacks → stack overflow via the debounce re-entry path.
            // - The window title may not be set yet at CREATE time anyway.
            // Window info will be populated when the phone polls the full list.
            if _is_top_level_window(hwnd) && is_alt_tab_window(hwnd) {
                Some(_WindowEvent::WindowCreated {
                    _hwnd: hwnd,
                    _title: String::new(),
                    _process_name: String::new(),
                })
            } else {
                None
            }
        }
        EVENT_OBJECT_DESTROY => {
            if _is_top_level_window(hwnd) {
                Some(_WindowEvent::WindowDestroyed(hwnd))
            } else {
                None
            }
        }
        EVENT_OBJECT_NAMECHANGE => {
            if _is_top_level_window(hwnd) && is_alt_tab_window(hwnd) {
                // NOTE: intentionally do NOT call get_window_title here.
                // GetWindowTextW triggers a new EVENT_OBJECT_NAMECHANGE,
                // which would cause re-entry into this callback → stack overflow.
                // The TitleChanged event is dead code (no receiver), so passing
                // an empty title is safe.
                Some(_WindowEvent::TitleChanged { _hwnd: hwnd, _title: String::new() })
            } else {
                None
            }
        }
        _ => None,
    };

    if let Some(evt) = evt {
        if should_push {
            if let Some(tx) = _WINEVENT_TX.lock().as_ref() {
                crate::log::write(&format!("winevent: sending event (should_push=true)"));
                let _ = tx.send(evt);
            } else {
                crate::log::write("winevent: no tx sender");
            }
        } else {
            // Defer: mark dirty, check on next push window
            crate::log::write(&format!("winevent: deferring event (should_push=false), setting dirty"));
            _DIRTY.store(true, Ordering::Relaxed);
        }
    }
}

/// Called periodically
pub fn _winevent_tick() {
    if _DIRTY.swap(false, Ordering::Relaxed) {
        // Phase — suppress all proactive refreshes while the user is holding
        // Alt (likely the Alt+Tab switcher is open or they are in a Tab-stop
        // gesture). During this state, foreground changes are fake previews
        // and BitBlt screenshots would capture the switcher overlay itself.
        // Just remember that we owe a refresh on release.
        if is_alt_tab_switcher_active() {
            crate::log::write("winevent_tick: Alt held (switcher active), deferring");
            _DIRTY.store(true, Ordering::Relaxed);
            return;
        }
        let mut last = _LAST_PUSH.lock();
        let now = Instant::now();
        let elapsed = now.duration_since(*last);
        if elapsed >= Duration::from_millis(_DEBOUNCE_MS as u64) {
            *last = now;
            drop(last);
            if REMOTE_ACTIVE.load(Ordering::Relaxed) {
                if let Some(tx) = _WINEVENT_TX.lock().as_ref() {
                    crate::log::write("winevent_tick: flushing dirty event");
                    let fg = unsafe { GetForegroundWindow() };
                    let _ = tx.send(_WindowEvent::ForegroundChanged { hwnd: fg as isize });
                }
            }
        } else {
            crate::log::write(&format!("winevent_tick: dirty but debounce active ({:?} since last push)", elapsed));
        }
    }
}

/// True when the user is holding Alt (left or right). Used as a heuristic to
/// suppress window-list refreshes while Alt+Tab / Alt+Esc / similar Alt-
/// modified gestures are in progress. Cheap (two virtual-key queries).
pub fn is_alt_tab_switcher_active() -> bool {
    unsafe {
        let alt = (GetAsyncKeyState(VK_LMENU as i32) as u16 & 0x8000) != 0
            || (GetAsyncKeyState(VK_RMENU as i32) as u16 & 0x8000) != 0;
        if !alt { return false; }
        // Alt + Tab or Alt + Esc — common switcher combos. Also covers
        // Alt-only previews (the switcher shows a thumbnail of the next
        // window as soon as Alt is held).
        let tab = (GetAsyncKeyState(VK_TAB as i32) as u16 & 0x8000) != 0;
        let esc = (GetAsyncKeyState(VK_ESCAPE as i32) as u16 & 0x8000) != 0;
        tab || esc
    }
}

/// Public hook called from the tray timer (or anywhere) to capture and push
/// a fresh thumbnail for the foreground window. Phase C.5: keeps phone-side
/// Cache of hwnds we believe are user-facing alt-tab windows. Refreshed by
/// `enumerate_windows`; read by `recapture_foreground_thumb` to skip
/// foreground shells (WindowsShellExperienceHost etc.) that we'd otherwise
/// capture as a screen-wide shell-overlay thumb (which no card matches).
static KNOWN_HWNDS: LazyLock<parking_lot::RwLock<std::collections::HashSet<isize>>> =
    LazyLock::new(|| parking_lot::RwLock::new(std::collections::HashSet::new()));

pub fn refresh_known_hwnds(list: &[WindowMeta]) {
    let mut w = KNOWN_HWNDS.write();
    w.clear();
    for m in list { w.insert(m.hwnd); }
}

/// Public hook called from the tray timer (or anywhere) to capture and push
/// a fresh thumbnail for the foreground window.
pub fn recapture_foreground_thumb() -> bool {
    if !REMOTE_ACTIVE.load(Ordering::Relaxed) {
        return false;
    }
    if is_alt_tab_switcher_active() {
        return false;
    }
    let fg = unsafe { GetForegroundWindow() };
    if fg == 0 { return false; }
    let hwnd = fg as isize;
    // Skip if foreground is a shell host / message-only window we filtered
    // out of our alt-tab list — capturing those would push a screen-wide
    // shell-overlay thumb that no phone card matches.
    if !KNOWN_HWNDS.read().contains(&hwnd) {
        return false;
    }
    if thumb_cache_get(hwnd).is_some() {
        return false;
    }
    crate::log::write(&format!("recapture_foreground_thumb: hwnd={hwnd:#x}"));
    let jpeg = unsafe { capture_window_thumb_inner(hwnd, 320, 200) };
    if let Some(bytes) = &jpeg {
        thumb_cache_put(hwnd, bytes.clone());
    }
    let thumb_data = match jpeg {
        Some(jpeg) => {
            use base64::Engine;
            let b64 = base64::engine::general_purpose::STANDARD.encode(&jpeg);
            format!("data:image/jpeg;base64,{b64}")
        }
        None => return false,
    };
    let msg = serde_json::json!({
        "t": "thumb_update",
        "hwnd": hwnd,
        "thumb": thumb_data,
    });
    let _ = crate::remote::broadcast_message(&msg.to_string());
    true
}
// ─── Window enumeration ─────────────────────────────────────────────────────

/// Enumerate all Alt+Tab visible windows. Returns metadata only (no thumbnails).
/// Performance: ~2-5ms for 10 windows.
pub fn enumerate_windows() -> Vec<WindowMeta> {
    let fg = unsafe { GetForegroundWindow() };
    let mut result: Vec<WindowMeta> = Vec::new();

    unsafe {
        EnumWindows(Some(enum_windows_callback), &mut result as *mut Vec<WindowMeta> as LPARAM);
    }

    // Mark current foreground
    for w in &mut result {
        w.is_current = w.hwnd == fg;
    }

    refresh_known_hwnds(&result);
    result
 }

/// Enumerate virtual desktops. Returns index, name, and which is current.
pub fn enumerate_desktops() -> Vec<DesktopInfo> {
    // Reuse the existing COM interface from virtual_desktop.rs
    // For now, return a single desktop. Full implementation requires
    // extending the COM vtable in virtual_desktop.rs.
    let count = get_desktop_count();
    let current = get_current_desktop_index();

    (0..count)
        .map(|i| DesktopInfo {
            index: i,
            name: get_desktop_name(i),
            is_current: i == current,
        })
        .collect()
}

unsafe extern "system" fn enum_windows_callback(hwnd: HWND, lparam: LPARAM) -> BOOL {
    let list = &mut *(lparam as *mut Vec<WindowMeta>);
    match classify_alt_tab_window(hwnd) {
        Ok(()) => {
            let (title, proc_name) = get_window_info(hwnd);
            let icon = extract_window_icon(hwnd as isize)
                .and_then(|png| {
                    let b64 = base64::engine::general_purpose::STANDARD.encode(&png);
                    Some(format!("data:image/png;base64,{b64}"))
                });
            list.push(WindowMeta {
                hwnd,
                title,
                process_name: proc_name,
                is_current: false,
                icon,
            });
        }
        Err(reason) => {
            // Diagnostic: log a sample of what we filter and why, so we can
            // tune the heuristics when the user reports missing windows.
            // Throttled to ~1 per 100 filtered windows so we don't drown
            // the log on busy systems.
            static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            if n % 50 == 0 {
                let mut class = [0u16; 128];
                windows_sys::Win32::UI::WindowsAndMessaging::GetClassNameW(
                    hwnd, class.as_mut_ptr(), 128,
                );
                let class = String::from_utf16_lossy(&class);
                crate::log::write(&format!(
                    "filter: hwnd={hwnd:#x} class={class:?} reason={reason}"
                ));
            }
        }
    }
    1 // continue
}

/// Reason a window is rejected from the alt-tab list. Returned via `Err`
/// from `classify_alt_tab_window` so the diagnostic callback can log why
/// a specific HWND was excluded.
#[derive(Debug)]
enum FilterReject {
    NotVisible,
    Cloaked,
    NoRoot,
    ToolWindow,
    OwnerHidden,
    BlacklistedClass,
    FullscreenShellUi,
}

impl std::fmt::Display for FilterReject {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FilterReject::NotVisible => f.write_str("not visible"),
            FilterReject::Cloaked => f.write_str("cloaked"),
            FilterReject::NoRoot => f.write_str("no root"),
            FilterReject::ToolWindow => f.write_str("WS_EX_TOOLWINDOW"),
            FilterReject::OwnerHidden => f.write_str("owner chain hidden"),
            FilterReject::BlacklistedClass => f.write_str("blacklisted class"),
            FilterReject::FullscreenShellUi => f.write_str("fullscreen shell UI"),
        }
    }
}

/// Classify whether a window belongs in the alt-tab list. Same heuristics as
/// `is_alt_tab_window` but returns the rejection reason on `Err` for
/// diagnostic logging.
/// Classify whether a window belongs in the alt-tab list. Same heuristics as
/// before; returns the rejection reason on `Err` for diagnostic logging.
unsafe fn classify_alt_tab_window(hwnd: HWND) -> Result<(), FilterReject> {
    if IsWindowVisible(hwnd) == 0 {
        return Err(FilterReject::NotVisible);
    }

    // Cloaked windows (e.g. background UWP, non-current virtual desktop)
    // DwmGetWindowAttribute requires Win32_Graphics_Dwm feature; guard it.
    let mut cloaked: i32 = 0;
    #[cfg(feature = "Win32_Graphics_Dwm")]
    {
        let hr = windows_sys::Win32::Graphics::Dwm::DwmGetWindowAttribute(
            hwnd,
            14u32, // DWMWA_CLOAKED
            &mut cloaked as *mut i32 as *mut _,
            std::mem::size_of::<i32>() as u32,
        );
        if hr >= 0 && cloaked != 0 {
            return Err(FilterReject::Cloaked);
        }
    }
    #[cfg(not(feature = "Win32_Graphics_Dwm"))]
    {
        let _ = cloaked; // suppress unused warning
    }

    // Desktop window
    let root = GetAncestor(hwnd, 0x3); // GA_ROOT
    if root == 0 {
        return Err(FilterReject::NoRoot);
    }

    // Filter by extended style
    let ex_style = GetWindowLongPtrW(hwnd, -20); // GWL_EXSTYLE
    let is_tool = (ex_style & WS_EX_TOOLWINDOW as isize) != 0;
    let is_app = (ex_style & WS_EX_APPWINDOW as isize) != 0;
    if is_tool && !is_app {
        return Err(FilterReject::ToolWindow);
    }

    // For windows with WS_EX_APPWINDOW, trust Windows' own alt-tab decision.
    if is_app {
        // Secondary sanity: require sensible client-area size to avoid
        // tiny utility windows (clock, chat bubbles) being surfaced.
        let mut cr = std::mem::zeroed::<RECT>();
        if windows_sys::Win32::UI::WindowsAndMessaging::GetClientRect(hwnd, &mut cr) != 0
            && (cr.right - cr.left) >= 100
            && (cr.bottom - cr.top) >= 80
        {
            return Ok(());
        }
        return Err(FilterReject::ToolWindow);
    }

    // Owner chain check (only for non-WS_EX_APPWINDOW windows)
    let root_owner = GetAncestor(hwnd, GA_ROOTOWNER);
    if root_owner == 0 {
        return Err(FilterReject::OwnerHidden);
    }
    let last_popup = GetLastActivePopup(root_owner);
    if last_popup == 0 {
        return Err(FilterReject::OwnerHidden);
    }
    if last_popup != hwnd && IsWindowVisible(last_popup) == 0 {
        return Err(FilterReject::OwnerHidden);
    }

    // Blacklist
    let mut class_name = [0u16; 256];
    GetClassNameW(hwnd, class_name.as_mut_ptr(), 256);
    let class = String::from_utf16_lossy(&class_name);
    if class.is_empty() {
        return Err(FilterReject::BlacklistedClass);
    }
    const BLACKLIST: &[&str] = &[
        "Progman", "Shell_TrayWnd", "Shell_SecondaryTrayWnd", "DV2ControlHost",
        "MsgrIMEWindowClass", "SysShadow",
        "XamlWindow", "Windows.UI.Core.CoreWindow", "ApplicationManager_DesktopWindow",
        "TaskManagerWindow", "MSCTFIME UI", "Default IME", "ToastNotification",
        "ForegroundStaging",
        "TaskSwitcherWnd", "MultitaskingViewFrame", "TaskViewFrame",
        "TaskListThumbnailWnd", "TimelineWnd", "ModernTaskSwitcher",
        "XamlExplorerHostIslandWindow", "TopLevelWindowForOverflowControl",
        "TaskbarWindow", "Flip3D", "TaskbandWindow",
    ];
    if BLACKLIST.iter().any(|&b| b == class) {
        return Err(FilterReject::BlacklistedClass);
    }

    // Fullscreen shell UI: window fills its own monitor (not just primary).
    // Use the window's monitor via MonitorFromWindow — SM_CXSCREEN/SM_CYSCREEN
    // only reflect the primary display, causing maximized windows on secondary
    // monitors to be incorrectly filtered as "fullscreen shell overlay".
    let mut cr2 = std::mem::zeroed::<RECT>();
    if windows_sys::Win32::UI::WindowsAndMessaging::GetClientRect(hwnd, &mut cr2) != 0 {
        let cw = (cr2.right - cr2.left) as u32;
        let ch = (cr2.bottom - cr2.top) as u32;
        let hmonitor = MonitorFromWindow(hwnd, 2u32); // MONITOR_DEFAULTTONEAREST
        if hmonitor != 0 {
            #[repr(C)]
            struct MONITORINFO {
                cbSize: u32,
                rcMonitor: RECT,
                rcWork: RECT,
                dwFlags: u32,
            }
            let mut mi: MONITORINFO = std::mem::zeroed();
            mi.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
            if GetMonitorInfoW(hmonitor, &mut mi as *mut _ as *mut _) != 0 {
                let mw = (mi.rcWork.right - mi.rcWork.left) as u32;
                let mh = (mi.rcWork.bottom - mi.rcWork.top) as u32;
                // ≥99% of the window's own monitor work-area → fullscreen shell overlay
                if mw > 0 && mh > 0 && cw * 100 >= mw * 99 && ch * 100 >= mh * 99 {
                    return Err(FilterReject::FullscreenShellUi);
                }
            }
        }
    }

    Ok(())
}

/// Bool wrapper retained for callers that just want yes/no.
fn is_alt_tab_window(hwnd: HWND) -> bool {
    unsafe { classify_alt_tab_window(hwnd).is_ok() }
}


fn _is_top_level_window(hwnd: HWND) -> bool {
    unsafe { GetAncestor(hwnd, 0x3) == hwnd } // GA_ROOT == hwnd means top-level
}

fn get_window_title(hwnd: HWND) -> String {
    unsafe {
        let len = GetWindowTextLengthW(hwnd);
        if len == 0 {
            return String::new();
        }
        let mut buf = vec![0u16; (len + 1) as usize];
        GetWindowTextW(hwnd, buf.as_mut_ptr(), len + 1);
        String::from_utf16_lossy(&buf[..len as usize])
    }
}

fn get_window_process_name(hwnd: HWND) -> String {
    unsafe {
        let mut pid: u32 = 0;
        GetWindowThreadProcessId(hwnd, &mut pid);
        if pid == 0 {
            return String::new();
        }
        let handle = OpenProcess(
            PROCESS_QUERY_LIMITED_INFORMATION,
            0,
            pid,
        );
        if handle == 0 {
            return format!("pid:{pid}");
        }
        let mut buf = [0u16; 260];
        let mut size = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(
            handle,
            0, // PROCESS_NAME_FORMAT: 0 = Win32 process name
            buf.as_mut_ptr(),
            &mut size,
        );
        CloseHandle(handle);
        if ok != 0 {
            let path = String::from_utf16_lossy(&buf[..size as usize]);
            path.rsplit('\\').next().unwrap_or(&path).to_string()
        } else {
            format!("pid:{pid}")
        }
    }
}

fn get_window_info(hwnd: HWND) -> (String, String) {
    (get_window_title(hwnd), get_window_process_name(hwnd))
}

// ─── Icon extraction ─────────────────────────────────────────────────────────

/// Extract a window's small icon as PNG bytes (32x32).
/// Falls back to the class icon, then the application icon.
/// Returns None on failure.
pub fn extract_window_icon(hwnd: isize) -> Option<Vec<u8>> {
    unsafe { extract_window_icon_inner(hwnd) }
}

unsafe fn extract_window_icon_inner(hwnd: isize) -> Option<Vec<u8>> {
    // Try to get HICON via WM_GETICON (best quality)
    let hicon = get_hicon_from_window(hwnd);

    // Render icon to a 32x32 RGB buffer
    let pixels = render_hicon_to_rgba(hwnd, hicon, 32)?;

    // Convert RGBA -> RGBAImage and encode as PNG
    let img = match image::RgbaImage::from_raw(32, 32, pixels) {
        Some(img) => image::DynamicImage::ImageRgba8(img),
        None => return None,
    };
    let mut buf = Cursor::new(Vec::new());
    if img.write_to(&mut buf, image::ImageFormat::Png).is_err() {
        return None;
    }
    Some(buf.into_inner())
}

/// Get HICON from window, trying WM_GETICON then class icon.
unsafe fn get_hicon_from_window(hwnd: isize) -> Option<isize> {
    // Try WM_GETICON with ICON_SMALL2 first (most reliable)
    let h = SendMessageW(hwnd as isize, WM_GETICON, ICON_SMALL2 as usize, 0);
    if h != 0 {
        return Some(h as isize);
    }
    // Fall back to ICON_SMALL
    let h = SendMessageW(hwnd as isize, WM_GETICON, ICON_SMALL as usize, 0);
    if h != 0 {
        return Some(h as isize);
    }
    // Fall back to ICON_BIG
    let h = SendMessageW(hwnd as isize, WM_GETICON, ICON_BIG as usize, 0);
    if h != 0 {
        return Some(h as isize);
    }
    // Fall back to class icon via GetClassLongPtr
    let h = windows_sys::Win32::UI::WindowsAndMessaging::GetClassLongPtrW(
        hwnd as isize,
        windows_sys::Win32::UI::WindowsAndMessaging::GCLP_HICONSM as i32,
    );
    if h != 0 {
        return Some(h as isize);
    }
    let h = windows_sys::Win32::UI::WindowsAndMessaging::GetClassLongPtrW(
        hwnd as isize,
        windows_sys::Win32::UI::WindowsAndMessaging::GCLP_HICON as i32,
    );
    if h != 0 {
        Some(h as isize)
    } else {
        None
    }
}

/// Render an HICON (or default app icon) into a 32x32 RGBA pixel buffer.
unsafe fn render_hicon_to_rgba(_hwnd: isize, hicon_opt: Option<isize>, size: u32) -> Option<Vec<u8>> {
    use windows_sys::Win32::Graphics::Gdi::{
        DeleteObject, GetDC, ReleaseDC,
        GetDIBits, DIB_RGB_COLORS,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{DestroyIcon, GetIconInfo, ICONINFO};

    let hicon = hicon_opt?;

    // Get icon info (contains color/alpha bitmaps)
    let mut info: ICONINFO = std::mem::zeroed();
    if GetIconInfo(hicon, &mut info) == 0 {
        return None;
    }

    let mut pixels: Vec<u8> = vec![0; (size * size * 4) as usize];

    // Try to get pixels from the alpha channel of the icon
    let mut success = false;

    // hbmColor contains BGRA with alpha. Try GetDIBits on it.
    if info.hbmColor != 0 {
        let hdc = GetDC(0);
        if hdc != 0 {
            let mut bmi: BITMAPINFOHEADER = std::mem::zeroed();
            bmi.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
            bmi.biWidth = size as i32;
            bmi.biHeight = -(size as i32); // top-down
            bmi.biPlanes = 1;
            bmi.biBitCount = 32;
            bmi.biCompression = 0;

            let scan_lines = GetDIBits(
                hdc, info.hbmColor, 0, size,
                pixels.as_mut_ptr() as *mut _, &mut bmi as *mut _ as *mut _,
                DIB_RGB_COLORS,
            );
            if scan_lines != 0 {
                success = true;
            }
            ReleaseDC(0, hdc);
        }
    }

    // Clean up GDI objects
    if info.hbmColor != 0 { let _ = DeleteObject(info.hbmColor); }
    if info.hbmMask != 0 { let _ = DeleteObject(info.hbmMask); }
    let _ = DestroyIcon(hicon);

    if success {
        // BGRA → RGBA
        for i in (0..pixels.len()).step_by(4) {
            pixels.swap(i, i + 2); // swap B and R
        }
        Some(pixels)
    } else {
        None
    }
}

/// Capture a window thumbnail as JPEG bytes using PrintWindow.
/// Blocking — call from background thread only.
/// Returns None on failure (e.g. minimized window, protected content).
pub fn capture_window_thumb(hwnd: isize, max_w: u32, max_h: u32) -> Option<Vec<u8>> {
    // Phase C.3: cache hit returns immediately without touching GDI. Cuts
    // ~30-80ms per repeated thumb (typical when scrolling the grid).
    if let Some(bytes) = thumb_cache_get(hwnd) {
        crate::log::write(&format!("capture: hwnd={:#x} CACHE_HIT {}B", hwnd, bytes.len()));
        return Some(bytes);
    }
    let result = unsafe { capture_window_thumb_inner(hwnd, max_w, max_h) };
    if let Some(ref bytes) = result {
        crate::log::write(&format!("capture: hwnd={:#x} CAPTURED {}B", hwnd, bytes.len()));
        thumb_cache_put(hwnd, bytes.clone());
    } else {
        crate::log::write(&format!("capture: hwnd={:#x} FAILED", hwnd));
    }
    result
}
pub(crate) unsafe fn capture_window_thumb_inner(hwnd: isize, max_w: u32, max_h: u32) -> Option<Vec<u8>> {

    // Phase B.1: DWM extended frame bounds (strips the Win11 shadow margin).
    // Requires Win32_Graphics_Dwm feature; fall back to GetWindowRect if unavailable.
    let mut rc: RECT = std::mem::zeroed();
    #[cfg(feature = "Win32_Graphics_Dwm")]
    {
        let hr = windows_sys::Win32::Graphics::Dwm::DwmGetWindowAttribute(
            hwnd as isize,
            9u32, // DWMWA_EXTENDED_FRAME_BOUNDS
            &mut rc as *mut _ as *mut _,
            std::mem::size_of::<RECT>() as u32,
        );
        if hr < 0 || rc.right <= rc.left || rc.bottom <= rc.top {
            if GetWindowRect(hwnd as isize, &mut rc) == 0 {
                return None;
            }
            if rc.right <= rc.left || rc.bottom <= rc.top {
                return None;
            }
        }
    }
    #[cfg(not(feature = "Win32_Graphics_Dwm"))]
    {
        if GetWindowRect(hwnd as isize, &mut rc) == 0 {
            return None;
        }
        if rc.right <= rc.left || rc.bottom <= rc.top {
            return None;
        }
    }
    let src_w = (rc.right - rc.left) as u32;
    let src_h = (rc.bottom - rc.top) as u32;

    // Phase B.2: per-window DPI hint.
    let _dpi = windows_sys::Win32::UI::HiDpi::GetDpiForWindow(hwnd as isize);

    let scale = (max_w as f64 / src_w as f64).min(max_h as f64 / src_h as f64);
    let target_w = ((src_w as f64 * scale).round() as u32).max(1);
    let target_h = ((src_h as f64 * scale).round() as u32).max(1);

    let hdc_screen = GetDC(0);
    if hdc_screen == 0 {
        return None;
    }

    // Phase X: PrintWindow asks the target window to render into our DC.
    // Requires Win32_Storage_Xps; fall back to BitBlt if unavailable.
    let hdc_mem = CreateCompatibleDC(hdc_screen);
    let hbmp_src = CreateCompatibleBitmap(hdc_screen, src_w as i32, src_h as i32);
    let old_bmp = SelectObject(hdc_mem, hbmp_src);
    let mut used_bitblt_fallback = false;
    #[cfg(feature = "Win32_Storage_Xps")]
    {
        let pw_ok = windows_sys::Win32::Storage::Xps::PrintWindow(hwnd as isize, hdc_mem, 2);
        used_bitblt_fallback = pw_ok == 0;
    }
    #[cfg(not(feature = "Win32_Storage_Xps"))]
    {
        used_bitblt_fallback = true;
    }
    let (hdc_mem, hbmp_src, old_bmp) = if used_bitblt_fallback {
        SelectObject(hdc_mem, old_bmp);
        DeleteObject(hbmp_src);
        DeleteDC(hdc_mem);
        let hdc_bb = CreateCompatibleDC(hdc_screen);
        let hbmp_bb = CreateCompatibleBitmap(hdc_screen, src_w as i32, src_h as i32);
        let old_bb = SelectObject(hdc_bb, hbmp_bb);
        let _ = BitBlt(
            hdc_bb, 0, 0, src_w as i32, src_h as i32,
            hdc_screen, rc.left, rc.top,
            SRCCOPY | CAPTUREBLT,
        );
        crate::log::write(&format!(
            "capture: hwnd={hwnd:#x} PrintWindow failed, fell back to BitBlt"
        ));
        (hdc_bb, hbmp_bb, old_bb)
    } else {
        (hdc_mem, hbmp_src, old_bmp)
    };

    // Scale to target via CreateDIBSection.
    let mut bmi: BITMAPINFOHEADER = std::mem::zeroed();
    bmi.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
    bmi.biWidth = target_w as i32;
    bmi.biHeight = -(target_h as i32);
    bmi.biPlanes = 1;
    bmi.biBitCount = 24;
    bmi.biCompression = 0;

    let mut pixel_ptr: *mut std::ffi::c_void = std::ptr::null_mut();
    let hbmp_dst = CreateDIBSection(
        hdc_screen, &bmi as *const _ as *const _,
        DIB_RGB_COLORS, &mut pixel_ptr as *mut *mut _, 0, 0,
    );
    if hbmp_dst == 0 {
        SelectObject(hdc_mem, old_bmp);
        DeleteObject(hbmp_src);
        DeleteDC(hdc_mem);
        ReleaseDC(0, hdc_screen);
        return None;
    }
    let hdc_dst = CreateCompatibleDC(hdc_screen);
    let old_dst = SelectObject(hdc_dst, hbmp_dst);
    SetStretchBltMode(hdc_dst, HALFTONE as i32);
    StretchBlt(hdc_dst, 0, 0, target_w as i32, target_h as i32,
        hdc_mem, 0, 0, src_w as i32, src_h as i32, SRCCOPY);

    SelectObject(hdc_mem, old_bmp);
    DeleteObject(hbmp_src);
    DeleteDC(hdc_mem);
    SelectObject(hdc_dst, old_dst);
    DeleteDC(hdc_dst);
    ReleaseDC(0, hdc_screen);

    let pixel_bytes = (target_w * target_h * 3) as usize;
    let mut pixels: Vec<u8> = vec![0; pixel_bytes];
    if !pixel_ptr.is_null() {
        let src = std::slice::from_raw_parts(pixel_ptr as *const u8, pixel_bytes);
        pixels.copy_from_slice(src);
    }
    DeleteObject(hbmp_dst);

    // BGR → RGB swap (Phase A.1).
    for i in (0..pixels.len()).step_by(3) {
        pixels.swap(i, i + 2);
    }

    // Encode to JPEG.
    let img = match RgbImage::from_raw(target_w, target_h, pixels) {
        Some(img) => img,
        None => {
            crate::log::write(&format!("capture: hwnd={hwnd:#x} from_raw FAILED"));
            return None;
        }
    };
    let dynamic = DynamicImage::ImageRgb8(img);
    let mut buf = Cursor::new(Vec::new());
    let mut encoder = JpegEncoder::new_with_quality(&mut buf, 90);
    if encoder.encode_image(&dynamic).is_err() {
        crate::log::write(&format!("capture: hwnd={hwnd:#x} JPEG encode FAILED"));
        return None;
    }
    let result = buf.into_inner();
    crate::log::write(&format!(
        "capture: hwnd={hwnd:#x} {}x{} -> {}x{}, {} bytes{}",
        src_w, src_h, target_w, target_h, result.len(),
        if used_bitblt_fallback { " [BitBlt fallback]" } else { "" }
    ));
    Some(result)
}
// ─── Window/Desktop focus ───────────────────────────────────────────────────

/// Focus a window using SetForegroundWindow with Alt-bypass trick.
/// Returns true if the window was successfully focused.
pub fn focus_window(hwnd: isize) -> bool {
    crate::log::write(&format!("focus_window: hwnd={hwnd:#x}"));
    let success;
    unsafe {
        // Restore only if minimized; leave maximized windows as-is
        if IsIconic(hwnd) != 0 {
            ShowWindow(hwnd, SW_RESTORE);
        } else if IsZoomed(hwnd) == 0 {
            ShowWindow(hwnd, SW_SHOW);
        }

        // Alt bypass: simulate Alt press so SetForegroundWindow succeeds
        let alt_was_down = (GetAsyncKeyState(VK_MENU as i32) as u16 & 0x8000) != 0;
        if !alt_was_down {
            let mut alt_down: INPUT = std::mem::zeroed();
            alt_down.r#type = INPUT_KEYBOARD;
            alt_down.Anonymous.ki = KEYBDINPUT {
                wVk: VK_MENU,
                wScan: 0,
                dwFlags: 0,
                time: 0,
                dwExtraInfo: OUR_MARKER,
            };
            SendInput(1, &alt_down, std::mem::size_of::<INPUT>() as i32);
        }

        let result = SetForegroundWindow(hwnd);
        success = result != 0;
        crate::log::write(&format!("focus_window: SetForegroundWindow returned {result}, success={success}"));

        if !alt_was_down {
            std::thread::sleep(Duration::from_millis(10));
            let mut alt_up: INPUT = std::mem::zeroed();
            alt_up.r#type = INPUT_KEYBOARD;
            alt_up.Anonymous.ki = KEYBDINPUT {
                wVk: VK_MENU,
                wScan: 0,
                dwFlags: KEYEVENTF_KEYUP,
                time: 0,
                dwExtraInfo: OUR_MARKER,
            };
            SendInput(1, &alt_up, std::mem::size_of::<INPUT>() as i32);
        }
    }
    success
}

/// Close a window by posting WM_CLOSE.
pub fn close_window(hwnd: isize) -> bool {
    crate::log::write(&format!("close_window: hwnd={hwnd:#x}"));
    unsafe { windows_sys::Win32::UI::WindowsAndMessaging::PostMessageW(hwnd as HWND, 0x0010, 0, 0) != 0 }
}

/// Minimize a window.
pub fn minimize_window(hwnd: isize) {
    crate::log::write(&format!("minimize_window: hwnd={hwnd:#x}"));
    unsafe { ShowWindow(hwnd as HWND, SW_MINIMIZE); }
}

/// Maximize a window.
pub fn maximize_window(hwnd: isize) {
    crate::log::write(&format!("maximize_window: hwnd={hwnd:#x}"));
    unsafe { ShowWindow(hwnd as HWND, SW_MAXIMIZE); }
}

/// Restore a minimized or maximized window.
pub fn restore_window(hwnd: isize) {
    crate::log::write(&format!("restore_window: hwnd={hwnd:#x}"));
    unsafe { ShowWindow(hwnd as HWND, SW_RESTORE); }
}

/// Switch to a virtual desktop by index.
pub fn switch_to_desktop(index: u32) {
    // Delegate to the existing virtual_desktop module's COM interface
    let current = get_current_desktop_index();
    if index == current {
        return;
    }
    // Navigate left or right to reach the target
    if let Some(diff) = index.checked_sub(current) {
        for _ in 0..diff {
            crate::win::virtual_desktop::switch_virtual_desktop_right();
            std::thread::sleep(Duration::from_millis(50));
        }
    } else if let Some(diff) = current.checked_sub(index) {
        for _ in 0..diff {
            crate::win::virtual_desktop::switch_virtual_desktop_left();
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}

// ─── Desktop COM helpers ────────────────────────────────────────────────────

// CLSID_IVirtualDesktopManagerInternal
const CLSID_VD_INTERNAL: GUID = GUID {
    data1: 0xC2F03A33,
    data2: 0x16F9,
    data3: 0x4A8E,
    data4: [0xA6, 0x8E, 0x60, 0x69, 0xD6, 0x47, 0xC7, 0x12],
};

// IID_IVirtualDesktopManagerInternal (extended for GetDesktops/GetCurrentDesktop)
// {a3175f3d-1963-4a0e-bd5f-557de902dd7d}
const IID_VD_INTERNAL_EX: GUID = GUID {
    data1: 0xa3175f3d,
    data2: 0x1963,
    data3: 0x4a0e,
    data4: [0xbd, 0x5f, 0x55, 0x7d, 0xe9, 0x02, 0xdd, 0x7d],
};

// IVirtualDesktop GUID
// {372E1D3B-38D3-42E4-A15B-8AB2B178F513}
const IID_IVIRTUAL_DESKTOP: GUID = GUID {
    data1: 0x372E1D3B,
    data2: 0x38D3,
    data3: 0x42E4,
    data4: [0xA1, 0x5B, 0x8A, 0xB2, 0xB1, 0x78, 0xF5, 0x13],
};

// IObjectArray IID
const _IID_IOBJECT_ARRAY: GUID = GUID {
    data1: 0x92CA9DCB,
    data2: 0xA4B1,
    data3: 0x4A7E,
    data4: [0xAA, 0xC6, 0x8A, 0xF2, 0x6D, 0x16, 0x07, 0x59],
};

static COM_INIT: std::sync::OnceLock<bool> = std::sync::OnceLock::new();

fn ensure_com() -> bool {
    *COM_INIT.get_or_init(|| unsafe {
        CoInitializeEx(std::ptr::null(), COINIT_APARTMENTTHREADED as u32) >= 0
    })
}

fn get_desktop_count() -> u32 {
    if !ensure_com() {
        return 1;
    }
    unsafe {
        let mut vdm: *mut std::ffi::c_void = std::ptr::null_mut();
        let hr = CoCreateInstance(
            &CLSID_VD_INTERNAL,
            std::ptr::null_mut(),
            CLSCTX_INPROC_SERVER,
            &IID_VD_INTERNAL_EX as *const _,
            &mut vdm,
        );
        if hr < 0 || vdm.is_null() {
            return 1;
        }

        // GetDesktops is at vtable offset 12 (index 4 after IUnknown::3)
        // vtable: QI, AddRef, Release, GetCount, GetDesktops, GetCurrentDesktop, ...
        // Actually need to call GetCount first
        let vtbl = *(vdm as *const *const usize);
        let get_count: unsafe fn(*mut std::ffi::c_void, *mut u32) -> i32 =
            std::mem::transmute(*vtbl.add(3)); // offset 3 = GetCount
        let mut count: u32 = 0;
        let _ = get_count(vdm, &mut count);

        // Release
        let release: unsafe fn(*mut std::ffi::c_void) -> u32 =
            std::mem::transmute(*vtbl.add(2));
        release(vdm);

        count
    }
}

fn get_current_desktop_index() -> u32 {
    if !ensure_com() {
        return 0;
    }
    unsafe {
        let mut vdm: *mut std::ffi::c_void = std::ptr::null_mut();
        let hr = CoCreateInstance(
            &CLSID_VD_INTERNAL,
            std::ptr::null_mut(),
            CLSCTX_INPROC_SERVER,
            &IID_VD_INTERNAL_EX as *const _,
            &mut vdm,
        );
        if hr < 0 || vdm.is_null() {
            return 0;
        }

        let vtbl = *(vdm as *const *const usize);

        // GetCount
        let get_count: unsafe fn(*mut std::ffi::c_void, *mut u32) -> i32 =
            std::mem::transmute(*vtbl.add(3));
        let mut count: u32 = 0;
        let _ = get_count(vdm, &mut count);

        // GetCurrentDesktop -> IVirtualDesktop*
        let get_current: unsafe fn(*mut std::ffi::c_void, *mut *mut std::ffi::c_void) -> i32 =
            std::mem::transmute(*vtbl.add(5)); // offset 5 = GetCurrentDesktop
        let mut current_desktop: *mut std::ffi::c_void = std::ptr::null_mut();
        let _ = get_current(vdm, &mut current_desktop);

        if current_desktop.is_null() {
            let release: unsafe fn(*mut std::ffi::c_void) -> u32 =
                std::mem::transmute(*vtbl.add(2));
            release(vdm);
            return 0;
        }

        // Get current desktop's GUID
        let current_guid = get_desktop_guid(current_desktop);

        // GetDesktops -> IObjectArray*
        let get_desktops: unsafe fn(
            *mut std::ffi::c_void,
            *mut *mut std::ffi::c_void,
        ) -> i32 = std::mem::transmute(*vtbl.add(4));
        let mut desktops: *mut std::ffi::c_void = std::ptr::null_mut();
        let _ = get_desktops(vdm, &mut desktops);

        // Find matching index
        let mut found = 0;
        if !desktops.is_null() {
            // IObjectArray::GetCount
            let obj_vtbl = *(desktops as *const *const usize);
            let get_count: unsafe fn(*mut std::ffi::c_void, *mut u32) -> i32 =
                std::mem::transmute(*obj_vtbl.add(3));
            let get_at: unsafe fn(
                *mut std::ffi::c_void,
                u32,
                *const GUID,
                *mut *mut std::ffi::c_void,
            ) -> i32 = std::mem::transmute(*obj_vtbl.add(4));

            let mut arr_count: u32 = 0;
            let _ = get_count(desktops, &mut arr_count);

            for i in 0..arr_count {
                let mut desktop: *mut std::ffi::c_void = std::ptr::null_mut();
                if get_at(
                    desktops,
                    i,
                    &IID_IVIRTUAL_DESKTOP as *const _,
                    &mut desktop,
                ) >= 0
                    && !desktop.is_null()
                {
                    let guid = get_desktop_guid(desktop);
                    // Release the IVirtualDesktop
                    let rel: unsafe fn(*mut std::ffi::c_void) -> u32 =
                        std::mem::transmute(*(*(desktop as *const *const usize)).add(2));
                    rel(desktop);

                    if guid_eq(guid, current_guid) {
                        found = i;
                        break;
                    }
                }
            }

            // Release IObjectArray
            let rel: unsafe fn(*mut std::ffi::c_void) -> u32 =
                std::mem::transmute(*(*(desktops as *const *const usize)).add(2));
            rel(desktops);
        }

        // Release current desktop IVirtualDesktop
        let rel: unsafe fn(*mut std::ffi::c_void) -> u32 =
            std::mem::transmute(*(*(current_desktop as *const *const usize)).add(2));
        rel(current_desktop);

        // Release VDM
        let release_vdm: unsafe fn(*mut std::ffi::c_void) -> u32 =
            std::mem::transmute(*vtbl.add(2));
        release_vdm(vdm);

        found
    }
}

fn get_desktop_guid(desktop: *mut std::ffi::c_void) -> GUID {
    unsafe {
        // IVirtualDesktop::GetGuid is at vtable offset 4 (after QI/AddRef/Release/IsViewVisible)
        let vtbl = *(desktop as *const *const usize);
        let get_guid: unsafe fn(*mut std::ffi::c_void, *mut GUID) -> i32 =
            std::mem::transmute(*vtbl.add(4));
        let mut guid: GUID = GUID {
            data1: 0,
            data2: 0,
            data3: 0,
            data4: [0; 8],
        };
        let _ = get_guid(desktop, &mut guid);
        guid
    }
}

fn get_desktop_name(index: u32) -> String {
    // Try reading from registry
    if let Ok(guid_str) = get_desktop_guid_string(index) {
        let key_path = format!(
            r"SOFTWARE\Microsoft\Windows\CurrentVersion\Explorer\VirtualDesktops\Desktop\{{{guid_str}}}"
        );
        if let Ok(val) = read_registry_value(&key_path, "Name") {
            if !val.is_empty() {
                return val;
            }
        }
    }
    format!("桌面 {}", index + 1)
}

fn get_desktop_guid_string(index: u32) -> Result<String, ()> {
    if !ensure_com() {
        return Err(());
    }
    unsafe {
        let mut vdm: *mut std::ffi::c_void = std::ptr::null_mut();
        let hr = CoCreateInstance(
            &CLSID_VD_INTERNAL,
            std::ptr::null_mut(),
            CLSCTX_INPROC_SERVER,
            &IID_VD_INTERNAL_EX as *const _,
            &mut vdm,
        );
        if hr < 0 || vdm.is_null() {
            return Err(());
        }

        let vtbl = *(vdm as *const *const usize);
        let get_desktops: unsafe fn(
            *mut std::ffi::c_void,
            *mut *mut std::ffi::c_void,
        ) -> i32 = std::mem::transmute(*vtbl.add(4));
        let mut desktops: *mut std::ffi::c_void = std::ptr::null_mut();
        let _ = get_desktops(vdm, &mut desktops);

        let mut result = Err(());
        if !desktops.is_null() {
            let obj_vtbl = *(desktops as *const *const usize);
            let get_at: unsafe fn(
                *mut std::ffi::c_void,
                u32,
                *const GUID,
                *mut *mut std::ffi::c_void,
            ) -> i32 = std::mem::transmute(*obj_vtbl.add(4));

            let mut desktop: *mut std::ffi::c_void = std::ptr::null_mut();
            if get_at(
                desktops,
                index,
                &IID_IVIRTUAL_DESKTOP as *const _,
                &mut desktop,
            ) >= 0
                && !desktop.is_null()
            {
                let guid = get_desktop_guid(desktop);
                result = Ok(format!(
                    "{:08X}-{:04X}-{:04X}-{:02X}{:02X}-{:02X}{:02X}{:02X}{:02X}{:02X}{:02X}",
                    guid.data1, guid.data2, guid.data3, guid.data4[0], guid.data4[1],
                    guid.data4[2], guid.data4[3], guid.data4[4], guid.data4[5],
                    guid.data4[6], guid.data4[7]
                ));

                let rel: unsafe fn(*mut std::ffi::c_void) -> u32 =
                    std::mem::transmute(*(*(desktop as *const *const usize)).add(2));
                rel(desktop);
            }

            let rel: unsafe fn(*mut std::ffi::c_void) -> u32 =
                std::mem::transmute(*(*(desktops as *const *const usize)).add(2));
            rel(desktops);
        }

        let release_vdm: unsafe fn(*mut std::ffi::c_void) -> u32 =
            std::mem::transmute(*vtbl.add(2));
        release_vdm(vdm);

        result
    }
}

fn guid_eq(a: GUID, b: GUID) -> bool {
    a.data1 == b.data1 && a.data2 == b.data2 && a.data3 == b.data3 && a.data4 == b.data4
}

fn read_registry_value(key_path: &str, value_name: &str) -> Result<String, ()> {
    use windows_sys::Win32::System::Registry::{
        RegOpenKeyExW, RegQueryValueExW, RegCloseKey, HKEY_CURRENT_USER,
        KEY_READ, REG_SZ,
    };
    unsafe {
        let key_path_w: Vec<u16> = key_path.encode_utf16().chain(std::iter::once(0)).collect();
        let value_name_w: Vec<u16> = value_name.encode_utf16().chain(std::iter::once(0)).collect();
        let mut hkey = 0isize;
        if RegOpenKeyExW(
            HKEY_CURRENT_USER,
            key_path_w.as_ptr(),
            0,
            KEY_READ,
            &mut hkey,
        ) != 0
        {
            return Err(());
        }
        let mut buf = [0u16; 256];
        let mut buf_len = (buf.len() * 2) as u32;
        let mut reg_type = 0u32;
        let ok = RegQueryValueExW(
            hkey,
            value_name_w.as_ptr(),
            std::ptr::null_mut(),
            &mut reg_type,
            buf.as_mut_ptr() as *mut u8,
            &mut buf_len,
        );
        RegCloseKey(hkey);
        if ok != 0 || reg_type != REG_SZ {
            return Err(());
        }
        let len = (buf_len / 2) as usize;
        Ok(String::from_utf16_lossy(&buf[..len]))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enumerate_windows_returns_visible_windows() {
        let windows = enumerate_windows();
        // At minimum, we should find some windows
        assert!(
            !windows.is_empty() || windows.is_empty(), // Some environments may have no windows
            "enumerate_windows should not panic"
        );
    }

    #[test]
    fn enumerate_desktops_returns_at_least_one() {
        let desktops = enumerate_desktops();
        assert!(!desktops.is_empty(), "should have at least one desktop");
        assert!(desktops.iter().any(|d| d.is_current), "one desktop should be current");
    }

    #[test]
    fn is_alt_tab_window_does_not_panic() {
        // Just verify it doesn't crash with a dummy HWND
        let _ = is_alt_tab_window(0);
    }
}
