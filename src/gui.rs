//! Lightweight Win32 settings dialog — tabbed GUI for config.toml editing.
//!
//! Opens a modal dialog with 4 tabs (General / Scroll / Pointer / Buttons).
//! On OK, writes the updated config to disk and hot-applies it via `apply_config`.

use std::ptr::{null, null_mut};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow,
    LoadCursorW, RegisterClassExW, ShowWindow,
    WM_COMMAND, WM_CREATE, WM_DESTROY, WNDCLASSEXW,
    WS_CAPTION, WS_CHILD, WS_SYSMENU, WS_VISIBLE,
    WS_TABSTOP, WS_GROUP,
    MessageBoxW, PostQuitMessage, IDC_ARROW,
    SendDlgItemMessageW, GetDlgItem, SetWindowTextW, GetWindowTextW,
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

// Scroll tab (tab 1)
const IDC_CHK_SMOOTH: usize = 3101;
const IDC_LBL_DURATION: usize = 3102;
const IDC_EDT_DURATION: usize = 3103;
const IDC_LBL_DISTANCE: usize = 3104;
const IDC_EDT_DISTANCE: usize = 3105;

// Pointer tab (tab 2)
const IDC_CHK_ACCEL: usize = 3201;
const IDC_LBL_SENS: usize = 3202;
const IDC_EDT_SENS: usize = 3203;

// Buttons tab (tab 3)
const IDC_LBL_REMAP_COUNT: usize = 3301;
const IDC_ADD_REMAP: usize = 3302;

// Dialog buttons (always visible)
const IDC_OK: usize = 4001;
const IDC_CANCEL: usize = 4002;

const SETTINGS_CLASS: &str = "WinMouseFixSettings";

// ─── Control → tab mapping ─────────────────────────────────────────────────

struct CtrlDef { id: usize, tab: usize }

const CTRL_TABLE: &[CtrlDef] = &[
    CtrlDef { id: IDC_CHK_START_HIDDEN, tab: 0 },
    CtrlDef { id: IDC_CHK_REMOTE,      tab: 0 },
    CtrlDef { id: IDC_CHK_SMOOTH,      tab: 1 },
    CtrlDef { id: IDC_LBL_DURATION,    tab: 1 },
    CtrlDef { id: IDC_EDT_DURATION,    tab: 1 },
    CtrlDef { id: IDC_LBL_DISTANCE,    tab: 1 },
    CtrlDef { id: IDC_EDT_DISTANCE,    tab: 1 },
    CtrlDef { id: IDC_CHK_ACCEL,       tab: 2 },
    CtrlDef { id: IDC_LBL_SENS,        tab: 2 },
    CtrlDef { id: IDC_EDT_SENS,        tab: 2 },
    CtrlDef { id: IDC_LBL_REMAP_COUNT, tab: 3 },
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

            // ── Scroll tab ──
            create_checkbox(hwnd, hmod, IDC_CHK_SMOOTH, "启用平滑滚动", 30, 56);
            create_label(hwnd, hmod, IDC_LBL_DURATION, "平滑时长 (ms):", 30, 86);
            create_edit(hwnd, hmod, IDC_EDT_DURATION, "120", 160, 83, 80);
            create_label(hwnd, hmod, IDC_LBL_DISTANCE, "行距 (px):", 30, 116);
            create_edit(hwnd, hmod, IDC_EDT_DISTANCE, "40", 160, 113, 80);

            // ── Pointer tab ──
            create_checkbox(hwnd, hmod, IDC_CHK_ACCEL, "启用指针加速", 30, 56);
            create_label(hwnd, hmod, IDC_LBL_SENS, "灵敏度:", 30, 86);
            create_edit(hwnd, hmod, IDC_EDT_SENS, "1.0", 160, 83, 80);

            // ── Buttons tab ──
            let cfg = crate::CONFIG.read();
            let remap_count = cfg.buttons.advanced.len();
            let label = format!("已配置 {} 条按键映射", remap_count);
            create_label(hwnd, hmod, IDC_LBL_REMAP_COUNT, &label, 30, 56);
            create_button(hwnd, hmod, IDC_ADD_REMAP, "录制新映射...", 30, 84, 120, 26);
            drop(cfg);

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

// ─── Config I/O ─────────────────────────────────────────────────────────────

unsafe fn load_config_to_controls(hwnd: isize) {
    let cfg = crate::CONFIG.read();
    set_check(hwnd, IDC_CHK_START_HIDDEN, cfg.general.start_hidden);
    set_check(hwnd, IDC_CHK_REMOTE, cfg.remote.enabled);
    set_check(hwnd, IDC_CHK_SMOOTH, cfg.scroll.smooth);
    set_edit_text(hwnd, IDC_EDT_DURATION, &cfg.scroll.speed.to_string());
    set_edit_text(hwnd, IDC_EDT_DISTANCE, &cfg.scroll.step.to_string());
    set_check(hwnd, IDC_CHK_ACCEL, cfg.accel.enabled);
    set_edit_text(hwnd, IDC_EDT_SENS, &cfg.accel.sensitivity.to_string());
}

unsafe fn save_config_from_controls(hwnd: isize) -> Result<(), String> {
    {
        let mut cfg = crate::CONFIG.write();
        cfg.general.start_hidden = get_check(hwnd, IDC_CHK_START_HIDDEN);
        cfg.remote.enabled = get_check(hwnd, IDC_CHK_REMOTE);
        cfg.scroll.smooth = get_check(hwnd, IDC_CHK_SMOOTH);
        if let Ok(v) = get_edit_text(hwnd, IDC_EDT_DURATION).parse::<f64>() {
            cfg.scroll.speed = v;
        }
        if let Ok(v) = get_edit_text(hwnd, IDC_EDT_DISTANCE).parse::<f64>() {
            cfg.scroll.step = v;
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
