use std::ptr::{null, null_mut};

use windows_sys::Win32::UI::Shell::{
    NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, Shell_NotifyIconW,
};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::Foundation::POINT;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, CreateWindowExW, CreateIconFromResourceEx, DefWindowProcW, DestroyMenu, GetCursorPos,
    IDC_ARROW, IDI_APPLICATION, LoadCursorW, LoadIconW, MF_CHECKED, MF_STRING, MF_UNCHECKED, PostQuitMessage,
    RegisterClassExW, SetForegroundWindow, TrackPopupMenu, TPM_RETURNCMD, TPM_RIGHTBUTTON,
    WM_APP, WM_DESTROY, WM_RBUTTONUP, WNDCLASSEXW,
};

const WM_TRAYICON: u32 = WM_APP + 1;
const ID_EXIT: usize = 1001;
const ID_ABOUT: usize = 1002;
const ID_SMOOTH: usize = 1003;
const ID_REMAP: usize = 1004;

fn to_wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Build an `HICON` from the ICO embedded at compile time (assets/icon.ico).
/// Picks the largest image and lets Windows scale it. Falls back to the
/// default application icon if the embedded data can't be parsed.
fn load_embedded_icon() -> isize {
    const DATA: &[u8] = include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/assets/icon.ico"));
    if DATA.len() >= 6 {
        let count = u16::from_le_bytes([DATA[4], DATA[5]]) as usize;
        let mut best: Option<(usize, usize)> = None; // (offset, len)
        for i in 0..count {
            let e = 6 + i * 16;
            if e + 16 > DATA.len() {
                break;
            }
            let bytes_in_res =
                u32::from_le_bytes([DATA[e + 8], DATA[e + 9], DATA[e + 10], DATA[e + 11]]) as usize;
            let offset =
                u32::from_le_bytes([DATA[e + 12], DATA[e + 13], DATA[e + 14], DATA[e + 15]]) as usize;
            match best {
                Some((_, len)) if bytes_in_res <= len => {}
                _ => best = Some((offset, bytes_in_res)),
            }
        }
        if let Some((offset, len)) = best {
            if offset + len <= DATA.len() {
                let slice = &DATA[offset..offset + len];
                let h = unsafe {
                    CreateIconFromResourceEx(slice.as_ptr(), slice.len() as u32, 1, 0x0003_0000, 0, 0, 0)
                };
                if h != 0 {
                    return h;
                }
            }
        }
    }
    unsafe { LoadIconW(0, IDI_APPLICATION) }
}

/// Create a message-only window to host the tray icon and run the hook's
/// message pump against it.
pub fn create() -> Result<(), String> {
    unsafe {
        let hmod = GetModuleHandleW(null());
        let class_name = to_wide("WinMouseFixClass");
        let hicon = load_embedded_icon();
        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            style: 0,
            lpfnWndProc: Some(wnd_proc),
            cbClsExtra: 0,
            cbWndExtra: 0,
            hInstance: hmod,
            hIcon: hicon,
            hCursor: LoadCursorW(0, IDC_ARROW),
            hbrBackground: 0,
            lpszMenuName: null(),
            lpszClassName: class_name.as_ptr(),
            hIconSm: hicon,
        };
        RegisterClassExW(&wc);

        let hwnd = CreateWindowExW(
            0,
            class_name.as_ptr(),
            to_wide("Win Mouse Fix").as_ptr(),
            0,
            0,
            0,
            0,
            0,
            windows_sys::Win32::UI::WindowsAndMessaging::HWND_MESSAGE,
            0,
            hmod,
            null_mut(),
        );
        if hwnd == 0 {
            return Err("failed to create message-only window".into());
        }

        let mut nid: windows_sys::Win32::UI::Shell::NOTIFYICONDATAW = std::mem::zeroed();
        nid.cbSize = std::mem::size_of::<windows_sys::Win32::UI::Shell::NOTIFYICONDATAW>() as u32;
        nid.hWnd = hwnd;
        nid.uID = 1;
        nid.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP;
        nid.uCallbackMessage = WM_TRAYICON;
        nid.hIcon = hicon;
        let tip = to_wide("Win Mouse Fix");
        let len = tip.len().min(nid.szTip.len());
        std::ptr::copy_nonoverlapping(tip.as_ptr(), nid.szTip.as_mut_ptr(), len);

        if Shell_NotifyIconW(NIM_ADD, &nid) == 0 {
            return Err("failed to add tray icon".into());
        }
    }
    crate::log::write("tray icon created");
    Ok(())
}

unsafe extern "system" fn wnd_proc(hwnd: isize, msg: u32, wparam: usize, lparam: isize) -> isize {
    match msg {
        WM_TRAYICON => {
            if lparam as u32 == WM_RBUTTONUP {
                show_menu(hwnd);
            }
            0
        }
        WM_DESTROY => {
            PostQuitMessage(0);
            0
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

unsafe fn show_menu(hwnd: isize) {
    let menu = CreatePopupMenu();
    if menu == 0 {
        return;
    }

    // Checkable feature toggles, reflecting the current runtime state.
    let smooth_flags = MF_STRING
        | if crate::win::hooks::feature_enabled(crate::win::hooks::Feature::SmoothScroll) {
            MF_CHECKED
        } else {
            MF_UNCHECKED
        };
    let remap_flags = MF_STRING
        | if crate::win::hooks::feature_enabled(crate::win::hooks::Feature::ButtonRemap) {
            MF_CHECKED
        } else {
            MF_UNCHECKED
        };

    AppendMenuW(menu, smooth_flags, ID_SMOOTH, to_wide("平滑滚动").as_ptr());
    AppendMenuW(menu, remap_flags, ID_REMAP, to_wide("按键重映射").as_ptr());
    AppendMenuW(menu, MF_STRING, ID_ABOUT, to_wide("关于").as_ptr());
    AppendMenuW(menu, MF_STRING, ID_EXIT, to_wide("退出").as_ptr());
    SetForegroundWindow(hwnd);

    let mut pt: POINT = std::mem::zeroed();
    GetCursorPos(&mut pt);
    let cmd = TrackPopupMenu(
        menu,
        TPM_RIGHTBUTTON | TPM_RETURNCMD,
        pt.x,
        pt.y,
        0,
        hwnd,
        null_mut(),
    );
    DestroyMenu(menu);

    match cmd as usize {
        ID_SMOOTH => {
            crate::win::hooks::toggle_feature(crate::win::hooks::Feature::SmoothScroll)
        }
        ID_REMAP => {
            crate::win::hooks::toggle_feature(crate::win::hooks::Feature::ButtonRemap)
        }
        ID_EXIT => {
            let mut nid: windows_sys::Win32::UI::Shell::NOTIFYICONDATAW = std::mem::zeroed();
            nid.cbSize =
                std::mem::size_of::<windows_sys::Win32::UI::Shell::NOTIFYICONDATAW>() as u32;
            nid.hWnd = hwnd;
            nid.uID = 1;
            Shell_NotifyIconW(NIM_DELETE, &nid);
            PostQuitMessage(0);
        }
        _ => {}
    }
}
