use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::{Duration, Instant};

use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    INPUT, INPUT_MOUSE, MOUSEEVENTF_HWHEEL, MOUSEEVENTF_MOVE, MOUSEEVENTF_WHEEL, MOUSEINPUT, SendInput,
};

use crate::config::Config;
use crate::scroll::curve::HybridCurve;
use crate::scroll::engine::{base_carry_over_distance, carry_over_distance, WheelInput};
use crate::scroll::wheel_tracker::{ScrollAnalysis, WheelTracker, TIME_BETWEEN_TICKS_NONE, TICK_INTERVAL_MAX};
/// Tick cadence (~125 Hz = 8 ms).
const TICK_MS: u64 = 8;
/// Milliseconds after last tick before coast begins.
const COAST_DELAY_MS: f64 = 80.0;
const MAX_ANIMATION_SECS: f64 = 1.5;
pub struct ScrollInjector {
    rx: Receiver<WheelInput>,
    vertical: WheelTracker,
    horizontal: WheelTracker,

    // ── Vertical animation state ────────────────────────────────────────────────
    /// Whether vertical axis is coasting (post-wheel-stop deceleration).
    vert_animating: bool,
    /// Time when vertical coast animation started.
    vert_anim_start: Option<Instant>,
    /// Accumulated scroll distance from current animation frame.
    vert_accum: f64,
    /// HybridCurve for vertical coast deceleration.
    vert_curve: Option<HybridCurve>,
    /// Total distance for vertical coast animation.
    vert_total_dist: f64,
    /// Last evaluated fraction for vertical curve.
    vert_last_frac: f64,
    /// Timestamp of last physical vertical tick (for coast trigger).
    vert_last_tick: Option<Instant>,
    /// Accumulated scroll distance during active scroll (for coast distance).
    vert_scroll_dist: f64,
    /// Swipe count at last tick (for coast threshold).
    vert_swipe_count: f64,
    /// Current normalized time of vertical animation.
    vert_t: f64,
    /// Whether horizontal axis is coasting.
    horiz_animating: bool,
    /// Normalized time for horizontal animation.
    horiz_t: f64,
    /// Time when horizontal animation started.
    horiz_anim_start: Option<Instant>,
    /// Accumulated scroll distance for horizontal animation.
    horiz_accum: f64,
    /// HybridCurve for horizontal axis.
    horiz_curve: Option<HybridCurve>,
    /// Total distance for horizontal animation.
    horiz_total_dist: f64,
    /// Last evaluated fraction for horizontal curve.
    horiz_last_frac: f64,
    /// Timestamp of last physical horizontal tick.
    horiz_last_tick: Option<Instant>,
    /// Accumulated scroll distance for horizontal scroll.
    horiz_scroll_dist: f64,
}
impl ScrollInjector {
    /// Main loop: receive raw wheel events, drive trackers, emit smoothed deltas.
    fn run(mut self) {
        loop {
            let t0 = Instant::now();

            // ── 1. Receive pending wheel events (physical ticks) ─────────────────
            while let Ok(ev) = self.rx.try_recv() {
                crate::log::write(&format!(
                    "[wheel] rx_event delta={} horiz={}",
                    ev.delta, ev.horizontal
                ));
                if ev.horizontal {
                    let analysis = self.horizontal.on_tick(ev.delta, t0);
                    self.handle_physical_tick_horizontal(&analysis, ev.delta, t0);
                } else {
                    let analysis = self.vertical.on_tick(ev.delta, t0);
                    self.handle_physical_tick_vertical(&analysis, ev.delta, t0);
                }
            }

            // ── 2. Animation tick: advance both axes ───────────────────────────
            let now = Instant::now();
            let dv = self.advance_animation_vertical(now);
            if dv != 0 {
                crate::log::write(&format!(
                    "[wheel] coast_vert dv={} scroll_dist={:.0}",
                    dv, self.vert_scroll_dist
                ));
                unsafe { send_wheel(dv, false) };
            }
            let dh = self.advance_animation_horizontal(now);
            if dh != 0 {
                unsafe { send_wheel(dh, true) };
            }
            thread::sleep(Duration::from_millis(TICK_MS));
        }
    }

    // ── Physical tick handlers ────────────────────────────────────────────────

    /// Handle a physical tick on the vertical axis.
    /// Each physical notch sends 120px immediately, multiplied by the speedup
    /// factor for consecutive notches in the same swipe. No per-tick animation.
    fn handle_physical_tick_vertical(&mut self, analysis: &ScrollAnalysis, delta: i32, now: Instant) {
        // Cancel coast if wheel starts again.
        if self.vert_animating {
            self.cancel_vertical();
        }

        // Flush subpixelator at the start of a new swipe.
        if analysis.is_new_swipe {
            self.vertical.axis_mut().subpixel_flush_and_reset();
            self.vert_scroll_dist = 0.0;
        }

        // Cap multiplier at 5.0x so speed doesn't grow unboundedly.
        let multiplier = (self.vertical.axis().speedup_curve().evaluate(analysis.swipe_count)).min(5.0);
        let tick_dist = (delta.abs() as f64) * multiplier;
        self.vert_scroll_dist += tick_dist;

        crate::log::write(&format!(
            "[wheel] TICK dv={} swipe_count={:.1} multiplier={:.2} tick_dist={:.0}",
            delta, analysis.swipe_count, multiplier, tick_dist
        ));

        // ── Send immediately (each physical notch = 120px) ─────────────────────
        if delta != 0 {
            unsafe { send_wheel(delta, false) };
        }

        // Record tick time and swipe_count for coast detection.
        self.vert_last_tick = Some(now);
        self.vert_swipe_count = analysis.swipe_count;
    }

    /// Handle a physical tick on the horizontal axis.
    fn handle_physical_tick_horizontal(&mut self, analysis: &ScrollAnalysis, delta: i32, now: Instant) {
        if self.horiz_animating {
            self.cancel_horizontal();
        }

        if analysis.is_new_swipe {
            self.horizontal.axis_mut().subpixel_flush_and_reset();
            self.horiz_scroll_dist = 0.0;
        }

        let multiplier = (self.horizontal.axis().speedup_curve().evaluate(analysis.swipe_count)).min(5.0);
        let tick_dist = (delta.abs() as f64) * multiplier;
        self.horiz_scroll_dist += tick_dist;

        if delta != 0 {
            unsafe { send_wheel(delta, true) };
        }

        self.horiz_last_tick = Some(now);
    }

    /// Advance vertical coast. Triggered when no physical ticks arrive for
    /// COAST_DELAY_MS. Sends the remaining coast distance with deceleration.
    fn advance_animation_vertical(&mut self, now: Instant) -> i32 {
        // ── Coast trigger: only start coast if wheel was scrolled enough ────────
        // Require at least 2 consecutive notches (swipe_count >= 2) AND a meaningful
        // scroll distance (>= 240px ≈ 2 notches) before coasting kicks in.
        if !self.vert_animating {
            if let Some(last) = self.vert_last_tick {
                let elapsed = now.duration_since(last).as_secs_f64() * 1000.0;
                let enough_for_coast = self.vert_scroll_dist >= 240.0 && self.vert_swipe_count >= 2.0;
                if elapsed >= COAST_DELAY_MS && enough_for_coast {
                    self.start_vertical_coast(now);
                }
            }
            self.vertical.axis_mut().subpixel_flush_and_reset();
            self.vert_accum = 0.0;
            return 0;
        }

        // ── Advance the coast curve ─────────────────────────────────────────
        let dt = self
            .vert_anim_start
            .map(|s| now.duration_since(s).as_secs_f64())
            .unwrap_or(0.0)
            .min(MAX_ANIMATION_SECS);

        let total_dur = self.vert_curve.as_ref().map(|c| c.total_duration()).unwrap_or(0.5);
        let t_norm = (dt / total_dur).min(1.0);

        if let Some(ref curve) = self.vert_curve {
            let new_frac = curve.evaluate(t_norm);
            let new_accum = new_frac * self.vert_total_dist;
            let remaining = new_accum - self.vert_accum;

            self.vert_accum = new_accum;
            self.vert_last_frac = new_frac;

            if t_norm >= 1.0 || remaining.abs() < 0.5 {
                self.vert_animating = false;
                self.vert_curve = None;
                self.vert_scroll_dist = 0.0;
                return 0;
            }

            let signed_speed = self.vertical.axis().signed_speed();
            let signed_delta = if signed_speed < 0.0 {
                -(remaining.abs() as f64)
            } else {
                remaining.abs() as f64
            };

            return signed_delta as i32;
        }

        0
    }

    /// Advance horizontal coast.
    fn advance_animation_horizontal(&mut self, now: Instant) -> i32 {
        if !self.horiz_animating {
            if let Some(last) = self.horiz_last_tick {
                let elapsed = now.duration_since(last).as_secs_f64() * 1000.0;
                if elapsed >= COAST_DELAY_MS && self.horiz_scroll_dist > 0.0 {
                    self.start_horizontal_coast(now);
                }
            }
            self.horizontal.axis_mut().subpixel_flush_and_reset();
            self.horiz_accum = 0.0;
            return 0;
        }

        let dt = self
            .horiz_anim_start
            .map(|s| now.duration_since(s).as_secs_f64())
            .unwrap_or(0.0)
            .min(MAX_ANIMATION_SECS);

        let total_dur = self.horiz_curve.as_ref().map(|c| c.total_duration()).unwrap_or(0.5);
        let t_norm = (dt / total_dur).min(1.0);

        if let Some(ref curve) = self.horiz_curve {
            let new_frac = curve.evaluate(t_norm);
            let new_accum = new_frac * self.horiz_total_dist;
            let remaining = new_accum - self.horiz_accum;

            self.horiz_accum = new_accum;
            self.horiz_last_frac = new_frac;

            if t_norm >= 1.0 || remaining.abs() < 0.5 {
                self.horiz_animating = false;
                self.horiz_curve = None;
                self.horiz_scroll_dist = 0.0;
                return 0;
            }

            let signed_speed = self.horizontal.axis().signed_speed();
            let signed_delta = if signed_speed < 0.0 {
                -(remaining.abs() as f64)
            } else {
                remaining.abs() as f64
            };

            return self.horizontal.subpixel_add(signed_delta);
        }

        0
    }

    /// Start vertical coast deceleration from the accumulated scroll distance.
    fn start_vertical_coast(&mut self, now: Instant) {
        let signed_speed = self.vertical.axis().signed_speed();
        let direction = if signed_speed < 0.0 { -1.0 } else { 1.0 };
        let total_dist = self.vert_scroll_dist * direction;

        crate::log::write(&format!(
            "[wheel] START_COAST vert_scroll_dist={:.0} total_dist={:.0}",
            self.vert_scroll_dist, total_dist
        ));

        // Use a moderate base_ms for coast — not too fast, not too slow.
        let base_ms = 250.0;
        let curve = HybridCurve::new(
            self.vertical.axis().accel_curve(),
            base_ms,
            total_dist.abs(),
            self.vertical.config_drag_coefficient(),
            self.vertical.config_drag_exponent(),
            self.vertical.config_stop_speed(),
            0.2,
        );

        self.vert_animating = true;
        self.vert_anim_start = Some(now);
        self.vert_accum = 0.0;
        self.vert_curve = Some(curve);
        self.vert_total_dist = total_dist.abs();
        self.vert_last_frac = 0.0;
        self.vert_scroll_dist = 0.0;
    }

    /// Start horizontal coast deceleration.
    fn start_horizontal_coast(&mut self, now: Instant) {
        let signed_speed = self.horizontal.axis().signed_speed();
        let direction = if signed_speed < 0.0 { -1.0 } else { 1.0 };
        let total_dist = self.horiz_scroll_dist * direction;

        let base_ms = 250.0;
        let curve = HybridCurve::new(
            self.horizontal.axis().accel_curve(),
            base_ms,
            total_dist.abs(),
            self.horizontal.config_drag_coefficient(),
            self.horizontal.config_drag_exponent(),
            self.horizontal.config_stop_speed(),
            0.2,
        );

        self.horiz_animating = true;
        self.horiz_anim_start = Some(now);
        self.horiz_accum = 0.0;
        self.horiz_curve = Some(curve);
        self.horiz_total_dist = total_dist.abs();
        self.horiz_last_frac = 0.0;
        self.horiz_scroll_dist = 0.0;
    }

    fn cancel_vertical(&mut self) {
        self.vert_animating = false;
        self.vert_curve = None;
        self.vert_accum = 0.0;
        self.vert_t = 0.0;
        self.vert_last_frac = 0.0;
        self.vert_anim_start = None;
        self.vert_scroll_dist = 0.0;
    }

    fn cancel_horizontal(&mut self) {
        self.horiz_animating = false;
        self.horiz_curve = None;
        self.horiz_accum = 0.0;
        self.horiz_t = 0.0;
        self.horiz_last_frac = 0.0;
        self.horiz_anim_start = None;
        self.horiz_scroll_dist = 0.0;
    }
}

/// Spawn the injector thread and return a `Sender` for the hook layer to feed.
///
/// Returns `None` if smooth scrolling is disabled in `cfg`.
pub fn start(cfg: &Config) -> Option<Sender<WheelInput>> {
    eprintln!("[DEBUG] start() called: scroll.enabled={}, scroll.smooth={}", cfg.scroll.enabled, cfg.scroll.smooth);
    if !cfg.scroll.enabled || !cfg.scroll.smooth {
        eprintln!("[DEBUG] start() early return: enabled={}, smooth={}", cfg.scroll.enabled, cfg.scroll.smooth);
        return None;
    }
    let s = &cfg.scroll;
    let (tx, rx) = mpsc::channel();
    eprintln!("[DEBUG] start(): channel created, about to construct injector");
    let injector = ScrollInjector {
        rx,
        vertical: WheelTracker::new(
            s.drag_exponent, s.drag_coefficient, s.stop_speed, s.speed, s.step,
            s.time_smoothing_weight, s.velocity_a, s.velocity_y,
            s.swipe_threshold, s.swipe_max_interval, s.swipe_min_tick_speed,
            s.smoothness, s.precise, s.tick_interval_accel_end, s.tick_interval_max,
            s.base_ms_per_step,
            s.accel_x_min, s.accel_x_max, s.accel_y_min, s.accel_y_max,
            s.accel_curvature, s.fast_scroll_threshold, s.fast_scroll_initial,
            s.fast_scroll_exponential,
        ),
        horizontal: WheelTracker::new(
            s.drag_exponent, s.drag_coefficient, s.stop_speed, s.speed, s.step,
            s.time_smoothing_weight, s.velocity_a, s.velocity_y,
            s.swipe_threshold, s.swipe_max_interval, s.swipe_min_tick_speed,
            s.smoothness, s.precise, s.tick_interval_accel_end, s.tick_interval_max,
            s.base_ms_per_step,
            s.accel_x_min, s.accel_x_max, s.accel_y_min, s.accel_y_max,
            s.accel_curvature, s.fast_scroll_threshold, s.fast_scroll_initial,
            s.fast_scroll_exponential,
        ),
        vert_animating: false,
        vert_t: 0.0,
        vert_anim_start: None,
        vert_accum: 0.0,
        vert_curve: None,
        vert_total_dist: 0.0,
        vert_last_frac: 0.0,
        vert_last_tick: None,
        vert_scroll_dist: 0.0,
        vert_swipe_count: 0.0,
        horiz_animating: false,
        horiz_t: 0.0,
        horiz_anim_start: None,
        horiz_accum: 0.0,
        horiz_curve: None,
        horiz_total_dist: 0.0,
        horiz_last_frac: 0.0,
        horiz_last_tick: None,
        horiz_scroll_dist: 0.0,
    };
    eprintln!("[DEBUG] start(): injector constructed, spawning thread");
    std::thread::spawn(move || { eprintln!("[DEBUG] injector thread started"); injector.run(); });
    eprintln!("[DEBUG] start(): thread spawned, returning tx");
    Some(tx)
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
        dwExtraInfo: 0xFA57_0000,
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

