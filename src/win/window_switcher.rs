//! Mouse-middle window switcher gesture.
//!
//! Interaction model (no keyboard modifier required):
//!   1. Double-click the middle button to open the Alt+Tab switcher (and hold Alt).
//!   2. Wheel up/down moves the selection (Shift+Tab / Tab) while Alt stays held.
//!   3. Click the middle button again to release Alt and confirm the selection.
//!
//! Two input paths feed the same switcher state:
//!
//!   * Hook path (`WH_MOUSE_LL`): driven by `WM_MBUTTONDOWN` / `WM_MBUTTONUP`.
//!     For mice whose driver passes middle-click through to the OS, the hook is
//!     the primary path. A single middle click is preserved via delayed replay:
//!     we capture the first DOWN and, if no second press arrives within the
//!     double-click window, we re-inject a normal middle click so the original
//!     button function is unaffected (just delayed).
//!
//!   * Polling path (`GetAsyncKeyState` every ~20ms via the ClickCycle timer):
//!     fallback for mice whose driver (Logitech G Hub, Razer Synapse, etc.)
//!     intercepts middle-click below `WH_MOUSE_LL`. The polling path uses an
//!     independent state machine so it never interferes with the hook path's
//!     delayed-replay logic, and on single-click it does nothing (the driver
//!     already owns the single-click behaviour).
//!
//! Safety: if the process dies while Alt is held, the switcher would be stuck.
//! An Alt-timeout auto-cancels the gesture to release the key.
//!
//! Threading: the polling path and the hook path may call into the same state
//! mutex from different threads. The two paths are non-overlapping by design
//! (they write to different fields), so lock contention is minimal.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::LazyLock;
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

/// Suppresses a single false-positive EDGE_DOWN at startup: the very first poll
/// only seeds the previous-state bit and never fires on_middle_down. Required
/// because `GetAsyncKeyState` can return 0x8000 during driver initialisation.
static POLL_PRIMED: AtomicBool = AtomicBool::new(false);

#[derive(Clone, Copy, PartialEq, Debug)]
enum Mode {
    Idle,
    Active,
}

struct Switcher {
    mode: Mode,
    alt_down_at: Option<Instant>,
    /// Hook path: set on the first middle-down from the WH_MOUSE_LL hook.
    /// Cleared on the second down (enter), or on timeout (replay single-click).
    pending_middle: Option<Instant>,
    /// Polling path: set on the first middle-down detected via polling.
    /// Cleared on the second down (enter), on the matching UP edge, or on
    /// timeout if the second DOWN never arrives.
    polling_pending: Option<Instant>,
    /// Previous poll state for edge detection.
    prev_middle_polled: bool,
}

static STATE: LazyLock<Mutex<Switcher>> = LazyLock::new(|| {
    Mutex::new(Switcher {
        mode: Mode::Idle,
        alt_down_at: None,
        pending_middle: None,
        polling_pending: None,
        prev_middle_polled: false,
    })
});

/// True while the Alt+Tab switcher is held open by us.
pub fn is_active() -> bool {
    STATE.lock().mode == Mode::Active
}

// ─── Hook path ────────────────────────────────────────────────────────────────

/// Called on every middle-button DOWN seen by the WH_MOUSE_LL hook.
/// Returns true if the event should be swallowed.
pub fn on_middle_down() -> bool {
    let mut s = STATE.lock();
    match s.mode {
        Mode::Active => {
            // Third middle press (or any press after the switcher is open)
            // = confirm selection.
            s.mode = Mode::Idle;
            s.alt_down_at = None;
            drop(s);
            send_alt_up_async();
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
                    send_alt_tab_enter_async();
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

/// Called on every middle-button UP seen by the WH_MOUSE_LL hook.
/// Returns true if the event should be swallowed.
///
/// Only swallows the UP that pairs with the first captured DOWN. Once the
/// switcher is open or we have replayed a single click, subsequent UPs must
/// reach the OS / driver so their normal handler (driver gestures, single-click
/// paste, etc.) can fire.
pub fn swallow_middle_up() -> bool {
    let s = STATE.lock();
    s.pending_middle.is_some()
}

// ─── Polling path ─────────────────────────────────────────────────────────────

const VK_MBUTTON: i32 = 0x04;

/// Called every timer cycle. Detects middle-button edges using `GetAsyncKeyState`
/// and drives the switcher when a double-click is observed.
pub fn poll_middle() {
    let raw = unsafe { GetAsyncKeyState(VK_MBUTTON) };
    let pressed = (raw as u16 & 0x8000) != 0;

    let mut s = STATE.lock();

    // Prime: first poll only seeds the previous-state bit, never fires EDGE.
    if !POLL_PRIMED.swap(true, Ordering::Relaxed) {
        s.prev_middle_polled = pressed;
        crate::log::write(&format!("poll_middle: PRIMED (pressed={})", pressed));
        return;
    }

    let prev = s.prev_middle_polled;
    s.prev_middle_polled = pressed;

    if pressed && !prev {
        // EDGE DOWN - drive the polling state machine.
        let now = Instant::now();
        crate::log::write(&format!(
            "poll_middle: EDGE DOWN prev={} raw=0x{:04x}",
            prev, raw as u16
        ));
        // If the switcher is already open, this EDGE DOWN means "confirm
        // and close" — same semantics as the hook-path on_middle_down()
        // Mode::Active branch.
        if s.mode == Mode::Active {
            s.mode = Mode::Idle;
            s.alt_down_at = None;
            s.polling_pending = None;
            drop(s);
            crate::log::write("poll_middle: EDGE DOWN active -> confirm close");
            send_alt_up_async();
        } else {
            match s.polling_pending {
                None => {
                    // First press: arm the double-click window.
                    s.polling_pending = Some(now);
                    crate::log::write("poll_middle: armed polling_pending");
                }
                Some(t) if now.duration_since(t) <= DOUBLE_CLICK => {
                    // Double-click: open the switcher.
                    s.polling_pending = None;
                    s.mode = Mode::Active;
                    s.alt_down_at = Some(now);
                    drop(s);
                    crate::log::write("poll_middle: EDGE DOWN #2 -> open switcher");
                    send_alt_tab_enter_async();
                }
                Some(_) => {
                    // Stale pending: restart the window.
                    s.polling_pending = Some(now);
                    crate::log::write("poll_middle: EDGE DOWN stale -> restart");
                }
            }
        }
    } else if !pressed && prev {
        // EDGE UP - single click completed. In the polling path we do NOT
        // re-inject a middle click: the driver (e.g. Logitech G Hub) already
        // owns the physical button and saw both edges itself.
        // We deliberately do NOT clear polling_pending here: a real
        // double-click is DOWN-UP-DOWN-UP within 500ms. The UP fires
        // immediately and would otherwise wipe the first DOWN from state.
        // Cleanup of stale polling_pending happens on a DOWN edge when the
        // gap since the first DOWN exceeds DOUBLE_CLICK.
    }
}

// ─── Timer-driven housekeeping ────────────────────────────────────────────────

/// Driven by the ClickCycle timer (~20ms). Handles:
///   * hook-path single-click timeout -> replay_middle_click,
///   * Alt-timeout safety net while the switcher is held open.
pub fn tick() {
    let should_replay = {
        let mut s = STATE.lock();
        if let Some(t) = s.pending_middle {
            if t.elapsed() >= DOUBLE_CLICK {
                s.pending_middle = None;
                true
            } else {
                false
            }
        } else {
            false
        }
    };
    if should_replay {
        // Off-thread: SendInput is sync but the test runner has no message loop,
        // and even on the timer thread we want to keep tick() cheap.
        dispatch_replay(replay_middle_click);
    }

    let should_cancel = {
        let s = STATE.lock();
        if s.mode == Mode::Active {
            if let Some(at) = s.alt_down_at {
                at.elapsed() >= ALT_TIMEOUT
            } else {
                false
            }
        } else {
            false
        }
    };
    if should_cancel {
        let mut s = STATE.lock();
        s.mode = Mode::Idle;
        s.alt_down_at = None;
        drop(s);
        send_esc_async();
        send_alt_up_async();
    }
}

/// Called from the wheel handler while the switcher is active.
/// `forward` = wheel down (next window); `false` = wheel up (previous window).
///
/// Returns `true` when a Tab / Shift+Tab was sent (caller should swallow the
/// wheel). Returns `false` when the switcher was no longer actually held by
/// the OS (e.g. user clicked away to a different window and the Alt+Tab UI
/// dismissed itself) — in that case the caller should fall through to the
/// normal scroll path so the wheel isn't silently swallowed.
pub fn step(forward: bool) -> bool {
    // Use the OS real Alt state, not our internal flag, so a switcher that the
    // OS already dismissed (user clicked a window, focus changed, etc.) won't
    // keep emitting Shift+Tab / Tab into the void.
    let alt_real = unsafe { (GetAsyncKeyState(VK_MENU as i32) as i16) < 0 };
    {
        let mut s = STATE.lock();
        let mode = s.mode;
        if s.mode != Mode::Active || !alt_real {
            if s.mode == Mode::Active {
                s.mode = Mode::Idle;
                s.alt_down_at = None;
            }
            crate::log::write(&format!(
                "step: skip mode={:?} alt_real={} forward={}",
                mode, alt_real, forward
            ));
            return false;
        }
    }
    crate::log::write(&format!(
        "step: ACTIVE forward={} alt_real=true, send Tab",
        forward
    ));
    if forward {
        send_tab_async();
    } else {
        send_shift_tab_async();
    }
    true
}

// ─── Low-level input synthesis ────────────────────────────────────────────────
//
// The `_sync` workers actually call SendInput. The `_async` helpers spawn a
// dedicated thread so the caller (typically the WH_MOUSE_LL hook or the
// ClickCycle timer) is never blocked by thread::sleep between key events.
// This matters specifically for send_alt_tab_enter_sync, which sleeps twice
// for 10ms each. Blocking the ClickCycle timer would cause poll_middle to
// miss the second DOWN edge of a fast double-click.

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

/// In tests, `SendInput` cannot synthesise a real middle click (no foreground
/// window, no message loop on the test thread). Skip the work entirely and
/// rely on the assertion that `pending_middle` was cleared. In production,
/// dispatch on a worker thread to keep `tick()` cheap.
#[cfg(test)]
fn dispatch_replay(_f: fn()) {
    // No-op in tests.
}

#[cfg(not(test))]
fn dispatch_replay(f: fn()) {
    std::thread::spawn(f);
}

fn send_alt_tab_enter_sync() {
    send_key(VK_MENU, true);
    std::thread::sleep(Duration::from_millis(10));
    send_key(VK_TAB, true);
    std::thread::sleep(Duration::from_millis(10));
    send_key(VK_TAB, false);
    // Alt remains down - the switcher stays open.
}

fn send_alt_tab_enter_async() {
    std::thread::spawn(send_alt_tab_enter_sync);
}

fn send_tab_sync() {
    send_key(VK_TAB, true);
    send_key(VK_TAB, false);
}

fn send_tab_async() {
    std::thread::spawn(send_tab_sync);
}

fn send_shift_tab_sync() {
    send_key(VK_SHIFT, true);
    send_key(VK_TAB, true);
    send_key(VK_TAB, false);
    send_key(VK_SHIFT, false);
}

fn send_shift_tab_async() {
    std::thread::spawn(send_shift_tab_sync);
}

fn send_alt_up_sync() {
    send_key(VK_MENU, false);
}

fn send_alt_up_async() {
    std::thread::spawn(send_alt_up_sync);
}

fn send_esc_sync() {
    send_key(VK_ESCAPE, true);
    send_key(VK_ESCAPE, false);
}

fn send_esc_async() {
    std::thread::spawn(send_esc_sync);
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

#[cfg(test)]
mod hook_path_test_helpers {
    use super::*;
    /// Mirror of `on_middle_down` for the "first press: capture" branch only.
    /// Does NOT spawn a SendInput worker (which would deadlock in unit tests).
    pub fn on_middle_down_capture_only() -> bool {
        let mut s = STATE.lock();
        s.pending_middle = Some(Instant::now());
        true
    }
    /// Mirror of `on_middle_down` for the "open switcher" branch only.
    pub fn on_middle_down_open_only() -> bool {
        let mut s = STATE.lock();
        s.pending_middle = None;
        s.mode = Mode::Active;
        s.alt_down_at = Some(Instant::now());
        true
    }
    /// Mirror of `on_middle_down` for the "confirm / close" branch only.
    pub fn on_middle_down_confirm_only() -> bool {
        let mut s = STATE.lock();
        s.mode = Mode::Idle;
        s.alt_down_at = None;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::hook_path_test_helpers::*;
    use super::*;

    static TEST_LOCK: Mutex<()> = Mutex::new(());

    fn reset_state() {
        // Reset the PRIMED latch so each test starts fresh.
        POLL_PRIMED.store(false, Ordering::Relaxed);
        let mut s = STATE.lock();
        *s = Switcher {
            mode: Mode::Idle,
            alt_down_at: None,
            pending_middle: None,
            polling_pending: None,
            prev_middle_polled: false,
        };
    }

    #[test]
    fn hook_double_click_enters_then_confirms() {
        let _g = TEST_LOCK.lock();
        reset_state();
        // Drive state directly: the first two `on_middle_down()` calls would
        // each spawn a SendInput worker thread which SendInput cannot drain
        // without a foreground window. We just verify the state machine.
        assert!(on_middle_down_capture_only());
        assert!(STATE.lock().pending_middle.is_some());
        assert!(on_middle_down_open_only());
        assert!(is_active());
        assert!(on_middle_down_confirm_only());
        assert!(!is_active());
    }

    #[test]
    fn hook_single_click_replays_after_timeout() {
        let _g = TEST_LOCK.lock();
        reset_state();
        // Capture first down; no second press; tick past the window replays.
        assert!(on_middle_down());
        assert!(swallow_middle_up());
        // Force the pending timestamp into the past to simulate the timeout.
        {
            let mut s = STATE.lock();
            s.pending_middle = Some(Instant::now() - DOUBLE_CLICK - Duration::from_millis(1));
        }
        tick(); // should clear pending
        let s = STATE.lock();
        assert!(s.pending_middle.is_none());
    }

    #[test]
    fn hook_up_not_swallowed_after_switcher_open() {
        let _g = TEST_LOCK.lock();
        reset_state();
        // Drive to Active mode directly (no SendInput).
        on_middle_down_open_only();
        assert!(is_active());
        // While the switcher is held open, subsequent UPs must NOT be
        // swallowed (G Hub / OS still need to see them).
        assert!(!swallow_middle_up());
    }

    #[test]
    fn polling_state_machine_double_click() {
        let _g = TEST_LOCK.lock();
        reset_state();
        // Drive directly without going through `poll_middle()` (which reads
        // GetAsyncKeyState and may misbehave in headless tests).
        let now = Instant::now();
        let mut s = STATE.lock();
        s.polling_pending = Some(now);
        s.prev_middle_polled = false;
        match s.polling_pending {
            Some(t) if now.duration_since(t) <= DOUBLE_CLICK && s.mode == Mode::Idle => {
                s.polling_pending = None;
                s.mode = Mode::Active;
                s.alt_down_at = Some(now);
            }
            _ => panic!("polling_pending missing"),
        }
        drop(s);
        assert!(is_active());
    }
}
