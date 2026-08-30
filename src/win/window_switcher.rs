//! Mouse-middle window switcher gesture.
//!
//! Interaction model (no keyboard modifier required):
//!   1. Double-click the middle button → open the Alt+Tab switcher and *hold* Alt.
//!   2. Wheel up/down → move selection (Shift+Tab / Tab) while Alt stays held.
//!   3. Click the middle button again → release Alt and confirm the selection.
//!
//! A single middle click is preserved (not swallowed) via delayed replay: the
//! first middle-down is captured and, if no second press arrives within the
//! double-click window, an equivalent middle click is re-injected so the original
//! button function is unaffected.
//!
//! Safety: if the process dies while Alt is held, the switcher would be stuck.
//! An Alt-timeout auto-cancels the gesture to release the key.

use std::sync::OnceLock;
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, SendInput, INPUT, INPUT_KEYBOARD, INPUT_MOUSE, KEYBDINPUT, KEYEVENTF_KEYUP,
    MOUSEEVENTF_MIDDLEDOWN, MOUSEEVENTF_MIDDLEUP, MOUSEINPUT, VK_ESCAPE, VK_MENU, VK_SHIFT, VK_TAB,
};

/// Marker so our injected events are ignored by our own low-level hooks.
const EXTRA: usize = crate::win::hooks::OUR_MARKER;

/// Max gap between the two middle presses to count as a double-click.
const DOUBLE_CLICK: Duration = Duration::from_millis(500);

/// Auto-cancel the switcher if Alt has been held this long without confirmation.
const ALT_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Clone, Copy, PartialEq, Debug)]
enum Mode {
    Idle,
    Active,
}

struct Switcher {
    mode: Mode,
    alt_down_at: Option<Instant>,
    /// Set on the first middle-down; cleared on double-click (enter) or on timeout (replay).
    pending_middle: Option<Instant>,
}

static STATE: OnceLock<Mutex<Switcher>> = OnceLock::new();

fn state() -> &'static Mutex<Switcher> {
    STATE.get_or_init(|| {
        Mutex::new(Switcher {
            mode: Mode::Idle,
            alt_down_at: None,
            pending_middle: None,
        })
    })
}

/// True while the Alt+Tab switcher is held open by us.
pub fn is_active() -> bool {
    state().lock().mode == Mode::Active
}

/// Called on every middle-button down. Returns true if the event was consumed
/// (should be swallowed by the hook).
pub fn on_middle_down() -> bool {
    let mut s = state().lock();
    match s.mode {
        Mode::Active => {
            // Third middle press = confirm selection.
            s.mode = Mode::Idle;
            s.alt_down_at = None;
            drop(s);
            send_alt_up();
            true
        }
        Mode::Idle => {
            let now = Instant::now();
            match s.pending_middle {
                None => {
                    // First press: capture, wait for a possible second press.
                    s.pending_middle = Some(now);
                    true
                }
                Some(t) if now.duration_since(t) <= DOUBLE_CLICK => {
                    // Second press within window: open the switcher.
                    s.pending_middle = None;
                    s.mode = Mode::Active;
                    s.alt_down_at = Some(now);
                    drop(s);
                    send_alt_tab_enter();
                    true
                }
                Some(_) => {
                    // Stale pending (timeout path should have cleared it); treat as new first.
                    s.pending_middle = Some(now);
                    true
                }
            }
        }
    }
}

/// Called on every middle-button up. Returns true if the event should be swallowed.
pub fn swallow_middle_up() -> bool {
    let s = state().lock();
    s.pending_middle.is_some() || s.mode == Mode::Active
}

/// Driven by the ClickCycle timer (~20ms). Handles delayed single-click replay
/// and the Alt-timeout safety net.
pub fn tick() {
    {
        let mut s = state().lock();
        if let Some(t) = s.pending_middle {
            if t.elapsed() >= DOUBLE_CLICK {
                // No second press arrived: replay a normal middle click so the
                // original single-click function is preserved (just delayed).
                s.pending_middle = None;
                drop(s);
                replay_middle_click();
                return;
            }
        }
        if s.mode == Mode::Active {
            if let Some(at) = s.alt_down_at {
                if at.elapsed() >= ALT_TIMEOUT {
                    s.mode = Mode::Idle;
                    s.alt_down_at = None;
                    drop(s);
                    send_esc();
                    send_alt_up();
                }
            }
        }
    }
}

/// Called from the wheel handler while the switcher is active.
/// `forward` = wheel down (next window); `false` = wheel up (previous window).
pub fn step(forward: bool) {
    {
        let mut s = state().lock();
        match s.mode {
            Mode::Active => {
                if !alt_still_down() {
                    // The system already left the switcher (e.g. user clicked a window).
                    s.mode = Mode::Idle;
                    s.alt_down_at = None;
                    return;
                }
            }
            Mode::Idle => return,
        }
    }
    if forward {
        send_tab();
    } else {
        send_shift_tab();
    }
}

// ─── Low-level input synthesis ────────────────────────────────────────────────

fn send_key(vk: u16, down: bool) {
    unsafe {
        let mut input = INPUT {
            r#type: INPUT_KEYBOARD,
            ..std::mem::zeroed()
        };
        input.Anonymous.ki = KEYBDINPUT {
            wVk: vk,
            wScan: 0,
            dwFlags: if down { 0 } else { KEYEVENTF_KEYUP },
            time: 0,
            dwExtraInfo: EXTRA,
        };
        SendInput(1, &input, std::mem::size_of::<INPUT>() as i32);
    }
}

fn send_alt_tab_enter() {
    send_key(VK_MENU, true); // Alt down (held)
    send_key(VK_TAB, true); // Tab down
    send_key(VK_TAB, false); // Tab up — Alt remains down so the switcher stays open
}

fn send_tab() {
    send_key(VK_TAB, true);
    send_key(VK_TAB, false);
}

fn send_shift_tab() {
    send_key(VK_SHIFT, true);
    send_key(VK_TAB, true);
    send_key(VK_TAB, false);
    send_key(VK_SHIFT, false);
}

fn send_alt_up() {
    send_key(VK_MENU, false);
}

fn send_esc() {
    send_key(VK_ESCAPE, true);
    send_key(VK_ESCAPE, false);
}

fn replay_middle_click() {
    unsafe {
        let mut down = INPUT {
            r#type: INPUT_MOUSE,
            ..std::mem::zeroed()
        };
        down.Anonymous.mi = MOUSEINPUT {
            dx: 0,
            dy: 0,
            mouseData: 0,
            dwFlags: MOUSEEVENTF_MIDDLEDOWN,
            time: 0,
            dwExtraInfo: EXTRA,
        };
        let mut up = INPUT {
            r#type: INPUT_MOUSE,
            ..std::mem::zeroed()
        };
        up.Anonymous.mi = MOUSEINPUT {
            dx: 0,
            dy: 0,
            mouseData: 0,
            dwFlags: MOUSEEVENTF_MIDDLEUP,
            time: 0,
            dwExtraInfo: EXTRA,
        };
        SendInput(1, &down, std::mem::size_of::<INPUT>() as i32);
        SendInput(1, &up, std::mem::size_of::<INPUT>() as i32);
    }
}

fn alt_still_down() -> bool {
    // The high-order bit of the return value is set while the key is down.
    unsafe { GetAsyncKeyState(VK_MENU as i32) < 0 }
}

#[cfg(test)]
mod tests {
    use super::*;
    use parking_lot::Mutex;

    // Serialize the tests: they share the global gesture state.
    static TEST_LOCK: Mutex<()> = Mutex::new(());

    fn reset_state() {
        let mut s = state().lock();
        *s = Switcher {
            mode: Mode::Idle,
            alt_down_at: None,
            pending_middle: None,
        };
    }

    #[test]
    fn middle_double_click_enters_then_confirms() {
        let _g = TEST_LOCK.lock();
        reset_state();
        // First down captured.
        assert!(on_middle_down());
        // Second down within window opens switcher.
        assert!(on_middle_down());
        assert!(is_active());
        // Third down confirms.
        assert!(on_middle_down());
        assert!(!is_active());
    }

    #[test]
    fn single_click_replays_after_timeout() {
        let _g = TEST_LOCK.lock();
        reset_state();
        // Capture first down; no second press; tick past the window replays.
        assert!(on_middle_down());
        // Force the pending timestamp into the past to simulate the timeout.
        {
            let mut s = state().lock();
            s.pending_middle = Some(Instant::now() - DOUBLE_CLICK - Duration::from_millis(1));
        }
        tick(); // should replay and clear pending
        let s = state().lock();
        assert!(s.pending_middle.is_none());
    }
}
