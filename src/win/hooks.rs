use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, SetWindowsHookExW, UnhookWindowsHookEx, WH_KEYBOARD_LL, WH_MOUSE_LL,
};

use crate::CONFIG;

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

// NOTE: Feature logic (smooth scroll, button remap) plugs in here later.
// For now every event is passed through unchanged.
unsafe extern "system" fn mouse_proc(code: i32, wparam: usize, lparam: isize) -> isize {
    CallNextHookEx(0, code, wparam, lparam)
}

unsafe extern "system" fn keyboard_proc(code: i32, wparam: usize, lparam: isize) -> isize {
    CallNextHookEx(0, code, wparam, lparam)
}
