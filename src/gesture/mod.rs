//! Window-drag gesture recognition (MMF-style Space-drag + button drag).
//!
//! The controller is pure state: it decides *when* a drag starts/ends and
//! computes the target window position from the cursor. All Win32 side effects
//! (window lookup, `SetWindowPos`) live in `crate::win::window` and are driven
//! by the hook layer, so the recognition logic stays unit-testable.

use crate::remap::MouseButton;

/// Button that initiates a window drag.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TriggerButton {
    /// Left button while Space is held (MMF signature behavior).
    Left,
    /// Middle button (three-finger-style drag, no Space needed).
    Middle,
    /// X1 (forward) button.
    X1,
    /// X2 (back) button.
    X2,
}

/// Parse a config string into a [`TriggerButton`]. Unknown values default to `Left`.
pub fn parse_button(s: &str) -> TriggerButton {
    match s {
        "middle" => TriggerButton::Middle,
        "x1" => TriggerButton::X1,
        "x2" => TriggerButton::X2,
        _ => TriggerButton::Left,
    }
}

/// Active drag: the window being moved and the cursor offset captured at grab.
struct ActiveDrag {
    hwnd: isize,
    /// cursor.x - window.left at grab time.
    grab_dx: i32,
    /// cursor.y - window.top at grab time.
    grab_dy: i32,
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

    /// The window handle of the in-progress drag, or 0 when idle.
    pub fn hwnd(&self) -> isize {
        self.active.as_ref().map(|a| a.hwnd).unwrap_or(0)
    }

    /// Whether `btn` going `down` should begin a drag, given Space state.
    pub fn matches_trigger(&self, btn: MouseButton, down: bool, space: bool) -> bool {
        if !down {
            return false;
        }
        match self.button {
            TriggerButton::Left => btn == MouseButton::Left && space,
            TriggerButton::Middle => btn == MouseButton::Middle,
            TriggerButton::X1 => btn == MouseButton::X1,
            TriggerButton::X2 => btn == MouseButton::X2,
        }
    }

    /// Begin a drag for `hwnd`, capturing the grab offset (`cursor - window.top_left`).
    pub fn begin(&mut self, hwnd: isize, grab_dx: i32, grab_dy: i32) {
        self.active = Some(ActiveDrag {
            hwnd,
            grab_dx,
            grab_dy,
        });
    }

    /// Target top-left for the current cursor, if a drag is active.
    pub fn target_pos(&self, cursor_x: i32, cursor_y: i32) -> Option<(i32, i32)> {
        self.active
            .as_ref()
            .map(|a| (cursor_x - a.grab_dx, cursor_y - a.grab_dy))
    }

    /// Whether releasing `btn` ends the active drag (must be the trigger button).
    pub fn matches_release(&self, btn: MouseButton) -> bool {
        if self.active.is_none() {
            return false;
        }
        let trigger = match self.button {
            TriggerButton::Left => MouseButton::Left,
            TriggerButton::Middle => MouseButton::Middle,
            TriggerButton::X1 => MouseButton::X1,
            TriggerButton::X2 => MouseButton::X2,
        };
        btn == trigger
    }

    /// End the active drag.
    pub fn end(&mut self) {
        self.active = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::remap::MouseButton;

    #[test]
    fn left_trigger_requires_space() {
        let c = DragController::new(TriggerButton::Left);
        assert!(c.matches_trigger(MouseButton::Left, true, true));
        assert!(!c.matches_trigger(MouseButton::Left, true, false));
        assert!(!c.matches_trigger(MouseButton::Left, false, true));
        assert!(!c.matches_trigger(MouseButton::Middle, true, true));
    }

    #[test]
    fn middle_trigger_ignores_space() {
        let c = DragController::new(TriggerButton::Middle);
        assert!(c.matches_trigger(MouseButton::Middle, true, false));
        assert!(!c.matches_trigger(MouseButton::Left, true, true));
    }

    #[test]
    fn drag_keeps_grab_offset() {
        let mut c = DragController::new(TriggerButton::Left);
        assert!(!c.is_active());
        c.begin(0x1234, 10, 20);
        assert!(c.is_active());
        assert_eq!(c.hwnd(), 0x1234);
        // Cursor moved by (100,100) from grab -> window follows by the same amount.
        assert_eq!(c.target_pos(110, 120), Some((100, 100)));
        assert_eq!(c.target_pos(60, 70), Some((50, 50)));

        // Only the trigger button ends the drag.
        assert!(c.matches_release(MouseButton::Left));
        assert!(!c.matches_release(MouseButton::Middle));
        c.end();
        assert!(!c.is_active());
        assert_eq!(c.target_pos(110, 120), None);
    }

    #[test]
    fn parse_button_defaults_to_left() {
        assert_eq!(parse_button("middle"), TriggerButton::Middle);
        assert_eq!(parse_button("x1"), TriggerButton::X1);
        assert_eq!(parse_button("x2"), TriggerButton::X2);
        assert_eq!(parse_button("left"), TriggerButton::Left);
        assert_eq!(parse_button("bogus"), TriggerButton::Left);
    }
}
