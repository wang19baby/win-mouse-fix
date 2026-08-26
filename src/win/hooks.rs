use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, MSLLHOOKSTRUCT, SetWindowsHookExW, UnhookWindowsHookEx, WH_KEYBOARD_LL, WH_MOUSE_LL, WM_MOUSEMOVE, WM_MOUSEWHEEL, WM_MOUSEHWHEEL, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_RBUTTONDOWN, WM_RBUTTONUP, WM_MBUTTONDOWN, WM_MBUTTONUP, WM_XBUTTONDOWN, WM_XBUTTONUP, XBUTTON2, KBDLLHOOKSTRUCT, WM_KEYDOWN, WM_KEYUP, WM_SYSKEYDOWN, WM_SYSKEYUP,
};

use crate::CONFIG;
use crate::config::Config;
use crate::gesture::DragController;
use std::sync::atomic::{AtomicBool, Ordering};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::VK_SPACE;
use std::sync::mpsc::Sender;
use parking_lot::{Mutex, RwLock};
use crate::scroll::engine::WheelInput;
use crate::remap::{MouseButton, RemapTable};

/// Sender to the injector thread. `None` while smooth scroll is disabled.
/// Wrapped in a Mutex only for shared access from the hook procedure
/// (mpsc::Sender is Send but not Sync); the injector owns the receiving end.
static SCROLL_TX: Mutex<Option<Sender<WheelInput>>> = Mutex::new(None);

/// Low-level hook flag: the event was synthesized via `SendInput` (by us), so it
/// must not be re-smoothed (would cause a feedback loop).
const LLMHF_INJECTED: u32 = 0x01;

/// Which top-level feature a tray-menu toggle acts on.
#[derive(Clone, Copy)]
pub enum Feature {
    SmoothScroll,
    ButtonRemap,
}

/// Built from the button-remap config; `None` disables remapping.
static REMAP_TABLE: RwLock<Option<RemapTable>> = RwLock::new(None);

/// Space key state, tracked by the keyboard hook for Space-drag gestures.
static SPACE_HELD: AtomicBool = AtomicBool::new(false);

/// Active window-drag gesture controller; `None` disables gestures.
static DRAG: RwLock<Option<DragController>> = RwLock::new(None);

/// Fully (re)initialize the runtime from `cfg`: tear down hooks + injector,
/// swap in the new config, then bring hooks back up. Safe to call repeatedly
/// (at startup, and on each tray-menu toggle).
pub fn apply_config(cfg: Config) {
    uninstall();
    stop_scroll();
    *REMAP_TABLE.write() = None;

    *CONFIG.write() = cfg;
    let cfg = CONFIG.read();

    if cfg.scroll.enabled {
        let tx = crate::scroll::injector::start(&*cfg);
        *SCROLL_TX.lock() = Some(tx);
    }
    if cfg.buttons.enabled {
        *REMAP_TABLE.write() = Some(RemapTable::from_entries(&cfg.buttons.remaps));
    }
    if cfg.drag.enabled {
        *DRAG.write() = Some(DragController::new(crate::gesture::parse_button(
            &cfg.drag.button,
        )));
    } else {
        *DRAG.write() = None;
    }
    drop(cfg);

    let _ = install();
}

/// Drop the injector sender so its thread disconnects and exits.
pub fn stop_scroll() {
    *SCROLL_TX.lock() = None;
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
    let mut cfg = Config::load_or_default();
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
        "hooks installed (scroll.enabled={}, buttons.enabled={})",
        cfg.scroll.enabled, cfg.buttons.enabled
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
}

unsafe extern "system" fn mouse_proc(code: i32, wparam: usize, lparam: isize) -> isize {
    if code >= 0 {
        let ev = wparam as u32;
        let ms = &*(lparam as *const MSLLHOOKSTRUCT);
        // Ignore events we ourselves synthesized (feedback loop guard).
        if (ms.flags & LLMHF_INJECTED) != 0 {
            return CallNextHookEx(0, code, wparam, lparam);
        }

        let cfg = CONFIG.read();

        // Smooth scrolling: swallow the raw wheel event; the injector replays it.
        if (ev == WM_MOUSEWHEEL || ev == WM_MOUSEHWHEEL) && cfg.scroll.enabled {
            let mut delta = (ms.mouseData >> 16) as i16 as i32;
            let mut horizontal = ev == WM_MOUSEHWHEEL;
            // Modifier behaviors (Shift to accelerate / swap axis).
            let shift = crate::modifiers::shift_held();
            (delta, horizontal) = crate::modifiers::apply_scroll_modifiers(
                delta,
                horizontal,
                shift,
                cfg.scroll.shift_speedup,
                cfg.scroll.shift_horizontal,
            );
            if cfg.scroll.smooth {
                if let Some(tx) = SCROLL_TX.lock().as_ref() {
                    let _ = tx.send(WheelInput { delta, horizontal });
                }
                return 1; // swallow original; the injector replays it smoothly
            }
        }

        // Drag gesture: move the window under the cursor while the trigger
        // button is held. Runs on cursor moves (which carry no button event).
        if ev == WM_MOUSEMOVE {
            if let Some(ctrl) = DRAG.read().as_ref() {
                if let Some((x, y)) = ctrl.target_pos(ms.pt.x, ms.pt.y) {
                    crate::win::window::move_window(ctrl.hwnd(), x, y);
                }
            }
        } else if let Some((btn, down)) = button_event(ev, ms) {
            // Drag start/end takes precedence over remap for the trigger button.
            {
                let mut drag = DRAG.write();
                if let Some(ctrl) = drag.as_mut() {
                    if down
                        && ctrl.matches_trigger(btn, down, SPACE_HELD.load(Ordering::Relaxed))
                        && !ctrl.is_active()
                    {
                        if let Some(hwnd) = crate::win::window::window_at_cursor(&ms.pt) {
                            if let Some(rect) = crate::win::window::get_window_rect(hwnd) {
                                ctrl.begin(hwnd, ms.pt.x - rect.left, ms.pt.y - rect.top);
                                return 1; // swallow the initiating button-down
                            }
                        }
                    } else if !down && ctrl.is_active() && ctrl.matches_release(btn) {
                        ctrl.end();
                        return 1; // swallow the terminating button-up
                    }
                }
            }
            // Button remapping: swallow the source button, synthesize the target.
            if cfg.buttons.enabled {
                if let Some(table) = REMAP_TABLE.read().as_ref() {
                    if let Some(action) = table.lookup(btn) {
                        crate::remap::execute(action, down);
                        return 1; // swallow original; target is synthesized
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
