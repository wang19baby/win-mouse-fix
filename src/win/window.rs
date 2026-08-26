//! Thin Win32 helpers for the drag-gesture feature: locate the window under
//! the cursor, read its rect, and move it. Kept separate from `hooks` so the
//! gesture state machine (`crate::gesture`) stays pure and testable.
//!
//! Note: in windows-sys 0.52 `HWND` is a type alias for `isize`, so window
//! handles are passed around as raw `isize` values.

use windows_sys::Win32::Foundation::{POINT, RECT};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GetAncestor, GetClassNameW, GetDesktopWindow, GetWindowRect, IsIconic, IsZoomed,
    SetWindowPos, WindowFromPoint, GA_ROOT, HWND_TOP, SWP_NOSIZE, SWP_NOACTIVATE, SWP_NOZORDER,
};

/// The movable top-level window under `pt`, or `None` if no suitable target
/// (desktop, taskbar, iconic/zoomed window).
pub fn window_at_cursor(pt: &POINT) -> Option<isize> {
    unsafe {
        let mut hwnd = WindowFromPoint(*pt);
        if hwnd == 0 {
            return None;
        }
        // Drag the whole top-level window, not an inner control.
        let root = GetAncestor(hwnd, GA_ROOT);
        if root != 0 {
            hwnd = root;
        }
        if is_movable(hwnd) {
            Some(hwnd)
        } else {
            None
        }
    }
}

/// Current rect of `hwnd` (passed as the raw isize handle).
pub fn get_window_rect(hwnd: isize) -> Option<RECT> {
    unsafe {
        let mut rect: RECT = std::mem::zeroed();
        if GetWindowRect(hwnd, &mut rect) != 0 {
            Some(rect)
        } else {
            None
        }
    }
}

/// Move `hwnd` (raw isize handle) to `(x, y)`, preserving size and z-order.
pub fn move_window(hwnd: isize, x: i32, y: i32) {
    unsafe {
        SetWindowPos(
            hwnd,
            HWND_TOP,
            x,
            y,
            0,
            0,
            SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
        );
    }
}

/// Whether `hwnd` is a window the user can reasonably drag (skips desktop,
/// taskbar, and iconic/zoomed windows).
fn is_movable(hwnd: isize) -> bool {
    unsafe {
        if IsIconic(hwnd) != 0 || IsZoomed(hwnd) != 0 {
            return false;
        }
        if hwnd == GetDesktopWindow() {
            return false;
        }
        let mut buf = [0u16; 256];
        let n = GetClassNameW(hwnd, buf.as_mut_ptr(), buf.len() as i32);
        if n > 0 {
            let class = String::from_utf16_lossy(&buf[..n as usize]);
            if matches!(class.as_str(), "Progman" | "WorkerW" | "Shell_TrayWnd") {
                return false;
            }
        }
        true
    }
}
