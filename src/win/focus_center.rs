//! Focus-center: moves the cursor to the center of the newly focused window
//! after Alt+Tab / Win+Tab.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{sync_channel, SyncSender, TrySendError};
use std::sync::LazyLock;
use windows_sys::Win32::Foundation::{HWND, RECT};
use windows_sys::Win32::UI::Accessibility::{SetWinEventHook, UnhookWinEvent, HWINEVENTHOOK};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    SendInput, INPUT, INPUT_MOUSE, MOUSEEVENTF_ABSOLUTE, MOUSEEVENTF_MOVE,
    MOUSEEVENTF_MOVE_NOCOALESCE, MOUSEEVENTF_VIRTUALDESK, MOUSEINPUT,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GetClassNameW, GetSystemMetrics, GetWindowRect, IsIconic, EVENT_SYSTEM_FOREGROUND,
    SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN,
};

/// Whether the focus-center feature is enabled.
pub static FOCUS_CENTER_ENABLED: AtomicBool = AtomicBool::new(false);

/// WinEvent hook handle for the foreground hook.
static mut FOREGROUND_HOOK: HWINEVENTHOOK = 0;

/// Dedicated output thread. `SendInput` must not run on the thread that owns
/// the low-level hooks because Windows may wait for that same thread to process
/// the injected event.
static CENTER_TX: LazyLock<Option<SyncSender<HWND>>> = LazyLock::new(|| {
    let (tx, rx) = sync_channel::<HWND>(4);
    std::thread::Builder::new()
        .name("focus-center".into())
        .spawn(move || {
            while let Ok(hwnd) = rx.recv() {
                let _ = center_window(hwnd);
            }
        })
        .ok()
        .map(|_| tx)
});

pub(crate) fn init_worker() -> Result<(), String> {
    CENTER_TX
        .as_ref()
        .map(|_| ())
        .ok_or_else(|| "failed to start focus-center worker".to_string())
}

pub(crate) fn request_window(hwnd: HWND) -> bool {
    if hwnd == 0 {
        return false;
    }
    match CENTER_TX.as_ref().map(|tx| tx.try_send(hwnd)) {
        Some(Ok(())) => true,
        Some(Err(TrySendError::Full(_))) | Some(Err(TrySendError::Disconnected(_))) | None => false,
    }
}


fn normalize_absolute_coordinate(value: i32, origin: i32, span: i32) -> Option<i32> {
    let maximum = span.checked_sub(1)?;
    if maximum <= 0 {
        return None;
    }
    let relative = i64::from(value)
        .saturating_sub(i64::from(origin))
        .clamp(0, i64::from(maximum));
    let normalized = (relative * 65_535 + i64::from(maximum) / 2) / i64::from(maximum);
    Some(normalized as i32)
}

fn wide_equals_ascii(wide: &[u16], ascii: &str) -> bool {
    wide.len() == ascii.len()
        && wide
            .iter()
            .zip(ascii.bytes())
            .all(|(&wide_char, ascii_char)| wide_char == u16::from(ascii_char))
}

fn wide_starts_with_ascii(wide: &[u16], ascii: &str) -> bool {
    wide.len() >= ascii.len()
        && wide
            .iter()
            .zip(ascii.bytes())
            .all(|(&wide_char, ascii_char)| wide_char == u16::from(ascii_char))
}

fn is_skipped_window_class(class_name: &[u16]) -> bool {
    const SKIPPED_CLASSES: &[&str] = &[
        "#32768",
        "#32769",
        "WorkerW",
        "ToolTip",
        "Shell_TrayWnd",
        "MyDockAPP",
        "MyFinderApp",
        "DXWindows",
        "OperationStatusWindow",
        "TaskSlinger",
    ];

    wide_starts_with_ascii(class_name, "WinMouseFix")
        || SKIPPED_CLASSES
            .iter()
            .any(|class| wide_equals_ascii(class_name, class))
}

fn window_center(rect: &RECT) -> Option<(i32, i32)> {
    if rect.right <= rect.left || rect.bottom <= rect.top {
        return None;
    }
    let x = (i64::from(rect.left) + i64::from(rect.right)) / 2;
    let y = (i64::from(rect.top) + i64::from(rect.bottom)) / 2;
    Some((x as i32, y as i32))
}

/// Move the cursor through the same marked `SendInput` path used elsewhere in
/// the app. Absolute coordinates are normalized against the virtual desktop.
unsafe fn move_cursor(x: i32, y: i32) -> bool {
    let virtual_x = GetSystemMetrics(SM_XVIRTUALSCREEN);
    let virtual_y = GetSystemMetrics(SM_YVIRTUALSCREEN);
    let virtual_width = GetSystemMetrics(SM_CXVIRTUALSCREEN);
    let virtual_height = GetSystemMetrics(SM_CYVIRTUALSCREEN);
    let Some(normalized_x) = normalize_absolute_coordinate(x, virtual_x, virtual_width) else {
        return false;
    };
    let Some(normalized_y) = normalize_absolute_coordinate(y, virtual_y, virtual_height) else {
        return false;
    };

    let mut input = INPUT {
        r#type: INPUT_MOUSE,
        ..std::mem::zeroed()
    };
    input.Anonymous.mi = MOUSEINPUT {
        dx: normalized_x,
        dy: normalized_y,
        mouseData: 0,
        dwFlags: MOUSEEVENTF_MOVE
            | MOUSEEVENTF_ABSOLUTE
            | MOUSEEVENTF_VIRTUALDESK
            | MOUSEEVENTF_MOVE_NOCOALESCE,
        time: 0,
        dwExtraInfo: crate::win::hooks::OUR_MARKER,
    };

    SendInput(1, &input, std::mem::size_of::<INPUT>() as i32) == 1
}

pub(crate) fn center_window(hwnd: HWND) -> bool {
    if hwnd == 0 || unsafe { IsIconic(hwnd) } != 0 {
        return false;
    }

    let mut class_name = [0u16; 256];
    let class_len =
        unsafe { GetClassNameW(hwnd, class_name.as_mut_ptr(), class_name.len() as i32) };
    if class_len == 0 || is_skipped_window_class(&class_name[..class_len as usize]) {
        return false;
    }

    let mut rect = RECT {
        left: 0,
        top: 0,
        right: 0,
        bottom: 0,
    };
    if unsafe { GetWindowRect(hwnd, &mut rect) } == 0 {
        return false;
    }
    let Some((center_x, center_y)) = window_center(&rect) else {
        return false;
    };

    unsafe { move_cursor(center_x, center_y) }
}


/// WinEvent callback — fires on every foreground window change.
#[allow(non_snake_case)]
unsafe extern "system" fn foreground_callback(
    _hEventHook: HWINEVENTHOOK,
    event: u32,
    hwnd: HWND,
    _idObject: i32,
    _idChild: i32,
    _ideventThread: u32,
    _dwmEventTime: u32,
) {
    if event == EVENT_SYSTEM_FOREGROUND && FOCUS_CENTER_ENABLED.load(Ordering::Relaxed) {
        let _ = request_window(hwnd);
    }
}

/// Start the foreground WinEvent hook. Idempotent.
pub fn start() {
    if !FOCUS_CENTER_ENABLED.load(Ordering::Relaxed) {
        return;
    }
    unsafe {
        if FOREGROUND_HOOK != 0 {
            return;
        }
        let hook = SetWinEventHook(
            EVENT_SYSTEM_FOREGROUND,
            EVENT_SYSTEM_FOREGROUND,
            0isize,
            Some(foreground_callback),
            0,
            0,
            0,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn virtual_desktop_coordinates_map_both_endpoints() {
        assert_eq!(normalize_absolute_coordinate(-1920, -1920, 3840), Some(0));
        assert_eq!(
            normalize_absolute_coordinate(1919, -1920, 3840),
            Some(65_535)
        );
    }

    #[test]
    fn invalid_virtual_desktop_span_is_rejected() {
        assert_eq!(normalize_absolute_coordinate(0, 0, 0), None);
        assert_eq!(normalize_absolute_coordinate(0, 0, 1), None);
    }

    #[test]
    fn system_class_filter_uses_exact_names() {
        let taskbar: Vec<u16> = "Shell_TrayWnd".encode_utf16().collect();
        let app_with_prefix: Vec<u16> = "Shell_TrayWndClone".encode_utf16().collect();
        assert!(is_skipped_window_class(&taskbar));
        assert!(!is_skipped_window_class(&app_with_prefix));
    }

    #[test]
    fn window_center_rejects_empty_rectangles() {
        assert_eq!(
            window_center(&RECT {
                left: -100,
                top: 20,
                right: 100,
                bottom: 220,
            }),
            Some((0, 120))
        );
        assert_eq!(
            window_center(&RECT {
                left: 10,
                top: 10,
                right: 10,
                bottom: 30,
            }),
            None
        );
    }
}
