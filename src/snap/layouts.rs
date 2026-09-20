//! Snap Layouts: predefined window grid arrangements.
//!
//! Preset grid layouts (2x2, 2x3, 3x3) with hotkeys to snap the foreground
//! window to a specific grid cell. Also supports custom layouts defined in config.

use windows_sys::Win32::Foundation::RECT;

/// A grid layout: rows × columns, with gaps between cells.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Layout {
    pub rows: u8,
    pub cols: u8,
    pub gap: i32,
}

impl Layout {
    /// 2x2 grid (default 4-window layout)
    pub const fn two_by_two() -> Self {
        Layout {
            rows: 2,
            cols: 2,
            gap: 4,
        }
    }

    /// 2x3 grid (6-window layout)
    pub const fn two_by_three() -> Self {
        Layout {
            rows: 2,
            cols: 3,
            gap: 4,
        }
    }

    /// 3x3 grid (9-window layout)
    pub const fn three_by_three() -> Self {
        Layout {
            rows: 3,
            cols: 3,
            gap: 4,
        }
    }

    /// Number of cells in this layout.
    pub fn cell_count(&self) -> usize {
        (self.rows as usize) * (self.cols as usize)
    }

    /// Compute the RECT for cell `index` (0-based, row-major order) within `work`.
    /// Returns None if index is out of range.
    pub fn cell_rect(&self, work: &RECT, index: usize) -> Option<RECT> {
        let total = self.cell_count();
        if index >= total {
            return None;
        }
        let col = (index as i32) % (self.cols as i32);
        let row = (index as i32) / (self.cols as i32);

        let usable_w = work.right - work.left;
        let usable_h = work.bottom - work.top;
        let gap_total_x = (self.cols as i32 - 1) * self.gap;
        let gap_total_y = (self.rows as i32 - 1) * self.gap;
        let cell_w = (usable_w - gap_total_x) / (self.cols as i32);
        let cell_h = (usable_h - gap_total_y) / (self.rows as i32);

        let x = work.left + col * (cell_w + self.gap);
        let y = work.top + row * (cell_h + self.gap);
        Some(RECT {
            left: x,
            top: y,
            right: x + cell_w,
            bottom: y + cell_h,
        })
    }
}

/// Apply `layout` to `hwnd` so it occupies the `index`-th cell of its monitor's grid.
pub fn apply_layout_to_cell(hwnd: isize, layout: Layout, index: usize) {
    if let Some(work) = crate::snap::work_area_of(hwnd) {
        if let Some(rect) = layout.cell_rect(&work, index) {
            crate::win::window::set_window_rect(hwnd, &rect);
        }
    }
}

/// Built-in layouts available via hotkeys.
#[allow(dead_code)]
pub const LAYOUT_2X2: Layout = Layout::two_by_two();
#[allow(dead_code)]
pub const LAYOUT_2X3: Layout = Layout::two_by_three();
pub const LAYOUT_3X3: Layout = Layout::three_by_three();

// ── Config-driven layout cache ────────────────────────────────────────────────

use std::sync::OnceLock;

static LAYOUTS: OnceLock<parking_lot::Mutex<Vec<Layout>>> = OnceLock::new();

/// Update the cached layout list from `configs`. Called on config reload.
pub fn refresh_layouts(configs: &[crate::config::LayoutPreset]) {
    let layouts: Vec<Layout> = configs.iter().map(|c| c.to_layout()).collect();
    LAYOUTS.get_or_init(|| parking_lot::Mutex::new(Vec::new()));
    // Best-effort refresh: if locked, just keep the old list
    if let Some(l) = LAYOUTS.get() {
        *l.lock() = layouts;
    }
}

/// Apply the layout at `layout_index` to `hwnd` at cell `cell_index`.
/// Falls back to LAYOUT_3X3 if no layouts are configured.
pub fn apply_layout_with_index(hwnd: isize, layout_index: usize, cell_index: usize) {
    let layouts = LAYOUTS.get().map(|l| l.lock());
    let layout = match layouts.as_ref() {
        Some(l) if !l.is_empty() => {
            let idx = layout_index % l.len();
            l[idx]
        }
        _ => LAYOUT_3X3,
    };
    let effective_cell = cell_index % layout.cell_count();
    apply_layout_to_cell(hwnd, layout, effective_cell);
}
#[cfg(test)]
mod tests {
    use super::*;

    fn work() -> RECT {
        RECT {
            left: 0,
            top: 0,
            right: 1920,
            bottom: 1080,
        }
    }

    #[test]
    fn test_two_by_two_cells() {
        let layout = LAYOUT_2X2;
        // Cell 0 (top-left)
        let r = layout.cell_rect(&work(), 0).unwrap();
        assert_eq!(r.left, 0);
        assert_eq!(r.top, 0);
        assert_eq!(r.right, 958); // (1920 - 4) / 2
        assert_eq!(r.bottom, 538); // (1080 - 4) / 2

        // Cell 1 (top-right)
        let r = layout.cell_rect(&work(), 1).unwrap();
        assert_eq!(r.left, 962); // 958 + gap
        assert_eq!(r.top, 0);
        assert_eq!(r.right, 1920);
        assert_eq!(r.bottom, 538);

        // Cell 2 (bottom-left)
        let r = layout.cell_rect(&work(), 2).unwrap();
        assert_eq!(r.left, 0);
        assert_eq!(r.top, 542); // 538 + gap
        assert_eq!(r.right, 958);
        assert_eq!(r.bottom, 1080);

        // Cell 3 (bottom-right)
        let r = layout.cell_rect(&work(), 3).unwrap();
        assert_eq!(r.left, 962);
        assert_eq!(r.top, 542);
        assert_eq!(r.right, 1920);
        assert_eq!(r.bottom, 1080);

        // Out of range
        assert!(layout.cell_rect(&work(), 4).is_none());
    }

    #[test]
    fn test_two_by_three_cells() {
        let layout = LAYOUT_2X3;
        assert_eq!(layout.cell_count(), 6);

        // Cell 0 (top-left): x=0, y=0, w=(1920-2*4)/3=637, h=538
        let r = layout.cell_rect(&work(), 0).unwrap();
        assert_eq!(r.left, 0);
        assert_eq!(r.top, 0);
        assert_eq!(r.right, 637);
        // Cell 2: row-major col=2, row=0 → top-right
        let r = layout.cell_rect(&work(), 2).unwrap();
        assert_eq!(r.left, 1282); // 2*(637+4)
        assert_eq!(r.top, 0);

        // Cell 3: row=1, col=0 → bottom-left
        let r = layout.cell_rect(&work(), 3).unwrap();
        assert_eq!(r.left, 0);
        assert_eq!(r.top, 542); // 538 + gap
    }
}
