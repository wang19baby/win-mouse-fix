use std::ptr::{null, null_mut};


use windows_sys::Win32::UI::Shell::{
    NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NIM_MODIFY, NOTIFYICONDATAW,
    Shell_NotifyIconW, ShellExecuteW,
};
use windows_sys::Win32::Graphics::Gdi::{
    BeginPaint, CreateBitmap, CreateCompatibleBitmap, CreateCompatibleDC, CreateSolidBrush,
    DeleteDC, DeleteObject, DT_CENTER, DT_SINGLELINE, DT_VCENTER, DT_WORDBREAK, DrawTextW, EndPaint, FillRect,
    GetDC, GetStockObject, HBRUSH, PAINTSTRUCT, ReleaseDC, SelectObject, SetBkMode,
    SetTextColor, TRANSPARENT, WHITE_BRUSH, BLACK_BRUSH,
};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::Foundation::{POINT, RECT};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, CreateWindowExW, CreateIconFromResourceEx,
    CreateIconIndirect, DefWindowProcW, DestroyIcon,     DestroyMenu, DestroyWindow, GetClientRect,
    DrawIcon, GetCursorPos, GetSystemMetrics, ICONINFO, IDC_ARROW,
    IDI_APPLICATION, KillTimer, LoadCursorW, LoadIconW, MB_ICONINFORMATION, MB_OK,



    MessageBoxW, IDYES, MF_CHECKED, MF_SEPARATOR, MF_STRING, MF_UNCHECKED, PostQuitMessage,
    RegisterClassExW, SetForegroundWindow, SetTimer, ShowWindow, SM_CXICON, SW_SHOW,
    TrackPopupMenu, TPM_RETURNCMD, TPM_RIGHTBUTTON, WM_APP, WM_COMMAND, WM_CREATE,
    WM_DESTROY, WM_RBUTTONUP, WM_TIMER, WNDCLASSEXW, WS_CAPTION, WS_CHILD, WS_SYSMENU,



    WS_VISIBLE, MB_YESNO, MB_ICONQUESTION,
};

use windows_sys::Win32::System::DataExchange::{
    OpenClipboard, EmptyClipboard, CloseClipboard, SetClipboardData,
};
use windows_sys::Win32::System::Memory::{
    GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE,
};

const WM_TRAYICON: u32 = WM_APP + 1;
const ID_EXIT: usize = 1001;
const ID_ABOUT: usize = 1002;
const ID_SMOOTH: usize = 1003;
const ID_REMAP: usize = 1004;
const ID_ADDMODE: usize = 1005;
const ID_BATTERY: usize = 1006;
const ID_DPI: usize = 1007;

const ID_REMOTE: usize = 1008;
static FW_DECLINED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

const ID_TIMER_ADDMODE: usize = 3001;
const ID_TIMER_CONFIG: usize = 3002;
const CONFIG_RELOAD_MS: u32 = 1000;
const ID_TIMER_PROFILE: usize = 3003;
const PROFILE_POLL_MS: u32 = 500;
const ID_TIMER_BATTERY: usize = 3004;
const BATTERY_POLL_MS: u32 = 2000;
const ID_TIMER_DPI: usize = 3005;
const DPI_POLL_MS: u32 = 250;

const ABOUT_CLASS: &str = "WinMouseFixAboutClass";
const ABOUT_OK_ID: usize = 2002;
const REPO_URL: &str = "https://github.com/wang19baby/win-mouse-fix";

/// Last observed mtime of config.toml; used to detect changes for hot-reload.
static CONFIG_MTIME: parking_lot::Mutex<Option<std::time::SystemTime>> = parking_lot::Mutex::new(None);
/// Original embedded tray icon; never destroyed.
static BASE_HICON: parking_lot::Mutex<isize> = parking_lot::Mutex::new(0);
/// Currently displayed (overlay) icon; destroyed before each replacement.
static CUR_OVERLAY: parking_lot::Mutex<isize> = parking_lot::Mutex::new(0);

fn to_wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Build an `HICON` from the embedded ICO.
fn load_embedded_icon() -> isize {
    const DATA: &[u8] = include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/assets/icon.ico"));
    if DATA.len() >= 6 {
        let count = u16::from_le_bytes([DATA[4], DATA[5]]) as usize;
        let mut best: Option<(usize, usize)> = None;
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
                    CreateIconFromResourceEx(
                        slice.as_ptr(),
                        slice.len() as u32,
                        1,
                        0x0003_0000,
                        0,
                        0,
                        0,
                    )
                };
                if h != 0 {
                    return h;
                }
            }
        }
    }
    unsafe { LoadIconW(0, IDI_APPLICATION) }
}

/// Create a message-only window to host the tray icon.
pub fn create() -> Result<(), String> {
    unsafe {
        let hmod = GetModuleHandleW(null());
        let class_name = to_wide("WinMouseFixClass");
        let hicon = load_embedded_icon();
        *BASE_HICON.lock() = hicon;
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
            0isize, // hMenu — null for message-only window
            hmod,
            null_mut(),
        );
        if hwnd == 0 {
            return Err("failed to create message-only window".into());
        }

        let mut nid: windows_sys::Win32::UI::Shell::NOTIFYICONDATAW = std::mem::zeroed();
        nid.cbSize =
            std::mem::size_of::<windows_sys::Win32::UI::Shell::NOTIFYICONDATAW>() as u32;
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

        // Hot-reload: poll config.toml mtime via the tray timer (see check_reload_config).
        SetTimer(hwnd, ID_TIMER_CONFIG, CONFIG_RELOAD_MS, None);
        SetTimer(hwnd, ID_TIMER_PROFILE, PROFILE_POLL_MS, None);
        SetTimer(hwnd, ID_TIMER_BATTERY, BATTERY_POLL_MS, None);
        SetTimer(hwnd, ID_TIMER_DPI, DPI_POLL_MS, None);
        // Seed last-mtime so the first tick doesn't trigger a redundant reload.
        *CONFIG_MTIME.lock() =
            std::fs::metadata(crate::config::config_path()).ok().and_then(|m| m.modified().ok());

        crate::log::write("tray icon created");
        Ok(())
    }
}

/// Refresh the battery cache and update the tray icon + tooltip.
    unsafe fn update_battery_icon(hwnd: isize) {
        let info = crate::device::cache::update();
        let base = *BASE_HICON.lock();
        let (icon, tip) = match info {
            Some(b) => {
                let icon = make_battery_icon(base, b.percent, b.low);
                let tip = if b.low {
                    format!("⚠ 鼠标电量低 {}%", b.percent)
                } else {
                    format!(
                        "鼠标电量 {}%{}",
                        b.percent,
                        if b.charging { "（充电中）" } else { "" }
                    )
                };
                (icon, tip)
            }
            None => (base, "Win Mouse Fix".to_string()),
        };

        // Destroy the previous overlay before swapping in the new one.
        let mut ov = CUR_OVERLAY.lock();
        if *ov != 0 && *ov != base {
            DestroyIcon(*ov);
        }
        *ov = if icon != base { icon } else { 0 };
        drop(ov);

        let mut nid: NOTIFYICONDATAW = std::mem::zeroed();
        nid.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
        nid.hWnd = hwnd;
        nid.uID = 1;
        nid.uFlags = NIF_ICON | NIF_TIP;
        nid.hIcon = icon;
        let wtip = to_wide(&tip);
        let len = wtip.len().min(nid.szTip.len());
        std::ptr::copy_nonoverlapping(wtip.as_ptr(), nid.szTip.as_mut_ptr(), len);
        Shell_NotifyIconW(NIM_MODIFY, &nid);
    }

    /// Draw the battery percentage onto a copy of `base`, returning a new HICON.
    /// Falls back to `base` on any GDI failure (so the icon is never broken).
    unsafe fn make_battery_icon(base: isize, percent: u8, low: bool) -> isize {
        let size = GetSystemMetrics(SM_CXICON);
        if size <= 0 {
            return base;
        }
        let screen = GetDC(0);
        if screen == 0 {
            return base;
        }
        let mem = CreateCompatibleDC(screen);
        let bmp = CreateCompatibleBitmap(screen, size, size);
        if mem == 0 || bmp == 0 {
            if mem != 0 {
                DeleteDC(mem);
            }
            if bmp != 0 {
                DeleteObject(bmp);
            }
            ReleaseDC(0, screen);
            return base;
        }
        let old = SelectObject(mem, bmp);
        let mut rect = RECT { left: 0, top: 0, right: size, bottom: size };
        let brush = CreateSolidBrush(0x00FF_FFFF); // white
        if brush != 0 {
            FillRect(mem, &rect, brush);
            DeleteObject(brush);
        }
        DrawIcon(mem, 0, 0, base);
        let text = to_wide(&format!("{percent}"));
        SetBkMode(mem, TRANSPARENT as i32);
        // COLORREF is 0x00BBGGRR; red = 0x0000FF, black = 0.
        SetTextColor(mem, if low { 0x0000FF } else { 0x000000 });
        DrawTextW(mem, text.as_ptr(), -1, &mut rect, DT_CENTER | DT_VCENTER | DT_SINGLELINE);
        SelectObject(mem, old);

        let mask = CreateBitmap(size, size, 1, 1, std::ptr::null());
        let mut ii: ICONINFO = std::mem::zeroed();
        ii.fIcon = 1;
        ii.hbmMask = mask;
        ii.hbmColor = bmp;
        let hicon = CreateIconIndirect(&ii);
        ReleaseDC(0, screen);
        DeleteDC(mem);
        if mask != 0 {
            DeleteObject(mask);
        }
        if hicon != 0 {
            DeleteObject(bmp); // icon takes ownership
            hicon
        } else {
            if bmp != 0 {
                DeleteObject(bmp);
            }
            base
        }
    }

unsafe extern "system" fn wnd_proc(
    hwnd: isize, msg: u32, wparam: usize, lparam: isize,
) -> isize {
    match msg {
        WM_TRAYICON => {
            if lparam as u32 == WM_RBUTTONUP {
                show_menu(hwnd);
            }
            0
        }
        WM_TIMER => {
            if wparam == ID_TIMER_ADDMODE {
                poll_addmode_capture(hwnd);
            } else if wparam == ID_TIMER_CONFIG {
                check_reload_config();
            } else if wparam == ID_TIMER_PROFILE {
                crate::win::hooks::poll_foreground_profile();
            } else if wparam == ID_TIMER_BATTERY {
                update_battery_icon(hwnd);
            } else if wparam == ID_TIMER_DPI {
                crate::device::dpi::poll_dpi_autoswitch();
            }
            0
        }
        WM_COMMAND => {
            if wparam as usize == ID_ADDMODE {
                start_addmode(hwnd);
            }
            0
        }
        WM_DESTROY => {
            KillTimer(hwnd, ID_TIMER_ADDMODE);
            KillTimer(hwnd, ID_TIMER_CONFIG);
            KillTimer(hwnd, ID_TIMER_PROFILE);
            KillTimer(hwnd, ID_TIMER_BATTERY);
            KillTimer(hwnd, ID_TIMER_DPI);

            let ov = *CUR_OVERLAY.lock();
            if ov != 0 {
                DestroyIcon(ov);
            }
            PostQuitMessage(0);
            0
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

/// Poll config.toml mtime; on change reload + re-apply hooks without restart.
unsafe fn check_reload_config() {
    let path = crate::config::config_path();
    let mut last = CONFIG_MTIME.lock();
    if crate::config::config_changed(&path, &mut last) {
        drop(last);
        crate::log::write("config.toml changed — reloading");
        crate::win::hooks::reload_active_config();
    }
}

unsafe fn show_menu(hwnd: isize) {
    let menu = CreatePopupMenu();
    if menu == 0 {
        return;
    }

    let smooth_flags =
        MF_STRING | if crate::win::hooks::feature_enabled(crate::win::hooks::Feature::SmoothScroll)
        {
            MF_CHECKED
        } else {
            MF_UNCHECKED
        };
    let remap_flags =
        MF_STRING | if crate::win::hooks::feature_enabled(crate::win::hooks::Feature::ButtonRemap)
        {
            MF_CHECKED
        } else {
            MF_UNCHECKED
        };

    AppendMenuW(menu, smooth_flags, ID_SMOOTH, to_wide("平滑滚动").as_ptr());
    AppendMenuW(menu, remap_flags, ID_REMAP, to_wide("按键重映射").as_ptr());
    AppendMenuW(menu, MF_SEPARATOR, 0, null());

    let addmode_active = crate::add_mode::is_active();
    let addmode_flags =
        MF_STRING | if addmode_active { MF_CHECKED } else { MF_UNCHECKED };
    AppendMenuW(
        menu,
        addmode_flags,
        ID_ADDMODE,
        to_wide(" 录制新按键映射").as_ptr(),
    );

    AppendMenuW(menu, MF_STRING, ID_BATTERY, to_wide("电池状态").as_ptr());
    AppendMenuW(menu, MF_STRING, ID_DPI, to_wide("DPI 同步当前屏").as_ptr());
    AppendMenuW(menu, MF_STRING, ID_REMOTE, to_wide("手机妙控板").as_ptr());

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
        ID_ABOUT => {
            show_about();
        }
        ID_BATTERY => {
            crate::device::log_battery_status();
        }
        ID_DPI => {
            let cfg = crate::config::Config::load_or_default();
            let monitors = crate::device::dpi::enumerate_monitors();
            let ref_dpi = monitors
                .iter()
                .find(|m| m.primary)
                .or_else(|| monitors.first())
                .map(|m| m.dpi_x)
                .unwrap_or(96);
            // Refresh the shared DPI cache and read the computed target.
            match crate::device::cache::update_dpi(
                cfg.dpi.base_dpi,
                ref_dpi,
                cfg.dpi.min_dpi,
                cfg.dpi.max_dpi,
            ) {
                Some(state) => {
                    let applied =
                        crate::device::dpi::apply_dpi_to_first_device(state.target_hw_dpi);
                    let msg = match applied {
                        Some(actual) => format!(
                            "当前屏逻辑 DPI: {}\n基准屏 DPI: {}\n目标硬件 DPI: {} → 已写入 {}",
                            state.monitor_dpi, ref_dpi, state.target_hw_dpi, actual,
                        ),
                        None => format!(
                            "当前屏逻辑 DPI: {}\n基准屏 DPI: {}\n目标硬件 DPI: {}\n(未找到支持 DPI 的 Logitech 设备)",
                            state.monitor_dpi, ref_dpi, state.target_hw_dpi,
                        ),
                    };
                    MessageBoxW(
                        hwnd,
                        to_wide(&msg).as_ptr(),
                        to_wide("DPI 同步").as_ptr(),
                        MB_OK | MB_ICONINFORMATION,
                    );
                }
                None => {
                    MessageBoxW(
                        hwnd,
                        to_wide("未检测到显示器").as_ptr(),
                        to_wide("DPI 同步").as_ptr(),
                        MB_OK | MB_ICONINFORMATION,
                    );
                }
            }
        }
        ID_REMOTE => {
            show_remote_qr(hwnd);
        }
        _ => {}
    }
}

/// Start AddMode: enable capture and start a timer to poll for captured triggers.
unsafe fn start_addmode(hwnd: isize) {
    if crate::add_mode::enable() {
        SetTimer(hwnd, ID_TIMER_ADDMODE, 50, None);
        crate::log::write(
            "AddMode started — click, scroll or drag to capture a trigger",
        );
    }
}

/// Called every timer tick while AddMode is active.
unsafe fn poll_addmode_capture(hwnd: isize) {
    if let Some(payload) = crate::add_mode::get_payload() {
        KillTimer(hwnd, ID_TIMER_ADDMODE);
        crate::log::write(&format!(
            "AddMode captured: button={:?} scroll={} drag={} mods=0x{:x}",
            payload.button,
            payload.scroll_captured,
            payload.drag_captured,
            payload.active_mods.keyboard
        ));
        show_addmode_message(hwnd, &payload);
    }
}

/// Show a message box with captured trigger info and placeholder effect entry.
unsafe fn show_addmode_message(hwnd: isize, payload: &crate::add_mode::AddModePayload) {
    let trigger_desc = if payload.scroll_captured {
        "scroll"
    } else if payload.drag_captured {
        "drag"
    } else {
        match payload.button {
            Some(crate::remap::MouseButton::Left) => "left button",
            Some(crate::remap::MouseButton::Right) => "right button",
            Some(crate::remap::MouseButton::Middle) => "middle button",
            Some(crate::remap::MouseButton::X1) => "X1 button",
            Some(crate::remap::MouseButton::X2) => "X2 button",
            None => "unknown",
        }
    };

    let msg = format!(
        "捕获到触发器: {}\n点击次数: {}\n修饰键: 0x{:x}\n\n\
         当前效果设置为 PassThrough。\n\
         请在 config.toml 的 [[buttons.advanced]] 中编辑 effect 字段。",
        trigger_desc, payload.click_count, payload.active_mods.keyboard,
    );

    MessageBoxW(
        hwnd,
        to_wide(&msg).as_ptr(),
        to_wide("录制完成 — Win Mouse Fix").as_ptr(),
        MB_OK | MB_ICONINFORMATION,
    );

    // Log the RemapEntry for easy copy-paste into config.
    let entry =
        crate::add_mode::build_remap_entry(payload, crate::remap::Effect::PassThrough);
    crate::log::write(&format!("AddMode entry: {:?}", entry));
    let _ = crate::add_mode::disable();
}

/// Interactive About box.
fn show_about() {
    static INIT: std::sync::Once = std::sync::Once::new();
    INIT.call_once(|| unsafe {
        let hmod = GetModuleHandleW(null());
        let class_name = to_wide(ABOUT_CLASS);
        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            style: 0,
            lpfnWndProc: Some(about_wnd_proc),
            cbClsExtra: 0,
            cbWndExtra: 0,
            hInstance: hmod,
            hIcon: 0,
            hCursor: LoadCursorW(0, IDC_ARROW),
            hbrBackground: 0,
            lpszMenuName: null(),
            lpszClassName: class_name.as_ptr(),
            hIconSm: 0,
        };
        RegisterClassExW(&wc);
    });

    unsafe {
        let hwnd = CreateWindowExW(
            0,
            to_wide(ABOUT_CLASS).as_ptr(),
            to_wide("关于 Win Mouse Fix").as_ptr(),
            WS_CAPTION | WS_SYSMENU | WS_VISIBLE,
            200, 200, 380, 200,
            0, 0isize, GetModuleHandleW(null()), null_mut(),
        );
        if hwnd != 0 {
            ShowWindow(hwnd, SW_SHOW);
        }
    }
}

unsafe extern "system" fn about_wnd_proc(
    hwnd: isize, msg: u32, wparam: usize, lparam: isize,
) -> isize {
    match msg {
        WM_CREATE => {
            let hmod = GetModuleHandleW(null());
            CreateWindowExW(
                0, to_wide("Static").as_ptr(), to_wide("Win Mouse Fix").as_ptr(),
                WS_CHILD | WS_VISIBLE,
                20, 16, 320, 24,
                hwnd, 0isize, hmod, null_mut(),
            );
            let ver = format!("版本 {}", env!("CARGO_PKG_VERSION"));
            CreateWindowExW(
                0, to_wide("Static").as_ptr(), to_wide(&ver).as_ptr(),
                WS_CHILD | WS_VISIBLE,
                20, 44, 320, 20,
                hwnd, 0isize, hmod, null_mut(),
            );
            CreateWindowExW(
                0, to_wide("Static").as_ptr(),
                to_wide("Windows 按键映射与平滑滚动增强工具").as_ptr(),
                WS_CHILD | WS_VISIBLE,
                20, 68, 320, 20,
                hwnd, 0isize, hmod, null_mut(),
            );
            let link = format!("打开项目主页: {}", REPO_URL);
            CreateWindowExW(
                0, to_wide("Static").as_ptr(), to_wide(&link).as_ptr(),
                WS_CHILD | WS_VISIBLE,
                20, 92, 320, 20,
                hwnd, 0isize, hmod, null_mut(),
            );
            CreateWindowExW(
                0, to_wide("Button").as_ptr(), to_wide("确定").as_ptr(),
                WS_CHILD | WS_VISIBLE,
                150, 130, 80, 24,
                hwnd, ABOUT_OK_ID as isize, hmod, null_mut(),
            );
            0
        }
        WM_COMMAND => {
            if wparam as usize == ABOUT_OK_ID {
                DestroyWindow(hwnd);
            }
            0
        }
        WM_DESTROY => 0,
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}






/// Phase 11 — on first use, ask the user to open the firewall port for LAN access.
/// Windows blocks unsolicited inbound by default; adding the allow rule needs a
/// one-time admin grant (UAC). We prompt for consent, then elevate `netsh`.
fn parse_port(url: &str) -> u16 {
    if let Some(auth) = url.split("://").nth(1) {
        if let Some(colon) = auth.find(':') {
            let after = &auth[colon + 1..];
            let port_str = match after.find('/') {
                Some(s) => &after[..s],
                None => after,
            };
            if let Ok(p) = port_str.parse::<u16>() {
                return p;
            }
        }
    }
    18765
}

fn firewall_rule_exists(port: u16) -> bool {
    let name = format!("WinMouseFix-Trackpad-{port}");
    let out = std::process::Command::new("netsh")
        .args(["advfirewall", "firewall", "show", "rule", &format!("name={name}")])
        .output();
    match out {
        Ok(o) => o.status.success() && String::from_utf8_lossy(&o.stdout).contains(&name),
        Err(_) => false,
    }
}


fn add_firewall_rule_now(owner: isize, port: u16) {
    let name = format!("WinMouseFix-Trackpad-{port}");
    // Elevate once via UAC; netsh then adds the persistent inbound allow rule.
    // (runas is a no-op prompt when the process is already elevated.)
    let args = format!(
        "advfirewall firewall add rule name={name} dir=in action=allow protocol=TCP localport={port}"
    );
    unsafe {
        ShellExecuteW(
            owner,
            to_wide("runas").as_ptr(),
            to_wide("netsh").as_ptr(),
            to_wide(&args).as_ptr(),
            null_mut(),
            SW_SHOW,
        );
    }
}

/// Prompt once for consent, then ensure the inbound allow rule exists.
fn ensure_firewall_rule(owner: isize, port: u16) {
    if firewall_rule_exists(port) {
        return;
    }
    if FW_DECLINED.load(std::sync::atomic::Ordering::Relaxed) {
        return;
    }
    let r = unsafe {
        MessageBoxW(
            owner,
            to_wide(
                "手机妙控板需要开放防火墙端口,才能让您手机通过局域网连接。\n是否允许?(将弹出一次 Windows 用户账户控制确认)",
            )
            .as_ptr(),
            to_wide("手机妙控板").as_ptr(),
            MB_YESNO | MB_ICONQUESTION,
        )
    };
    if r == IDYES {
        add_firewall_rule_now(owner, port);
    } else {
        FW_DECLINED.store(true, std::sync::atomic::Ordering::Relaxed);
    }
}

fn show_remote_qr(_owner: isize) {
    match crate::remote::info() {

        Some(info) => {
            // Open the in-service QR page in the default browser. The page itself
            // renders the QR + copyable address, so no Win32 window is needed.
            let port = parse_port(&info.url);
            ensure_firewall_rule(_owner, port);

            let base = info
                .url
                .split('?')
                .next()
                .unwrap_or(&info.url)
                .trim_end_matches('/');
            let qr_url = format!("{}/qr", base);
            copy_text_to_clipboard(&info.url);
            let opened = unsafe {
                let r = ShellExecuteW(
                    0,
                    to_wide("open").as_ptr(),
                    to_wide(&qr_url).as_ptr(),
                    null_mut(),
                    null_mut(),
                    SW_SHOW,
                );
                r > 32
            };
            if !opened {
                unsafe {
                    MessageBoxW(
                        _owner,
                        to_wide(&format!(
                            "无法打开浏览器。连接地址已复制到剪贴板:\n{}",
                            info.url
                        ))
                        .as_ptr(),
                        to_wide("手机妙控板").as_ptr(),
                        MB_OK | MB_ICONINFORMATION,
                    );
                }
            }
        }
        None => unsafe {
            MessageBoxW(
                _owner,

                to_wide("手机妙控板服务尚未启动,请重启程序后重试。").as_ptr(),
                to_wide("手机妙控板").as_ptr(),
                MB_OK | MB_ICONINFORMATION,
            );
        },
    }
}

#[cfg(test)]
mod firewall_tests {
    use super::parse_port;

    #[test]
    fn parses_port_from_trackpad_url() {
        assert_eq!(parse_port("http://192.168.0.167:18765/?t=abc123"), 18765);
        assert_eq!(parse_port("http://10.0.0.5:18770/?t=xyz"), 18770);
    }

    #[test]
    fn falls_back_without_port() {
        assert_eq!(parse_port("http://host/?t=abc"), 18765);
        assert_eq!(parse_port("not-a-url"), 18765);
    }
}


/// Copy `text` to the clipboard as CF_UNICODETEXT (best-effort).
fn copy_text_to_clipboard(text: &str) {
    unsafe {
        if OpenClipboard(0) == 0 {
            return;
        }
        EmptyClipboard();
        let w: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
        let h = GlobalAlloc(GMEM_MOVEABLE, (w.len() * 2) as usize);
        if !h.is_null() {
            let p = GlobalLock(h) as *mut u16;
            if !p.is_null() {
                std::ptr::copy_nonoverlapping(w.as_ptr(), p, w.len());
                GlobalUnlock(h);

                SetClipboardData(13, h as isize);
            }
        }
        CloseClipboard();
    }
}


