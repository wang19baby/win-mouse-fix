//! Thin Win32 helpers for the drag-gesture feature: locate the window under
//! the cursor, read its rect, and move it. Kept separate from `hooks` so the
//! gesture state machine (`crate::gesture`) stays pure and testable.
//!
//! Note: in windows-sys 0.52 `HWND` is a type alias for `isize`, so window
//! handles are passed around as raw `isize` values.

use windows_sys::Win32::Foundation::{POINT, RECT};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GetAncestor, GetClassNameW, GetCursorPos, GetDesktopWindow, GetForegroundWindow,
    GetWindowModuleFileNameW, GetWindowRect, IsIconic, IsZoomed, SetWindowPos, WindowFromPoint,
    GA_ROOT, HWND_TOP, SWP_NOACTIVATE, SWP_NOSIZE, SWP_NOZORDER,
};

/// Returns the current cursor position as (x, y).
pub fn cursor_pos() -> (i32, i32) {
    unsafe {
        let mut pt: POINT = std::mem::zeroed();
        GetCursorPos(&mut pt);
        (pt.x, pt.y)
    }
}

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

/// Move and resize `hwnd` to `rect`.
pub fn set_window_rect(hwnd: isize, rect: &RECT) {
    unsafe {
        SetWindowPos(
            hwnd,
            HWND_TOP,
            rect.left,
            rect.top,
            rect.right - rect.left,
            rect.bottom - rect.top,
            SWP_NOZORDER | SWP_NOACTIVATE,
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

/// Basename of the foreground window's owning executable, or `None` if it
/// can't be determined. Used to select a per-app config profile.
pub fn foreground_exe() -> Option<String> {
    unsafe {
        let hwnd = GetForegroundWindow();
        if hwnd == 0 {
            return None;
        }
        let mut buf = [0u16; 1024];
        let n = GetWindowModuleFileNameW(hwnd, buf.as_mut_ptr(), buf.len() as u32);
        if n == 0 {
            return None;
        }
        let path = String::from_utf16_lossy(&buf[..n as usize]);
        let name = std::path::Path::new(&path)
            .file_name()?
            .to_string_lossy()
            .into_owned();
        Some(name)
    }
}

/// Returns true if `hwnd` is currently always-on-top (WS_EX_TOPMOST).
pub fn is_always_on_top(hwnd: isize) -> bool {
    unsafe {
        use windows_sys::Win32::UI::WindowsAndMessaging::GetWindowLongPtrW;
        let ex_style = GetWindowLongPtrW(hwnd, windows_sys::Win32::UI::WindowsAndMessaging::GWL_EXSTYLE) as u32;
        (ex_style & windows_sys::Win32::UI::WindowsAndMessaging::WS_EX_TOPMOST) != 0
    }
}

/// Toggle the always-on-top (pin) state of `hwnd`.
/// Returns the new state.
pub fn toggle_always_on_top(hwnd: isize) -> bool {
    let currently = is_always_on_top(hwnd);
    unsafe {
        use windows_sys::Win32::UI::WindowsAndMessaging::{SetWindowPos, HWND_TOPMOST};
        SetWindowPos(
            hwnd,
            if currently { windows_sys::Win32::UI::WindowsAndMessaging::HWND_NOTOPMOST } else { HWND_TOPMOST },
            0, 0, 0, 0,
            windows_sys::Win32::UI::WindowsAndMessaging::SWP_NOMOVE
                | windows_sys::Win32::UI::WindowsAndMessaging::SWP_NOSIZE
                | windows_sys::Win32::UI::WindowsAndMessaging::SWP_NOACTIVATE,
        );
    }
    !currently
}

/// Minimize `hwnd` to the taskbar.
pub fn minimize_window(hwnd: isize) {
    unsafe {
        use windows_sys::Win32::UI::WindowsAndMessaging::ShowWindow;
        ShowWindow(hwnd, windows_sys::Win32::UI::WindowsAndMessaging::SW_MINIMIZE);
    }
}

/// Restore `hwnd` from minimized state.
pub fn restore_window(hwnd: isize) {
    unsafe {
        use windows_sys::Win32::UI::WindowsAndMessaging::ShowWindow;
        ShowWindow(hwnd, windows_sys::Win32::UI::WindowsAndMessaging::SW_RESTORE);
    }
}

/// Returns true if `hwnd` is minimized.
pub fn is_minimized(hwnd: isize) -> bool {
    unsafe {
        use windows_sys::Win32::UI::WindowsAndMessaging::IsIconic;
        IsIconic(hwnd) != 0
    }
}

/// Returns true if `hwnd` is maximized.
#[allow(dead_code)]
pub fn is_maximized(hwnd: isize) -> bool {
    unsafe {
        IsZoomed(hwnd) != 0
    }
}

/// Get the title bar height of `hwnd` (approximate, using SM_CYCAPTION).
#[allow(dead_code)]
pub fn title_bar_height() -> i32 {
    unsafe {
        use windows_sys::Win32::UI::WindowsAndMessaging::GetSystemMetrics;
        use windows_sys::Win32::UI::WindowsAndMessaging::SM_CYCAPTION;
        GetSystemMetrics(SM_CYCAPTION)
    }
}

/// Roll up `hwnd` to just its title bar (windows classic "minimize to name").
/// Stores the original rect before rolling.
pub fn rollup_window(hwnd: isize, original_rect: &RECT) {
    unsafe {
        use windows_sys::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, SetWindowPos, SM_CYCAPTION, SWP_NOACTIVATE, SWP_NOZORDER};
        let title_h = GetSystemMetrics(SM_CYCAPTION);
        SetWindowPos(
            hwnd,
            0,
            original_rect.left,
            original_rect.top,
            original_rect.right - original_rect.left,
            title_h,
            SWP_NOACTIVATE | SWP_NOZORDER,
        );
    }
}

/// Restore a rolled-up `hwnd` to `original_rect`.
pub fn unroll_window(hwnd: isize, original_rect: &RECT) {
    set_window_rect(hwnd, original_rect);
}

/// Returns true if `hwnd` has a visible title bar with standard controls.
#[allow(dead_code)]
pub fn has_standard_title_bar(hwnd: isize) -> bool {
    if is_minimized(hwnd) || is_maximized(hwnd) {
        return false;
    }
    unsafe {
        use windows_sys::Win32::UI::WindowsAndMessaging::{GetWindowLongPtrW, GWL_STYLE};
        let style = GetWindowLongPtrW(hwnd, GWL_STYLE) as u32;
        // WS_CAPTION = 0x00C00000 = title bar with text
        // WS_SYSMENU = 0x00080000 = system menu icon
        (style & 0x00C00000) != 0 && (style & 0x00080000) != 0
    }
}

/// Returns the HWND of the foreground (active) window, or None.
pub fn foreground_hwnd() -> Option<isize> {
    unsafe {
        use windows_sys::Win32::UI::WindowsAndMessaging::GetForegroundWindow;
        let hwnd = GetForegroundWindow();
        if hwnd == 0 { None } else { Some(hwnd) }
    }
}

/// Toggle borderless (frameless) state of `hwnd`.
pub fn toggle_borderless(hwnd: isize) {
    unsafe {
        use windows_sys::Win32::UI::WindowsAndMessaging::{
            GetWindowLongPtrW, SetWindowLongPtrW, GWL_STYLE, SetWindowPos, SWP_FRAMECHANGED, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER,
        };
        let style = GetWindowLongPtrW(hwnd, GWL_STYLE) as u32;
        let has_caption = (style & 0x00C00000) != 0;
        let new_style = if has_caption {
            style & !0x00C00000 & !0x00080000 // remove caption + sysmenu
        } else {
            style | 0x00C00000 | 0x00080000   // add caption + sysmenu
        };
        SetWindowLongPtrW(hwnd, GWL_STYLE, new_style as isize);
        SetWindowPos(hwnd, 0, 0, 0, 0, 0, SWP_FRAMECHANGED | SWP_NOACTIVATE | SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER);
    }
}

/// Minimize all visible windows except `exclude_hwnd`.
#[allow(dead_code)]
pub fn minimize_other_windows(exclude_hwnd: isize) {
    unsafe {
        use windows_sys::Win32::UI::WindowsAndMessaging::{
            EnumWindows, GetWindowTextW, IsWindowVisible, ShowWindow, SW_MINIMIZE,
        };
        static EXCLUDE: std::sync::OnceLock<isize> = std::sync::OnceLock::new();
        EXCLUDE.set(exclude_hwnd).ok();

        unsafe extern "system" fn enum_proc(hwnd: isize, _lparam: isize) -> i32 {
            if hwnd == 0 {
                return 1; // continue
            }
            let exclude = *EXCLUDE.get().unwrap_or(&0);
            if hwnd == exclude {
                return 1; // continue
            }
            if IsWindowVisible(hwnd) == 0 {
                return 1; // continue
            }
            // Skip windows without a title (desktop, etc.)
            let mut buf = [0u16; 256];
            let len = GetWindowTextW(hwnd, buf.as_mut_ptr(), buf.len() as i32);
            if len == 0 {
                return 1; // continue
            }
            ShowWindow(hwnd, SW_MINIMIZE);
            1 // continue enumeration
        }

        EnumWindows(Some(enum_proc), 0);
    }
}

// ── Snap-I: Window List ───────────────────────────────────────────────────────

use std::sync::OnceLock;

#[allow(dead_code)]
static WINDOW_LIST: OnceLock<parking_lot::Mutex<Vec<WindowEntry>>> = OnceLock::new();

/// An entry in the window list used by Snap-I Window List feature.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct WindowEntry {
    pub hwnd: isize,
    pub title: String,
    pub exe: String,
}

/// Returns visible windows (hwnd, title, exe). Used by Win+Tab Window List.
#[allow(dead_code)]
pub fn window_list() -> &'static parking_lot::Mutex<Vec<WindowEntry>> {
    WINDOW_LIST.get_or_init(|| parking_lot::Mutex::new(refresh_window_list()))
}

/// Refresh the cached window list. Call this before showing the window list UI.
#[allow(dead_code)]
fn refresh_window_list() -> Vec<WindowEntry> {
    let mut result = Vec::new();
    unsafe {
        use windows_sys::Win32::UI::WindowsAndMessaging::{EnumWindows, GetWindowTextW, IsWindowVisible, GetWindowModuleFileNameW};

        unsafe extern "system" fn enum_proc(hwnd: isize, lparam: isize) -> i32 {
            if hwnd == 0 { return 1; }
            if IsWindowVisible(hwnd) == 0 { return 1; }

            // Skip windows without a title
            let mut title_buf = [0u16; 512];
            let title_len = GetWindowTextW(hwnd, title_buf.as_mut_ptr(), title_buf.len() as i32);
            // Get exe name from window (full path via GetWindowModuleFileNameW)
            let mut exe_buf = [0u16; 1024];
            let exe_len = GetWindowModuleFileNameW(hwnd, exe_buf.as_mut_ptr(), exe_buf.len() as u32);
            let exe_name = if exe_len > 0 {
                std::path::Path::new(&String::from_utf16_lossy(&exe_buf[..exe_len as usize]))
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default()
            } else {
                String::new()
            };

            let title = String::from_utf16_lossy(&title_buf[..title_len as usize]);
            let entry = WindowEntry { hwnd, title, exe: exe_name };
            let list = &mut *(lparam as *mut Vec<WindowEntry>);
            list.push(entry);
            1
        }

        EnumWindows(Some(enum_proc), &mut result as *mut Vec<WindowEntry> as isize);
    }
    result
}
