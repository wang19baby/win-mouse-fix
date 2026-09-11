use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    SendInput, INPUT, INPUT_KEYBOARD, INPUT_MOUSE, KEYBDINPUT, KEYEVENTF_KEYUP, KEYEVENTF_UNICODE,
    MOUSEEVENTF_HWHEEL, MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP, MOUSEEVENTF_MIDDLEDOWN,
    MOUSEEVENTF_MIDDLEUP, MOUSEEVENTF_MOVE, MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP,
    MOUSEEVENTF_WHEEL, MOUSEEVENTF_XDOWN, MOUSEEVENTF_XUP, MOUSEINPUT, VK_SPACE,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, KillTimer, SetTimer, SetWindowsHookExW, UnhookWindowsHookEx, KBDLLHOOKSTRUCT,
    MSLLHOOKSTRUCT, WH_KEYBOARD_LL, WH_MOUSE_LL, WM_KEYDOWN, WM_KEYUP, WM_LBUTTONDOWN,
    WM_LBUTTONUP, WM_MBUTTONDOWN, WM_MBUTTONUP, WM_MOUSEHWHEEL, WM_MOUSEMOVE, WM_MOUSEWHEEL,
    WM_RBUTTONDOWN, WM_RBUTTONUP, WM_SYSKEYDOWN, WM_SYSKEYUP, WM_XBUTTONDOWN, WM_XBUTTONUP,
    XBUTTON1, XBUTTON2,
};

use crate::config::Config;
use crate::gesture::{drag_scroll_refine, DragController};
use crate::remap::{
    ActiveModifiers, ClickCycleTracker, Effect, ModifiedDragType, ModifiedScrollModification,
    MouseButton, RemapEngine, SimpleRemapTable,
};
use crate::scroll::engine::WheelInput;
use crate::CONFIG;
use parking_lot::{Mutex, RwLock};
use std::sync::atomic::{AtomicBool, Ordering};

/// Sender to the injector thread. `None` when scroll is disabled or smooth is off.
/// Uses `SyncSender` so `try_send()` is available in the hook (never blocks).
static SCROLL_TX: Mutex<Option<std::sync::mpsc::SyncSender<WheelInput>>> = Mutex::new(None);

/// Low-level hook flag: the event was synthesized via `SendInput` (by us), so it
/// must not be re-smoothed (would cause a feedback loop).
const LLMHF_INJECTED: u32 = 0x01;

/// Extra-info marker we stamp onto every injected event. The hook procedure
/// checks this alongside `LLMHF_INJECTED` to robustly skip our own synthesis
/// (some Windows versions don't reliably set `LLMHF_INJECTED` for SendInput).
pub const OUR_MARKER: usize = 0xFA57_0000;

#[inline]
fn is_our_injected_event(flags: u32, extra_info: usize) -> bool {
    (flags & LLMHF_INJECTED) != 0 || extra_info == OUR_MARKER
}
/// Timer ID for ClickCycle level-expiration ticks.
pub(crate) const CLICK_TIMER_ID: usize = 3006;

/// Which top-level feature a tray-menu toggle acts on.
#[derive(Clone, Copy)]
pub enum Feature {
    SmoothScroll,
    ButtonRemap,
}

/// Built from the button-remap config; `None` disables remapping.
static REMAP_TABLE: RwLock<Option<SimpleRemapTable>> = RwLock::new(None);

/// Advanced remap engine with ClickCycle support.
static REMAP_ENGINE: RwLock<Option<RemapEngine>> = RwLock::new(None);

/// Click cycle tracker for multi-click / hold detection.
static CLICK_TRACKER: std::sync::OnceLock<RwLock<ClickCycleTracker>> = std::sync::OnceLock::new();

fn get_tracker() -> &'static RwLock<ClickCycleTracker> {
    CLICK_TRACKER.get_or_init(|| RwLock::new(ClickCycleTracker::new()))
}

/// Space key state, tracked by the keyboard hook for Space-drag gestures.
static SPACE_HELD: AtomicBool = AtomicBool::new(false);

/// Active window-drag gesture controller; `None` disables gestures.
static DRAG: RwLock<Option<DragController>> = RwLock::new(None);

/// Output mode for a button-drag gesture (selected by `cfg.drag.mode`).
/// Discriminants are stored in `DRAG_MODE` as a u8 — must be consecutive 0..4.
#[allow(dead_code)]
enum DragOutput {
    Move = 0,
    Scroll = 1,
    Navigate = 2,
    TaskView = 3,    // drag → Win+Tab (virtual desktop overview)
    ShowDesktop = 4, // drag → Win+D (minimize all / restore)
}

/// Map the config `drag.mode` string to a [`DragOutput`].
#[allow(dead_code)]
fn drag_output(mode: &str) -> DragOutput {
    match mode {
        "scroll" => DragOutput::Scroll,
        "navigate" => DragOutput::Navigate,
        "taskview" => DragOutput::TaskView,
        "showdesktop" => DragOutput::ShowDesktop,
        _ => DragOutput::Move,
    }
}

/// Atomic flags for hot-path config reads — avoids taking CONFIG.read() lock on every mouse event.
static SCROLL_ENABLED: AtomicBool = AtomicBool::new(false);
static BUTTONS_ENABLED: AtomicBool = AtomicBool::new(false);
static DRAG_ENABLED: AtomicBool = AtomicBool::new(false);
static WINDOW_SWITCHER_ENABLED: AtomicBool = AtomicBool::new(false);
/// Cached drag mode for the mousemove hot path — avoids CONFIG.read() on every move.
/// 0=Move, 1=Scroll, 2=Navigate, 3=TaskView, 4=ShowDesktop (matches DragOutput discriminant).
static DRAG_MODE: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);

/// Set while `TrackPopupMenu` is active on the tray. The hook skips all button
/// events when this is true so the menu receives clicks.
pub static MENU_ACTIVE: AtomicBool = AtomicBool::new(false);

/// Tracks active drag gesture state: which drag type is active and accumulated delta.
#[derive(Debug, Clone)]
pub struct DragState {
    pub drag_type: ModifiedDragType,
    /// The button that triggered this drag (needed for FakeDrag button-up).
    pub trigger_button: MouseButton,
    #[allow(dead_code)]
    pub origin_x: i32,
    #[allow(dead_code)]
    pub origin_y: i32,
}

/// Active drag gesture; `None` when no drag is in progress.
static DRAG_EFFECT: RwLock<Option<DragState>> = RwLock::new(None);
/// Currently active profile key (foreground exe basename), or `None` for base.
/// Lets the foreground-poll timer skip a reload when the app hasn't changed.
static CURRENT_PROFILE_KEY: parking_lot::Mutex<Option<String>> = parking_lot::Mutex::new(None);
fn hook_settings_equal(left: &Config, right: &Config) -> bool {
    left.scroll == right.scroll && left.buttons == right.buttons && left.drag == right.drag
}

fn reconcile_active_config(next: Config) -> Result<(), String> {
    let needs_hook_restart = {
        let current = CONFIG.read();
        if *current == next {
            return Ok(());
        }
        !hook_settings_equal(&current, &next)
    };
    if needs_hook_restart {
        apply_config(next)
    } else {
        *CONFIG.write() = next;
        Ok(())
    }
}

/// swap in the new config, then bring hooks back up. Safe to call repeatedly
/// (at startup, and on each tray-menu toggle).
pub fn apply_config(cfg: Config) -> Result<(), String> {
    uninstall();
    stop_scroll();
    *REMAP_TABLE.write() = None;

    *CONFIG.write() = cfg;
    let cfg = CONFIG.read();

    // Start the injector only when scroll is enabled AND smooth mode is enabled.
    // When smooth is off, wheel events pass through unmodified (with modifiers applied).
    if cfg.scroll.enabled && cfg.scroll.smooth {
        #[allow(clippy::explicit_auto_deref)]
        if let Some(tx) = crate::scroll::injector::start(&*cfg) {
            *SCROLL_TX.lock() = Some(tx);
        }
    }
    if cfg.buttons.enabled {
        let entries: Vec<(crate::remap::MouseButton, crate::remap::ButtonAction)> = cfg
            .buttons
            .remaps
            .iter()
            .filter_map(|e| {
                let source = e.source;
                let action = e.action.clone();
                let target = e.target;
                let vk = e.vk;
                match action.as_str() {
                    "button" => target.map(|b| (source, crate::remap::ButtonAction::Button(b))),
                    "key" => vk.map(|k| (source, crate::remap::ButtonAction::Key(k))),
                    _ => Some((source, crate::remap::ButtonAction::Disabled)),
                }
            })
            .collect();
        *REMAP_TABLE.write() = Some(crate::remap::SimpleRemapTable::from_entries(&entries));

        // Build the advanced remap engine from new-format RemapEntry entries.
        if !cfg.buttons.advanced.is_empty() {
            let engine = RemapEngine::new(cfg.buttons.advanced.clone());
            *REMAP_ENGINE.write() = Some(engine);
        } else {
            *REMAP_ENGINE.write() = None;
        }
    } else {
        *REMAP_ENGINE.write() = None;
    }
    if cfg.drag.enabled {
        *DRAG.write() = Some(DragController::new(crate::gesture::parse_button(
            &cfg.drag.button,
        )));
    } else {
        *DRAG.write() = None;
    }
    // Update atomic hot-path flags.
    SCROLL_ENABLED.store(cfg.scroll.enabled, Ordering::Relaxed);
    BUTTONS_ENABLED.store(cfg.buttons.enabled, Ordering::Relaxed);
    DRAG_ENABLED.store(cfg.drag.enabled, Ordering::Relaxed);
    WINDOW_SWITCHER_ENABLED.store(cfg.buttons.window_switcher, Ordering::Relaxed);
    // Cache drag mode as u8 for the lock-free mousemove hot path.
    let mode_u8 = match cfg.drag.mode.as_str() {
        "scroll" => 1u8,
        "navigate" => 2u8,
        "taskview" => 3u8,
        "showdesktop" => 4u8,
        _ => 0u8,
    };
    DRAG_MODE.store(mode_u8, Ordering::Relaxed);
    drop(cfg);

    if let Err(e) = install() {
        stop_scroll();
        return Err(e);
    }

    // Start the ClickCycle timer if button remapping is enabled.
    start_click_timer();
    Ok(())
}

/// Reload the base config from disk and re-apply it merged with the profile
/// that matches the current foreground window. Idempotent and re-entrant —
/// safe to call from the message-loop timer (same thread as `install`).
pub fn reload_active_config() -> Result<(), String> {
    let base = Config::load_or_default();
    let active = base.resolve_for_exe(crate::win::window::foreground_exe().as_deref());
    reconcile_active_config(active)
}

/// Called by the tray's profile-poll timer. If the foreground window's exe
/// changed since the last call, reload + re-apply the matching profile.
pub fn poll_foreground_profile() {
    // The common case has no per-app profiles. Avoid a process lookup, disk
    // parse, hook teardown, and injector restart every 500 ms in that case.
    if !CONFIG
        .read()
        .profiles
        .iter()
        .any(|p| p.match_type == "exe" && p.match_exe.is_some())
    {
        return;
    }

    let exe = crate::win::window::foreground_exe();
    let mut cur = CURRENT_PROFILE_KEY.lock();
    if *cur == exe {
        return;
    }
    *cur = exe.clone();
    drop(cur);

    let base = Config::load_or_default();
    let active = base.resolve_for_exe(exe.as_deref());
    if *CONFIG.read() != active {
        crate::log::write(&format!(
            "foreground window changed → {exe:?}; applying profile"
        ));
        if let Err(e) = reconcile_active_config(active) {
            crate::log::write(&format!("profile switch failed to install hooks: {e}"));
        }
    }
}

/// Drop the injector sender so its thread disconnects and exits.
pub fn stop_scroll() {
    *SCROLL_TX.lock() = None;
}

// ─── Phone-trackpad (Phase 11) remote input bridge ──────────────────────────
// Called by the WebSocket server in `src/remote.rs`. All funnel through the
// same `SendInput` path as the local hook layer, so feel is identical and the
// `LLMHF_INJECTED` guard prevents feedback loops.

/// Feed a remote scroll delta from the phone trackpad.
///
/// Touch input bypasses the smooth-scroll pipeline entirely — the smoother is
/// tuned for discrete 120-unit physical wheel ticks and produces floaty output
/// when fed continuous small touch deltas. Instead, we inject raw wheel events
/// directly via SendInput, which gives a responsive feel matching the phone's
/// own gain/acceleration applied in the JS layer.
pub fn push_remote_scroll(delta: i32, horizontal: bool) {
    unsafe { send_raw_wheel(delta, horizontal) };
}

unsafe fn send_raw_wheel(delta: i32, horizontal: bool) {
    let mut input = INPUT {
        r#type: INPUT_MOUSE,
        ..std::mem::zeroed()
    };
    input.Anonymous.mi = MOUSEINPUT {
        dx: 0,
        dy: 0,
        mouseData: delta as u32,
        dwFlags: if horizontal {
            MOUSEEVENTF_HWHEEL
        } else {
            MOUSEEVENTF_WHEEL
        },
        time: 0,
        dwExtraInfo: OUR_MARKER,
    };
    SendInput(1, &input, std::mem::size_of::<INPUT>() as i32);
}

/// Send a mouse click (tap) from the phone trackpad. `right` selects the button.
pub fn send_remote_click(right: bool) {
    unsafe {
        let (down, up) = if right {
            (MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP)
        } else {
            (MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP)
        };
        for flags in [down, up] {
            let mut input = INPUT {
                r#type: INPUT_MOUSE,
                ..std::mem::zeroed()
            };
            input.Anonymous.mi = MOUSEINPUT {
                dx: 0,
                dy: 0,
                mouseData: 0,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: OUR_MARKER,
            };
            SendInput(1, &input, std::mem::size_of::<INPUT>() as i32);
        }
    }
}

/// Inject a text string into the focused window via Unicode keyboard events.
/// Each character is sent as a KEYDOWN + KEYUP pair with `KEYEVENTF_UNICODE`.
/// Works with all input fields regardless of keyboard layout, and supports
/// CJK, emoji, and other Unicode characters from phone IME / voice input.
pub fn send_remote_text(text: &str) {
    unsafe {
        let mut events: Vec<INPUT> = Vec::with_capacity(text.len() * 2);
        for ch in text.chars() {
            let code = ch as u16;
            if code == 0 {
                continue;
            }
            let mut down = INPUT {
                r#type: INPUT_KEYBOARD,
                ..std::mem::zeroed()
            };
            down.Anonymous.ki = KEYBDINPUT {
                wVk: 0,
                wScan: code,
                dwFlags: KEYEVENTF_UNICODE,
                time: 0,
                dwExtraInfo: OUR_MARKER,
            };
            events.push(down);
            let mut up = INPUT {
                r#type: INPUT_KEYBOARD,
                ..std::mem::zeroed()
            };
            up.Anonymous.ki = KEYBDINPUT {
                wVk: 0,
                wScan: code,
                dwFlags: KEYEVENTF_UNICODE | KEYEVENTF_KEYUP,
                time: 0,
                dwExtraInfo: OUR_MARKER,
            };
            events.push(up);
        }
        if !events.is_empty() {
            SendInput(
                events.len() as u32,
                events.as_ptr(),
                std::mem::size_of::<INPUT>() as i32,
            );
        }
    }
}

/// Send a multi-finger navigation / window gesture from the phone trackpad.
///
/// 3-finger (navigation):
///   "taskview"    -> Win+Tab      (Task View)
///   "showdesktop" -> Win+D        (Show Desktop)
///   "desk_l"      -> Ctrl+Win+Left  (switch virtual desktop)
///   "desk_r"      -> Ctrl+Win+Right
/// 4-finger (window snapping):
///   "snap_l"      -> Win+Left
///   "snap_r"      -> Win+Right
///   "snap_up"     -> Win+Up       (maximize)
///   "snap_down"   -> Win+Down     (restore/minimize)
pub fn send_remote_gesture(g: &str) {
    // Long-press drag: simulate left button hold for window dragging
    match g {
        "longpress_drag" => {
            send_fake_drag_button_down(MouseButton::Left);
            return;
        }
        "longpress_end" => {
            send_fake_drag_button_up(MouseButton::Left);
            return;
        }
        // 4-finger diagonal corner snaps = two sequential Win+Arrow chords
        "snap_up_l" => {
            send_remote_gesture("snap_l");
            send_remote_gesture("snap_up");
            return;
        }
        "snap_up_r" => {
            send_remote_gesture("snap_r");
            send_remote_gesture("snap_up");
            return;
        }
        "snap_down_l" => {
            send_remote_gesture("snap_l");
            send_remote_gesture("snap_down");
            return;
        }
        "snap_down_r" => {
            send_remote_gesture("snap_r");
            send_remote_gesture("snap_down");
            return;
        }
        _ => {}
    }
    unsafe {
        let keys: &[u16] = match g {
            "taskview" => &[0x5B, 0x09],     // LWIN, TAB
            "showdesktop" => &[0x5B, 0x44],  // LWIN, D
            "desk_l" => &[0x11, 0x5B, 0x25], // CTRL, LWIN, LEFT
            "desk_r" => &[0x11, 0x5B, 0x27], // CTRL, LWIN, RIGHT
            "snap_l" => &[0x5B, 0x25],       // LWIN, LEFT
            "snap_r" => &[0x5B, 0x27],       // LWIN, RIGHT
            "snap_up" => &[0x5B, 0x26],      // LWIN, UP
            "snap_down" => &[0x5B, 0x28],    // LWIN, DOWN
            // 3-finger diagonals: top pair = Alt+Tab app switching,
            // bottom pair = move window between monitors (multi-monitor).
            "up_l" => &[0x10, 0x12, 0x09], // SHIFT, ALT, TAB -> previous app (Shift+Alt+Tab)
            "up_r" => &[0x12, 0x09],       // ALT, TAB        -> next app (Alt+Tab)
            "down_l" => &[0x5B, 0x10, 0x25], // LWIN, SHIFT, LEFT -> window to left monitor
            "down_r" => &[0x5B, 0x10, 0x27], // LWIN, SHIFT, RIGHT -> window to right monitor
            _ => return,
        };
        let mut events: Vec<INPUT> = Vec::with_capacity(keys.len() * 2);
        for &vk in keys {
            let mut input = INPUT {
                r#type: INPUT_KEYBOARD,
                ..std::mem::zeroed()
            };
            input.Anonymous.ki = KEYBDINPUT {
                wVk: vk,
                wScan: 0,
                dwFlags: 0,
                time: 0,
                dwExtraInfo: OUR_MARKER,
            };
            events.push(input);
        }
        for &vk in keys.iter().rev() {
            let mut input = INPUT {
                r#type: INPUT_KEYBOARD,
                ..std::mem::zeroed()
            };
            input.Anonymous.ki = KEYBDINPUT {
                wVk: vk,
                wScan: 0,
                dwFlags: KEYEVENTF_KEYUP,
                time: 0,
                dwExtraInfo: OUR_MARKER,
            };
            events.push(input);
        }
        SendInput(
            events.len() as u32,
            events.as_ptr(),
            std::mem::size_of::<INPUT>() as i32,
        );
    }
}

/// Send a pinch-zoom from the phone trackpad: Ctrl + mouse wheel.
/// `delta > 0` zooms in, `delta < 0` zooms out (one wheel notch per ~120).
pub fn send_remote_zoom(delta: i32) {
    unsafe {
        let mut events: Vec<INPUT> = Vec::with_capacity(3);
        // Ctrl down
        let mut ctrl_down = INPUT {
            r#type: INPUT_KEYBOARD,
            ..std::mem::zeroed()
        };
        ctrl_down.Anonymous.ki = KEYBDINPUT {
            wVk: 0x11, // VK_CONTROL
            wScan: 0,
            dwFlags: 0,
            time: 0,
            dwExtraInfo: OUR_MARKER,
        };
        events.push(ctrl_down);
        // wheel
        let mut wheel = INPUT {
            r#type: INPUT_MOUSE,
            ..std::mem::zeroed()
        };
        wheel.Anonymous.mi = MOUSEINPUT {
            dx: 0,
            dy: 0,
            mouseData: delta as u32,
            dwFlags: MOUSEEVENTF_WHEEL,
            time: 0,
            dwExtraInfo: OUR_MARKER,
        };
        events.push(wheel);
        // Ctrl up
        let mut ctrl_up = INPUT {
            r#type: INPUT_KEYBOARD,
            ..std::mem::zeroed()
        };
        ctrl_up.Anonymous.ki = KEYBDINPUT {
            wVk: 0x11,
            wScan: 0,
            dwFlags: KEYEVENTF_KEYUP,
            time: 0,
            dwExtraInfo: OUR_MARKER,
        };
        events.push(ctrl_up);
        SendInput(
            events.len() as u32,
            events.as_ptr(),
            std::mem::size_of::<INPUT>() as i32,
        );
    }
}

/// Persisted base on/off state shown by tray checkmarks. Per-app profiles may
/// still override the effective state while their application is foreground.
pub fn feature_enabled(f: Feature) -> bool {
    let base = Config::load_or_default();
    match f {
        Feature::SmoothScroll => base.scroll.enabled,
        Feature::ButtonRemap => base.buttons.enabled,
    }
}

/// Toggle a feature, persist to `config.toml`, and hot-reload.
pub fn toggle_feature(f: Feature) -> Result<(), String> {
    // Persist the base document, never the active profile overlay. Otherwise a
    // tray click while a profile is active would flatten its overrides into the
    // global config and silently change other applications.
    let mut base = Config::load_or_default();
    match f {
        Feature::SmoothScroll => base.scroll.enabled = !base.scroll.enabled,
        Feature::ButtonRemap => base.buttons.enabled = !base.buttons.enabled,
    }
    base.save()
        .map_err(|e| format!("无法保存 config.toml: {e}"))?;
    let active = base.resolve_for_exe(crate::win::window::foreground_exe().as_deref());
    apply_config(active)
}

/// Map a low-level mouse hook event to a (button, is_down) pair, if it is a
/// button event. For XBUTTONs the button id lives in the high word of `mouseData`.
fn button_event(ev: u32, ms: &MSLLHOOKSTRUCT) -> Option<(MouseButton, bool)> {
    match ev {
        WM_LBUTTONDOWN => Some((MouseButton::Left, true)),
        WM_LBUTTONUP => Some((MouseButton::Left, false)),
        WM_RBUTTONDOWN => Some((MouseButton::Right, true)),
        WM_RBUTTONUP => Some((MouseButton::Right, false)),
        WM_MBUTTONDOWN => Some((MouseButton::Middle, true)),
        WM_MBUTTONUP => Some((MouseButton::Middle, false)),
        WM_XBUTTONDOWN => {
            let xb = (ms.mouseData >> 16) as u16;
            Some((
                if xb == XBUTTON2 {
                    MouseButton::X2
                } else {
                    MouseButton::X1
                },
                true,
            ))
        }
        WM_XBUTTONUP => {
            let xb = (ms.mouseData >> 16) as u16;
            Some((
                if xb == XBUTTON2 {
                    MouseButton::X2
                } else {
                    MouseButton::X1
                },
                false,
            ))
        }
        _ => None,
    }
}

// Low-level hooks must be installed from a thread that runs a message loop
// (our main thread). We keep the handles only to uninstall on exit.
static mut MOUSE_HOOK: isize = 0;
static mut KEY_HOOK: isize = 0;

#[allow(static_mut_refs)]
pub fn install() -> Result<(), String> {
    unsafe {
        let hmod = GetModuleHandleW(std::ptr::null());

        let mh = SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_proc), hmod, 0);
        if mh == 0 {
            return Err("failed to install low-level mouse hook".into());
        }
        MOUSE_HOOK = mh;

        let kh = SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_proc), hmod, 0);
        if kh == 0 {
            UnhookWindowsHookEx(mh);
            MOUSE_HOOK = 0;
            return Err("failed to install low-level keyboard hook".into());
        }
        KEY_HOOK = kh;
    }

    let cfg = CONFIG.read();
    crate::log::write(&format!(
        "hooks installed (scroll.enabled={}, smooth={}, buttons.enabled={})",
        cfg.scroll.enabled, cfg.scroll.smooth, cfg.buttons.enabled
    ));
    Ok(())
}

#[allow(static_mut_refs)]
pub fn uninstall() {
    unsafe {
        if MOUSE_HOOK != 0 {
            UnhookWindowsHookEx(MOUSE_HOOK);
            MOUSE_HOOK = 0;
        }
        if KEY_HOOK != 0 {
            UnhookWindowsHookEx(KEY_HOOK);
            KEY_HOOK = 0;
        }
    }
    stop_click_timer();
}

/// Cursor pixels moved per wheel notch (WHEEL_DELTA = 120 units) in precision
/// mode. Small by design: a single notch is a fine, controllable nudge rather
/// than the 120px jump you'd get from passing the raw delta straight through.
const PRECISION_PIXELS_PER_NOTCH: f64 = 8.0;

/// Map a wheel delta to cursor pixels for precision (pointer-move) mode.
///
/// `horizontal` selects which axis the cursor moves on; the other axis is 0.
/// Pure function — easy to unit test without the hook globals.
fn precision_cursor_delta(delta: i32, horizontal: bool) -> (i32, i32) {
    let notches = delta as f64 / 120.0;
    let px = (notches * PRECISION_PIXELS_PER_NOTCH).round() as i32;
    if horizontal {
        (px, 0)
    } else {
        (0, px)
    }
}

#[inline]
fn enqueue_smooth_wheel(
    sender: Option<&std::sync::mpsc::SyncSender<WheelInput>>,
    input: WheelInput,
) -> bool {
    sender.is_some_and(|tx| tx.try_send(input).is_ok())
}

/// Process a raw wheel event: apply invert, modifiers, and either inject
/// into the smooth-scroll pipeline or pass through to the system unchanged.
unsafe fn process_wheel(delta: i32, horizontal: bool, cfg: &Config) -> Option<WheelInput> {
    let mut delta = delta;
    let mut horizontal = horizontal;

    // 1. Apply user invert preference.
    if cfg.scroll.invert {
        delta = -delta;
    }

    // 2. Apply modifier behaviors (Shift = accelerate / swap axis).
    let shift = crate::modifiers::shift_held();
    (delta, horizontal) = crate::modifiers::apply_scroll_modifiers(
        delta,
        horizontal,
        shift,
        cfg.scroll.shift_speedup,
        cfg.scroll.shift_horizontal,
    );

    // 3. Apply ScrollTrigger remap effects (e.g., ModifiedScrollModification).
    //    Check the advanced engine first, then fall back to legacy config.
    let modifier_buttons = get_tracker().read().held_modifier_buttons();
    let active_mods = ActiveModifiers {
        keyboard: crate::modifiers::state() as u32,
        buttons: modifier_buttons,
    };
    // AddMode: capture scroll trigger and pass through unmodified.
    if crate::add_mode::on_scroll_event(&active_mods) {
        return None; // pass through
    }

    // Set when a ModifiedScroll::Precision effect matches. Handled after the loop
    // so it can never fall through into the smooth-inject path (which would scroll
    // *and* move the cursor — a double fire).
    let mut precision_mode = false;
    if let Some(engine) = REMAP_ENGINE.read().as_ref() {
        let effects = engine.resolve_scroll_effects(&active_mods);
        for effect in &effects {
            if let Effect::ModifiedScroll { modification } = effect {
                match modification {
                    ModifiedScrollModification::Precision => {
                        // Precision mode: wheel becomes pointer movement instead of
                        // scrolling. Flag it and handle after the loop (see below) so
                        // the code cannot accidentally fall through into the smooth
                        // inject path, which would scroll *and* move — a double fire.
                        precision_mode = true;
                    }
                    ModifiedScrollModification::Horizontal => {
                        // Force horizontal scroll.
                        horizontal = true;
                    }
                    ModifiedScrollModification::Fast => {
                        // Apply extra acceleration factor.
                        delta = (delta as f64 * 2.0) as i32;
                    }
                    ModifiedScrollModification::Zoom => {
                        // Zoom: handled by the injector if supported.
                    }
                    _ => {
                        // Other scroll modifications not yet implemented in this path.
                    }
                }
            }
        }
    }

    // 3b. Precision mode: swallow the wheel and move the cursor only. This must
    //     run *before* step 4 so the original scroll is never injected into the
    //     smooth pipeline (which would scroll *and* move — double fire). One wheel
    //     notch moves the cursor a small, controllable number of pixels.
    if precision_mode {
        let (dx, dy) = precision_cursor_delta(delta, horizontal);
        crate::scroll::injector::send_mouse_move(dx, dy);
        return Some(WheelInput { delta, horizontal }); // swallowed: cursor moved, no scroll
    }

    // 4. If smooth mode is enabled, enqueue without blocking the hook. Only
    // swallow the physical event after the worker accepted it; if the worker
    // failed, disconnected, or is overloaded, raw scrolling remains usable.
    if cfg.scroll.smooth {
        let input = WheelInput { delta, horizontal };
        if enqueue_smooth_wheel(SCROLL_TX.lock().as_ref(), input) {
            return Some(input);
        }
    }
    None
}

unsafe extern "system" fn mouse_proc(code: i32, wparam: usize, lparam: isize) -> isize {
    if code >= 0 {
        let ev = wparam as u32;
        let ms = &*(lparam as *const MSLLHOOKSTRUCT);
        // Only bypass events Windows marks as injected or events carrying our
        // marker. Unmarked events are indistinguishable from physical input;
        // classifying them by button type would disable legitimate remaps and
        // drag gestures for that button.
        if is_our_injected_event(ms.flags, ms.dwExtraInfo) {
            return CallNextHookEx(0, code, wparam, lparam);
        }
        // Bypass unmarked LMB DOWN injected by external tools (e.g. WeChat screenshot).
        // WeChat injects LMB DOWN at the anchor point without any flags or marker.
        // This must pass through untouched so the native drag-select works.
        if ev == WM_LBUTTONDOWN && ms.flags == 0 && ms.dwExtraInfo == 0 {
            return CallNextHookEx(0, code, wparam, lparam);
        }

        // ── Wheel events ──────────────────────────────────────────────────────────
        if ev == WM_MOUSEWHEEL || ev == WM_MOUSEHWHEEL {
            if crate::add_mode::is_active() {
                let active_mods = ActiveModifiers {
                    keyboard: crate::modifiers::state() as u32,
                    buttons: get_tracker().read().held_modifier_buttons(),
                };
                let _ = crate::add_mode::on_scroll_event(&active_mods);
                return CallNextHookEx(0, code, wparam, lparam);
            }
            // Window-switcher gesture: wheel navigates the Alt+Tab list while held.
            // step() may detect that the OS dismissed the switcher (e.g. user
            // clicked away to a different window) — in that case it resets
            // mode and we fall through to the normal scroll path so the
            if WINDOW_SWITCHER_ENABLED.load(Ordering::Relaxed)
                && crate::win::window_switcher::is_active()
            {
                let raw_delta = (ms.mouseData >> 16) as i16 as i32;
                let hwheel = ev == WM_MOUSEHWHEEL;
                // Direction mapping:
                //   WM_MOUSEHWHEEL: wheel right (delta > 0) -> forward/next (Tab)
                //   WM_MOUSEWHEEL:  wheel down  (delta > 0) -> backward/prev.
                //                   Vertical wheel maps to the horizontal axis of
                //                   Alt+Tab (UI is laid out horizontally); user expects
                //                   wheel down to move selection to the right.
                let forward = if hwheel { raw_delta > 0 } else { raw_delta < 0 };
                if crate::win::window_switcher::step(forward) {
                    return 1; // swallowed: drove the switcher
                }
            }
            if SCROLL_ENABLED.load(Ordering::Relaxed) {
                let raw_delta = (ms.mouseData >> 16) as i16 as i32;
                let cfg = CONFIG.read();
                let horizontal = ev == WM_MOUSEHWHEEL;
                let injected = process_wheel(raw_delta, horizontal, &cfg);
                if injected.is_some() {
                    return 1; // swallowed: injector will replay it smoothly
                }
                // smooth is off: fall through to pass raw event to system
            }
        }
        // ── Mousemove ────────────────────────────────────────────────────────────
        if ev == WM_MOUSEMOVE {
            // Window-drag gesture: behaviour depends on DRAG_MODE atomic.
            if DRAG_ENABLED.load(Ordering::Relaxed) {
                match DRAG_MODE.load(Ordering::Relaxed) {
                    0u8 => {
                        // DragOutput::Move
                        if let Some(ctrl) = DRAG.read().as_ref() {
                            if let Some((x, y)) = ctrl.target_pos(ms.pt.x, ms.pt.y) {
                                crate::win::window::move_window(ctrl.hwnd(), x, y);
                            }
                        }
                    }
                    1u8 => {
                        // DragOutput::Scroll
                        let mut drag = DRAG.write();
                        if let Some(ctrl) = drag.as_mut() {
                            let (dx, dy) = ctrl.consume_delta(ms.pt.x, ms.pt.y);
                            if let Some(tx) = SCROLL_TX.lock().as_ref() {
                                let refine = drag_scroll_refine(
                                    crate::modifiers::shift_held(),
                                    (crate::modifiers::state() & crate::modifiers::CTRL) != 0,
                                );
                                let scale = if refine.precision { 0.5_f64 } else { 1.0 };
                                if refine.horizontal_only {
                                    let h = (-(dx as f64 + dy as f64) * scale).round() as i32;
                                    if h != 0 {
                                        let _ = tx.try_send(WheelInput {
                                            delta: h,
                                            horizontal: true,
                                        });
                                    }
                                } else {
                                    let v = (-(dy as f64) * scale).round() as i32;
                                    let h = (-(dx as f64) * scale).round() as i32;
                                    if v != 0 {
                                        let _ = tx.try_send(WheelInput {
                                            delta: v,
                                            horizontal: false,
                                        });
                                    }
                                    if h != 0 {
                                        let _ = tx.try_send(WheelInput {
                                            delta: h,
                                            horizontal: true,
                                        });
                                    }
                                }
                            }
                        }
                    }
                    2u8 => {
                        // DragOutput::Navigate
                        let mut drag = DRAG.write();
                        if let Some(ctrl) = drag.as_mut() {
                            ctrl.consume_delta(ms.pt.x, ms.pt.y);
                            let (dx, dy) = ctrl.total_delta();
                            const NAV_THRESH: i32 = 60;
                            if dx.abs() > dy.abs() && dx.abs() > NAV_THRESH {
                                let direction = if dx > 0 {
                                    crate::remap::SwipeDirection::Forward
                                } else {
                                    crate::remap::SwipeDirection::Back
                                };
                                crate::remap::execute_effect(
                                    &crate::remap::Effect::NavigationSwipe { direction },
                                );
                                ctrl.reset_accum();
                            }
                        }
                    }
                    3u8 => {
                        // DragOutput::TaskView
                        let mut drag = DRAG.write();
                        if let Some(ctrl) = drag.as_mut() {
                            ctrl.consume_delta(ms.pt.x, ms.pt.y);
                            let (_dx, dy) = ctrl.total_delta();
                            const TASKVIEW_THRESH: i32 = 80;
                            if dy < -TASKVIEW_THRESH {
                                crate::remap::execute_effect(&crate::remap::Effect::TaskView);
                                ctrl.reset_accum();
                            }
                        }
                    }
                    4u8 => {
                        // DragOutput::ShowDesktop
                        let mut drag = DRAG.write();
                        if let Some(ctrl) = drag.as_mut() {
                            ctrl.consume_delta(ms.pt.x, ms.pt.y);
                            let (_dx, dy) = ctrl.total_delta();
                            const SHOWDESKTOP_THRESH: i32 = 80;
                            if dy > SHOWDESKTOP_THRESH {
                                crate::remap::execute_effect(&crate::remap::Effect::ShowDesktop);
                                ctrl.reset_accum();
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
        if let Some((btn, down)) = button_event(ev, ms) {
            // Skip all button processing when the tray popup menu is active so
            // the menu receives clicks.
            if MENU_ACTIVE.load(Ordering::Relaxed) {
                return CallNextHookEx(0, code, wparam, lparam);
            }
            // Recording works even when remapping and scrolling are disabled.
            // Keep the physical event untouched and bypass all configured effects
            // until the tray finishes the capture dialog.
            if crate::add_mode::is_active() {
                if down {
                    let active_mods = ActiveModifiers {
                        keyboard: crate::modifiers::state() as u32,
                        buttons: get_tracker().read().held_modifier_buttons(),
                    };
                    let _ = crate::add_mode::on_button_event(btn, 1, false, &active_mods);
                }
                return CallNextHookEx(0, code, wparam, lparam);
            }
            // Never intercept right-click: the system tray needs it for the context
            // menu. Right-click remapping is not a common use case.
            if btn == MouseButton::Right {
                return CallNextHookEx(0, code, wparam, lparam);
            }
            let window_switcher_on = WINDOW_SWITCHER_ENABLED.load(Ordering::Relaxed);
            let buttons_on = BUTTONS_ENABLED.load(Ordering::Relaxed);
            // Middle-button window-switcher gesture takes priority over normal remap.
            // A single middle click is preserved via delayed replay (see window_switcher).
            if window_switcher_on && btn == MouseButton::Middle {
                if down {
                    if crate::win::window_switcher::on_middle_down() {
                        return 1;
                    }
                } else if crate::win::window_switcher::swallow_middle_up() {
                    return 1;
                }
            }
            // Button remapping with ClickCycle support.
            if buttons_on {
                let kb_state = crate::modifiers::state();
                let held_btns = get_tracker().read().held_modifier_buttons();
                let active_mods = ActiveModifiers {
                    keyboard: kb_state as u32,
                    buttons: held_btns,
                };

                if down {
                    let mut tracker = get_tracker().write();
                    let (click_count, is_new) = tracker.on_button_down(btn);
                    // Single read of REMAP_ENGINE for the entire button-down path.
                    let engine_guard = REMAP_ENGINE.read();
                    let engine = engine_guard.as_ref();
                    if is_new {
                        if let Some(e) = engine {
                            let max_level = e.max_level_for_button(btn);
                            tracker.set_max_level(btn, max_level);
                        }
                    }
                    let results = engine
                        .map(|e| e.resolve_effects(btn, click_count, false, &active_mods))
                        .unwrap_or_default();
                    if !results.is_empty() {
                        // Activate drag state if this is a drag trigger.
                        for (effect, _) in &results {
                            if let Effect::ModifiedDrag { drag_type, .. } = effect {
                                let origin = crate::win::window::cursor_pos();
                                *DRAG_EFFECT.write() = Some(DragState {
                                    drag_type: *drag_type,
                                    trigger_button: btn,
                                    origin_x: origin.0,
                                    origin_y: origin.1,
                                });
                                if *drag_type == ModifiedDragType::FakeDrag {
                                    send_fake_drag_button_down(btn);
                                }
                            }
                        }
                        return 1; // swallow; effect fires on up or timer tick
                    } else if let Some(table) = REMAP_TABLE.read().as_ref() {
                        if let Some(action) = table.lookup(btn) {
                            crate::remap::execute(action, down);
                            return 1;
                        }
                    }
                } else {
                    // Button up: end click cycle and fire effects.
                    // FakeDrag: send button-up for the trigger button and swallow.
                    let fake_drag_trigger = DRAG_EFFECT.read().as_ref().and_then(|drag| {
                        (drag.drag_type == ModifiedDragType::FakeDrag && drag.trigger_button == btn)
                            .then_some(drag.trigger_button)
                    });
                    if let Some(trigger) = fake_drag_trigger {
                        send_fake_drag_button_up(trigger);
                        *DRAG_EFFECT.write() = None;
                        return 1; // swallowed: completed the synthetic drag
                    }
                    *DRAG_EFFECT.write() = None;
                    let mut tracker = get_tracker().write();
                    let mut engine_guard = REMAP_ENGINE.write();
                    let engine = engine_guard.as_mut();
                    let effects = tracker.on_button_up(btn, engine, &active_mods);
                    for (effect, phase) in &effects {
                        crate::remap::execute_effect_phase(effect, *phase);
                    }
                    if effects.is_empty() {
                        if let Some(table) = REMAP_TABLE.read().as_ref() {
                            if let Some(action) = table.lookup(btn) {
                                crate::remap::execute(action, down);
                                return 1;
                            }
                        }
                    }
                }
            }

            // Window-drag takes precedence over remap for its trigger button.
            // NOTE: do NOT swallow on button-down — that would eat ordinary clicks.
            // The drag activates lazily on mousemove once movement is detected.
            {
                let mut drag = DRAG.write();
                if let Some(ctrl) = drag.as_mut() {
                    if down
                        && ctrl.matches_trigger(btn, down, SPACE_HELD.load(Ordering::Relaxed))
                        && !ctrl.is_active()
                    {
                        if let Some(hwnd) = crate::win::window::window_at_cursor(&ms.pt) {
                            if let Some(rect) = crate::win::window::get_window_rect(hwnd) {
                                // Store hwnd + offset; swallowing happens in mousemove.
                                ctrl.begin(
                                    hwnd,
                                    ms.pt.x - rect.left,
                                    ms.pt.y - rect.top,
                                    ms.pt.x,
                                    ms.pt.y,
                                );
                            }
                        }
                    } else if !down && ctrl.is_active() && ctrl.matches_release(btn) {
                        ctrl.end();
                        // Do NOT swallow the UP: the foreground app must see the
                        // button release to reset its internal "button held"
                        // state, otherwise subsequent left-clicks silently
                        // fail because Windows still thinks the button is
                        // pressed. Drag-to-move operates entirely via injected
                        // mouse moves and does not need to capture UP.
                    }
                }
            }
        }
    }

    CallNextHookEx(0, code, wparam, lparam)
}

unsafe extern "system" fn keyboard_proc(code: i32, wparam: usize, lparam: isize) -> isize {
    if code >= 0 {
        let ev = wparam as u32;
        if ev == WM_KEYDOWN || ev == WM_SYSKEYDOWN || ev == WM_KEYUP || ev == WM_SYSKEYUP {
            let ks = &*(lparam as *const KBDLLHOOKSTRUCT);
            let down = ev == WM_KEYDOWN || ev == WM_SYSKEYDOWN;
            crate::modifiers::set_vk(ks.vkCode, down);
            if ks.vkCode == VK_SPACE as u32 {
                SPACE_HELD.store(down, Ordering::Relaxed);
            }
        }
    }
    CallNextHookEx(0, code, wparam, lparam)
}

// ─── ClickCycle timer ────────────────────────────────────────────────────────

/// Starts a Windows timer that fires every 20ms to drive ClickCycle level expiration.
/// The timer posts WM_TIMER which is handled inside `mouse_proc`.
fn start_click_timer() {
    let cfg = CONFIG.read();
    if !cfg.buttons.enabled && !cfg.buttons.window_switcher {
        return;
    }
    drop(cfg);
    // Attach the tick timer to the tray's message-only window so WM_TIMER is
    // actually dispatched. A NULL-hwnd timer would post WM_TIMER to the thread
    // queue, but a low-level mouse hook callback never receives WM_TIMER.
    unsafe {
        SetTimer(crate::win::tray::hwnd(), CLICK_TIMER_ID, 20, None);
    }
}

/// Stops the ClickCycle timer.
fn stop_click_timer() {
    unsafe {
        KillTimer(crate::win::tray::hwnd(), CLICK_TIMER_ID);
    }
}

/// Called on each WM_TIMER message to advance ClickCycle hold detection.
/// Runs tick() on the tracker and fires any hold effects that have expired.
/// Invoked from the tray window's `wnd_proc` (the LL mouse hook never sees WM_TIMER).
pub(crate) fn run_click_tick() {
    // Poll middle-button via GetAsyncKeyState (for mice whose driver intercepts
    // middle-click below the LL hook). Guard so one crash never cascades.
    if std::panic::catch_unwind(std::panic::AssertUnwindSafe(
        crate::win::window_switcher::poll_middle,
    ))
    .is_err()
    {
        crate::log::write("run_click_tick: poll_middle panicked");
    }
    // Guard tick() so Alt-stuck state is never caused by a panic in the timer path.
    if std::panic::catch_unwind(std::panic::AssertUnwindSafe(
        crate::win::window_switcher::tick,
    ))
    .is_err()
    {
        crate::log::write("run_click_tick: window_switcher::tick panicked");
    }

    let tracker_lock = get_tracker();
    let mut tracker = tracker_lock.write();
    if tracker.active_count() == 0 {
        return;
    }

    let modifier_buttons = tracker.held_modifier_buttons();
    let active_mods = ActiveModifiers {
        keyboard: crate::modifiers::state() as u32,
        buttons: modifier_buttons,
    };

    let mut engine_guard = REMAP_ENGINE.write();
    if let Some(engine) = engine_guard.as_mut() {
        tracker.tick(engine, &active_mods);
    }
}

// ─── FakeDrag helpers ─────────────────────────────────────────────────────────

#[allow(dead_code)]
/// Send a mouse move event while a button is held (for FakeDrag).
/// The button was already sent as held-down when the drag started.
fn send_fake_drag_move(btn: MouseButton, dx: i32, dy: i32) {
    let flags = MOUSEEVENTF_MOVE;
    let data = match btn {
        MouseButton::X1 => XBUTTON1 as u32,
        MouseButton::X2 => XBUTTON2 as u32,
        _ => 0u32,
    };
    unsafe {
        let mut input = INPUT {
            r#type: INPUT_MOUSE,
            ..std::mem::zeroed()
        };
        input.Anonymous.mi = MOUSEINPUT {
            dx,
            dy,
            mouseData: data,
            dwFlags: flags,
            time: 0,
            dwExtraInfo: OUR_MARKER,
        };
        SendInput(1, &input, std::mem::size_of::<INPUT>() as i32);
    }
}

/// Send a button-up event to conclude a FakeDrag.
fn send_fake_drag_button_up(btn: MouseButton) {
    let (flags, data) = match btn {
        MouseButton::Left => (MOUSEEVENTF_LEFTUP, 0u32),
        MouseButton::Right => (MOUSEEVENTF_RIGHTUP, 0u32),
        MouseButton::Middle => (MOUSEEVENTF_MIDDLEUP, 0u32),
        MouseButton::X1 => (MOUSEEVENTF_XUP, XBUTTON1 as u32),
        MouseButton::X2 => (MOUSEEVENTF_XUP, XBUTTON2 as u32),
    };
    unsafe {
        let mut input = INPUT {
            r#type: INPUT_MOUSE,
            ..std::mem::zeroed()
        };
        input.Anonymous.mi = MOUSEINPUT {
            dx: 0,
            dy: 0,
            mouseData: data,
            dwFlags: flags,
            time: 0,
            dwExtraInfo: OUR_MARKER,
        };
        SendInput(1, &input, std::mem::size_of::<INPUT>() as i32);
    }
}

/// Send a button-down event to start a FakeDrag (so the system sees a complete click).
fn send_fake_drag_button_down(btn: MouseButton) {
    let (flags, data) = match btn {
        MouseButton::Left => (MOUSEEVENTF_LEFTDOWN, 0u32),
        MouseButton::Right => (MOUSEEVENTF_RIGHTDOWN, 0u32),
        MouseButton::Middle => (MOUSEEVENTF_MIDDLEDOWN, 0u32),
        MouseButton::X1 => (MOUSEEVENTF_XDOWN, XBUTTON1 as u32),
        MouseButton::X2 => (MOUSEEVENTF_XDOWN, XBUTTON2 as u32),
    };
    unsafe {
        let mut input = INPUT {
            r#type: INPUT_MOUSE,
            ..std::mem::zeroed()
        };
        input.Anonymous.mi = MOUSEINPUT {
            dx: 0,
            dy: 0,
            mouseData: data,
            dwFlags: flags,
            time: 0,
            dwExtraInfo: OUR_MARKER,
        };
        SendInput(1, &input, std::mem::size_of::<INPUT>() as i32);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn precision_cursor_delta_vertical_notch() {
        assert_eq!(precision_cursor_delta(120, false), (0, 8));
        assert_eq!(precision_cursor_delta(-120, false), (0, -8));
    }

    #[test]
    fn precision_cursor_delta_horizontal_notch() {
        assert_eq!(precision_cursor_delta(120, true), (8, 0));
        assert_eq!(precision_cursor_delta(240, true), (16, 0));
    }

    #[test]
    fn precision_cursor_delta_zero_is_noop() {
        assert_eq!(precision_cursor_delta(0, false), (0, 0));
        assert_eq!(precision_cursor_delta(0, true), (0, 0));
    }

    #[test]
    fn precision_cursor_delta_half_notch_rounds() {
        assert_eq!(precision_cursor_delta(60, false), (0, 4));
    }

    #[test]
    fn injection_filter_keeps_unmarked_physical_events() {
        assert!(!is_our_injected_event(0, 0));
        assert!(is_our_injected_event(LLMHF_INJECTED, 0));
        assert!(is_our_injected_event(0, OUR_MARKER));
    }

    #[test]
    fn hook_restart_filter_ignores_remote_only_changes() {
        let base = Config::default();
        let mut remote_change = base.clone();
        remote_change.remote.enabled = !remote_change.remote.enabled;
        remote_change.touch.gain += 1.0;
        assert!(hook_settings_equal(&base, &remote_change));

        let mut scroll_change = base.clone();
        scroll_change.scroll.speed += 0.5;
        assert!(!hook_settings_equal(&base, &scroll_change));
    }

    #[test]
    fn smooth_scroll_only_swallows_an_accepted_event() {
        let input = WheelInput {
            delta: 120,
            horizontal: false,
        };
        assert!(!enqueue_smooth_wheel(None, input));

        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        assert!(enqueue_smooth_wheel(Some(&tx), input));
        assert!(!enqueue_smooth_wheel(Some(&tx), input));
        assert_eq!(rx.recv().unwrap().delta, 120);
        drop(rx);
        assert!(!enqueue_smooth_wheel(Some(&tx), input));
    }
}
