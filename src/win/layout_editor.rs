//! Snap Layout Editor Dialog.
//!
//! Modal dialog launched from the tray menu that lets the user add, edit, and
//! delete `[[snap.layouts]]` presets.  Changes are saved to config.toml on OK.

use std::ptr::null;
use std::sync::OnceLock;
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, FindWindowExW, GetDlgItem,
    GetMessageW, GetWindowTextW, LoadCursorW, PostQuitMessage, RegisterClassExW, SendMessageW,
    SetWindowTextW, TranslateMessage, BS_PUSHBUTTON, EN_KILLFOCUS, ES_NUMBER, IDC_ARROW,
    LBN_SELCHANGE, LBS_NOTIFY, LB_ADDSTRING, LB_GETCOUNT, LB_GETCURSEL, LB_RESETCONTENT,
    LB_SETCURSEL, MSG, WM_COMMAND, WM_CREATE, WM_DESTROY, WNDCLASSEXW, WS_BORDER, WS_CAPTION,
    WS_CHILD, WS_SYSMENU, WS_VISIBLE,
};

// UpDown / Spin control constants (u32)
const UDS_ARROWKEYS: u32 = 32;
const UDS_AUTOBUDDY: u32 = 16;
const UDS_SETBUDDYINT: u32 = 2;
const UDM_SETBUDDY: u32 = 1129;
const UDM_SETRANGE32: u32 = 1135;

/// Cached wide-string for the window class name — the pointer from as_ptr()
/// must outlive the call to RegisterClassExW.
static CLASS_NAME_WIDE: OnceLock<Vec<u16>> = OnceLock::new();

macro_rules! style {
    ($($c:expr),*) => { ($(($c as u32) | )* 0) }
}

// ── Constants ──────────────────────────────────────────────────────────────────

const CLASS_NAME: &str = "WinMouseFixLayoutEditor";

const IDC_LIST: usize = 4001;
const IDC_NAME_EDIT: usize = 4002;
const IDC_ROWS_EDIT: usize = 4003;
const IDC_COLS_EDIT: usize = 4004;
const IDC_GAP_EDIT: usize = 4005;
const IDC_SPIN_ROWS: usize = 4006;
const IDC_SPIN_COLS: usize = 4007;
const IDC_SPIN_GAP: usize = 4008;
const IDC_BTN_ADD: usize = 4009;
const IDC_BTN_DEL: usize = 4010;
const IDC_BTN_OK: usize = 4011;
const IDC_BTN_CANCEL: usize = 4012;

// ── State ─────────────────────────────────────────────────────────────────────

/// Working copy of layout presets for this dialog session.
static WORKING: OnceLock<parking_lot::Mutex<Vec<crate::config::LayoutPreset>>> = OnceLock::new();

// ── Public API ─────────────────────────────────────────────────────────────────

/// Open the layout editor dialog as a modal window anchored to `parent`.
pub unsafe fn start(parent: isize) {
    let current: Vec<_> = crate::CONFIG.read().snap.layouts.clone();
    *WORKING
        .get_or_init(|| parking_lot::Mutex::new(Vec::new()))
        .lock() = current;

    register_class();

    let hwnd = CreateWindowExW(
        0,
        to_wide(CLASS_NAME).as_ptr(),
        to_wide("Snap 布局编辑器").as_ptr(),
        WS_CAPTION | WS_SYSMENU | WS_VISIBLE,
        420,
        180,
        500,
        440,
        parent,
        0,
        GetModuleHandleW(null()),
        null(),
    );
    if hwnd == 0 {
        return;
    }

    let mut msg: MSG = std::mem::zeroed();
    while GetMessageW(&mut msg, hwnd, 0, 0) > 0 {
        let _ = TranslateMessage(&msg);
        DispatchMessageW(&msg);
    }
}

// ── Helpers ────────────────────────────────────────────────────────────────────

fn to_wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn layout_display(l: &crate::config::LayoutPreset) -> String {
    format!("{}  {}x{}  gap={}", l.name, l.rows, l.cols, l.gap)
}

unsafe fn refresh_list(hwnd: HWND) {
    let list = FindWindowExW(hwnd, 0, to_wide("ListBox").as_ptr(), null());
    if list == 0 {
        return;
    }
    SendMessageW(list, LB_RESETCONTENT, 0, 0);
    let layouts = WORKING.get().unwrap().lock();
    for l in layouts.iter() {
        let w = to_wide(&layout_display(l));
        SendMessageW(list, LB_ADDSTRING, 0, w.as_ptr() as _);
    }
    if !layouts.is_empty() {
        let _ = SendMessageW(list, LB_SETCURSEL, 0, 0);
    }
}

fn selected_index(hwnd: HWND) -> Option<usize> {
    unsafe {
        let list = FindWindowExW(hwnd, 0, to_wide("ListBox").as_ptr(), null());
        if list == 0 {
            return None;
        }
        let sel = SendMessageW(list, LB_GETCURSEL, 0, 0) as usize;
        let count = SendMessageW(list, LB_GETCOUNT, 0, 0) as usize;
        if sel >= count {
            return None;
        }
        Some(sel)
    }
}

unsafe fn show_layout(hwnd: HWND, index: usize) {
    // Copy data out of the lock guard so we can drop the lock before UI calls
    let (name, rows, cols, gap) = {
        let layouts = WORKING.get().unwrap().lock();
        match layouts.get(index) {
            Some(l) => (l.name.clone(), l.rows, l.cols, l.gap),
            None => return,
        }
    };

    let set_text = |id: usize, text: &str| {
        let ctrl = GetDlgItem(hwnd, id as _);
        if ctrl != 0 {
            SetWindowTextW(ctrl, to_wide(text).as_ptr());
        }
    };

    set_text(IDC_NAME_EDIT, &name);
    set_text(IDC_ROWS_EDIT, &rows.to_string());
    set_text(IDC_COLS_EDIT, &cols.to_string());
    set_text(IDC_GAP_EDIT, &gap.to_string());
}

unsafe fn read_edit_fields(hwnd: HWND, index: usize) {
    let get_text = |id: usize| -> String {
        let ctrl = GetDlgItem(hwnd, id as _);
        if ctrl == 0 {
            return String::new();
        }
        let mut buf = [0u16; 128];
        let len = GetWindowTextW(ctrl, buf.as_mut_ptr(), buf.len() as i32) as usize;
        String::from_utf16_lossy(&buf[..len])
    };

    let name = get_text(IDC_NAME_EDIT);
    let rows: u8 = get_text(IDC_ROWS_EDIT).trim().parse().unwrap_or(2);
    let cols: u8 = get_text(IDC_COLS_EDIT).trim().parse().unwrap_or(2);
    let gap: i32 = get_text(IDC_GAP_EDIT).trim().parse().unwrap_or(4);

    let layouts = &mut *WORKING.get().unwrap().lock();
    if index < layouts.len() {
        layouts[index] = crate::config::LayoutPreset {
            name,
            rows,
            cols,
            gap,
        };
    }
}

fn save_and_close(hwnd: HWND) {
    let layouts = WORKING.get().unwrap().lock().clone();
    {
        let mut cfg = crate::CONFIG.write();
        cfg.snap.layouts = layouts;
    }
    if let Err(e) = crate::config::Config::save(&*crate::CONFIG.read()) {
        crate::log::write(&format!("layout editor save error: {e}"));
    }
    crate::snap::refresh_layouts(&crate::CONFIG.read().snap.layouts);
    unsafe { DestroyWindow(hwnd) };
}

unsafe fn register_class() {
    let hmod = GetModuleHandleW(null());
    let wc = WNDCLASSEXW {
        cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
        style: 0,
        lpfnWndProc: Some(window_proc),
        cbClsExtra: 0,
        cbWndExtra: 0,
        hInstance: hmod,
        hIcon: 0,
        hCursor: LoadCursorW(0, IDC_ARROW),
        hbrBackground: 0,
        lpszMenuName: null(),
        lpszClassName: CLASS_NAME_WIDE.get_or_init(|| to_wide(CLASS_NAME)).as_ptr(),
        hIconSm: 0,
    };
    RegisterClassExW(&wc);
}

// ── Window Procedure ───────────────────────────────────────────────────────────

unsafe extern "system" fn window_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    _lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_CREATE => create_window(hwnd),

        WM_COMMAND => {
            let id = (wparam & 0xFFFF) as usize;
            let code = ((wparam >> 16) & 0xFFFF) as u32;

            match id {
                IDC_BTN_ADD => {
                    let layouts = &mut *WORKING.get().unwrap().lock();
                    let n = layouts.len() + 1;
                    layouts.push(crate::config::LayoutPreset {
                        name: format!("布局 {}", n),
                        rows: 2,
                        cols: 2,
                        gap: 4,
                    });
                    refresh_list(hwnd);
                    let list = FindWindowExW(hwnd, 0, to_wide("ListBox").as_ptr(), null());
                    if list != 0 {
                        let _ = SendMessageW(list, LB_SETCURSEL, layouts.len() - 1, 0);
                    }
                    show_layout(hwnd, layouts.len() - 1);
                    0
                }

                IDC_BTN_DEL => {
                    if let Some(idx) = selected_index(hwnd) {
                        let layouts = &mut *WORKING.get().unwrap().lock();
                        if idx < layouts.len() {
                            layouts.remove(idx);
                            refresh_list(hwnd);
                            if !layouts.is_empty() {
                                show_layout(hwnd, idx.min(layouts.len() - 1));
                            }
                        }
                    }
                    0
                }

                IDC_BTN_OK => {
                    if let Some(idx) = selected_index(hwnd) {
                        read_edit_fields(hwnd, idx);
                    }
                    save_and_close(hwnd);
                    0
                }

                IDC_BTN_CANCEL => {
                    DestroyWindow(hwnd);
                    0
                }

                IDC_LIST if code == LBN_SELCHANGE as u32 => {
                    if let Some(idx) = selected_index(hwnd) {
                        show_layout(hwnd, idx);
                    }
                    0
                }

                _ if id == IDC_NAME_EDIT
                    || id == IDC_ROWS_EDIT
                    || id == IDC_COLS_EDIT
                    || id == IDC_GAP_EDIT =>
                {
                    if code == EN_KILLFOCUS {
                        if let Some(idx) = selected_index(hwnd) {
                            read_edit_fields(hwnd, idx);
                            refresh_list(hwnd);
                        }
                    }
                    0
                }

                _ => DefWindowProcW(hwnd, msg, wparam, _lparam),
            }
        }

        WM_DESTROY => {
            PostQuitMessage(0);
            0
        }

        _ => DefWindowProcW(hwnd, msg, wparam, _lparam),
    }
}

// ── WM_CREATE ─────────────────────────────────────────────────────────────────

unsafe fn create_window(hwnd: HWND) -> LRESULT {
    let hmod = GetModuleHandleW(null());

    // List box
    let _list = CreateWindowExW(
        0,
        to_wide("ListBox").as_ptr(),
        null(),
        style!(WS_CHILD, WS_VISIBLE, WS_BORDER, LBS_NOTIFY),
        12,
        10,
        460,
        190,
        hwnd,
        IDC_LIST as _,
        hmod,
        null(),
    );
    refresh_list(hwnd);

    // Labels
    let mk_label = |txt: &str, x: i32, y: i32, id: usize| -> isize {
        CreateWindowExW(
            0,
            to_wide("Static").as_ptr(),
            to_wide(txt).as_ptr(),
            style!(WS_CHILD, WS_VISIBLE),
            x,
            y,
            60,
            18,
            hwnd,
            id as _,
            hmod,
            null(),
        )
    };
    mk_label("名称:", 12, 208, 100);
    mk_label("行数:", 155, 208, 101);
    mk_label("列数:", 275, 208, 102);
    mk_label("间距:", 385, 208, 103);

    // Edit fields
    let rows_edit = CreateWindowExW(
        0,
        to_wide("Edit").as_ptr(),
        null(),
        style!(WS_CHILD, WS_VISIBLE, WS_BORDER, ES_NUMBER),
        155,
        226,
        65,
        24,
        hwnd,
        IDC_ROWS_EDIT as _,
        hmod,
        null(),
    );
    let cols_edit = CreateWindowExW(
        0,
        to_wide("Edit").as_ptr(),
        null(),
        style!(WS_CHILD, WS_VISIBLE, WS_BORDER, ES_NUMBER),
        275,
        226,
        65,
        24,
        hwnd,
        IDC_COLS_EDIT as _,
        hmod,
        null(),
    );
    let gap_edit = CreateWindowExW(
        0,
        to_wide("Edit").as_ptr(),
        null(),
        style!(WS_CHILD, WS_VISIBLE, WS_BORDER, ES_NUMBER),
        385,
        226,
        80,
        24,
        hwnd,
        IDC_GAP_EDIT as _,
        hmod,
        null(),
    );
    let _name_edit = CreateWindowExW(
        0,
        to_wide("Edit").as_ptr(),
        null(),
        style!(WS_CHILD, WS_VISIBLE, WS_BORDER),
        12,
        226,
        130,
        24,
        hwnd,
        IDC_NAME_EDIT as _,
        hmod,
        null(),
    );

    // Spin controls
    let spin_rows = CreateWindowExW(
        0,
        to_wide("msctls_updown32").as_ptr(),
        null(),
        style!(
            WS_CHILD,
            WS_VISIBLE,
            UDS_SETBUDDYINT,
            UDS_ARROWKEYS,
            UDS_AUTOBUDDY
        ),
        220,
        226,
        20,
        24,
        hwnd,
        IDC_SPIN_ROWS as _,
        hmod,
        null(),
    );
    let spin_cols = CreateWindowExW(
        0,
        to_wide("msctls_updown32").as_ptr(),
        null(),
        style!(
            WS_CHILD,
            WS_VISIBLE,
            UDS_SETBUDDYINT,
            UDS_ARROWKEYS,
            UDS_AUTOBUDDY
        ),
        340,
        226,
        20,
        24,
        hwnd,
        IDC_SPIN_COLS as _,
        hmod,
        null(),
    );
    let spin_gap = CreateWindowExW(
        0,
        to_wide("msctls_updown32").as_ptr(),
        null(),
        style!(
            WS_CHILD,
            WS_VISIBLE,
            UDS_SETBUDDYINT,
            UDS_ARROWKEYS,
            UDS_AUTOBUDDY
        ),
        465,
        226,
        20,
        24,
        hwnd,
        IDC_SPIN_GAP as _,
        hmod,
        null(),
    );

    // Buddy + range for spins
    SendMessageW(spin_rows, UDM_SETBUDDY, rows_edit as _, 0);
    SendMessageW(spin_cols, UDM_SETBUDDY, cols_edit as _, 0);
    SendMessageW(spin_gap, UDM_SETBUDDY, gap_edit as _, 0);
    SendMessageW(spin_rows, UDM_SETRANGE32, 1, 12);
    SendMessageW(spin_cols, UDM_SETRANGE32, 1, 12);
    SendMessageW(spin_gap, UDM_SETRANGE32, 0, 50);

    // Buttons
    let mk_btn = |txt: &str, x: i32, y: i32, w: i32, h: i32, id: usize| -> isize {
        CreateWindowExW(
            0,
            to_wide("Button").as_ptr(),
            to_wide(txt).as_ptr(),
            style!(WS_CHILD, WS_VISIBLE, BS_PUSHBUTTON),
            x,
            y,
            w,
            h,
            hwnd,
            id as _,
            hmod,
            null(),
        )
    };
    mk_btn("新增", 12, 265, 80, 28, IDC_BTN_ADD);
    mk_btn("删除", 100, 265, 80, 28, IDC_BTN_DEL);
    mk_btn("保存", 280, 360, 90, 34, IDC_BTN_OK);
    mk_btn("取消", 380, 360, 90, 34, IDC_BTN_CANCEL);

    show_layout(hwnd, 0);
    0
}
