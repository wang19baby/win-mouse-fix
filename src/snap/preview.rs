//! Snap Zone Preview Overlay.
//!
//! Renders a semi-transparent overlay window showing the snap target region
//! during window drag. Uses a layered topmost window with per-pixel alpha.

use std::sync::atomic::{AtomicBool, Ordering};
use parking_lot::Mutex;
use windows_sys::Win32::Foundation::{
    HWND, HANDLE, LPARAM, LRESULT, POINT as WIN_POINT, RECT, SIZE as WIN_SIZE, WPARAM,
};
use windows_sys::Win32::Graphics::Gdi::{
    CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, GetDC, ReleaseDC, SelectObject,
    BITMAPINFO, BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS, HBITMAP, HBRUSH, HDC,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, RegisterClassExW, SetWindowPos, ShowWindow,
    UpdateLayeredWindow, CS_HREDRAW, CS_VREDRAW, SWP_NOACTIVATE, SWP_NOZORDER,
    WM_DESTROY, WM_PAINT, WNDCLASSEXW,
    WS_EX_LAYERED, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
};

use super::SnapResult;

/// RGBA color for the preview overlay.
const PREVIEW_COLOR_R: u8 = 0x33;
const PREVIEW_COLOR_G: u8 = 0x99;
const PREVIEW_COLOR_B: u8 = 0xFF;
const PREVIEW_ALPHA: u8 = 80; // ~31% opacity

// ─── Global state ─────────────────────────────────────────────────────────────

static OVERLAY_VISIBLE: AtomicBool = AtomicBool::new(false);
static OVERLAY_HWND: std::sync::LazyLock<Mutex<Option<HWND>>> =
    std::sync::LazyLock::new(|| Mutex::new(None));

// ─── Window class registration ────────────────────────────────────────────────

unsafe extern "system" fn overlay_wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_PAINT => {
            let mut ps = std::mem::zeroed::<PAINTSTRUCT>();
            let hdc = BeginPaint(hwnd, &mut ps);
            if hdc != 0 {
                let color = ((PREVIEW_COLOR_R as u32) << 0)
                    | ((PREVIEW_COLOR_G as u32) << 8)
                    | ((PREVIEW_COLOR_B as u32) << 16);
                let brush = CreateSolidBrush(color);
                if brush != 0 {
                    let mut rc = std::mem::zeroed::<RECT>();
                    GetClientRect(hwnd, &mut rc);
                    FillRect(hdc, &rc, brush);
                    DeleteObject(brush as isize);
                }
                EndPaint(hwnd, &ps);
            }
            0
        }
        WM_DESTROY => 0,
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

#[repr(C)]
struct PAINTSTRUCT {
    hdc: i64,
    f_erase: i32,
    rc_paint: RECT,
    f_restore: i32,
    f_inc_update: i32,
    rgb_reserved: [u8; 32],
}

#[link(name = "user32")]
extern "system" {
    fn BeginPaint(hwnd: HWND, lpPaint: *mut PAINTSTRUCT) -> i64;
    fn EndPaint(hwnd: HWND, lpPaint: *const PAINTSTRUCT) -> i32;
    fn FillRect(hdc: i64, lprc: *const RECT, hbr: HBRUSH) -> i32;
    fn GetClientRect(hwnd: HWND, lpRect: *mut RECT) -> i32;
    fn CreateSolidBrush(color: u32) -> HBRUSH;
}

fn init_overlay_class() -> bool {
    static REGISTERED: AtomicBool = AtomicBool::new(false);
    if REGISTERED.load(Ordering::SeqCst) {
        return true;
    }

    let class_name: Vec<u16> = "AltSnapPreview\0".encode_utf16().collect();
    let wc = WNDCLASSEXW {
        cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
        style: CS_HREDRAW | CS_VREDRAW,
        lpfnWndProc: Some(overlay_wnd_proc),
        cbClsExtra: 0,
        cbWndExtra: 0,
        hInstance: 0,
        hIcon: 0,
        hCursor: 0,
        hbrBackground: 0,
        lpszMenuName: std::ptr::null(),
        lpszClassName: class_name.as_ptr() as *const u16,
        hIconSm: 0,
    };

    let result = unsafe { RegisterClassExW(&wc) };
    let ok = result != 0; // 0 means failure
    if ok {
        REGISTERED.store(true, Ordering::SeqCst);
    }
    ok
}

/// Show a preview overlay for the given snap result rect.
pub fn show_preview(result: &SnapResult) {
    if !OVERLAY_VISIBLE.load(Ordering::SeqCst) {
        show_overlay_window(result);
        OVERLAY_VISIBLE.store(true, Ordering::SeqCst);
    } else {
        update_overlay_position(result);
    }
}

fn show_overlay_window(result: &SnapResult) {
    if !init_overlay_class() {
        return;
    }

    let class_name: Vec<u16> = "AltSnapPreview\0".encode_utf16().collect();
    let hwnd = unsafe {
        CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_LAYERED,
            class_name.as_ptr() as *const u16,
            std::ptr::null(),
            WS_POPUP,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            std::ptr::null(),
        )
    };

    if hwnd == 0 {
        return;
    }

    *OVERLAY_HWND.lock() = Some(hwnd);
    update_overlay_position(result);

    unsafe {
        ShowWindow(hwnd, 1);
    }
}

fn update_overlay_position(result: &SnapResult) {
    let hwnd = *OVERLAY_HWND.lock();
    let Some(hwnd) = hwnd else { return };

    let rect = result.target;
    let width = rect.right - rect.left;
    let height = rect.bottom - rect.top;

    if width <= 0 || height <= 0 {
        return;
    }

    unsafe {
        // Position and size
        SetWindowPos(
            hwnd,
            0,
            rect.left,
            rect.top,
            width,
            height,
            SWP_NOACTIVATE | SWP_NOZORDER,
        );

        let screen_dc: HDC = GetDC(0);
        if screen_dc == 0 {
            return;
        }

        let mem_dc: HDC = CreateCompatibleDC(screen_dc);

        let mut bmi: BITMAPINFO = std::mem::zeroed();
        bmi.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
        bmi.bmiHeader.biWidth = width;
        bmi.bmiHeader.biHeight = -height; // top-down bitmap
        bmi.bmiHeader.biPlanes = 1;
        bmi.bmiHeader.biBitCount = 32;
        bmi.bmiHeader.biCompression = BI_RGB;

        let mut bits_ptr: *mut std::ffi::c_void = std::ptr::null_mut();
        let hbm: HBITMAP =
            CreateDIBSection(mem_dc, &bmi, DIB_RGB_COLORS, &mut bits_ptr, 0 as HANDLE, 0);

        if hbm != 0 && !bits_ptr.is_null() {
            let old_bmp = SelectObject(mem_dc, hbm);

            // Fill BGRA pixels: B | G | R | A
            let num_pixels = ((width * height) as usize).max(1);
            let pixels = std::slice::from_raw_parts_mut(bits_ptr as *mut u32, num_pixels);
            for pixel in pixels {
                *pixel = ((PREVIEW_ALPHA as u32) << 24)
                    | ((PREVIEW_COLOR_B as u32) << 16)
                    | ((PREVIEW_COLOR_G as u32) << 8)
                    | (PREVIEW_COLOR_R as u32);
            }

            let window_dc: HDC = GetDC(hwnd);

            let dst_origin = WIN_POINT { x: rect.left, y: rect.top };
            let dst_size = WIN_SIZE { cx: width, cy: height };
            let src_origin = WIN_POINT { x: 0, y: 0 };

            UpdateLayeredWindow(
                hwnd,
                screen_dc,
                &dst_origin,
                &dst_size,
                mem_dc,
                &src_origin,
                0,
                std::ptr::null_mut(),
                0,
            );

            SelectObject(mem_dc, old_bmp);
            DeleteObject(hbm as isize);
            ReleaseDC(hwnd, window_dc);
        }

        DeleteDC(mem_dc);
        ReleaseDC(0, screen_dc);
    }
}

/// Hide the preview overlay.
pub fn hide_preview() {
    if OVERLAY_VISIBLE.load(Ordering::SeqCst) {
        OVERLAY_VISIBLE.store(false, Ordering::SeqCst);
        if let Some(hwnd) = OVERLAY_HWND.lock().take() {
            unsafe {
                // SWP_HIDEWINDOW = 0x0001
                SetWindowPos(
                    hwnd,
                    0,
                    0,
                    0,
                    0,
                    0,
                    SWP_NOACTIVATE | SWP_NOZORDER | 0x0001,
                );
            }
        }
    }
}

/// Returns true if the preview overlay is currently visible.
#[allow(dead_code)]
pub fn is_visible() -> bool {
    OVERLAY_VISIBLE.load(Ordering::SeqCst)
}

/// Destroy the preview overlay window (call on shutdown).
pub fn destroy() {
    hide_preview();
}
