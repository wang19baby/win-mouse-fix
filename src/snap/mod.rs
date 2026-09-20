//! Snap Zones — window edge/corner snapping for win-mouse-fix.
//!
//! AltSnap-inspired window snapping: edges, corners, and multi-monitor aware.
//! Phase A delivers the foundational zone model + snap algorithm.
mod preview;
pub use preview::{destroy, hide_preview, show_preview};
mod layouts;
pub use layouts::{apply_layout_with_index, refresh_layouts, Layout};
use serde::{Deserialize, Serialize};
use windows_sys::Win32::Foundation::{POINT, RECT};
use windows_sys::Win32::Graphics::Gdi::{GetMonitorInfoW, MONITORINFO, MonitorFromPoint, MonitorFromWindow};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GetSystemMetrics, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN,
};

/// Sentinel value for an invalid monitor handle.
pub const NULL_HMONITOR: isize = 0;

fn rect_fmt(r: &RECT) -> String {
    format!("({},{} {},{})", r.left, r.top, r.right, r.bottom)
}

// ─── Monitor / Display geometry ─────────────────────────────────────────────────

/// Information about one physical monitor, obtained via Win32 `GetMonitorInfoW`.
#[derive(Clone)]
#[allow(dead_code)]
pub struct MonitorInfo {
    /// Win32 HMONITOR as raw isize.
    pub hmonitor: isize,
    /// Full monitor rect in virtual-screen coordinates (includes taskbar).
    pub rc_monitor: RECT,
    /// Work area (excludes taskbar / docked windows) in virtual-screen coords.
    pub rc_work: RECT,
    /// Human-readable name like "\\.\DISPLAY1".
    pub name: String,
}

impl std::fmt::Debug for MonitorInfo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MonitorInfo")
            .field("hmonitor", &self.hmonitor)
            .field("rc_monitor", &rect_fmt(&self.rc_monitor))
            .field("rc_work", &rect_fmt(&self.rc_work))
            .field("name", &self.name)
            .finish()
    }
}

/// All physical monitors currently present, plus the virtual screen bounds.
#[derive(Clone)]
#[allow(dead_code)]
pub struct DisplayState {
    pub monitors: Vec<MonitorInfo>,
    /// Virtual screen rect (union of all monitors).
    pub virtual_screen: RECT,
}

impl std::fmt::Debug for DisplayState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DisplayState")
            .field("monitors", &self.monitors)
            .field("virtual_screen", &rect_fmt(&self.virtual_screen))
            .finish()
    }
}

/// Refresh the display state from Win32 APIs.
#[allow(dead_code)]
fn refresh_displays() -> DisplayState {
    let mut monitors: Vec<MonitorInfo> = Vec::new();

    // Collect all monitors by probing a virtual-screen grid.
    // We probe a 5×5 grid of points and de-dup by HMONITOR handle.
    // This avoids the EnumDisplayMonitors callback API and is faster for ≤8 monitors.
    let vx = unsafe { GetSystemMetrics(SM_XVIRTUALSCREEN) };
    let vy = unsafe { GetSystemMetrics(SM_YVIRTUALSCREEN) };
    let vw = unsafe { GetSystemMetrics(SM_CXVIRTUALSCREEN) };
    let vh = unsafe { GetSystemMetrics(SM_CYVIRTUALSCREEN) };

    let virtual_screen = RECT { left: vx, top: vy, right: vx + vw, bottom: vy + vh };

    let step_x = if vw > 4 { vw / 4 } else { 1 };
    let step_y = if vh > 4 { vh / 4 } else { 1 };

    let mut seen: Vec<isize> = Vec::new();

    for row in 0..=4 {
        for col in 0..=4 {
            let px = vx + col * step_x;
            let py = vy + row * step_y;
            let pt = POINT { x: px, y: py };
            let hmon = unsafe { MonitorFromPoint(pt, 2 /* MONITOR_DEFAULTTONEAREST */) };
            if hmon == NULL_HMONITOR || seen.contains(&hmon) {
                continue;
            }
            seen.push(hmon);

            let mut info: MONITORINFOEX = unsafe { std::mem::zeroed() };
            info.monitor_info.cbSize = std::mem::size_of::<MONITORINFOEX>() as u32;
            let ok = unsafe { GetMonitorInfoW(hmon, &mut info as *mut _ as *mut _) };
            if ok == 0 {
                continue;
            }

            let name = unsafe {
                let mut len = 0;
                while len < 32 && info.sz_device[len] != 0 {
                    len += 1;
                }
                let slice = std::slice::from_raw_parts(info.sz_device.as_ptr(), len as usize);
                String::from_utf16_lossy(slice)
            };

            monitors.push(MonitorInfo {
                hmonitor: hmon,
                rc_monitor: info.monitor_info.rcMonitor,
                rc_work: info.monitor_info.rcWork,
                name,
            });
        }
    }

    DisplayState { monitors, virtual_screen }
}

/// Returns the work-area rect for the monitor containing `hwnd`.
pub fn work_area_of(hwnd: isize) -> Option<RECT> {
    let hmon = unsafe { MonitorFromWindow(hwnd, 2 /* MONITOR_DEFAULTTONEAREST */) };
    if hmon == NULL_HMONITOR {
        return None;
    }
    let mut info: MONITORINFOEX = unsafe { std::mem::zeroed() };
    info.monitor_info.cbSize = std::mem::size_of::<MONITORINFOEX>() as u32;
    let ok = unsafe { GetMonitorInfoW(hmon, &mut info as *mut _ as *mut _) };
    if ok == 0 {
        return None;
    }
    Some(info.monitor_info.rcWork)
}

// ─── Zone model ───────────────────────────────────────────────────────────────

/// Which edge or corner a zone anchors to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ZoneAnchor {
    Top,
    Bottom,
    Left,
    Right,
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
    Center,
    /// Fill the entire monitor work area (Aero Snap)
    Monitor,
}

impl ZoneAnchor {
    /// Human-readable label for preview overlays.
    #[allow(dead_code)]
fn label(&self) -> &'static str {
        match self {
            ZoneAnchor::Top => "Top",
            ZoneAnchor::Bottom => "Bottom",
            ZoneAnchor::Left => "Left",
            ZoneAnchor::Right => "Right",
            ZoneAnchor::TopLeft => "Top-Left",
            ZoneAnchor::TopRight => "Top-Right",
            ZoneAnchor::BottomLeft => "Bottom-Left",
            ZoneAnchor::BottomRight => "Bottom-Right",
            ZoneAnchor::Center => "Center",
            ZoneAnchor::Monitor => "Monitor",
        }
    }
}

/// A snap zone — a region near a screen edge or corner that triggers snapping.
#[derive(Clone)]
pub struct Zone {
    pub anchor: ZoneAnchor,
    /// The snap trigger region (virtual-screen coords).
    /// When the window edge enters this rect, snapping triggers.
    pub trigger: RECT,
    /// The resulting window rect after snapping is applied.
    pub target: RECT,
    /// Pixel distance the window edge must be within to trigger snap.
    pub threshold: i32,
}

impl std::fmt::Debug for Zone {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Zone")
            .field("anchor", &self.anchor)
            .field("trigger", &rect_fmt(&self.trigger))
            .field("target", &rect_fmt(&self.target))
            .field("threshold", &self.threshold)
            .finish()
    }
}

/// The result of a snap computation: which anchor and the final target rect.
#[derive(Clone, Copy)]
pub struct SnapResult {
    pub anchor: ZoneAnchor,
    pub target: RECT,
}

impl std::fmt::Debug for SnapResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SnapResult")
            .field("anchor", &self.anchor)
            .field("target", &rect_fmt(&self.target))
            .finish()
    }
}
// ─── Zone construction ────────────────────────────────────────────────────────

/// Default snap threshold in pixels (Aero-style).
#[allow(dead_code)]
pub const DEFAULT_SNAP_THRESHOLD: i32 = 20;

/// Build all snap zones for a given monitor's work area.
pub fn build_zones(work: RECT, threshold: i32) -> Vec<Zone> {
    let w = rect_width(&work);
    let h = rect_height(&work);
    let l = work.left;
    let t = work.top;

    vec![
        // ── Edges ──────────────────────────────────────────────────
        Zone {
            anchor: ZoneAnchor::Top,
            trigger: RECT { left: l, top: t, right: l + w, bottom: t + threshold },
            target: RECT { left: l, top: t, right: l + w, bottom: t + h / 2 },
            threshold,
        },
        Zone {
            anchor: ZoneAnchor::Bottom,
            trigger: RECT { left: l, top: work.bottom - threshold, right: l + w, bottom: work.bottom },
            target: RECT { left: l, top: t + h / 2, right: l + w, bottom: work.bottom },
            threshold,
        },
        Zone {
            anchor: ZoneAnchor::Left,
            trigger: RECT { left: l, top: t, right: l + threshold, bottom: t + h },
            target: RECT { left: l, top: t, right: l + w / 2, bottom: t + h },
            threshold,
        },
        Zone {
            anchor: ZoneAnchor::Right,
            trigger: RECT { left: work.right - threshold, top: t, right: work.right, bottom: t + h },
            target: RECT { left: l + w / 2, top: t, right: work.right, bottom: t + h },
            threshold,
        },
        // ── Corners ────────────────────────────────────────────────
        Zone {
            anchor: ZoneAnchor::TopLeft,
            trigger: RECT { left: l, top: t, right: l + w / 2, bottom: t + threshold },
            target: RECT { left: l, top: t, right: l + w / 2, bottom: t + h / 2 },
            threshold,
        },
        Zone {
            anchor: ZoneAnchor::TopRight,
            trigger: RECT { left: l + w / 2, top: t, right: l + w, bottom: t + threshold },
            target: RECT { left: l + w / 2, top: t, right: l + w, bottom: t + h / 2 },
            threshold,
        },
        Zone {
            anchor: ZoneAnchor::BottomLeft,
            trigger: RECT { left: l, top: work.bottom - threshold, right: l + w / 2, bottom: work.bottom },
            target: RECT { left: l, top: t + h / 2, right: l + w / 2, bottom: work.bottom },
            threshold,
        },
        Zone {
            anchor: ZoneAnchor::BottomRight,
            trigger: RECT { left: l + w / 2, top: work.bottom - threshold, right: l + w, bottom: work.bottom },
            target: RECT { left: l + w / 2, top: t + h / 2, right: l + w, bottom: work.bottom },
            threshold,
        },
        // ── Full monitor (Aero Snap) ───────────────────────────────
        Zone {
            anchor: ZoneAnchor::Monitor,
            trigger: work,
            target: work,
            threshold,
        },
    ]
}

/// ─── Core snap algorithm ──────────────────────────────────────────────────────

/// Returns the most-specific [`SnapResult`] for a window being dragged to `window_rect`,
/// given all zones on the current monitor. `None` if no zone matches.
///
/// Corners always win over edges regardless of zone list order.
pub fn compute_snap(window_rect: &RECT, zones: &[Zone]) -> Option<SnapResult> {
    // First pass: corners only.
    for zone in zones {
        if matches!(zone.anchor, ZoneAnchor::TopLeft | ZoneAnchor::TopRight | ZoneAnchor::BottomLeft | ZoneAnchor::BottomRight) {
            if rect_intersects(window_rect, &zone.trigger) {
                return Some(SnapResult { anchor: zone.anchor, target: zone.target });
            }
        }
    }
    // Second pass: edges (Monitor zone excluded from auto-snap).
    for zone in zones {
        if matches!(zone.anchor, ZoneAnchor::Top | ZoneAnchor::Bottom | ZoneAnchor::Left | ZoneAnchor::Right) {
            if rect_intersects(window_rect, &zone.trigger) {
                return Some(SnapResult { anchor: zone.anchor, target: zone.target });
            }
        }
    }
    None
}

/// Returns the zone whose trigger region contains `cursor` (used for resize direction).
/// Returns `None` if cursor is in the center non-trigger area.
/// Zones are checked in specificity order: corners > edges > monitor.
#[allow(dead_code)]
pub fn compute_resize_zone(cursor: &POINT, _window_rect: &RECT, zones: &[Zone]) -> Option<SnapResult> {
    // Check zones in priority order: corners first, then edges, then monitor.
    // We do two passes so that specificity always wins over insertion order.
    // First pass: corners only.
    for zone in zones {
        if matches_zone_anchor(zone, &[ZoneAnchor::TopLeft, ZoneAnchor::TopRight, ZoneAnchor::BottomLeft, ZoneAnchor::BottomRight]) {
            if pt_in_rect(cursor, &zone.trigger) {
                return Some(SnapResult { anchor: zone.anchor, target: zone.target });
            }
        }
    }
    // Second pass: edges only (Monitor excluded — it's not a resize direction).
    for zone in zones {
        if matches!(zone.anchor, ZoneAnchor::Top | ZoneAnchor::Bottom | ZoneAnchor::Left | ZoneAnchor::Right) {
            if pt_in_rect(cursor, &zone.trigger) {
                return Some(SnapResult { anchor: zone.anchor, target: zone.target });
            }
        }
    }
    None
}

#[allow(dead_code)]
fn matches_zone_anchor(zone: &Zone, candidates: &[ZoneAnchor]) -> bool {
    candidates.iter().any(|&a| zone.anchor == a)
}
/// Find the best snap when a window edge crosses a trigger boundary.
/// `x` and `y` are the cursor coordinates; `horizontal` selects which axis to use:
/// - `true` → use `x` (for left/right drags → checks Left/Right zones)
/// - `false` → use `y` (for up/down drags → checks Top/Bottom zones)
pub fn compute_edge_snap(x: i32, y: i32, horizontal: bool, zones: &[Zone]) -> Option<SnapResult> {
    let mut best: Option<(i32, SnapResult)> = None;

    for zone in zones {
        let trigger_edge = match (zone.anchor, horizontal) {
            (ZoneAnchor::Left, true) => Some(zone.trigger.right),   // x axis
            (ZoneAnchor::Right, true) => Some(zone.trigger.left),
            (ZoneAnchor::Top, false) => Some(zone.trigger.bottom),   // y axis
            (ZoneAnchor::Bottom, false) => Some(zone.trigger.top),
            _ => None, // corners not relevant for threshold-crossing
        };

        let Some(edge) = trigger_edge else { continue };

        let coord = if horizontal { x } else { y };
        let dist = (coord - edge).abs();
        if dist <= zone.threshold {
            match best {
                None => {
                    best = Some((dist, SnapResult { anchor: zone.anchor, target: zone.target }));
                }
                Some((best_dist, _)) if dist < best_dist => {
                    best = Some((dist, SnapResult { anchor: zone.anchor, target: zone.target }));
                }
                _ => {}
            }
        }
    }

    best.map(|(_, r)| r)
}

// ─── Rect helpers ─────────────────────────────────────────────────────────────

#[inline]
pub fn rect_width(r: &RECT) -> i32 {
    r.right - r.left
}

#[inline]
pub fn rect_height(r: &RECT) -> i32 {
    r.bottom - r.top
}

#[inline]
#[allow(dead_code)]
pub fn rect_center_x(r: &RECT) -> i32 {
    r.left + rect_width(r) / 2
}

#[inline]
#[allow(dead_code)]
pub fn rect_center_y(r: &RECT) -> i32 {
    r.top + rect_height(r) / 2
}

#[inline]
#[allow(dead_code)]
pub fn pt_in_rect(pt: &POINT, r: &RECT) -> bool {
    pt.x >= r.left && pt.x < r.right && pt.y >= r.top && pt.y < r.bottom
}

#[inline]
pub fn rect_intersects(a: &RECT, b: &RECT) -> bool {
    a.left < b.right && a.right > b.left && a.top < b.bottom && a.bottom > b.top
}

/// Apply snap `result` to `window` rect, returning the snapped rect.
///
/// - Edge snaps (Top/Bottom/Left/Right): resize to fill half the screen in that axis,
///   preserving the opposite edge position and orthogonal dimension.
/// - Corner/monitor snaps: replace entirely with target rect.
#[allow(dead_code)]
pub fn apply_snap(window: &RECT, result: &SnapResult, _drag_delta: (i32, i32)) -> RECT {
    match result.anchor {
        ZoneAnchor::Top => {
            // Fill top half: bottom edge → target bottom, preserve left/right edges.
            RECT { left: window.left, top: result.target.top, right: window.right, bottom: result.target.bottom }
        }
        ZoneAnchor::Bottom => {
            // Fill bottom half: top edge → target top, preserve left/right edges.
            RECT { left: window.left, top: result.target.top, right: window.right, bottom: result.target.bottom }
        }
        ZoneAnchor::Left => {
            // Fill left half: right edge → target right, preserve top/bottom edges.
            RECT { left: result.target.left, top: window.top, right: result.target.right, bottom: window.bottom }
        }
        ZoneAnchor::Right => {
            // Fill right half: left edge → target left, preserve top/bottom edges.
            RECT { left: result.target.left, top: window.top, right: result.target.right, bottom: window.bottom }
        }
        // Corner and monitor snaps: replace entirely.
        _ => result.target,
    }
}

/// Clamp `rect` so it stays within `bounds`.
#[allow(dead_code)]
pub fn clamp_to_bounds(rect: &RECT, bounds: &RECT) -> RECT {
    let mut r = *rect;
    if r.left < bounds.left {
        let d = bounds.left - r.left;
        r.left += d;
        r.right += d;
    }
    if r.right > bounds.right {
        let d = r.right - bounds.right;
        r.left -= d;
        r.right -= d;
    }
    if r.top < bounds.top {
        let d = bounds.top - r.top;
        r.top += d;
        r.bottom += d;
    }
    if r.bottom > bounds.bottom {
        let d = r.bottom - bounds.bottom;
        r.top -= d;
        r.bottom -= d;
    }
    r
}

// ─── Win32 FFI ────────────────────────────────────────────────────────────────

#[repr(C)]
struct MONITORINFOEX {
    monitor_info: MONITORINFO,
    sz_device: [u16; 32],
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn work_1920x1080() -> RECT {
        RECT { left: 0, top: 0, right: 1920, bottom: 1080 }
    }

    #[test]
    fn test_build_zones_count() {
        let zones = build_zones(work_1920x1080(), DEFAULT_SNAP_THRESHOLD);
        assert_eq!(zones.len(), 9); // 4 edges + 4 corners + 1 monitor
    }

    #[test]
    fn test_compute_snap_left_edge() {
        let zones = build_zones(work_1920x1080(), DEFAULT_SNAP_THRESHOLD);
        // Window at left edge, x=5, but y in middle (not in corner y-range).
        // Corner TopLeft/BottomLeft y-ranges: [0,20) and [1060,1080).
        // Window y=(200,600) avoids both → pure Left snap.
        let window = RECT { left: 5, top: 200, right: 960, bottom: 600 };
        let result = compute_snap(&window, &zones);
        assert!(result.is_some());
        assert_eq!(result.unwrap().anchor, ZoneAnchor::Left);
    }

    #[test]
    fn test_compute_resize_zone_top_left() {
        let zones = build_zones(work_1920x1080(), DEFAULT_SNAP_THRESHOLD);
        let window = work_1920x1080();
        // Cursor near top edge of window
        let cursor = POINT { x: 100, y: 5 };
        let result = compute_resize_zone(&cursor, &window, &zones);
        assert!(result.is_some());
        assert_eq!(result.unwrap().anchor, ZoneAnchor::TopLeft);
    }
    #[test]
    fn test_compute_resize_zone_center_returns_none() {
        let zones = build_zones(work_1920x1080(), DEFAULT_SNAP_THRESHOLD);
        let window = work_1920x1080();
        // Cursor at center — falls inside Monitor zone trigger but Monitor is not
        // a resize trigger (it's used for explicit double-click Aero Snap).
        // Since Monitor is in the second pass and nothing else matches center, returns None.
        let cursor = POINT { x: 960, y: 540 };
        let result = compute_resize_zone(&cursor, &window, &zones);
        // Center falls in Monitor zone which is not a resize zone → None
        assert!(result.is_none());
    }

    #[test]
    fn test_apply_snap_top() {
        let target = RECT { left: 0, top: 0, right: 1920, bottom: 540 };
        let window = RECT { left: 100, top: 10, right: 1000, bottom: 400 };
        let result = SnapResult { anchor: ZoneAnchor::Top, target };
        let snapped = apply_snap(&window, &result, (0, 0));
        assert_eq!(snapped.top, 0);
        assert_eq!(snapped.bottom, 540);
        assert_eq!(snapped.left, 100); // x unchanged for edge snap
    }

    #[test]
    fn test_apply_snap_corner() {
        let target = RECT { left: 0, top: 0, right: 960, bottom: 540 };
        let window = RECT { left: 100, top: 10, right: 1000, bottom: 400 };
        let result = SnapResult { anchor: ZoneAnchor::TopLeft, target };
        let snapped = apply_snap(&window, &result, (0, 0));
        // Corner snap replaces the entire rect.
        assert_eq!(snapped.left, target.left);
        assert_eq!(snapped.top, target.top);
        assert_eq!(snapped.right, target.right);
        assert_eq!(snapped.bottom, target.bottom);
    }

    #[test]
    fn test_clamp_to_bounds_no_overflow() {
        let bounds = work_1920x1080();
        let r = RECT { left: 500, top: 300, right: 900, bottom: 700 };
        let clamped = clamp_to_bounds(&r, &bounds);
        assert_eq!(clamped.left, r.left);
        assert_eq!(clamped.top, r.top);
        assert_eq!(clamped.right, r.right);
        assert_eq!(clamped.bottom, r.bottom);
    }


    #[test]
    fn test_pt_in_rect() {
        let r = RECT { left: 0, top: 0, right: 100, bottom: 100 };
        assert!(pt_in_rect(&POINT { x: 0, y: 0 }, &r));
        assert!(pt_in_rect(&POINT { x: 50, y: 50 }, &r));
        assert!(!pt_in_rect(&POINT { x: 100, y: 50 }, &r)); // exclusive right
        assert!(!pt_in_rect(&POINT { x: 50, y: 100 }, &r)); // exclusive bottom
    }

    #[test]
    fn test_edge_snap_left() {
        let zones = build_zones(work_1920x1080(), DEFAULT_SNAP_THRESHOLD);
        // Cursor x=15, horizontal drag → Left trigger right=20 → within threshold 20
        let result = compute_edge_snap(15, 0, true, &zones);
        assert!(result.is_some());
        assert_eq!(result.unwrap().anchor, ZoneAnchor::Left);
    }

    #[test]
    fn test_edge_snap_no_match() {
        let zones = build_zones(work_1920x1080(), DEFAULT_SNAP_THRESHOLD);
        // Cursor x=500, no Left/Right zone is within threshold
        let result = compute_edge_snap(500, 0, true, &zones);
        assert!(result.is_none());
    }
}
