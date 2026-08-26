//! Global keyboard modifier state.
//!
//! The low-level keyboard hook records Shift/Ctrl/Alt state here; the mouse
//! hook reads it to apply modifier-based scroll behaviors (mac-mouse-fix's
//! "hold a key while scrolling" tricks). Stored as a single atomic bitmask so
//! the hook thread and the scroll injector thread can both touch it cheaply.

use std::sync::atomic::{AtomicU8, Ordering};

use windows_sys::Win32::UI::Input::KeyboardAndMouse::{VK_CONTROL, VK_MENU, VK_SHIFT};

pub const SHIFT: u8 = 1 << 0;
pub const CTRL: u8 = 1 << 1;
pub const ALT: u8 = 1 << 2;

static STATE: AtomicU8 = AtomicU8::new(0);

/// Record a modifier press/release. `vk` is a Windows virtual-key code.
pub fn set_vk(vk: u32, down: bool) {
    let bit = match vk as u16 {
        VK_SHIFT => SHIFT,
        VK_CONTROL => CTRL,
        VK_MENU => ALT,
        _ => return,
    };
    let mut cur = STATE.load(Ordering::SeqCst);
    if down {
        cur |= bit;
    } else {
        cur &= !bit;
    }
    STATE.store(cur, Ordering::SeqCst);
}

pub fn shift_held() -> bool {
    STATE.load(Ordering::SeqCst) & SHIFT != 0
}

/// Apply modifier-based scroll transforms. Pure: `shift_held` is passed in so
/// this is unit-testable without touching global state.
pub fn apply_scroll_modifiers(
    delta: i32,
    horizontal: bool,
    shift_held: bool,
    speedup: f64,
    swap_axis: bool,
) -> (i32, bool) {
    let mut delta = delta as f64;
    let mut horizontal = horizontal;
    if shift_held {
        delta *= speedup;
        if swap_axis {
            horizontal = !horizontal;
        }
    }
    (delta as i32, horizontal)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_shift_leaves_input_unchanged() {
        assert_eq!(apply_scroll_modifiers(120, false, false, 3.0, true), (120, false));
        assert_eq!(apply_scroll_modifiers(120, true, false, 3.0, true), (120, true));
    }

    #[test]
    fn shift_speeds_up_and_swaps_axis() {
        // speedup 3.0: 120 -> 360; swap: vertical -> horizontal.
        assert_eq!(apply_scroll_modifiers(120, false, true, 3.0, true), (360, true));
        // no swap: stays vertical, but sped up.
        assert_eq!(apply_scroll_modifiers(120, false, true, 3.0, false), (360, false));
        // swap without speedup.
        assert_eq!(apply_scroll_modifiers(120, false, true, 1.0, true), (120, true));
    }
}
