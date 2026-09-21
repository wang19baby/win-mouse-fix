//! Focus-center: moves the cursor to the center of the newly focused window
//! after Alt+Tab / Win+Tab.
//!
//! Uses a WinEvent hook on `EVENT_SYSTEM_FOREGROUND` to detect focus changes
//! and `SetCursorPos` to reposition the mouse.

use std::sync::atomic::{AtomicBool, Ordering};
use windows_sys::Win32::Foundation::POINT;
use windows_sys::Win32::UI::Accessibility::{SetWinEventHook, UnhookWinEvent, HWINEVENTHOOK};
use windows_sys::Win32::UI::WindowsAndMessaging::{EVENT_SYSTEM_FOREGROUND, SetCursorPos};

/// Whether the focus-center feature is enabled.
pub static FOCUS_CENTER_ENABLED: AtomicBool = AtomicBool::new(false);

/// WinEvent hook handle for the foreground hook.
static mut FOREGROUND_HOOK: HWINEVENTHOOK = 0;

/// WinEvent callback — fires on every foreground window change.
#[allow(non_snake_case)]
unsafe extern "system" fn foreground_callback(
    _hEventHook: HWINEVENTHOOK,
    event: u32,
    hwnd: windows_sys::Win32::Foundation::HWND,
    _idObject: i32,
    _idChild: i32,
    _ideventThread: u32,
    _dwmEventTime: u32,
) {
    if event != EVENT_SYSTEM_FOREGROUND {
        return;
    }
    if hwnd == 0 {
        return;
    }
    // Skip if feature is disabled
    if !FOCUS_CENTER_ENABLED.load(Ordering::Relaxed) {
        return;
    }

    // Get window rect
    let rect = match crate::win::window::get_window_rect(hwnd) {
        Some(r) => r,
        None => return,
    };

    // Compute center
    let cx = ((rect.right as i32) + (rect.left as i32)) / 2;
    let cy = ((rect.bottom as i32) + (rect.top as i32)) / 2;

    // Move cursor to window center
    let pt = POINT { x: cx, y: cy };
    let _ok = SetCursorPos(pt.x, pt.y);
}

/// Start the foreground WinEvent hook. Idempotent.
pub fn start() {
    if !FOCUS_CENTER_ENABLED.load(Ordering::Relaxed) {
        return;
    }
    unsafe {
        if FOREGROUND_HOOK != 0 {
            return; // already installed
        }
        let hook = SetWinEventHook(
            EVENT_SYSTEM_FOREGROUND,
            EVENT_SYSTEM_FOREGROUND,
            0isize,
            Some(foreground_callback),
            0, // all processes
            0, // all threads
            0, // WINEVENT_OUTOFCONTEXT
        );
        FOREGROUND_HOOK = hook;
        crate::log::write(&format!(
            "focus_center: WinEvent hook installed handle={:#x}",
            hook
        ));
    }
}

/// Stop and uninstall the foreground WinEvent hook.
pub fn stop() {
    unsafe {
        if FOREGROUND_HOOK != 0 {
            UnhookWinEvent(FOREGROUND_HOOK);
            crate::log::write("focus_center: WinEvent hook uninstalled");
            FOREGROUND_HOOK = 0;
        }
    }
}

/// Update the enabled state from config.
pub fn set_enabled(enabled: bool) {
    FOCUS_CENTER_ENABLED.store(enabled, Ordering::Relaxed);
}
