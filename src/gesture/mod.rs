//! Window-drag gesture recognition (MMF-style Space-drag + button drag).

use crate::remap::MouseButton;

/// Button that initiates a window drag.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TriggerButton {
    Left,
    Middle,
    X1,
    X2,
}

/// Parse a config string into a [`TriggerButton`].
pub fn parse_button(s: &str) -> TriggerButton {
    match s {
        "middle" => TriggerButton::Middle,
        "x1" => TriggerButton::X1,
        "x2" => TriggerButton::X2,
        _ => TriggerButton::Left,
    }
}

/// Refinement of drag-to-scroll semantics from live modifier keys.
///
/// Mirrors mac `Core/Modifiers/` behaviour: holding a modifier while dragging
/// switches the scroll semantics rather than just scrolling.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DragScrollRefine {
    /// Force all motion onto the horizontal axis (mac: Shift makes vertical
    /// scroll horizontal).
    pub horizontal_only: bool,
    /// Slow the scroll down (mac: Ctrl = precision).
    pub precision: bool,
}

/// Derive drag-to-scroll refinement from the current modifier keys.
pub fn drag_scroll_refine(shift: bool, ctrl: bool) -> DragScrollRefine {
    DragScrollRefine {
        horizontal_only: shift,
        precision: ctrl,
    }
}

struct ActiveDrag {
    hwnd: isize,
    grab_dx: i32,
    grab_dy: i32,
    /// Last seen cursor position, for incremental delta tracking.
    last_x: i32,
    last_y: i32,
    /// Accumulated drag delta since `begin`, for navigate-mode thresholds.
    accum_dx: i32,
    accum_dy: i32,
}

/// Recognizes window-drag gestures from mouse/keyboard events.
pub struct DragController {
    button: TriggerButton,
    active: Option<ActiveDrag>,
}

impl DragController {
    pub fn new(button: TriggerButton) -> Self {
        Self {
            button,
            active: None,
        }
    }

    pub fn is_active(&self) -> bool {
        self.active.is_some()
    }

    pub fn hwnd(&self) -> isize {
        self.active.as_ref().map(|a| a.hwnd).unwrap_or(0)
    }

    pub fn matches_trigger(&self, btn: MouseButton, down: bool, _space: bool) -> bool {
        if !down {
            return false;
        }
        match self.button {
            TriggerButton::Left => btn == MouseButton::Left,
            TriggerButton::Middle => btn == MouseButton::Middle,
            TriggerButton::X1 => btn == MouseButton::X1,
            TriggerButton::X2 => btn == MouseButton::X2,
        }
    }

    /// Begin a drag. `cursor_x`/`cursor_y` seed the incremental tracker.
    pub fn begin(&mut self, hwnd: isize, grab_dx: i32, grab_dy: i32, cursor_x: i32, cursor_y: i32) {
        self.active = Some(ActiveDrag {
            hwnd,
            grab_dx,
            grab_dy,
            last_x: cursor_x,
            last_y: cursor_y,
            accum_dx: 0,
            accum_dy: 0,
        });
    }

    pub fn target_pos(&self, cursor_x: i32, cursor_y: i32) -> Option<(i32, i32)> {
        let a = self.active.as_ref()?;
        Some((cursor_x - a.grab_dx, cursor_y - a.grab_dy))
    }

    /// Incremental drag delta since the last call, also accumulating the total.
    /// Returns `(0, 0)` when no drag is active.
    pub fn consume_delta(&mut self, x: i32, y: i32) -> (i32, i32) {
        let a = match self.active.as_mut() {
            Some(a) => a,
            None => return (0, 0),
        };
        let dx = x - a.last_x;
        let dy = y - a.last_y;
        a.last_x = x;
        a.last_y = y;
        a.accum_dx += dx;
        a.accum_dy += dy;
        (dx, dy)
    }

    /// Total drag delta accumulated since `begin` (for navigate-mode thresholds).
    /// Returns `(0, 0)` when no drag is active.
    pub fn total_delta(&self) -> (i32, i32) {
        let a = match self.active.as_ref() {
            Some(a) => a,
            None => return (0, 0),
        };
        (a.accum_dx, a.accum_dy)
    }

    /// Reset the accumulated total (call after a navigate swipe fires).
    pub fn reset_accum(&mut self) {
        if let Some(a) = self.active.as_mut() {
            a.accum_dx = 0;
            a.accum_dy = 0;
        }
    }

    pub fn matches_release(&self, btn: MouseButton) -> bool {
        if self.active.is_none() {
            return false;
        }
        match self.button {
            TriggerButton::Left => btn == MouseButton::Left,
            TriggerButton::Middle => btn == MouseButton::Middle,
            TriggerButton::X1 => btn == MouseButton::X1,
            TriggerButton::X2 => btn == MouseButton::X2,
        }
    }

    pub fn end(&mut self) {
        self.active = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drag_controller_idle() {
        let ctrl = DragController::new(TriggerButton::Left);
        assert!(!ctrl.is_active());
        assert_eq!(ctrl.hwnd(), 0);
    }

    #[test]
    fn drag_controller_begin_end() {
        let mut ctrl = DragController::new(TriggerButton::Middle);
        ctrl.begin(0x1000, 10, 20, 100, 100);
        assert!(ctrl.is_active());
        assert_eq!(ctrl.hwnd(), 0x1000);
        assert_eq!(ctrl.target_pos(110, 120), Some((100, 100)));
        ctrl.end();
        assert!(!ctrl.is_active());
    }

    #[test]
    fn parse_button_variants() {
        assert!(matches!(parse_button("middle"), TriggerButton::Middle));
        assert!(matches!(parse_button("x1"), TriggerButton::X1));
        assert!(matches!(parse_button("x2"), TriggerButton::X2));
        assert!(matches!(parse_button("left"), TriggerButton::Left));
        assert!(matches!(parse_button("unknown"), TriggerButton::Left));
    }

    #[test]
    fn consume_delta_tracks_incremental_and_total() {
        let mut ctrl = DragController::new(TriggerButton::Left);
        // No active drag -> zero delta, no panic.
        assert_eq!(ctrl.consume_delta(5, 5), (0, 0));

        ctrl.begin(0x100, 10, 20, 100, 100);
        // Move to (120, 90): dx=+20, dy=-10; total equals incremental.
        assert_eq!(ctrl.consume_delta(120, 90), (20, -10));
        assert_eq!(ctrl.total_delta(), (20, -10));
        // Move to (125, 95): dx=+5, dy=+5; total accumulates.
        assert_eq!(ctrl.consume_delta(125, 95), (5, 5));
        assert_eq!(ctrl.total_delta(), (25, -5));
        // Reset clears accumulation but keeps the running last position.
        ctrl.reset_accum();
        assert_eq!(ctrl.total_delta(), (0, 0));
        assert_eq!(ctrl.consume_delta(135, 100), (10, 5));
        assert_eq!(ctrl.total_delta(), (10, 5));
    }

    #[test]
    fn drag_scroll_refine_from_modifiers() {
        assert_eq!(
            drag_scroll_refine(false, false),
            DragScrollRefine {
                horizontal_only: false,
                precision: false
            }
        );
        assert_eq!(
            drag_scroll_refine(true, false),
            DragScrollRefine {
                horizontal_only: true,
                precision: false
            }
        );
        assert_eq!(
            drag_scroll_refine(false, true),
            DragScrollRefine {
                horizontal_only: false,
                precision: true
            }
        );
        assert_eq!(
            drag_scroll_refine(true, true),
            DragScrollRefine {
                horizontal_only: true,
                precision: true
            }
        );
    }

    #[test]
    fn test_drag_scroll_refine_shift_only() {
        let r = drag_scroll_refine(true, false);
        assert!(r.horizontal_only);
        assert!(!r.precision);
    }

    #[test]
    fn test_drag_scroll_refine_ctrl_only() {
        let r = drag_scroll_refine(false, true);
        assert!(!r.horizontal_only);
        assert!(r.precision);
    }

    #[test]
    fn test_drag_scroll_refine_both() {
        let r = drag_scroll_refine(true, true);
        assert!(r.horizontal_only);
        assert!(r.precision);
    }

    #[test]
    fn test_drag_scroll_refine_neither() {
        let r = drag_scroll_refine(false, false);
        assert!(!r.horizontal_only);
        assert!(!r.precision);
    }
}
