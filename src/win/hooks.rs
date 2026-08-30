use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, DispatchMessageW, GetMessageW, KillTimer, MSLLHOOKSTRUCT, MSG, SetTimer,
    SetWindowsHookExW, TranslateMessage, UnhookWindowsHookEx, WH_KEYBOARD_LL, WH_MOUSE_LL,
    WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE, WM_MOUSEWHEEL, WM_MOUSEHWHEEL, WM_RBUTTONDOWN,
    WM_RBUTTONUP, WM_MBUTTONDOWN, WM_MBUTTONUP, WM_XBUTTONDOWN, WM_XBUTTONUP, XBUTTON1,
    XBUTTON2, KBDLLHOOKSTRUCT, WM_KEYDOWN, WM_KEYUP, WM_SYSKEYDOWN, WM_SYSKEYUP,
};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    INPUT, INPUT_KEYBOARD, INPUT_MOUSE, KEYBDINPUT, KEYEVENTF_KEYUP, MOUSEEVENTF_HWHEEL,
    MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP, MOUSEEVENTF_MIDDLEDOWN, MOUSEEVENTF_MIDDLEUP,
    MOUSEEVENTF_MOVE, MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP, MOUSEEVENTF_WHEEL,
    MOUSEEVENTF_XDOWN, MOUSEEVENTF_XUP, MOUSEINPUT, SendInput,
};

use crate::CONFIG;
use crate::config::Config;
use crate::gesture::{drag_scroll_refine, DragController};
use crate::accel::PointerAccel;
use std::sync::atomic::{AtomicBool, Ordering};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::VK_SPACE;
use std::sync::mpsc::Sender;
use parking_lot::{Mutex, RwLock};
use crate::scroll::engine::WheelInput;
use crate::remap::{MouseButton, SimpleRemapTable, ButtonAction, RemapEngine, ClickCycleTracker, ActiveModifiers, Trigger, Effect, ModifiedScrollModification, ModifiedDragType};

/// Sender to the injector thread. `None` when scroll is disabled or smooth is off.
/// Wrapped in a `Mutex` for shared access from the hook procedure.
static SCROLL_TX: Mutex<Option<Sender<WheelInput>>> = Mutex::new(None);

/// Low-level hook flag: the event was synthesized via `SendInput` (by us), so it
/// must not be re-smoothed (would cause a feedback loop).
const LLMHF_INJECTED: u32 = 0x01;

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
enum DragOutput {
    Move,
    Scroll,
    Navigate,
}

/// Map the config `drag.mode` string to a [`DragOutput`].
fn drag_output(mode: &str) -> DragOutput {
    match mode {
        "scroll" => DragOutput::Scroll,
        "navigate" => DragOutput::Navigate,
        _ => DragOutput::Move,
    }
}

/// Pointer-acceleration controller; `None` disables acceleration.
static ACCEL: RwLock<Option<PointerAccel>> = RwLock::new(None);

/// Tracks active drag gesture state: which drag type is active and accumulated delta.
#[derive(Debug, Clone)]
pub struct DragState {
    pub drag_type: ModifiedDragType,
    /// The button that triggered this drag (needed for FakeDrag button-up).
    pub trigger_button: MouseButton,
    pub origin_x: i32,
    pub origin_y: i32,
}

/// Active drag gesture; `None` when no drag is in progress.
static DRAG_EFFECT: RwLock<Option<DragState>> = RwLock::new(None);
/// Currently active profile key (foreground exe basename), or `None` for base.
/// Lets the foreground-poll timer skip a reload when the app hasn't changed.
static CURRENT_PROFILE_KEY: parking_lot::Mutex<Option<String>> = parking_lot::Mutex::new(None);
/// swap in the new config, then bring hooks back up. Safe to call repeatedly
/// (at startup, and on each tray-menu toggle).
pub fn apply_config(cfg: Config) {
    uninstall();
    stop_scroll();
    *REMAP_TABLE.write() = None;

    *CONFIG.write() = cfg;
    let cfg = CONFIG.read();

    // Start the injector only when scroll is enabled AND smooth mode is enabled.
    // When smooth is off, wheel events pass through unmodified (with modifiers applied).
    if cfg.scroll.enabled && cfg.scroll.smooth {
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
    if cfg.accel.enabled {
        *ACCEL.write() = Some(PointerAccel::new());
    } else {
        *ACCEL.write() = None;
    }
    drop(cfg);

    let _ = install();

    // Start the ClickCycle timer if button remapping is enabled.
    start_click_timer();
}

/// Reload the base config from disk and re-apply it merged with the profile
/// that matches the current foreground window. Idempotent and re-entrant —
/// safe to call from the message-loop timer (same thread as `install`).
pub fn reload_active_config() {
    let base = Config::load_or_default();
    let active = base.resolve_for_exe(crate::win::window::foreground_exe().as_deref());
    apply_config(active);
}

/// Called by the tray's profile-poll timer. If the foreground window's exe
/// changed since the last call, reload + re-apply the matching profile.
pub fn poll_foreground_profile() {
    let exe = crate::win::window::foreground_exe();
    let mut cur = CURRENT_PROFILE_KEY.lock();
    if *cur != exe {
        *cur = exe.clone();
        drop(cur);
        crate::log::write(&format!("foreground window changed → {exe:?}; reloading profile"));
        reload_active_config();
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

/// Feed a remote scroll delta into the smooth-scroll pipeline. If the smoother
/// is disabled (`SCROLL_TX` is `None`), fall back to a raw wheel injection so
/// the phone trackpad still scrolls.
pub fn push_remote_scroll(delta: i32, horizontal: bool) {
    let tx = SCROLL_TX.lock();
    if let Some(tx) = tx.as_ref() {
        let _ = tx.send(WheelInput { delta, horizontal });
    } else {
        drop(tx);
        unsafe { send_raw_wheel(delta, horizontal) };
    }
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
        dwExtraInfo: 0,
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
                dwExtraInfo: 0,
            };
            SendInput(1, &input, std::mem::size_of::<INPUT>() as i32);
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
    // 4-finger diagonal corner snaps = two sequential Win+Arrow chords
    // (e.g. up-left = Win+Left then Win+Up -> quarter window at top-left).
    match g {
        "snap_up_l"   => { send_remote_gesture("snap_l"); send_remote_gesture("snap_up"); return; }
        "snap_up_r"   => { send_remote_gesture("snap_r"); send_remote_gesture("snap_up"); return; }
        "snap_down_l" => { send_remote_gesture("snap_l"); send_remote_gesture("snap_down"); return; }
        "snap_down_r" => { send_remote_gesture("snap_r"); send_remote_gesture("snap_down"); return; }
        _ => {}
    }
    unsafe {
        let keys: &[u16] = match g {
            "taskview" => &[0x5B, 0x09], // LWIN, TAB
            "showdesktop" => &[0x5B, 0x44], // LWIN, D
            "desk_l" => &[0x11, 0x5B, 0x25], // CTRL, LWIN, LEFT
            "desk_r" => &[0x11, 0x5B, 0x27], // CTRL, LWIN, RIGHT
            "snap_l" => &[0x5B, 0x25],      // LWIN, LEFT
            "snap_r" => &[0x5B, 0x27],      // LWIN, RIGHT
            "snap_up" => &[0x5B, 0x26],     // LWIN, UP
            "snap_down" => &[0x5B, 0x28],   // LWIN, DOWN
            // 3-finger diagonals: top pair = Alt+Tab app switching,
            // bottom pair = move window between monitors (multi-monitor).
            "up_l" => &[0x10, 0x12, 0x09],  // SHIFT, ALT, TAB -> previous app (Shift+Alt+Tab)
            "up_r" => &[0x12, 0x09],        // ALT, TAB        -> next app (Alt+Tab)
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
                dwExtraInfo: 0,
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
                dwExtraInfo: 0,
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
            dwExtraInfo: 0,
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
            dwExtraInfo: 0,
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
            dwExtraInfo: 0,
        };
        events.push(ctrl_up);
        SendInput(
            events.len() as u32,
            events.as_ptr(),
            std::mem::size_of::<INPUT>() as i32,
        );
    }
}

/// Current on/off state of a feature (for the tray checkmarks).
pub fn feature_enabled(f: Feature) -> bool {
    match f {
        Feature::SmoothScroll => CONFIG.read().scroll.enabled,
        Feature::ButtonRemap => CONFIG.read().buttons.enabled,
    }
}

/// Toggle a feature, persist to `config.toml`, and hot-reload.
pub fn toggle_feature(f: Feature) {
    // Base the toggle on the currently-loaded config rather than re-reading from
    // disk, so a transient read error can never overwrite the user's file with a
    // fallback default (which would silently drop settings like `window_switcher`).
    let mut cfg = CONFIG.read().clone();
    match f {
        Feature::SmoothScroll => cfg.scroll.enabled = !cfg.scroll.enabled,
        Feature::ButtonRemap => cfg.buttons.enabled = !cfg.buttons.enabled,
    }
    let _ = cfg.save();
    apply_config(cfg);
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
            Some((if xb == XBUTTON2 { MouseButton::X2 } else { MouseButton::X1 }, true))
        }
        WM_XBUTTONUP => {
            let xb = (ms.mouseData >> 16) as u16;
            Some((if xb == XBUTTON2 { MouseButton::X2 } else { MouseButton::X1 }, false))
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

    // 4. If smooth mode is enabled, inject into the injector pipeline.
    //    The hook swallows the original event; the injector replays it smoothly.
    //    If smooth mode is off, return None so the original event passes through.
    if cfg.scroll.smooth {
        crate::log::write(&format!("[wheel] process_wheel smooth=true delta={} horiz={}", delta, horizontal));
        if let Some(tx) = SCROLL_TX.lock().as_ref() {
            let _ = tx.send(WheelInput { delta, horizontal });
        }
        Some(WheelInput { delta, horizontal })
    } else {
        None
    }
}

unsafe extern "system" fn mouse_proc(code: i32, wparam: usize, lparam: isize) -> isize {
    if code >= 0 {
        let ev = wparam as u32;
        let ms = &*(lparam as *const MSLLHOOKSTRUCT);
        // Ignore events we synthesized (check LLMHF_INJECTED + our dwExtraInfo marker).
        let is_our_injection = (ms.flags & LLMHF_INJECTED) != 0
            || ms.dwExtraInfo == 0xFA57_0000;
        if is_our_injection {
            return CallNextHookEx(0, code, wparam, lparam);
        }

        let cfg = CONFIG.read();

        // ── Wheel events ──────────────────────────────────────────────────────────
        if ev == WM_MOUSEWHEEL || ev == WM_MOUSEHWHEEL {
            // Window-switcher gesture: wheel navigates the Alt+Tab list while held.
            if crate::win::window_switcher::is_active() {
                let raw_delta = (ms.mouseData >> 16) as i16 as i32;
                crate::win::window_switcher::step(raw_delta < 0);
                return 1; // swallowed: drives the switcher, never scrolls
            }
            if cfg.scroll.enabled {
                let raw_delta = (ms.mouseData >> 16) as i16 as i32;
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
            // Window-drag gesture: behaviour depends on cfg.drag.mode.
            let mut dragging = false;
            match drag_output(&cfg.drag.mode) {
                DragOutput::Move => {
                    if let Some(ctrl) = DRAG.read().as_ref() {
                        if let Some((x, y)) = ctrl.target_pos(ms.pt.x, ms.pt.y) {
                            crate::win::window::move_window(ctrl.hwnd(), x, y);
                            dragging = true;
                        }
                    }
                }
                DragOutput::Scroll => {
                    // Feed pointer delta into the smooth-scroll injector (mac drag-to-scroll).
                    // Held modifiers switch semantics (mac Core/Modifiers/): Shift forces
                    // horizontal, Ctrl slows to precision.
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
                                // Vertical drag becomes horizontal scroll (mac Shift behaviour).
                                let h = (-(dx as f64 + dy as f64) * scale).round() as i32;
                                if h != 0 {
                                    let _ = tx.send(WheelInput { delta: h, horizontal: true });
                                }
                            } else {
                                let v = (-(dy as f64) * scale).round() as i32;
                                let h = (-(dx as f64) * scale).round() as i32;
                                if v != 0 {
                                    let _ = tx.send(WheelInput { delta: v, horizontal: false });
                                }
                                if h != 0 {
                                    let _ = tx.send(WheelInput { delta: h, horizontal: true });
                                }
                            }
                        }
                        dragging = true;
                    }
                }
                DragOutput::Navigate => {
                    // Horizontal drag -> browser back/forward (mac TwoFingerSwipe).
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
                            crate::remap::execute_effect(&crate::remap::Effect::NavigationSwipe { direction });
                            ctrl.reset_accum();
                        }
                        dragging = true;
                    }
                }
            }
            // ModifiedDrag: TwoFingerSwipe / ThreeFingerSwipe → navigation swipe.
            // FakeDrag → synthesized mouse drag (move while button held).
            if !dragging {
                if let Some(drag_state) = DRAG_EFFECT.read().as_ref() {
                    let dx = ms.pt.x - drag_state.origin_x;
                    let dy = ms.pt.y - drag_state.origin_y;
                    match drag_state.drag_type {
                        ModifiedDragType::TwoFingerSwipe => {
                            // Two-finger horizontal swipe → browser back/forward.
                            if dx.abs() > 4 || dy.abs() > 4 {
                                let direction = if dx > 0 {
                                    crate::remap::SwipeDirection::Forward
                                } else {
                                    crate::remap::SwipeDirection::Back
                                };
                                crate::remap::execute_effect(&crate::remap::Effect::NavigationSwipe { direction });
                                *DRAG_EFFECT.write() = Some(DragState {
                                    drag_type: drag_state.drag_type.clone(),
                                    trigger_button: drag_state.trigger_button,
                                    origin_x: ms.pt.x,
                                    origin_y: ms.pt.y,
                                });
                            }
                            dragging = true;
                        }
                        ModifiedDragType::ThreeFingerSwipe | ModifiedDragType::FourFingerSwipe => {
                            // Three/four-finger swipe → virtual desktop switch (Win+Ctrl+Left/Right).
                            // Mirrors mac's TouchSimulator postDockSwipeEventWithDelta.
                            if dx.abs() > 4 || dy.abs() > 4 {
                                let direction = if dx > 0 {
                                    crate::remap::SwipeDirection::Forward
                                } else {
                                    crate::remap::SwipeDirection::Back
                                };
                                crate::remap::send_virtual_desktop_switch(direction);
                                *DRAG_EFFECT.write() = Some(DragState {
                                    drag_type: drag_state.drag_type.clone(),
                                    trigger_button: drag_state.trigger_button,
                                    origin_x: ms.pt.x,
                                    origin_y: ms.pt.y,
                                });
                            }
                            dragging = true;
                        }
                        ModifiedDragType::FakeDrag => {
                            // Send the accumulated delta as a mouse move while the trigger button is held.
                            if dx != 0 || dy != 0 {
                                send_fake_drag_move(drag_state.trigger_button, dx, dy);
                                // Update origin to track next delta.
                                *DRAG_EFFECT.write() = Some(DragState {
                                    drag_type: drag_state.drag_type.clone(),
                                    trigger_button: drag_state.trigger_button,
                                    origin_x: ms.pt.x,
                                    origin_y: ms.pt.y,
                                });
                            }
                            dragging = true;
                        }
                    }
                }
            }
            // Pointer acceleration: amplify the cursor move and swallow.
            if !dragging {
                let mut accel = ACCEL.write();
                if let Some(ctrl) = accel.as_mut() {
                    if let Some((dx, dy)) = ctrl.on_move(ms.pt.x, ms.pt.y, &cfg.accel) {
                        if dx != 0 || dy != 0 {
                            crate::scroll::injector::send_mouse_move(dx, dy);
                            return 1;
                        }
                    }
                }
            }
        }

        // ── Button events ───────────────────────────────────────────────────────
        if let Some((btn, down)) = button_event(ev, ms) {
            // Middle-button window-switcher gesture takes priority over normal remap.
            // A single middle click is preserved via delayed replay (see window_switcher).
            if cfg.buttons.window_switcher && btn == MouseButton::Middle {
                if down {
                    if crate::win::window_switcher::on_middle_down() {
                        return 1;
                    }
                } else if crate::win::window_switcher::swallow_middle_up() {
                    return 1;
                }
            }
            // Button remapping with ClickCycle support.
            if cfg.buttons.enabled {
                let kb_state = crate::modifiers::state();
                let held_btns = get_tracker().read().held_modifier_buttons();
                let active_mods = ActiveModifiers {
                    keyboard: kb_state as u32,
                    buttons: held_btns,
                };

                // AddMode: capture the trigger and pass through unmodified.
                if crate::add_mode::on_button_event(btn, 1, down, &active_mods) {
                    return 1;
                }

                if down {
                    let mut tracker = get_tracker().write();
                    let (click_count, is_new) = tracker.on_button_down(btn);
                    if is_new {
                        if let Some(engine) = REMAP_ENGINE.read().as_ref() {
                            let max_level = engine.max_level_for_button(btn);
                            tracker.set_max_level(btn, max_level);
                        }
                    }
                    let has_remap = {
                        if let Some(engine) = REMAP_ENGINE.read().as_ref() {
                            let results = engine.resolve_effects(btn, click_count, false, &active_mods);
                            !results.is_empty()
                        } else {
                            false
                        }
                    };
                    if has_remap {
                        // Activate drag state if this is a drag trigger.
                        if let Some(engine) = REMAP_ENGINE.read().as_ref() {
                            let results = engine.resolve_effects(btn, click_count, false, &active_mods);
                            for (effect, _) in &results {
                                if let Effect::ModifiedDrag { drag_type, .. } = effect {
                                    let origin = crate::win::window::cursor_pos();
                                    *DRAG_EFFECT.write() = Some(DragState {
                                        drag_type: drag_type.clone(),
                                        trigger_button: btn,
                                        origin_x: origin.0,
                                        origin_y: origin.1,
                                    });
                                    // For FakeDrag: send the button-down now so the system
                                    // sees a click; we will send button-up on release.
                                    if *drag_type == ModifiedDragType::FakeDrag {
                                        send_fake_drag_button_down(btn);
                                    }
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
                    if let Some(drag_state) = DRAG_EFFECT.read().clone() {
                        if drag_state.drag_type == ModifiedDragType::FakeDrag
                            && drag_state.trigger_button == btn
                        {
                            // Send the button-up for the fake drag.
                            send_fake_drag_button_up(drag_state.trigger_button);
                            *DRAG_EFFECT.write() = None;
                            return 1; // swallow the button-up
                        }
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
            {
                let mut drag = DRAG.write();
                if let Some(ctrl) = drag.as_mut() {
                    if down
                        && ctrl.matches_trigger(btn, down, SPACE_HELD.load(Ordering::Relaxed))
                        && !ctrl.is_active()
                    {
                        if let Some(hwnd) = crate::win::window::window_at_cursor(&ms.pt) {
                            if let Some(rect) = crate::win::window::get_window_rect(hwnd) {
                                ctrl.begin(hwnd, ms.pt.x - rect.left, ms.pt.y - rect.top, ms.pt.x, ms.pt.y);
                                return 1;
                            }
                        }
                    } else if !down && ctrl.is_active() && ctrl.matches_release(btn) {
                        ctrl.end();
                        return 1;
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
            crate::modifiers::set_vk(ks.vkCode as u32, down);
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
    // Drive the window-switcher gesture (delayed replay + Alt-timeout) regardless
    // of whether any ClickCycle remap is currently active.
    crate::win::window_switcher::tick();

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
        let mut input = INPUT { r#type: INPUT_MOUSE, ..std::mem::zeroed() };
        input.Anonymous.mi = MOUSEINPUT {
            dx, dy, mouseData: data, dwFlags: flags, time: 0, dwExtraInfo: 0,
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
        let mut input = INPUT { r#type: INPUT_MOUSE, ..std::mem::zeroed() };
        input.Anonymous.mi = MOUSEINPUT {
            dx: 0, dy: 0, mouseData: data, dwFlags: flags, time: 0, dwExtraInfo: 0,
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
        let mut input = INPUT { r#type: INPUT_MOUSE, ..std::mem::zeroed() };
        input.Anonymous.mi = MOUSEINPUT {
            dx: 0, dy: 0, mouseData: data, dwFlags: flags, time: 0, dwExtraInfo: 0,
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
}