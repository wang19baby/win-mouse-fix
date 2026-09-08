//! Global keyboard modifier state.
//!
//! The low-level keyboard hook records Shift/Ctrl/Alt state here; the mouse
//! hook reads it to apply modifier-based scroll behaviors (mac-mouse-fix's
//! "hold a key while scrolling" tricks). Stored as a single atomic bitmask so
//! the hook thread and the scroll injector thread can both touch it cheaply.

use std::sync::atomic::{AtomicU64, AtomicU8, Ordering};
use std::sync::LazyLock;
use std::time::Instant;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{VK_CONTROL, VK_MENU, VK_SHIFT};

pub const SHIFT: u8 = 1 << 0;
pub const CTRL: u8 = 1 << 1;
pub const ALT: u8 = 1 << 2;
static STATE: AtomicU8 = AtomicU8::new(0);
/// Nanoseconds since the lazy epoch captured when Shift was last pressed.
/// Zero means "Shift has never been pressed in this process" (or the program
/// started after Shift was already down — caller treats as 0 hold).
static SHIFT_PRESSED_AT_NANOS: AtomicU64 = AtomicU64::new(0);
/// Anchor for shift hold time measurement. Initialised lazily on the first
/// call to `now_nanos`. Lazy avoids racing on Instant::now() at static-init.
static SHIFT_PRESSED_EPOCH: LazyLock<Instant> = LazyLock::new(Instant::now);
/// Capture current Instant relative to the lazy epoch. Returns 0 if the
/// Instant somehow predates the epoch (defensive — shouldn't happen for a
/// monotonic clock). Caller treats 0 as "no measurable hold yet".
fn now_nanos() -> u64 {
    Instant::now()
        .checked_duration_since(*SHIFT_PRESSED_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0)
}
/// Record a modifier press/release. `vk` is a Windows virtual-key code.
///
/// For Shift, the press time is also recorded so that the scroll injector
/// can query `shift_hold_secs()` for nonlinear Shift acceleration (PR-B).
pub fn set_vk(vk: u32, down: bool) {
    let bit = match vk as u16 {
        VK_SHIFT => SHIFT,
        VK_CONTROL => CTRL,
        VK_MENU => ALT,
        _ => return,
    };
    if bit == SHIFT && down {
        // Force-init the epoch BEFORE reading now_nanos: the first time we run,
        // LazyLock::new(Instant::now) would set epoch == now and now_nanos
        // would return 0, which shift_hold_secs() interprets as "never
        // pressed". Touching the LazyLock here anchors the epoch to this
        // instant and then we read the same instant again, guaranteeing a
        // positive nanosecond delta on every press.
        let _ = *SHIFT_PRESSED_EPOCH;
        let pressed = now_nanos();
        // Guard against the (extremely unlikely) case where two consecutive
        // Instant::now() calls return the same value within one press.
        SHIFT_PRESSED_AT_NANOS.store(pressed.max(1), Ordering::SeqCst);
    }
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

/// Seconds since Shift was last pressed down, or `0.0` if Shift is not held
/// or has never been pressed in this process.
///
/// PR-B throttle: the scroll injector calls this to compute the dynamic
/// Shift speed multiplier from the user-configured Bezier curve.
///
/// Atomicity: the read of `STATE` and `SHIFT_PRESSED_AT_NANOS` is not
/// transactional — a concurrent Shift-up event could in theory race with the
/// read and yield a tiny overshoot (we'd compute hold time a few ns past the
/// release). The injector immediately clamps via the curve's post-line, so
/// the visual effect is negligible.
pub fn shift_hold_secs() -> f64 {
    if !shift_held() {
        return 0.0;
    }
    let pressed = SHIFT_PRESSED_AT_NANOS.load(Ordering::SeqCst);
    if pressed == 0 {
        return 0.0;
    }
    let now = now_nanos();
    if now < pressed {
        return 0.0;
    }
    (now - pressed) as f64 / 1_000_000_000.0
}

/// Returns current modifier bitmask (Shift=bit0, Ctrl=bit1, Alt=bit2).
pub fn state() -> u8 {
    STATE.load(Ordering::SeqCst)
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
    use std::sync::Mutex;
    use std::sync::OnceLock;
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{VK_CONTROL, VK_SHIFT};
    static TEST_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    fn test_lock() -> std::sync::MutexGuard<'static, ()> {
        TEST_LOCK
            .get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap_or_else(|e| e.into_inner())
    }

    #[test]
    fn no_shift_leaves_input_unchanged() {
        assert_eq!(
            apply_scroll_modifiers(120, false, false, 3.0, true),
            (120, false)
        );
        assert_eq!(
            apply_scroll_modifiers(120, true, false, 3.0, true),
            (120, true)
        );
    }

    #[test]
    fn shift_speeds_up_and_swaps_axis() {
        // speedup 3.0: 120 -> 360; swap: vertical -> horizontal.
        assert_eq!(
            apply_scroll_modifiers(120, false, true, 3.0, true),
            (360, true)
        );
        // no swap: stays vertical, but sped up.
        assert_eq!(
            apply_scroll_modifiers(120, false, true, 3.0, false),
            (360, false)
        );
        // swap without speedup.
        assert_eq!(
            apply_scroll_modifiers(120, false, true, 1.0, true),
            (120, true)
        );
    }

    // ── PR-B: shift hold-time tracking ────────────────────────────────────────

    fn reset_shift() {
        set_vk(VK_SHIFT as u32, false);
    }

    #[test]
    fn shift_hold_secs_returns_zero_when_not_held() {
        let _g = test_lock();
        reset_shift();
        assert_eq!(shift_hold_secs(), 0.0);
    }

    #[test]
    fn shift_hold_secs_advances_after_synthetic_press() {
        let _g = test_lock();
        reset_shift();
        set_vk(VK_SHIFT as u32, true);
        std::thread::sleep(std::time::Duration::from_millis(50));
        let h = shift_hold_secs();
        assert!(
            h >= 0.04,
            "hold_secs should be >= 0.04 after 50ms sleep; got {h}"
        );
        assert!(
            h < 1.0,
            "hold_secs should be < 1.0 right after a press; got {h}"
        );
        set_vk(VK_SHIFT as u32, false);
        assert_eq!(shift_hold_secs(), 0.0);
    }

    #[test]
    fn shift_hold_secs_increases_monotonically_over_time() {
        let _g = test_lock();
        reset_shift();
        set_vk(VK_SHIFT as u32, true);
        let h1 = shift_hold_secs();
        std::thread::sleep(std::time::Duration::from_millis(20));
        let h2 = shift_hold_secs();
        assert!(
            h2 > h1,
            "hold_secs must increase over time: h1={h1}, h2={h2}"
        );
        set_vk(VK_SHIFT as u32, false);
    }

    #[test]
    fn non_shift_vk_does_not_record_press_time() {
        let _g = test_lock();
        reset_shift();
        set_vk(VK_CONTROL as u32, true);
        std::thread::sleep(std::time::Duration::from_millis(20));
        assert_eq!(shift_hold_secs(), 0.0);
        set_vk(VK_CONTROL as u32, false);
    }
}
