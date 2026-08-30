//! Lightweight Win32 settings dialog — tabbed GUI for config.toml editing.
//!
//! Opens a modal dialog with 4 tabs (General / Scroll / Pointer / Buttons).
//! On OK, writes the updated config to disk and hot-applies it via `apply_config`.

use std::ptr::{null, null_mut};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Registry::{
    RegOpenKeyExW, RegSetValueExW, RegDeleteValueW, RegCloseKey,
    HKEY_CURRENT_USER, KEY_ALL_ACCESS, KEY_READ, REG_SZ,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow,
    LoadCursorW, RegisterClassExW, ShowWindow,
    WM_COMMAND, WM_CREATE, WM_DESTROY, WNDCLASSEXW,
    WS_CAPTION, WS_CHILD, WS_SYSMENU, WS_VISIBLE,
    WS_TABSTOP, WS_GROUP, WS_VSCROLL,
    MessageBoxW, PostQuitMessage, IDC_ARROW,
    SendDlgItemMessageW, GetDlgItem, SetWindowTextW, GetWindowTextW,
    LB_ADDSTRING, LB_DELETESTRING, LB_GETCURSEL, LB_SETCURSEL,
    LBS_HASSTRINGS, LBS_NOTIFY, EN_CHANGE,
};

// ─── Control IDs (every control has a unique ID) ───────────────────────────

// Tab radio buttons
const IDC_TAB_GENERAL: usize = 2001;
const IDC_TAB_SCROLL: usize = 2002;
const IDC_TAB_POINTER: usize = 2003;
const IDC_TAB_BUTTONS: usize = 2004;

// General tab (tab 0)
const IDC_CHK_START_HIDDEN: usize = 3001;
const IDC_CHK_REMOTE: usize = 3002;
const IDC_CHK_DPI_AUTO: usize = 3003;
const IDC_LBL_DPI_BASE: usize = 3004;
const IDC_EDT_DPI_BASE: usize = 3005;
const IDC_CHK_AUTOSTART: usize = 3006;

// Scroll tab (tab 1)
const IDC_CHK_SMOOTH: usize = 3101;
const IDC_LBL_SPEED: usize = 3102;
const IDC_EDT_SPEED: usize = 3103;
const IDC_LBL_STEP: usize = 3104;
const IDC_EDT_STEP: usize = 3105;
const IDC_LBL_SHIFT_SPEEDUP: usize = 3106;
const IDC_EDT_SHIFT_SPEEDUP: usize = 3107;
const IDC_LBL_STOP_SPEED: usize = 3108;
const IDC_EDT_STOP_SPEED: usize = 3109;
const IDC_LBL_BASE_MS: usize = 3110;
const IDC_EDT_BASE_MS: usize = 3111;

// Pointer tab (tab 2)
const IDC_CHK_ACCEL: usize = 3201;
const IDC_LBL_SENS: usize = 3202;
const IDC_EDT_SENS: usize = 3203;

// Buttons tab (tab 3)
const IDC_LST_REMAPS: usize = 3301;
const IDC_BTN_DELETE: usize = 3302;
const IDC_ADD_REMAP: usize = 3303;

// Dialog buttons (always visible)
const IDC_OK: usize = 4001;
const IDC_CANCEL: usize = 4002;

const SETTINGS_CLASS: &str = "WinMouseFixSettings";

// ─── Control → tab mapping ─────────────────────────────────────────────────

struct CtrlDef { id: usize, tab: usize }

const CTRL_TABLE: &[CtrlDef] = &[
    CtrlDef { id: IDC_CHK_START_HIDDEN, tab: 0 },
    CtrlDef { id: IDC_CHK_REMOTE,      tab: 0 },
    CtrlDef { id: IDC_CHK_DPI_AUTO,    tab: 0 },
    CtrlDef { id: IDC_LBL_DPI_BASE,    tab: 0 },
    CtrlDef { id: IDC_EDT_DPI_BASE,    tab: 0 },
    CtrlDef { id: IDC_CHK_AUTOSTART,   tab: 0 },
    CtrlDef { id: IDC_CHK_SMOOTH,      tab: 1 },
    CtrlDef { id: IDC_LBL_SPEED,       tab: 1 },
    CtrlDef { id: IDC_EDT_SPEED,       tab: 1 },
    CtrlDef { id: IDC_LBL_STEP,        tab: 1 },
    CtrlDef { id: IDC_EDT_STEP,        tab: 1 },
    CtrlDef { id: IDC_LBL_SHIFT_SPEEDUP, tab: 1 },
    CtrlDef { id: IDC_EDT_SHIFT_SPEEDUP, tab: 1 },
    CtrlDef { id: IDC_LBL_STOP_SPEED,  tab: 1 },
    CtrlDef { id: IDC_EDT_STOP_SPEED,  tab: 1 },
    CtrlDef { id: IDC_LBL_BASE_MS,     tab: 1 },
    CtrlDef { id: IDC_EDT_BASE_MS,     tab: 1 },
    CtrlDef { id: IDC_CHK_ACCEL,       tab: 2 },
    CtrlDef { id: IDC_LBL_SENS,        tab: 2 },
    CtrlDef { id: IDC_EDT_SENS,        tab: 2 },
    CtrlDef { id: IDC_LST_REMAPS,      tab: 3 },
    CtrlDef { id: IDC_BTN_DELETE,      tab: 3 },
    CtrlDef { id: IDC_ADD_REMAP,       tab: 3 },
];

// ─── Public API ─────────────────────────────────────────────────────────────

pub fn open_settings(parent: isize) {
    use std::sync::Once;
    static INIT: Once = Once::new();

    INIT.call_once(|| unsafe {
        let hmod = GetModuleHandleW(null());
        let class_name = to_wide(SETTINGS_CLASS);
        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            style: 0,
            lpfnWndProc: Some(settings_wnd_proc),
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
            to_wide(SETTINGS_CLASS).as_ptr(),
            to_wide("设置 — Win Mouse Fix").as_ptr(),
            WS_CAPTION | WS_SYSMENU | WS_VISIBLE,
            300, 200, 500, 400,
            parent, 0, GetModuleHandleW(null()), null_mut(),
        );
        if hwnd != 0 {
            ShowWindow(hwnd, 9); // SW_SHOWNA
        }
    }
}

// ─── Window procedure ───────────────────────────────────────────────────────

unsafe extern "system" fn settings_wnd_proc(
    hwnd: isize, msg: u32, wparam: usize, _lparam: isize,
) -> isize {
    match msg {
        WM_CREATE => {
            let hmod = GetModuleHandleW(null());

            // ── Tab radio buttons (always visible) ──
            let tabs: &[(usize, &str)] = &[
                (IDC_TAB_GENERAL, "常规"),
                (IDC_TAB_SCROLL, "滚动"),
                (IDC_TAB_POINTER, "指针"),
                (IDC_TAB_BUTTONS, "按键"),
            ];
            for (i, (id, label)) in tabs.iter().enumerate() {
                let style = (0x08u32) // BS_AUTORADIOBUTTON
                    | WS_CHILD | WS_VISIBLE
                    | if i == 0 { WS_GROUP } else { 0 }
                    | WS_TABSTOP;
                CreateWindowExW(
                    0, to_wide("Button").as_ptr(), to_wide(label).as_ptr(),
                    style,
                    20 + (i as i32) * 100, 12, 90, 22,
                    hwnd, *id as isize, hmod, null_mut(),
                );
            }

            // ── Separator ──
            CreateWindowExW(
                0, to_wide("Static").as_ptr(), null(),
                0x0010u32 | WS_CHILD | WS_VISIBLE, // SS_ETCHEDHORZ
                10, 40, 474, 2,
                hwnd, 0, hmod, null_mut(),
            );

            // ── General tab ──
            create_checkbox(hwnd, hmod, IDC_CHK_START_HIDDEN, "启动时最小化到托盘", 30, 56);
            create_checkbox(hwnd, hmod, IDC_CHK_REMOTE, "启用手势服务（手机触控板）", 30, 84);
            create_checkbox(hwnd, hmod, IDC_CHK_DPI_AUTO, "跨屏时自动调整鼠标 DPI", 30, 112);
            create_label(hwnd, hmod, IDC_LBL_DPI_BASE, "基准 DPI:", 30, 142);
            create_edit(hwnd, hmod, IDC_EDT_DPI_BASE, "800", 160, 139, 80);
            create_checkbox(hwnd, hmod, IDC_CHK_AUTOSTART, "开机自动启动", 30, 170);

            // ── Scroll tab ──
            create_checkbox(hwnd, hmod, IDC_CHK_SMOOTH, "启用平滑滚动", 30, 56);
            create_label(hwnd, hmod, IDC_LBL_SPEED, "速度:", 30, 86);
            create_edit(hwnd, hmod, IDC_EDT_SPEED, "1.0", 160, 83, 80);
            create_label(hwnd, hmod, IDC_LBL_STEP, "步长 (px):", 30, 116);
            create_edit(hwnd, hmod, IDC_EDT_STEP, "120", 160, 113, 80);
            create_label(hwnd, hmod, IDC_LBL_SHIFT_SPEEDUP, "Shift 加速:", 30, 146);
            create_edit(hwnd, hmod, IDC_EDT_SHIFT_SPEEDUP, "2.0", 160, 143, 80);
            create_label(hwnd, hmod, IDC_LBL_STOP_SPEED, "停止速度:", 30, 176);
            create_edit(hwnd, hmod, IDC_EDT_STOP_SPEED, "30", 160, 173, 80);
            create_label(hwnd, hmod, IDC_LBL_BASE_MS, "基础帧间隔 (ms):", 30, 206);
            create_edit(hwnd, hmod, IDC_EDT_BASE_MS, "8", 160, 203, 80);

            // ── Pointer tab ──
            create_checkbox(hwnd, hmod, IDC_CHK_ACCEL, "启用指针加速", 30, 56);
            create_label(hwnd, hmod, IDC_LBL_SENS, "灵敏度:", 30, 86);
            create_edit(hwnd, hmod, IDC_EDT_SENS, "1.0", 160, 83, 80);

            // ── Buttons tab ──
            // ListBox showing existing remaps.
            create_listbox(hwnd, hmod, IDC_LST_REMAPS, 30, 56, 430, 250);
            create_button(hwnd, hmod, IDC_BTN_DELETE, "删除选中", 30, 314, 90, 26);
            create_button(hwnd, hmod, IDC_ADD_REMAP, "录制新映射...", 130, 314, 120, 26);
            populate_remap_list(hwnd);

            // ── OK / Cancel (always visible) ──
            create_button(hwnd, hmod, IDC_OK, "确定", 280, 350, 90, 28);
            create_button(hwnd, hmod, IDC_CANCEL, "取消", 380, 350, 90, 28);

            load_config_to_controls(hwnd);
            show_tab(hwnd, 0);
            0
        }
        WM_COMMAND => {
            let id = wparam & 0xFFFF;

            // Tab switching.
            if id == IDC_TAB_GENERAL { show_tab(hwnd, 0); return 0; }
            if id == IDC_TAB_SCROLL { show_tab(hwnd, 1); return 0; }
            if id == IDC_TAB_POINTER { show_tab(hwnd, 2); return 0; }
            if id == IDC_TAB_BUTTONS { show_tab(hwnd, 3); return 0; }

            match id {
                IDC_OK => {
                    if let Err(e) = save_config_from_controls(hwnd) {
                        MessageBoxW(
                            hwnd,
                            to_wide(&format!("保存失败: {e}")).as_ptr(),
                            to_wide("错误").as_ptr(),
                            0x10,
                        );
                        return 0;
                    }
                    DestroyWindow(hwnd);
                }
                IDC_CANCEL => { DestroyWindow(hwnd); }
                IDC_BTN_DELETE => {
                    let sel = SendDlgItemMessageW(hwnd, IDC_LST_REMAPS as i32, LB_GETCURSEL, 0, 0);
                    if sel >= 0 {
                        // Remove from config.
                        {
                            let mut cfg = crate::CONFIG.write();
                            let idx = sel as usize;
                            if idx < cfg.buttons.advanced.len() {
                                cfg.buttons.advanced.remove(idx);
                                let _ = cfg.save();
                            }
                        }
                        // Refresh list.
                        SendDlgItemMessageW(hwnd, IDC_LST_REMAPS as i32, LB_DELETESTRING, sel as usize, 0);
                        // Select next item or last.
                        let count = SendDlgItemMessageW(hwnd, IDC_LST_REMAPS as i32, 0x018B /* LB_GETCOUNT */, 0, 0);
                        let new_sel = if count > 0 { sel.min(count - 1) } else { 0 };
                        SendDlgItemMessageW(hwnd, IDC_LST_REMAPS as i32, LB_SETCURSEL, new_sel as usize, 0);
                    }
                }
                IDC_ADD_REMAP => {
                    DestroyWindow(hwnd);
                    if let Some(p) = get_parent_hwnd(hwnd) {
                        windows_sys::Win32::UI::WindowsAndMessaging::PostMessageW(
                            p, WM_COMMAND, 1005, 0,
                        );
                    }
                }
                _ => {}
            }
            0
        }
        WM_DESTROY => { PostQuitMessage(0); 0 }
        _ => DefWindowProcW(hwnd, msg, wparam, _lparam),
    }
}

// ─── Tab visibility ─────────────────────────────────────────────────────────

unsafe fn show_tab(hwnd: isize, tab: usize) {
    // Update radio checks.
    let tabs = [IDC_TAB_GENERAL, IDC_TAB_SCROLL, IDC_TAB_POINTER, IDC_TAB_BUTTONS];
    for (i, &id) in tabs.iter().enumerate() {
        SendDlgItemMessageW(hwnd, id as i32, 0x00F1, if i == tab { 1 } else { 0 }, 0);
    }
    // Show/hide controls by tab.
    for def in CTRL_TABLE {
        if let Some(h) = getDlgItem(hwnd, def.id) {
            ShowWindow(h, if def.tab == tab { 5 } else { 0 }); // SW_SHOW=5, SW_HIDE=0
        }
    }
}

// ─── Control creation ───────────────────────────────────────────────────────

unsafe fn create_checkbox(p: isize, h: isize, id: usize, label: &str, x: i32, y: i32) {
    CreateWindowExW(0, to_wide("Button").as_ptr(), to_wide(label).as_ptr(),
        0x0003u32 | WS_CHILD | WS_VISIBLE | WS_TABSTOP,
        x, y, 400, 20, p, id as isize, h, null_mut());
}
unsafe fn create_label(p: isize, h: isize, id: usize, text: &str, x: i32, y: i32) {
    CreateWindowExW(0, to_wide("Static").as_ptr(), to_wide(text).as_ptr(),
        WS_CHILD | WS_VISIBLE, x, y, 120, 20, p, id as isize, h, null_mut());
}
unsafe fn create_edit(p: isize, h: isize, id: usize, val: &str, x: i32, y: i32, w: i32) {
    CreateWindowExW(0, to_wide("Edit").as_ptr(), to_wide(val).as_ptr(),
        WS_CHILD | WS_VISIBLE | WS_TABSTOP | 0x0080u32,
        x, y, w, 22, p, id as isize, h, null_mut());
}
unsafe fn create_button(p: isize, h: isize, id: usize, label: &str, x: i32, y: i32, w: i32, h2: i32) {
    CreateWindowExW(0, to_wide("Button").as_ptr(), to_wide(label).as_ptr(),
        WS_CHILD | WS_VISIBLE | WS_TABSTOP,
        x, y, w, h2, p, id as isize, h, null_mut());
}
unsafe fn create_listbox(p: isize, h: isize, id: usize, x: i32, y: i32, w: i32, h2: i32) {
    CreateWindowExW(0, to_wide("ListBox").as_ptr(), null(),
        (0x00010000 /* WS_BORDER */ | 0x00200000 /* WS_VSCROLL */
         | 0x00000002 /* LBS_NOTIFY */ | 0x00000040 /* LBS_HASSTRINGS */
         | WS_CHILD | WS_VISIBLE | WS_TABSTOP) as u32,
        x, y, w, h2, p, id as isize, h, null_mut());
}

/// Populate the ListBox with formatted remap entries.
unsafe fn populate_remap_list(hwnd: isize) {
    let hlist = match getDlgItem(hwnd, IDC_LST_REMAPS) { Some(h) => h, None => return };
    // Clear existing items.
    while SendDlgItemMessageW(hwnd, IDC_LST_REMAPS as i32, LB_DELETESTRING, 0, 0) > 0 {}
    let cfg = crate::CONFIG.read();
    for entry in &cfg.buttons.advanced {
        let desc = format_remap_entry(entry);
        SendDlgItemMessageW(hwnd, IDC_LST_REMAPS as i32, LB_ADDSTRING, 0, to_wide(&desc).as_ptr() as isize);
    }
}

/// Format a RemapEntry as a human-readable string for the ListBox.
fn format_remap_entry(entry: &crate::remap::RemapEntry) -> String {
    use crate::remap::{Trigger, Effect, ClickDuration, SwipeDirection, MouseButton};

    let trigger_str = match &entry.trigger {
        Trigger::Button { button, level, duration } => {
            let btn = match button {
                MouseButton::Left => "左键", MouseButton::Right => "右键",
                MouseButton::Middle => "中键", MouseButton::X1 => "X1", MouseButton::X2 => "X2",
            };
            let dur = match duration { ClickDuration::Click => "单击", ClickDuration::Hold => "长按" };
            let lvl = if *level > 1 { format!(" x{level}") } else { String::new() };
            format!("{btn}{dur}{lvl}")
        }
        Trigger::Scroll => "滚轮".to_string(),
        Trigger::Drag => "拖拽".to_string(),
    };

    let mod_str = if entry.modifiers.keyboard != 0 {
        let mut parts = Vec::new();
        if entry.modifiers.keyboard & 0x100 != 0 { parts.push("Ctrl"); }
        if entry.modifiers.keyboard & 0x200 != 0 { parts.push("Shift"); }
        if entry.modifiers.keyboard & 0x400 != 0 { parts.push("Alt"); }
        if entry.modifiers.keyboard & 0x800 != 0 { parts.push("Win"); }
        format!("+{}", parts.join("+"))
    } else {
        String::new()
    };

    let effect_str = match &entry.effect {
        Effect::PassThrough => "PassThrough",
        Effect::Disabled => "禁用",
        Effect::TaskView => "TaskView (Win+Tab)",
        Effect::ShowDesktop => "ShowDesktop (Win+D)",
        Effect::NavigationSwipe { direction: SwipeDirection::Back } => "后退",
        Effect::NavigationSwipe { direction: SwipeDirection::Forward } => "前进",
        Effect::SymbolicHotkey { .. } => "热键",
        Effect::MouseButtonClicks { button, .. } => {
            match button {
                MouseButton::X1 => "X1 点击", MouseButton::X2 => "X2 点击",
                MouseButton::Left => "左键点击", MouseButton::Right => "右键点击",
                MouseButton::Middle => "中键点击",
            }
        }
        Effect::ModifiedScroll { .. } => "修饰滚动",
        Effect::ModifiedDrag { .. } => "修饰拖拽",
        Effect::SystemDefinedEvent { .. } => "系统事件",
    };

    format!("{trigger_str}{mod_str} → {effect_str}")
}

// ─── Config I/O ─────────────────────────────────────────────────────────────

unsafe fn load_config_to_controls(hwnd: isize) {
    let cfg = crate::CONFIG.read();
    set_check(hwnd, IDC_CHK_START_HIDDEN, cfg.general.start_hidden);
    set_check(hwnd, IDC_CHK_REMOTE, cfg.remote.enabled);
    set_check(hwnd, IDC_CHK_DPI_AUTO, cfg.dpi.auto_switch);
    set_edit_text(hwnd, IDC_EDT_DPI_BASE, &cfg.dpi.base_dpi.to_string());
    set_check(hwnd, IDC_CHK_AUTOSTART, is_autostart_enabled());
    set_check(hwnd, IDC_CHK_SMOOTH, cfg.scroll.smooth);
    set_edit_text(hwnd, IDC_EDT_SPEED, &cfg.scroll.speed.to_string());
    set_edit_text(hwnd, IDC_EDT_STEP, &cfg.scroll.step.to_string());
    set_edit_text(hwnd, IDC_EDT_SHIFT_SPEEDUP, &cfg.scroll.shift_speedup.to_string());
    set_edit_text(hwnd, IDC_EDT_STOP_SPEED, &cfg.scroll.stop_speed.to_string());
    set_edit_text(hwnd, IDC_EDT_BASE_MS, &cfg.scroll.base_ms_per_step.to_string());
    set_check(hwnd, IDC_CHK_ACCEL, cfg.accel.enabled);
    set_edit_text(hwnd, IDC_EDT_SENS, &cfg.accel.sensitivity.to_string());
}

unsafe fn save_config_from_controls(hwnd: isize) -> Result<(), String> {
    {
        let mut cfg = crate::CONFIG.write();
        cfg.general.start_hidden = get_check(hwnd, IDC_CHK_START_HIDDEN);
        cfg.remote.enabled = get_check(hwnd, IDC_CHK_REMOTE);
        cfg.dpi.auto_switch = get_check(hwnd, IDC_CHK_DPI_AUTO);
        if let Ok(v) = get_edit_text(hwnd, IDC_EDT_DPI_BASE).parse::<u16>() {
            cfg.dpi.base_dpi = v;
        }
        set_autostart(get_check(hwnd, IDC_CHK_AUTOSTART));
        cfg.scroll.smooth = get_check(hwnd, IDC_CHK_SMOOTH);
        if let Ok(v) = get_edit_text(hwnd, IDC_EDT_SPEED).parse::<f64>() {
            cfg.scroll.speed = v;
        }
        if let Ok(v) = get_edit_text(hwnd, IDC_EDT_STEP).parse::<f64>() {
            cfg.scroll.step = v;
        }
        if let Ok(v) = get_edit_text(hwnd, IDC_EDT_SHIFT_SPEEDUP).parse::<f64>() {
            cfg.scroll.shift_speedup = v;
        }
        if let Ok(v) = get_edit_text(hwnd, IDC_EDT_STOP_SPEED).parse::<f64>() {
            cfg.scroll.stop_speed = v;
        }
        if let Ok(v) = get_edit_text(hwnd, IDC_EDT_BASE_MS).parse::<f64>() {
            cfg.scroll.base_ms_per_step = v;
        }
        cfg.accel.enabled = get_check(hwnd, IDC_CHK_ACCEL);
        if let Ok(v) = get_edit_text(hwnd, IDC_EDT_SENS).parse::<f64>() {
            cfg.accel.sensitivity = v;
        }
        cfg.save().map_err(|e| e.to_string())?;
    }
    let cfg = crate::CONFIG.read().clone();
    crate::win::hooks::apply_config(cfg);
    Ok(())
}

// ─── Helpers ────────────────────────────────────────────────────────────────

unsafe fn set_check(hwnd: isize, id: usize, checked: bool) {
    SendDlgItemMessageW(hwnd, id as i32, 0x00F1, if checked { 1 } else { 0 }, 0);
}
unsafe fn get_check(hwnd: isize, id: usize) -> bool {
    SendDlgItemMessageW(hwnd, id as i32, 0x00F0, 0, 0) == 1
}
unsafe fn set_edit_text(hwnd: isize, id: usize, text: &str) {
    if let Some(h) = getDlgItem(hwnd, id) {
        SetWindowTextW(h, to_wide(text).as_ptr());
    }
}
unsafe fn get_edit_text(hwnd: isize, id: usize) -> String {
    let h = match getDlgItem(hwnd, id) { Some(h) => h, None => return String::new() };
    let mut buf = [0u16; 128];
    let len = GetWindowTextW(h, buf.as_mut_ptr(), 128);
    String::from_utf16_lossy(&buf[..len as usize])
}
unsafe fn getDlgItem(hwnd: isize, id: usize) -> Option<isize> {
    let h = GetDlgItem(hwnd, id as i32);
    if h != 0 { Some(h) } else { None }
}
unsafe fn get_parent_hwnd(hwnd: isize) -> Option<isize> {
    let p = windows_sys::Win32::UI::WindowsAndMessaging::GetParent(hwnd);
    if p != 0 { Some(p) } else { None }
}
fn to_wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

// ─── Auto-start registry helpers ────────────────────────────────────────────

const AUTOSTART_KEY: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Run";
const AUTOSTART_NAME: &str = "WinMouseFix";

/// Check if the auto-start registry key is set.
fn is_autostart_enabled() -> bool {
    unsafe {
        let mut key: isize = 0;
        let wide_key = to_wide(AUTOSTART_KEY);
        if RegOpenKeyExW(HKEY_CURRENT_USER, wide_key.as_ptr(), 0, KEY_READ, &mut key) != 0 {
            return false;
        }
        let mut buf = [0u16; 260];
        let mut buf_len = (buf.len() * 2) as u32;
        let wide_name = to_wide(AUTOSTART_NAME);
        let rc = RegQueryValueExW(key, wide_name.as_ptr(), null_mut(), null_mut(), buf.as_mut_ptr() as *mut u8, &mut buf_len);
        RegCloseKey(key);
        rc == 0 && buf_len > 0
    }
}

/// Set or clear the auto-start registry key.
fn set_autostart(enabled: bool) {
    unsafe {
        let mut key: isize = 0;
        let wide_key = to_wide(AUTOSTART_KEY);
        if RegOpenKeyExW(HKEY_CURRENT_USER, wide_key.as_ptr(), 0, KEY_ALL_ACCESS, &mut key) != 0 {
            return;
        }
        let wide_name = to_wide(AUTOSTART_NAME);
        if enabled {
            // Get current exe path.
            let mut exe_buf = [0u16; 260];
            let len = windows_sys::Win32::System::LibraryLoader::GetModuleFileNameW(
                0, exe_buf.as_mut_ptr(), 260,
            );
            if len == 0 { RegCloseKey(key); return; }
            let exe_path = &exe_buf[..len as usize];
            let data_len = (exe_len(exe_path) + 2) as u32; // +2 for null terminator bytes
            RegSetValueExW(key, wide_name.as_ptr(), 0, REG_SZ, exe_path.as_ptr() as *const u8, data_len);
        } else {
            RegDeleteValueW(key, wide_name.as_ptr());
        }
        RegCloseKey(key);
    }
}

fn exe_len(buf: &[u16]) -> usize {
    buf.iter().position(|&c| c == 0).unwrap_or(buf.len()) * 2
}

use windows_sys::Win32::System::Registry::RegQueryValueExW;

// ─── Per-app profiles dialog ────────────────────────────────────────────────

const PROFILES_CLASS: &str = "WinMouseFixProfiles";
const IDC_LST_PROFILES: usize = 5001;
const IDC_EDT_EXE: usize = 5002;
const IDC_BTN_ADD: usize = 5003;
const IDC_BTN_DEL: usize = 5004;
const IDCProfiles_OK: usize = 5005;
const IDCProfiles_CANCEL: usize = 5006;

/// Open the per-app profiles management dialog.
pub fn open_profiles(parent: isize) {
    use std::sync::Once;
    static INIT: Once = Once::new();
    INIT.call_once(|| unsafe {
        let hmod = GetModuleHandleW(null());
        let class_name = to_wide(PROFILES_CLASS);
        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            style: 0,
            lpfnWndProc: Some(profiles_wnd_proc),
            cbClsExtra: 0, cbWndExtra: 0, hInstance: hmod,
            hIcon: 0, hCursor: LoadCursorW(0, IDC_ARROW),
            hbrBackground: 0, lpszMenuName: null(),
            lpszClassName: class_name.as_ptr(), hIconSm: 0,
        };
        RegisterClassExW(&wc);
    });
    unsafe {
        let hwnd = CreateWindowExW(0,
            to_wide(PROFILES_CLASS).as_ptr(),
            to_wide("应用配置文件 — Win Mouse Fix").as_ptr(),
            WS_CAPTION | WS_SYSMENU | WS_VISIBLE,
            300, 200, 460, 360,
            parent, 0, GetModuleHandleW(null()), null_mut(),
        );
        if hwnd != 0 { ShowWindow(hwnd, 9); }
    }
}

unsafe extern "system" fn profiles_wnd_proc(
    hwnd: isize, msg: u32, wparam: usize, _lparam: isize,
) -> isize {
    match msg {
        WM_CREATE => {
            let hmod = GetModuleHandleW(null());
            // Exe name input.
            create_label(hwnd, hmod, 0, "进程名 (如 chrome.exe):", 20, 16);
            CreateWindowExW(0, to_wide("Edit").as_ptr(), null(),
                0x0080u32 | WS_CHILD | WS_VISIBLE | WS_TABSTOP,
                20, 38, 280, 22, hwnd, IDC_EDT_EXE as isize, hmod, null_mut());
            create_button(hwnd, hmod, IDC_BTN_ADD, "添加", 310, 36, 60, 26);
            create_button(hwnd, hmod, IDC_BTN_DEL, "删除", 380, 36, 60, 26);
            // Profiles list.
            CreateWindowExW(0, to_wide("ListBox").as_ptr(), null(),
                (0x00010000 | 0x00200000 | 0x00000002 | 0x00000040 | WS_CHILD | WS_VISIBLE | WS_TABSTOP) as u32,
                20, 72, 420, 210, hwnd, IDC_LST_PROFILES as isize, hmod, null_mut());
            // OK / Cancel.
            create_button(hwnd, hmod, IDCProfiles_OK, "确定", 240, 300, 90, 28);
            create_button(hwnd, hmod, IDCProfiles_CANCEL, "取消", 340, 300, 90, 28);
            populate_profile_list(hwnd);
            0
        }
        WM_COMMAND => {
            let id = wparam & 0xFFFF;
            match id {
                IDC_BTN_ADD => {
                    let exe = get_edit_text(hwnd, IDC_EDT_EXE);
                    if !exe.is_empty() {
                        let mut cfg = crate::CONFIG.write();
                        cfg.profiles.push(crate::config::Profile {
                            match_type: "exe".to_string(),
                            match_exe: Some(exe),
                            match_device: None,
                            config: toml::value::Table::new(),
                        });
                        let _ = cfg.save();
                        drop(cfg);
                        populate_profile_list(hwnd);
                        set_edit_text(hwnd, IDC_EDT_EXE, "");
                    }
                }
                IDC_BTN_DEL => {
                    let sel = SendDlgItemMessageW(hwnd, IDC_LST_PROFILES as i32, LB_GETCURSEL, 0, 0);
                    if sel >= 0 {
                        let mut cfg = crate::CONFIG.write();
                        let idx = sel as usize;
                        if idx < cfg.profiles.len() {
                            cfg.profiles.remove(idx);
                            let _ = cfg.save();
                        }
                        drop(cfg);
                        populate_profile_list(hwnd);
                    }
                }
                IDCProfiles_OK => { DestroyWindow(hwnd); }
                IDCProfiles_CANCEL => { DestroyWindow(hwnd); }
                _ => {}
            }
            0
        }
        WM_DESTROY => 0,
        _ => DefWindowProcW(hwnd, msg, wparam, _lparam),
    }
}

unsafe fn populate_profile_list(hwnd: isize) {
    let hlist = match getDlgItem(hwnd, IDC_LST_PROFILES) { Some(h) => h, None => return };
    while SendDlgItemMessageW(hwnd, IDC_LST_PROFILES as i32, LB_DELETESTRING, 0, 0) > 0 {}
    let cfg = crate::CONFIG.read();
    for p in &cfg.profiles {
        let exe = p.match_exe.as_deref().unwrap_or("(any)");
        let desc = format!("{exe} → {} 项覆盖", p.config.len());
        SendDlgItemMessageW(hwnd, IDC_LST_PROFILES as i32, LB_ADDSTRING, 0, to_wide(&desc).as_ptr() as isize);
    }
}
