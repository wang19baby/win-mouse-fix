//! Cross-screen DPI detection and HID++ DPI application (Phase 10).
//!
//! Monitor density is read via `EnumDisplayMonitors` + `GetDeviceCaps` (logical
//! DPI per monitor), which needs only the GDI feature already enabled. The HID++
//! DPI write path targets the Adjustable DPI feature (0x2201); it requires a
//! connected, capable device and is best-effort (probe degradation, no errors).
#![allow(dead_code)]
use std::ptr::null_mut;
use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE, POINT, RECT};
use windows_sys::Win32::Graphics::Gdi::{
    EnumDisplayMonitors, GetDeviceCaps, HDC, HMONITOR, LOGPIXELSX,
};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING, ReadFile, WriteFile,
};
use windows_sys::Win32::UI::WindowsAndMessaging::GetCursorPos;
use crate::device::enumerate;
use crate::device::hidpp::{self, feature};
use crate::CONFIG;
use std::sync::atomic::{AtomicU16, AtomicU8, Ordering};

/// `GetDeviceCaps` index for vertical logical DPI (LOGPIXELSY). Not re-exported
/// by the crate's prelude, so referenced by its documented value.
const LOGPIXELSY: i32 = 90;

/// A display monitor and its effective (logical) DPI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Monitor {
    pub index: usize,
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
    pub dpi_x: i32,
    pub dpi_y: i32,
    /// `true` when the monitor's top-left is at the virtual-screen origin (0,0),
    /// which is always the primary monitor.
    pub primary: bool,
}

/// Enumerate all currently-present display monitors with their DPI.
pub fn enumerate_monitors() -> Vec<Monitor> {
    unsafe {
        let mut monitors: Vec<Monitor> = Vec::new();
        let data = &mut monitors as *mut Vec<Monitor> as isize;
        EnumDisplayMonitors(0, std::ptr::null(), Some(monitor_cb), data);
        for m in &mut monitors {
            m.primary = m.left == 0 && m.top == 0;
        }
        monitors
    }
}

unsafe extern "system" fn monitor_cb(
    _hmon: HMONITOR,
    hdc: HDC,
    rect: *mut RECT,
    data: isize,
) -> i32 {
    let monitors = &mut *(data as *mut Vec<Monitor>);
    let r = *rect;
    let dpi_x = if hdc != 0 { GetDeviceCaps(hdc, LOGPIXELSX as i32) } else { 96 };
    let dpi_y = if hdc != 0 { GetDeviceCaps(hdc, LOGPIXELSY as i32) } else { 96 };
    monitors.push(Monitor {
        index: monitors.len(),
        left: r.left,
        top: r.top,
        right: r.right,
        bottom: r.bottom,
        dpi_x,
        dpi_y,
        primary: false, // resolved by the caller after enumeration
    });
    1
}

/// The monitor currently under the cursor, or `None` if undeterminable.
/// Falls back to the primary monitor when the cursor is outside any rect.
pub fn cursor_monitor() -> Option<Monitor> {
    unsafe {
        let mut pt: POINT = std::mem::zeroed();
        GetCursorPos(&mut pt);
        let monitors = enumerate_monitors();
        let hit = monitors
            .iter()
            .find(|m| pt.x >= m.left && pt.x < m.right && pt.y >= m.top && pt.y < m.bottom);
        hit.cloned().or_else(|| monitors.iter().find(|m| m.primary).cloned())
    }
}

/// Target hardware DPI for a monitor: `base_dpi * monitor_dpi / ref_dpi`,
/// clamped to `[min_dpi, max_dpi]`. Pure and unit-tested.
///
/// `ref_dpi` is the logical DPI of the reference (anchor) monitor; when the
/// active monitor matches it, the result equals `base_dpi`.
pub fn target_dpi(
    base_dpi: u16,
    ref_dpi: i32,
    monitor_dpi: i32,
    min_dpi: u16,
    max_dpi: u16,
) -> u16 {
    if ref_dpi <= 0 || monitor_dpi <= 0 {
        return base_dpi;
    }
    let scaled = (base_dpi as i64) * (monitor_dpi as i64) / (ref_dpi as i64);
    scaled.clamp(min_dpi as i64, max_dpi as i64) as u16
}

/// Resolve the Adjustable DPI feature and read the current sensor DPI.
/// Returns `None` if the feature is absent or the device doesn't respond.
pub fn read_dpi(path: &str) -> Option<u16> {
    unsafe {
        let handle = open(path)?;
        let result = read_dpi_handle(handle);
        CloseHandle(handle);
        result
    }
}

/// Resolve the Adjustable DPI feature and set the sensor DPI.
/// Returns `Some(actual)` on success, `None` if the device can't be reached or
/// lacks the feature (probe degradation — no error is raised).
pub fn apply_dpi(path: &str, dpi: u16) -> Option<u16> {
    unsafe {
        let handle = open(path)?;
        let result = apply_dpi_handle(handle, dpi);
        CloseHandle(handle);
        result
    }
}

/// Apply `dpi` to the first Logitech device that supports Adjustable DPI.
/// Returns `Some(actual)` if any device accepted it, else `None`.
pub fn apply_dpi_to_first_device(dpi: u16) -> Option<u16> {
    for d in enumerate::enumerate_logitech() {
        if let Some(actual) = apply_dpi(&d.path, dpi) {
            return Some(actual);
        }
    }
    None
}

unsafe fn open(path: &str) -> Option<isize> {
    let wide: Vec<u16> = path.encode_utf16().chain(std::iter::once(0)).collect();
    let handle = CreateFileW(
        wide.as_ptr(),
        0,
        FILE_SHARE_READ | FILE_SHARE_WRITE,
        null_mut(),
        OPEN_EXISTING,
        0,
        0,
    );
    if handle == INVALID_HANDLE_VALUE {
        None
    } else {
        Some(handle)
    }
}

unsafe fn read_dpi_handle(handle: isize) -> Option<u16> {
    let req = hidpp::get_feature_index_request(0x01, feature::ADJUSTABLE_DPI);
    let mut out = [0u8; 7];
    if !write_report(handle, &req.encode(), &mut out) {
        return None;
    }
    let resp = hidpp::ShortMessage::decode(&out)?;
    let idx = hidpp::feature_index_from_response(&resp)?;
    let req2 = hidpp::get_dpi_request(0x01, idx, 0);
    if !write_report(handle, &req2.encode(), &mut out) {
        return None;
    }
    let resp2 = hidpp::ShortMessage::decode(&out)?;
    hidpp::dpi_from_response(&resp2)
}

unsafe fn apply_dpi_handle(handle: isize, dpi: u16) -> Option<u16> {
    let req = hidpp::get_feature_index_request(0x01, feature::ADJUSTABLE_DPI);
    let mut out = [0u8; 7];
    if !write_report(handle, &req.encode(), &mut out) {
        return None;
    }
    let resp = hidpp::ShortMessage::decode(&out)?;
    let idx = hidpp::feature_index_from_response(&resp)?;
    let req2 = hidpp::set_dpi_request(0x01, idx, 0, dpi);
    if !write_report(handle, &req2.encode(), &mut out) {
        return None;
    }
    // The device confirms asynchronously; echo the requested value as the result.
    Some(dpi)
}

unsafe fn write_report(handle: isize, send: &[u8], recv: &mut [u8]) -> bool {
    let mut written: u32 = 0;
    if WriteFile(
        handle,
        send.as_ptr() as *const _,
        send.len() as u32,
        &mut written,
        null_mut(),
    ) == 0
    {
        return false;
    }
    let mut read: u32 = 0;
    if ReadFile(
        handle,
        recv.as_mut_ptr() as *mut _,
        recv.len() as u32,
        &mut read,
        null_mut(),
    ) == 0
    {
        return false;
    }
    read == recv.len() as u32
}

/// Number of consecutive polls a target DPI must persist before we write it to
/// the device. Prevents thrashing the sensor while the cursor flickers across a
/// monitor boundary.
const DPI_STABLE_THRESHOLD: u8 = 3;

/// Last DPI value successfully applied to a device (0 = unknown).
static LAST_APPLIED_DPI: AtomicU16 = AtomicU16::new(0);
/// Target DPI currently pending confirmation.
static PENDING_DPI: AtomicU16 = AtomicU16::new(0);
/// How many consecutive polls `PENDING_DPI` has persisted.
static PENDING_COUNT: AtomicU8 = AtomicU8::new(0);

/// Decision from a single autoswitch poll. Pure so it can be unit-tested
/// independently of the global statics and device I/O.
enum DpiDecision {
    /// Target equals the last-applied value; pending state should reset.
    Stable,
    /// Target differs but isn't confirmed yet; pending counter is now `count`.
    Pending { count: u8 },
    /// Target persisted long enough; apply `dpi` to the device.
    Apply { dpi: u16 },
}

fn decide_dpi(last: u16, pending: u16, count: u8, target: u16, threshold: u8) -> DpiDecision {
    if target == last {
        return DpiDecision::Stable;
    }
    if pending == target {
        let next = count.saturating_add(1);
        if next >= threshold {
            DpiDecision::Apply { dpi: target }
        } else {
            DpiDecision::Pending { count: next }
        }
    } else {
        DpiDecision::Pending { count: 1 }
    }
}

/// Poll for cross-screen DPI changes and auto-apply the target sensor DPI.
///
/// Reads the cursor's monitor, scales `base_dpi` by the monitor's logical DPI
/// relative to the primary (anchor) monitor via [`target_dpi`], and writes the
/// result to the first capable Logitech device — debounced so the device is only
/// touched after the target is stable for `DPI_STABLE_THRESHOLD` polls.
///
/// No-op when `dpi.auto_switch` is off, the cursor monitor is unknown, or no
/// capable device responds (best-effort, probe degradation).
pub fn poll_dpi_autoswitch() {
    let ref_dpi = enumerate_monitors()
        .into_iter()
        .find(|m| m.primary)
        .map(|m| m.dpi_x)
        .unwrap_or(96);
    let target = {
        let cfg = crate::CONFIG.read();
        if !cfg.dpi.auto_switch {
            return;
        }
        let monitor = match cursor_monitor() {
            Some(m) => m,
            None => return,
        };
        target_dpi(
            cfg.dpi.base_dpi,
            ref_dpi,
            monitor.dpi_x,
            cfg.dpi.min_dpi,
            cfg.dpi.max_dpi,
        )
    };
    // Config lock released before the (potentially slow) device write.

    match decide_dpi(
        LAST_APPLIED_DPI.load(Ordering::SeqCst),
        PENDING_DPI.load(Ordering::SeqCst),
        PENDING_COUNT.load(Ordering::SeqCst),
        target,
        DPI_STABLE_THRESHOLD,
    ) {
        DpiDecision::Stable => {
            PENDING_DPI.store(0, Ordering::SeqCst);
            PENDING_COUNT.store(0, Ordering::SeqCst);
        }
        DpiDecision::Pending { count } => {
            PENDING_DPI.store(target, Ordering::SeqCst);
            PENDING_COUNT.store(count, Ordering::SeqCst);
        }
        DpiDecision::Apply { dpi } => {
            match apply_dpi_to_first_device(dpi) {
                Some(actual) => LAST_APPLIED_DPI.store(actual, Ordering::SeqCst),
                // No device responded; throttle retries to one per threshold cycle.
                None => PENDING_COUNT.store(DPI_STABLE_THRESHOLD - 1, Ordering::SeqCst),
            }
            PENDING_DPI.store(0, Ordering::SeqCst);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn target_dpi_scales_with_monitor() {
        // Reference 96, base 800 → monitor 144 gives 1200.
        assert_eq!(target_dpi(800, 96, 144, 200, 4000), 1200);
        // Monitor denser than reference scales up.
        assert_eq!(target_dpi(800, 96, 192, 200, 4000), 1600);
        // Equal to reference → base.
        assert_eq!(target_dpi(800, 96, 96, 200, 4000), 800);
    }

    #[test]
    fn target_dpi_clamps_to_device_range() {
        // 1600 would exceed max 1000.
        assert_eq!(target_dpi(800, 96, 192, 200, 1000), 1000);
        // Guard against degenerate reference/monitor DPI.
        assert_eq!(target_dpi(800, 0, 192, 200, 4000), 800);
        assert_eq!(target_dpi(800, 96, 0, 200, 4000), 800);
    }

    #[test]
    fn decide_dpi_stable_when_target_matches_last() {
        assert!(matches!(decide_dpi(800, 0, 0, 800, 3), DpiDecision::Stable));
    }

    #[test]
    fn decide_dpi_applies_after_threshold() {
        assert!(matches!(
            decide_dpi(800, 0, 0, 1200, 3),
            DpiDecision::Pending { count: 1 }
        ));
        assert!(matches!(
            decide_dpi(800, 1200, 1, 1200, 3),
            DpiDecision::Pending { count: 2 }
        ));
        assert!(matches!(
            decide_dpi(800, 1200, 2, 1200, 3),
            DpiDecision::Apply { dpi: 1200 }
        ));
    }

    #[test]
    fn decide_dpi_resets_pending_on_target_change() {
        // Pending on 1200, then a different target appears -> restart at count 1.
        assert!(matches!(
            decide_dpi(800, 1200, 2, 1600, 3),
            DpiDecision::Pending { count: 1 }
        ));
    }
}
