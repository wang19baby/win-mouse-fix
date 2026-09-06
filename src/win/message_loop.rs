use windows_sys::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, GetMessageW, PeekMessageW, TranslateMessage, PM_REMOVE, QS_ALLINPUT,
    WM_APP, WM_QUIT,
};
use windows_sys::Win32::Foundation::WPARAM;
use std::sync::atomic::{AtomicBool, Ordering};

// Hook installation requests from the WS/auth thread — processed on the main thread.
static HOOK_INSTALL_REQUESTED: AtomicBool = AtomicBool::new(false);
static HOOK_UNINSTALL_REQUESTED: AtomicBool = AtomicBool::new(false);

/// Request hook installation on the main thread. Thread-safe.
#[inline]
pub fn request_hook_install() {
    HOOK_INSTALL_REQUESTED.store(true, Ordering::Relaxed);
}

/// Request hook uninstallation on the main thread. Thread-safe.
#[inline]
pub fn request_hook_uninstall() {
    HOOK_UNINSTALL_REQUESTED.store(true, Ordering::Relaxed);
}

/// Process any pending hook requests. Called once per message-loop iteration.
#[inline]
fn process_hook_requests() {
    if HOOK_UNINSTALL_REQUESTED.load(Ordering::Relaxed) {
        HOOK_UNINSTALL_REQUESTED.store(false, Ordering::Relaxed);
        crate::win::window_list::stop_win_event_hooks();
    }
    if HOOK_INSTALL_REQUESTED.load(Ordering::Relaxed) {
        HOOK_INSTALL_REQUESTED.store(false, Ordering::Relaxed);
        crate::win::window_list::start_win_event_hooks();
    }
}

/// Standard Win32 message pump. Required for the low-level hooks and the
/// tray icon to receive messages. Blocks until WM_QUIT.
/// Also processes hook installation requests from background threads.
pub fn run() {
    unsafe {
        let mut msg: windows_sys::Win32::UI::WindowsAndMessaging::MSG = std::mem::zeroed();
        loop {
            // Process any pending hook requests first.
            process_hook_requests();

            // Block waiting for the next message.
            // On Windows, GetMessage returns -1 on error, 0 on WM_QUIT, >0 on message.
            let ret = GetMessageW(&mut msg, 0, 0, 0);
            if ret == -1 || ret == 0 {
                // Error or WM_QUIT — exit.
                break;
            }
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
            // Loop immediately to process hook requests before blocking again.
        }
    }
}
