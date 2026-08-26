use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, MSLLHOOKSTRUCT, SetWindowsHookExW, UnhookWindowsHookEx, WH_KEYBOARD_LL, WH_MOUSE_LL, WM_MOUSEWHEEL, WM_MOUSEHWHEEL,
};

use crate::CONFIG;
use std::sync::mpsc::Sender;
use std::sync::{Mutex, OnceLock};
use crate::scroll::engine::WheelInput;

/// Sender shared with the hook by `init_scroll_sender`; None when smooth scroll
/// is disabled. Wrapped in a Mutex because the injector thread and the hook
/// thread both touch it (mpsc::Sender is Send but not Sync).
static SCROLL_TX: OnceLock<Mutex<Sender<WheelInput>>> = OnceLock::new();

/// Low-level hook flag: the event was synthesized via `SendInput` (by us), so it
/// must not be re-smoothed (would cause a feedback loop).
const LLMHF_INJECTED: u32 = 0x01;

/// Called from `main` to share the injector's sender with the hook procedure.
pub fn init_scroll_sender(tx: Sender<WheelInput>) {
    let _ = SCROLL_TX.set(Mutex::new(tx));
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

    if let Some(cfg) = CONFIG.get() {
        crate::log::write(&format!(
            "hooks installed (scroll.enabled={}, buttons.enabled={})",
            cfg.scroll.enabled, cfg.buttons.enabled
        ));
    }
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
        if ev == WM_MOUSEWHEEL || ev == WM_MOUSEHWHEEL {
            let ms = &*(lparam as *const MSLLHOOKSTRUCT);
            // Ignore events we ourselves synthesized (feedback loop guard).
            if (ms.flags & LLMHF_INJECTED) != 0 {
                return CallNextHookEx(0, code, wparam, lparam);
            }
            if let Some(cfg) = CONFIG.get() {
                if cfg.scroll.enabled && cfg.scroll.smooth {
                    let raw = (ms.mouseData >> 16) as i16;
                    let delta = raw as i32;
                    let horizontal = ev == WM_MOUSEHWHEEL;
                    if let Some(tx) = SCROLL_TX.get() {
                        if let Ok(g) = tx.lock() {
                            let _ = g.send(WheelInput { delta, horizontal });
                        }
                    }
                    return 1; // swallow original; the injector replays it smoothly
                }
            }
        }
    }
    CallNextHookEx(0, code, wparam, lparam)
}

unsafe extern "system" fn keyboard_proc(code: i32, wparam: usize, lparam: isize) -> isize {
    CallNextHookEx(0, code, wparam, lparam)
}
