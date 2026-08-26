//! Injector thread: drives the smooth-scroll engines on a fixed cadence and
//! replays the resulting delta stream into the system via `SendInput`.
//!
//! The low-level mouse hook (`hooks.rs`) swallows the raw wheel event and sends
//! it here through a channel; this thread owns the only clock and the only
//! `SendInput` calls, keeping the hook procedure fast and allocation-free.

use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::{Duration, Instant};

use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    SendInput, INPUT, INPUT_MOUSE, MOUSEEVENTF_HWHEEL, MOUSEEVENTF_MOVE, MOUSEEVENTF_WHEEL,
    MOUSEINPUT,
};

use crate::config::Config;
use crate::scroll::engine::{ScrollAxis, WheelInput};

/// Tick cadence (~125 Hz). Fine enough for smooth scroll, cheap enough to idle.
const TICK_MS: u64 = 8;

/// Owns the two scroll axes and the channel from the hook layer.
pub struct ScrollInjector {
    rx: Receiver<WheelInput>,
    vertical: ScrollAxis,
    horizontal: ScrollAxis,
}

impl ScrollInjector {
    fn run(self) {
        let mut this = self;
        loop {
            match this.rx.recv_timeout(Duration::from_millis(TICK_MS)) {
                Ok(ev) => {
                    let now = Instant::now();
                    if ev.horizontal {
                        this.horizontal.on_wheel(ev.delta, now);
                    } else {
                        this.vertical.on_wheel(ev.delta, now);
                    }
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }

            let now = Instant::now();
            let dv = this.vertical.tick(now);
            if dv != 0 {
                unsafe { send_wheel(dv, false); }
            }
            let dh = this.horizontal.tick(now);
            if dh != 0 {
                unsafe { send_wheel(dh, true); }
            }
        }
    }
}

/// Spawn the injector thread and return a `Sender` for the hook layer to feed.
pub fn start(cfg: &Config) -> Sender<WheelInput> {
    let (tx, rx) = mpsc::channel();
    let s = &cfg.scroll;
    let injector = ScrollInjector {
        rx,
        vertical: ScrollAxis::new(s.smooth_level, s.smooth_trend, s.speed, s.friction, s.step),
        horizontal: ScrollAxis::new(s.smooth_level, s.smooth_trend, s.speed, s.friction, s.step),
    };
    thread::spawn(move || injector.run());
    tx
}

/// Synthesize one wheel event via `SendInput`.
unsafe fn send_wheel(delta: i32, horizontal: bool) {
    let mut input = INPUT {
        r#type: INPUT_MOUSE,
        ..std::mem::zeroed()
    };
    input.Anonymous.mi = MOUSEINPUT {
        dx: 0,
        dy: 0,
        mouseData: delta as u32,
        dwFlags: if horizontal {
            MOUSEEVENTF_HWHEEL
        } else {
            MOUSEEVENTF_WHEEL
        },
        time: 0,
        dwExtraInfo: 0,
    };
    SendInput(1, &input, std::mem::size_of::<INPUT>() as i32);
}

/// Synthesize a relative mouse move via `SendInput` (used by pointer accel).
pub fn send_mouse_move(dx: i32, dy: i32) {
    unsafe {
        let mut input = INPUT {
            r#type: INPUT_MOUSE,
            ..std::mem::zeroed()
        };
        input.Anonymous.mi = MOUSEINPUT {
            dx,
            dy,
            mouseData: 0,
            dwFlags: MOUSEEVENTF_MOVE,
            time: 0,
            dwExtraInfo: 0,
        };
        SendInput(1, &input, std::mem::size_of::<INPUT>() as i32);
    }
}
