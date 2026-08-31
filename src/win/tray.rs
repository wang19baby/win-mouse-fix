use std::ptr::{null, null_mut};
use std::sync::OnceLock;


use windows_sys::Win32::UI::Shell::{
    NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NIM_MODIFY, NOTIFYICONDATAW,
    Shell_NotifyIconW, ShellExecuteW,
};
use windows_sys::Win32::Graphics::Gdi::{
    CreateBitmap, CreateCompatibleBitmap, CreateCompatibleDC, CreateSolidBrush,
    DeleteDC, DeleteObject, DT_CENTER, DT_SINGLELINE, DT_VCENTER, DrawTextW, FillRect,
    GetDC, ReleaseDC, SelectObject, SetBkMode,
    SetTextColor, TRANSPARENT,
};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::Foundation::{POINT, RECT};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, CreateWindowExW, CreateIconFromResourceEx,
    CreateIconIndirect, DefWindowProcW, DestroyIcon, DestroyMenu, DestroyWindow,
    DrawIcon, GetCursorPos, GetSystemMetrics, ICONINFO, IDC_ARROW,
    IDI_APPLICATION, KillTimer, LoadCursorW, LoadIconW, MB_ICONINFORMATION, MB_OK,
    MessageBoxW, IDYES, MF_CHECKED, MF_SEPARATOR, MF_STRING, MF_UNCHECKED, PostQuitMessage,
    RegisterClassExW, SetForegroundWindow, SetTimer, ShowWindow, SM_CXICON, SW_SHOW,
    TrackPopupMenu, TPM_RETURNCMD, TPM_RIGHTBUTTON, WM_APP, WM_COMMAND, WM_CREATE,
    WM_DESTROY, WM_RBUTTONUP, WM_TIMER, WNDCLASSEXW, WS_CAPTION, WS_CHILD, WS_SYSMENU,
    WS_VISIBLE, MB_YESNO, MB_ICONQUESTION,
    BS_AUTORADIOBUTTON, WS_GROUP, WS_TABSTOP,
    GetMessageW, TranslateMessage, DispatchMessageW,
    SendDlgItemMessageW, BM_GETCHECK,
};

const BST_CHECKED: u32 = 0x0001;

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
const ID_SETTINGS: usize = 1009;
const ID_PROFILES: usize = 1010;
const ID_HELP: usize = 1011;
static FW_DECLINED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

const ID_TIMER_ADDMODE: usize = 3001;
const ID_TIMER_CONFIG: usize = 3002;
const CONFIG_RELOAD_MS: u32 = 1000;
const ID_TIMER_PROFILE: usize = 3003;
const PROFILE_POLL_MS: u32 = 500;
const ID_TIMER_BATTERY_ICON: usize = 3004;
const BATTERY_ICON_MS: u32 = 5000; // refresh tray icon from cache every 5s
const ID_TIMER_DPI: usize = 3005;
const DPI_POLL_MS: u32 = 250;

const ABOUT_CLASS: &str = "WinMouseFixAboutClass";
const ADDMODE_DIALOG_CLASS: &str = "WinMouseFixAddModeDialog";

// AddMode dialog control IDs (must not collide with menu IDs).
const IDC_EFFECT_PROMPT: usize = 2001;
const IDC_RADIO_PASSTHROUGH: usize = 2002;
const IDC_RADIO_NAVSWIPE: usize = 2003;
const IDC_RADIO_TASKVIEW: usize = 2004;
const IDC_RADIO_SHOWDESKTOP: usize = 2005;
const IDC_RADIO_XCLICK: usize = 2006;
const IDC_OK: usize = 2007;
const IDC_CANCEL: usize = 2008;

/// Selected effect after dialog closes. Static because the dialog runs in its own modal loop.
static SELECTED_EFFECT: std::sync::OnceLock<std::sync::Mutex<Option<crate::remap::Effect>>> =
    std::sync::OnceLock::new();
const ABOUT_OK_ID: usize = 2002;
const REPO_URL: &str = "https://github.com/wang19baby/win-mouse-fix";

/// Last observed mtime of config.toml; used to detect changes for hot-reload.
static TRAY_HWND: OnceLock<windows_sys::Win32::Foundation::HWND> = OnceLock::new();

/// HWND of the tray message-only window. Used as the anchor for the ClickCycle
/// tick timer so WM_TIMER is dispatched to `wnd_proc` (a low-level mouse hook
/// callback never receives WM_TIMER).
pub(crate) fn hwnd() -> windows_sys::Win32::Foundation::HWND {
    *TRAY_HWND
        .get()
        .unwrap_or(&windows_sys::Win32::Foundation::HWND::default())
}

static CONFIG_MTIME: parking_lot::Mutex<Option<std::time::SystemTime>> = parking_lot::Mutex::new(None);
/// Original embedded tray icon; never destroyed.
static BASE_HICON: parking_lot::Mutex<isize> = parking_lot::Mutex::new(0);
/// Currently displayed (overlay) icon; destroyed before each replacement.
static CUR_OVERLAY: parking_lot::Mutex<isize> = parking_lot::Mutex::new(0);

pub fn to_wide(s: &str) -> Vec<u16> {
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
        let _ = TRAY_HWND.set(hwnd);

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
        SetTimer(hwnd, ID_TIMER_BATTERY_ICON, BATTERY_ICON_MS, None);
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
        // Just read from the cache — the bg poll thread updates it periodically.
        let info = *crate::device::cache::BATTERY.read();
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
        // Push the latest status (PC battery, etc.) to any connected phone so
        // its HUD updates without a reconnect.
        crate::remote::broadcast_status();
    }
    /// Draw the battery percentage onto a copy of `base`, returning a new HICON.
    /// Falls back to `base` on any GDI failure (so the icon is never broken).
    unsafe fn make_battery_icon(base: isize, percent: u8, _low: bool) -> isize {
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
            if mem != 0 { DeleteDC(mem); }
            if bmp != 0 { DeleteObject(bmp); }
            ReleaseDC(0, screen);
            return base;
        }
        let old = SelectObject(mem, bmp);

        // Draw the base mouse icon first.
        DrawIcon(mem, 0, 0, base);

        // Draw a small battery bar at the bottom (2px tall, 60% width).
        let bar_w = size * 6 / 10;
        let bar_h = 2i32;
        let bar_x = (size - bar_w) / 2;
        let bar_y = size - bar_h - 1;
        let bar_color: u32 = if percent > 50 { 0x0000CC00 }  // green
            else if percent > 20 { 0x0000CCFF }  // yellow (0x00BBGGRR)
            else { 0x000000FF };  // red
        let bar_brush = CreateSolidBrush(bar_color);
        if bar_brush != 0 {
            let bar_rect = RECT { left: bar_x, top: bar_y, right: bar_x + bar_w, bottom: bar_y + bar_h };
            FillRect(mem, &bar_rect, bar_brush);
            DeleteObject(bar_brush);
        }

        // Draw percentage text with dark outline for readability on any taskbar.
        let text = to_wide(&format!("{percent}"));
        SetBkMode(mem, TRANSPARENT as i32);
        let mut rect = RECT { left: 0, top: 0, right: size, bottom: size - bar_h - 2 };
        // Dark outline: draw at 4 offsets.
        SetTextColor(mem, 0x00000000); // black
        for (dx, dy) in [(-1, 0), (1, 0), (0, -1), (0, 1)] {
            let mut r = rect;
            r.left += dx; r.right += dx;
            r.top += dy; r.bottom += dy;
            DrawTextW(mem, text.as_ptr(), -1, &mut r, DT_CENTER | DT_VCENTER | DT_SINGLELINE);
        }
        // White text on top.
        SetTextColor(mem, 0x00FFFFFF);
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
        if mask != 0 { DeleteObject(mask); }
        if hicon != 0 {
            DeleteObject(bmp);
            hicon
        } else {
            if bmp != 0 { DeleteObject(bmp); }
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
            } else if wparam == ID_TIMER_BATTERY_ICON {
                update_battery_icon(hwnd);
            } else if wparam == ID_TIMER_DPI {
                crate::device::dpi::poll_dpi_autoswitch();
            } else if wparam == crate::win::hooks::CLICK_TIMER_ID {
                crate::win::hooks::run_click_tick();
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
            KillTimer(hwnd, ID_TIMER_BATTERY_ICON);
            KillTimer(hwnd, ID_TIMER_DPI);
            KillTimer(hwnd, crate::win::hooks::CLICK_TIMER_ID);

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

    crate::win::hooks::MENU_ACTIVE.store(true, std::sync::atomic::Ordering::SeqCst);

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
    // Always show the trackpad entry. The server is bound to a specific LAN IP
    // and only opened on first click (no LAN listener / firewall rule unless
    // the user explicitly opts in). RemoteConfig.enabled is not consulted here
    // so the menu is discoverable even with the default-off setting.
    AppendMenuW(menu, MF_STRING, ID_REMOTE, to_wide("手机妙控板").as_ptr());
    AppendMenuW(menu, MF_SEPARATOR, 0, null());
    AppendMenuW(menu, MF_STRING, ID_SETTINGS, to_wide("设置").as_ptr());
    AppendMenuW(menu, MF_STRING, ID_PROFILES, to_wide("配置文件").as_ptr());
    AppendMenuW(menu, MF_STRING, ID_HELP, to_wide("帮助").as_ptr());
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
    crate::win::hooks::MENU_ACTIVE.store(false, std::sync::atomic::Ordering::SeqCst);
    DestroyMenu(menu);

    match cmd as usize {
        ID_SMOOTH => {
            crate::win::hooks::toggle_feature(crate::win::hooks::Feature::SmoothScroll)
        }
        ID_REMAP => {
            crate::win::hooks::toggle_feature(crate::win::hooks::Feature::ButtonRemap)
        }
        ID_EXIT => {
            let result = MessageBoxW(
                hwnd,
                to_wide("确定要退出 Win Mouse Fix 吗？").as_ptr(),
                to_wide("退出确认").as_ptr(),
                0x00000024, // MB_YESNO | MB_ICONQUESTION
            );
            if result == IDYES {
                let mut nid: windows_sys::Win32::UI::Shell::NOTIFYICONDATAW = std::mem::zeroed();
                nid.cbSize =
                    std::mem::size_of::<windows_sys::Win32::UI::Shell::NOTIFYICONDATAW>() as u32;
                nid.hWnd = hwnd;
                nid.uID = 1;
                Shell_NotifyIconW(NIM_DELETE, &nid);
                PostQuitMessage(0);
            }
        }
        ID_ABOUT => {
            show_about();
        }
        ID_HELP => {
            show_help();
        }
        ID_SETTINGS => {
            crate::gui::open_settings(hwnd);
        }
        ID_PROFILES => {
            crate::gui::open_profiles(hwnd);
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

/// Show effect selection dialog and save the chosen mapping to config.toml.
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

    // Show effect selection dialog (modal — blocks until user chooses).
    let chosen_effect = show_effect_dialog(payload);

    match chosen_effect {
        Some(effect) => {
            let entry = crate::add_mode::build_remap_entry(payload, effect.clone());
            crate::log::write(&format!("AddMode entry: {:?}", entry));

            // Write to config.toml.
            match crate::add_mode::save_to_config(&entry) {
                Ok(()) => {
                    let msg = format!(
                        "映射已保存到 config.toml\n\n\
                         触发器: {}\n点击次数: {}\n修饰键: 0x{:x}\n\n\
                         重启程序后生效。",
                        trigger_desc, payload.click_count, payload.active_mods.keyboard,
                    );
                    MessageBoxW(
                        hwnd,
                        to_wide(&msg).as_ptr(),
                        to_wide("录制完成 — Win Mouse Fix").as_ptr(),
                        MB_OK | MB_ICONINFORMATION,
                    );
                }
                Err(e) => {
                    let msg = format!("保存失败: {e}\n\n已记录到日志，请手动添加到 config.toml。");
                    MessageBoxW(
                        hwnd,
                        to_wide(&msg).as_ptr(),
                        to_wide("录制完成 — Win Mouse Fix").as_ptr(),
                        MB_OK | MB_ICONINFORMATION,
                    );
                }
            }
        }
        None => {
            crate::log::write("AddMode: user cancelled effect selection");
        }
    }

    let _ = crate::add_mode::disable();
}

/// Interactive About box.
fn show_help() {
    let help_text = "\
Win Mouse Fix 使用说明

基本操作：
• 滚轮：平滑滚动（可关闭）
• 中键双击：切换虚拟桌面
• 按键重映射：在「设置」中配置

手机妙控板：
1. 确保手机和电脑在同一局域网
2. 扫描托盘二维码连接
3. 手指在手机屏幕上滑动 = 移动光标
4. 单指点击 = 左键，双指点击 = 右键
5. 双指滑动 = 滚动
6. 三指上滑 = 任务视图
7. 点击 🎤 按钮可语音输入文字到 PC

快捷手势：
• 三指上滑：任务视图 (Win+Tab)
• 四指下滑：显示桌面 (Win+D)
• 三/四指左右滑：切换应用/虚拟桌面

配置文件：
• 右键托盘 → 设置 → 编辑 config.toml
• 支持按应用自动切换配置

问题反馈：
• 日志文件：config.toml 同目录下的 win-mouse-fix.log
• 崩溃日志：exe 同目录下的 crash.log";
    unsafe {
        MessageBoxW(
            0,
            to_wide(help_text).as_ptr(),
            to_wide("Win Mouse Fix 帮助").as_ptr(),
            0x00000040, // MB_OK | MB_ICONINFORMATION
        );
    }
}

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

// ─── AddMode effect selection dialog ──────────────────────────────────────────

/// Show a modal dialog letting the user choose which effect to assign to the
/// captured trigger. Returns the selected `Effect`, or `None` on cancel.
unsafe fn show_effect_dialog(_payload: &crate::add_mode::AddModePayload) -> Option<crate::remap::Effect> {
    // Register dialog class once.
    static INIT: std::sync::Once = std::sync::Once::new();
    INIT.call_once(|| {
        let hmod = GetModuleHandleW(null());
        let class_name = to_wide(ADDMODE_DIALOG_CLASS);
        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            style: 0,
            lpfnWndProc: Some(addmode_dialog_proc),
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

    // Initialize selection state.
    let sel = SELECTED_EFFECT.get_or_init(|| std::sync::Mutex::new(None));
    *sel.lock().unwrap() = Some(crate::remap::Effect::PassThrough);

    let hwnd = CreateWindowExW(
        0,
        to_wide(ADDMODE_DIALOG_CLASS).as_ptr(),
        to_wide("选择效果 — Win Mouse Fix").as_ptr(),
        WS_CAPTION | WS_SYSMENU | WS_VISIBLE,
        300, 250, 420, 300,
        0, 0isize, GetModuleHandleW(null()), null_mut(),
    );
    if hwnd == 0 {
        return None;
    }
    ShowWindow(hwnd, SW_SHOW);

    // Modal message loop — blocks until the dialog is closed.
    let mut msg: windows_sys::Win32::UI::WindowsAndMessaging::MSG = std::mem::zeroed();
    while GetMessageW(&mut msg, hwnd, 0, 0) > 0 {
        let _ = TranslateMessage(&msg);
        DispatchMessageW(&msg);
    }

    sel.lock().unwrap().take()
}

unsafe extern "system" fn addmode_dialog_proc(
    hwnd: isize, msg: u32, wparam: usize, _lparam: isize,
) -> isize {
    match msg {
        WM_CREATE => {
            let hmod = GetModuleHandleW(null());

            // Prompt text.
            CreateWindowExW(
                0, to_wide("Static").as_ptr(),
                to_wide("捕获到触发器 — 选择要映射的效果:").as_ptr(),
                WS_CHILD | WS_VISIBLE | 0, // SS_LEFT = 0
                20, 16, 380, 20,
                hwnd, IDC_EFFECT_PROMPT as isize, hmod, null_mut(),
            );

            // Radio buttons.
            let radios: &[(usize, &str)] = &[
                (IDC_RADIO_PASSTHROUGH, "PassThrough（不映射）"),
                (IDC_RADIO_NAVSWIPE,   "NavigationSwipe（前进/后退）"),
                (IDC_RADIO_TASKVIEW,   "TaskView（Win+Tab）"),
                (IDC_RADIO_SHOWDESKTOP,"ShowDesktop（Win+D）"),
                (IDC_RADIO_XCLICK,     "XButton Click（X1/X2 点击）"),
            ];
            for (i, (id, label)) in radios.iter().enumerate() {
                let style = (BS_AUTORADIOBUTTON as u32) | WS_CHILD | WS_VISIBLE
                    | if i == 0 { WS_GROUP } else { 0 }
                    | WS_TABSTOP;
                CreateWindowExW(
                    0, to_wide("Button").as_ptr(), to_wide(label).as_ptr(),
                    style,
                    30, 48 + (i as i32) * 28, 360, 24,
                    hwnd, *id as isize, hmod, null_mut(),
                );
            }

            // OK button.
            CreateWindowExW(
                0, to_wide("Button").as_ptr(), to_wide("确定").as_ptr(),
                WS_CHILD | WS_VISIBLE | WS_TABSTOP,
                120, 210, 80, 28,
                hwnd, IDC_OK as isize, hmod, null_mut(),
            );
            // Cancel button.
            CreateWindowExW(
                0, to_wide("Button").as_ptr(), to_wide("取消").as_ptr(),
                WS_CHILD | WS_VISIBLE | WS_TABSTOP,
                220, 210, 80, 28,
                hwnd, IDC_CANCEL as isize, hmod, null_mut(),
            );

            0
        }
        WM_COMMAND => {
            let id = wparam & 0xFFFF;
            match id {
                IDC_OK => {
                    // Read which radio is checked via BM_GETCHECK.
                    let checked = |id: usize| -> bool {
                        SendDlgItemMessageW(hwnd, id as i32, BM_GETCHECK, 0, 0) as u32 == BST_CHECKED
                    };
                    let effect = if checked(IDC_RADIO_NAVSWIPE) {
                        crate::remap::Effect::NavigationSwipe {
                            direction: crate::remap::SwipeDirection::Back,
                        }
                    } else if checked(IDC_RADIO_TASKVIEW) {
                        crate::remap::Effect::TaskView
                    } else if checked(IDC_RADIO_SHOWDESKTOP) {
                        crate::remap::Effect::ShowDesktop
                    } else if checked(IDC_RADIO_XCLICK) {
                        crate::remap::Effect::MouseButtonClicks {
                            button: crate::remap::MouseButton::X1,
                            n_of_clicks: 1,
                        }
                    } else {
                        crate::remap::Effect::PassThrough
                    };
                    *SELECTED_EFFECT.get_or_init(|| std::sync::Mutex::new(None)).lock().unwrap() = Some(effect);
                    DestroyWindow(hwnd);
                }
                IDC_CANCEL => {
                    *SELECTED_EFFECT.get_or_init(|| std::sync::Mutex::new(None)).lock().unwrap() = None;
                    DestroyWindow(hwnd);
                }
                _ => {}
            }
            0
        }
        WM_DESTROY => {
            PostQuitMessage(0);
            0
        }
        _ => DefWindowProcW(hwnd, msg, wparam, _lparam),
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
    // Lazy-start the server on first menu click. Default-off setting keeps
    // the app from binding a LAN port until the user explicitly opens this
    // menu item; once started the server stays up for the rest of the
    // process lifetime.
    if crate::remote::info().is_none() {
        crate::remote::start_server();
    }
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
                to_wide(
                    "手机妙控板服务启动失败。\n请检查防火墙设置或日志。",
                )
                .as_ptr(),
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


