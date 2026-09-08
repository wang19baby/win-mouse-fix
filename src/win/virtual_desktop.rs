//! Virtual desktop switching via IVirtualDesktopManagerInternal COM API.
//!
//! Bypasses the keyboard input chain entirely — no SendInput, no intercepted shortcuts.
//! Uses the same COM interface that Windows' Task View uses internally.

use windows_sys::core::GUID;
use windows_sys::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED,
};

// CLSID_IVirtualDesktopManagerInternal — {C2F03A33-16F9-4A8E-A68E-6069D647C712}
const CLSID_VD_INTERNAL: GUID = GUID {
    data1: 0xC2F03A33,
    data2: 0x16F9,
    data3: 0x4A8E,
    data4: [0xA6, 0x8E, 0x60, 0x69, 0xD6, 0x47, 0xC7, 0x12],
};

// IID_IVirtualDesktopManagerInternal — {1840BE8B-14F8-49B4-8776-449386C73432}
const IID_VD_INTERNAL: GUID = GUID {
    data1: 0x1840BE8B,
    data2: 0x14F8,
    data3: 0x49B4,
    data4: [0x87, 0x76, 0x44, 0x93, 0x86, 0xC7, 0x34, 0x32],
};

// ─── Raw COM vtable ─────────────────────────────────────────────────────────

#[repr(C)]
#[allow(non_snake_case)]
struct IVDInternalVtbl {
    QueryInterface: usize,
    AddRef: usize,
    Release: usize,
    GetCurrentDesktopIndex: usize,
    NavigateLeft: usize,
    NavigateRight: usize,
}

#[repr(C)]
#[allow(non_snake_case)]
struct IVirtualDesktopManagerInternal {
    lpVtbl: *const IVDInternalVtbl,
}

#[allow(non_snake_case)]
impl IVirtualDesktopManagerInternal {
    unsafe fn GetCurrentDesktopIndex(&self, idx: *mut u32) -> i32 {
        let fn_ptr = (*self.lpVtbl).GetCurrentDesktopIndex;
        let func: unsafe fn(*const IVDInternalVtbl, *mut u32) -> i32 = core::mem::transmute(fn_ptr);
        func(self.lpVtbl, idx)
    }

    unsafe fn NavigateLeft(&self, current: u32) -> i32 {
        let fn_ptr = (*self.lpVtbl).NavigateLeft;
        let func: unsafe fn(*const IVDInternalVtbl, u32) -> i32 = core::mem::transmute(fn_ptr);
        func(self.lpVtbl, current)
    }

    unsafe fn NavigateRight(&self, current: u32) -> i32 {
        let fn_ptr = (*self.lpVtbl).NavigateRight;
        let func: unsafe fn(*const IVDInternalVtbl, u32) -> i32 = core::mem::transmute(fn_ptr);
        func(self.lpVtbl, current)
    }
}

// ─── State ─────────────────────────────────────────────────────────────────

static COM_INIT: std::sync::OnceLock<bool> = std::sync::OnceLock::new();

fn ensure_com() -> bool {
    *COM_INIT.get_or_init(|| unsafe {
        CoInitializeEx(core::ptr::null(), COINIT_APARTMENTTHREADED as u32) >= 0
    })
}

// ─── Public API ────────────────────────────────────────────────────────────

/// Switch to the virtual desktop to the left of the current one.
pub fn switch_virtual_desktop_left() {
    switch_impl(-1);
}

/// Switch to the virtual desktop to the right of the current one.
pub fn switch_virtual_desktop_right() {
    switch_impl(1);
}

fn switch_impl(direction: i32) {
    if !ensure_com() {
        return;
    }

    unsafe {
        let mut vdm: *mut IVirtualDesktopManagerInternal = core::ptr::null_mut();
        let hr = CoCreateInstance(
            &CLSID_VD_INTERNAL,
            core::ptr::null_mut(),
            CLSCTX_INPROC_SERVER,
            &IID_VD_INTERNAL as *const _,
            &mut vdm as *mut *mut IVirtualDesktopManagerInternal as *mut *mut std::ffi::c_void,
        );

        if hr < 0 || vdm.is_null() {
            return;
        }

        let vdm = &*vdm;
        let mut idx: u32 = 0;
        if vdm.GetCurrentDesktopIndex(&mut idx) < 0 {
            return;
        }

        if direction < 0 {
            let _ = vdm.NavigateLeft(idx);
        } else {
            let _ = vdm.NavigateRight(idx);
        }
    }
}
