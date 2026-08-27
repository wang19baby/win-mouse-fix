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

/// Max animation duration (1.5 seconds) — mirrors MMF `maxAnimationDuration`.
const MAX_ANIMATION_SECS: f64 = 1.5;
/// Owns the two scroll axes (via WheelTracker) and the channel from the hook layer.
pub struct ScrollInjector {
    rx: Receiver<WheelInput>,
    vertical: WheelTracker,
    horizontal: WheelTracker,

    // ── Per-axis animation state ───────────────────────────────────────────────

    /// Whether vertical axis has an active animation.
    vert_animating: bool,
    /// Normalized time accumulator for vertical animation (0.0–1.0).
    vert_t: f64,
    /// Instant when vertical animation started.
    vert_anim_start: Option<Instant>,
    /// Accumulated scroll distance for vertical animation (wheel units).
    vert_accum: f64,
    /// HybridCurve for vertical axis.
    vert_curve: Option<HybridCurve>,
    /// Total distance for vertical animation.
    vert_total_dist: f64,
    /// Last evaluated fraction of the vertical animation (for leftover computation).
    vert_last_frac: f64,

    /// Whether horizontal axis has an active animation.
    horiz_animating: bool,
    /// Normalized time accumulator for horizontal animation.
    horiz_t: f64,
    /// Instant when horizontal animation started.
    horiz_anim_start: Option<Instant>,
    /// Accumulated scroll distance for horizontal animation.
    horiz_accum: f64,
    /// HybridCurve for horizontal axis.
    horiz_curve: Option<HybridCurve>,
    /// Total distance for horizontal animation.
    horiz_total_dist: f64,
    /// Last evaluated fraction of the horizontal animation (for leftover computation).
    horiz_last_frac: f64,
}

impl ScrollInjector {
    /// Main loop: receive raw wheel events, drive trackers, emit smoothed deltas.
    fn run(mut self) {
        loop {
            let now = Instant::now();

            // ── 1. Receive pending wheel events (physical ticks) ─────────────────
            while let Ok(ev) = self.rx.try_recv() {
                let t = Instant::now();
                if ev.horizontal {
                    let analysis = self.horizontal.on_tick(ev.delta, t);
                    self.handle_physical_tick_horizontal(&analysis, t);
                } else {
                    let analysis = self.vertical.on_tick(ev.delta, t);
                    self.handle_physical_tick_vertical(&analysis, t);
                }
            }

            // ── 2. Animation tick: advance both axes ───────────────────────────
            let dv = self.advance_animation_vertical(now);
            if dv != 0 {
                unsafe { send_wheel(dv, false); }
            }
            let dh = self.advance_animation_horizontal(now);
            if dh != 0 {
                unsafe { send_wheel(dh, true); }
            }

            thread::sleep(Duration::from_millis(TICK_MS));
        }
    }

    // ── Physical tick handlers ────────────────────────────────────────────────

    /// Handle a physical tick on the vertical axis.
    /// `analysis` is consumed here; we use it directly instead of re-querying.
    fn handle_physical_tick_vertical(&mut self, analysis: &ScrollAnalysis, now: Instant) {
        // Direction change: cancel any in-progress animation.
        // Mac Scroll.m lines 505-510: also requires currentAnimationSpeed > 0.
        // Since cancel_vertical() zeros speed, checking self.vert_animating
        // is equivalent (and correct: only cancel if animation was running).
        if analysis.direction_changed && self.vert_animating {
            self.cancel_vertical();
            // Mac Scroll.m line 509: When direction changes mid-swipe
            // (is_new_swipe=false), return without starting a new animation.
            // This "swallows" the direction-change tick and stops momentum cold.
            if !analysis.is_new_swipe {
                return;
            }
        }
        // New swipe: start fresh animation.
        if analysis.is_new_swipe {
            // cancel_vertical() already clears the animation state.
            // axis velocity/state is preserved across swipes per Mac behavior.
            self.start_vertical_animation(analysis, now);
        }
        // Consecutive tick: animation continues via advance_animation_vertical
    }

    /// Handle a physical tick on the horizontal axis.
    fn handle_physical_tick_horizontal(&mut self, analysis: &ScrollAnalysis, now: Instant) {
        if analysis.direction_changed && self.horiz_animating {
            self.cancel_horizontal();
            if !analysis.is_new_swipe {
                return;
            }
        }
        if analysis.is_new_swipe {
            // cancel_horizontal() already clears animation state.
            self.start_horizontal_animation(analysis, now);
        }
    }

    // ── Animation tick ────────────────────────────────────────────────────────

    /// Advance vertical animation by one tick. Returns the i32 delta to emit.
    fn advance_animation_vertical(&mut self, now: Instant) -> i32 {
        if !self.vert_animating {
            // Mac Scroll.m lines 584-586: When animator is not running,
            // flush remaining accumulation and reset subpixelator so the next
            // physical tick starts clean. Also reset accum so leftover distance
            // from the old animation doesn't leak into the new swipe.
            self.vertical.axis_mut().subpixel_flush_and_reset();
            self.vert_accum = 0.0;
            return 0;
        }

        // Current scroll speed from the axis (for direction sign).
        let speed = self.vertical.axis().current_speed();
        if speed <= 0.0 {
            self.cancel_vertical();
            return 0;
        }

        // Compute elapsed time since animation start.
        let dt = self
            .vert_anim_start
            .map(|s| now.duration_since(s).as_secs_f64())
            .unwrap_or(0.0)
            .min(MAX_ANIMATION_SECS);

        // Normalized time (0..1).
        let total_dur = self.vert_curve.as_ref().map(|c| c.total_duration()).unwrap_or(0.140);
        let t_norm = (dt / total_dur).min(1.0);

        if let Some(ref curve) = self.vert_curve {
            // Evaluate the curve to get accumulated distance fraction.
            let new_frac = curve.evaluate(t_norm);
            let new_accum = new_frac * self.vert_total_dist;
            let remaining = new_accum - self.vert_accum;
            self.vert_accum = new_accum;
            self.vert_last_frac = new_frac;
            self.vert_t = t_norm;

            if t_norm >= 1.0 {
                self.vert_animating = false;
                self.vert_curve = None;
            }

            // Sign: negative speed means scroll up (negative delta).
            let signed_delta = if speed < 0.0 {
                -(remaining.abs() as f64)
            } else {
                remaining.abs() as f64
            };

            return self.vertical.subpixel_add(signed_delta);
        }

        0
    }

    /// Advance horizontal animation by one tick. Returns the i32 delta to emit.
    fn advance_animation_horizontal(&mut self, now: Instant) -> i32 {
        if !self.horiz_animating {
            // Mac Scroll.m lines 584-586: flush+reset subpixelator, reset accum.
            self.horizontal.axis_mut().subpixel_flush_and_reset();
            self.horiz_accum = 0.0;
            return 0;
        }

        let speed = self.horizontal.axis().current_speed();
        if speed <= 0.0 {
            self.cancel_horizontal();
            return 0;
        }

        let dt = self
            .horiz_anim_start
            .map(|s| now.duration_since(s).as_secs_f64())
            .unwrap_or(0.0)
            .min(MAX_ANIMATION_SECS);

        let total_dur = self.horiz_curve.as_ref().map(|c| c.total_duration()).unwrap_or(0.140);
        let t_norm = (dt / total_dur).min(1.0);

        if let Some(ref curve) = self.horiz_curve {
            let new_frac = curve.evaluate(t_norm);
            let new_accum = new_frac * self.horiz_total_dist;
            let remaining = new_accum - self.horiz_accum;
            self.horiz_accum = new_accum;
            self.horiz_last_frac = new_frac;
            self.horiz_t = t_norm;

            if t_norm >= 1.0 {
                self.horiz_animating = false;
                self.horiz_curve = None;
            }

            let signed_delta = if speed < 0.0 {
                -(remaining.abs() as f64)
            } else {
                remaining.abs() as f64
            };

            return self.horizontal.subpixel_add(signed_delta);
        }

        0
    }

    // ── Animation lifecycle ───────────────────────────────────────────────────

    fn start_vertical_animation(&mut self, analysis: &ScrollAnalysis, now: Instant) {
        let _speed = self.vertical.axis().current_speed();

        // ── isSwipeSequenceStart: Mac lines 567-569 ─────────────────────────────
        // When a new swipe begins (tick_count==0 && swipe_count==0 after reset),
        // reset subpixelator so no stale accumulation carries into the new swipe.
        if analysis.tick_count == 0 && analysis.swipe_count == 0.0 {
            self.vertical.axis_mut().subpixel_flush_and_reset();
        }

        // ── baseMsPerStepCurve lookup: Mac lines 604-685 ───────────────────────
        // When base_ms_per_step == -1, the curve replaces the fixed base duration.
        // The curve maps tick interval to animation duration: fast wheel (8ms) →
        // max duration (180ms), slow wheel (160ms) → min duration (110ms).
        // This "speedup curve" makes fast swipes more responsive.
        let base_ms = if self.vertical.axis().config_base_ms_per_step() < 0.0 {
            let tbt = analysis.time_between_ticks;
            let t = (tbt - 0.008) / (0.160 - 0.008); // normalized [0,1]
            let t = t.clamp(0.0, 1.0);
            // Exponential curve matching Mac's baseMsPerStepCurve for LowInertia:
            // e1(x) = exp(x * 4.0) - 1, normalized, then scaled [180, 110]
            let exp_val = (t * 4.0_f64).exp() - 1.0;
            let norm = exp_val / ((4.0_f64).exp() - 1.0); // ~1.0 at t=0, ~0.437 at t=1
            180.0 - 70.0 * norm // 180ms at fast wheel, 110ms at slow wheel
        } else {
            self.vertical.axis().config_base_ms_per_step()
        };
        // New tick's contribution to total distance.
        // Mac Scroll.m line 454-460: Evaluate acceleration curve from speed (ticks/s)
        // to get pxToScrollForThisTick (wheel units). The BezierAccelCurve maps
        // tick interval → scroll distance.
        // Mac Scroll.m lines 443-445: When timeBetweenTicks == DBL_MAX (first tick
        // after gap), substitute consecutiveScrollTickIntervalMax.
        let dt = if analysis.time_between_ticks == TIME_BETWEEN_TICKS_NONE {
            TICK_INTERVAL_MAX
        } else {
            analysis.time_between_ticks
        };
        let scroll_speed = 1.0 / dt;
        let tick_dist = self.vertical.axis().accel_curve().evaluate(scroll_speed);
        // Mac Scroll.m line 475-491: Apply fastScrollFactor when swipe_count > 0.
        let speedup_factor = self.vertical.axis().speedup_curve().evaluate(analysis.swipe_count);
        let tick_dist = tick_dist * speedup_factor;
        let remaining = base_carry_over_distance(
            self.vert_animating,
            self.vert_total_dist,
            self.vert_curve.as_ref(),
            self.vert_t,
        );
        let total_dist = tick_dist + remaining;

        let curve = HybridCurve::new(
            self.vertical.axis().accel_curve(),
            base_ms,
            total_dist,
            self.vertical.config_drag_coefficient(),
            self.vertical.config_drag_exponent(),
            self.vertical.config_stop_speed(),
            0.2, // distance_epsilon
        );

        self.vert_animating = true;
        self.vert_t = 0.0;
        self.vert_anim_start = Some(now);
        self.vert_curve = Some(curve);
        self.vert_total_dist = total_dist;
    }
    fn start_horizontal_animation(&mut self, analysis: &ScrollAnalysis, now: Instant) {
        let _speed = self.horizontal.axis().current_speed();

        // ── isSwipeSequenceStart: Mac lines 567-569 ─────────────────────────────
        if analysis.tick_count == 0 && analysis.swipe_count == 0.0 {
            self.horizontal.axis_mut().subpixel_flush_and_reset();
        }

        // ── baseMsPerStepCurve lookup: Mac lines 604-685 ───────────────────────
        let base_ms = if self.horizontal.axis().config_base_ms_per_step() < 0.0 {
            let tbt = analysis.time_between_ticks;
            let t = (tbt - 0.008) / (0.160 - 0.008);
            let t = t.clamp(0.0, 1.0);
            let exp_val = (t * 4.0_f64).exp() - 1.0;
            let norm = exp_val / ((4.0_f64).exp() - 1.0);
            180.0 - 70.0 * norm
        } else {
            self.horizontal.axis().config_base_ms_per_step()
        };

        // New tick's contribution to total distance.
        // Mac Scroll.m line 454-460: Evaluate acceleration curve from speed (ticks/s)
        // to get pxToScrollForThisTick (wheel units). The BezierAccelCurve maps
        // tick interval → scroll distance.
        // Mac Scroll.m lines 443-445: When timeBetweenTicks == DBL_MAX (first tick
        // after gap), substitute consecutiveScrollTickIntervalMax.
        let dt = if analysis.time_between_ticks == TIME_BETWEEN_TICKS_NONE {
            TICK_INTERVAL_MAX
        } else {
            analysis.time_between_ticks
        };
        let scroll_speed = 1.0 / dt;
        let tick_dist = self.horizontal.axis().accel_curve().evaluate(scroll_speed);
        // Mac Scroll.m line 475-491: Apply fastScrollFactor when swipe_count > 0.
        let speedup_factor = self.horizontal.axis().speedup_curve().evaluate(analysis.swipe_count);
        let tick_dist = tick_dist * speedup_factor;
        let remaining = base_carry_over_distance(
            self.horiz_animating,
            self.horiz_total_dist,
            self.horiz_curve.as_ref(),
            self.horiz_t,
        );
        let total_dist = tick_dist + remaining;

        let curve = HybridCurve::new(
            self.horizontal.axis().accel_curve(),
            base_ms,
            total_dist,
            self.horizontal.config_drag_coefficient(),
            self.horizontal.config_drag_exponent(),
            self.horizontal.config_stop_speed(),
            0.2, // distance_epsilon
        );

        self.horiz_animating = true;
        self.horiz_t = 0.0;
        self.horiz_anim_start = Some(now);
        self.horiz_curve = Some(curve);
        self.horiz_total_dist = total_dist;
    }
    fn cancel_vertical(&mut self) {
        self.vert_animating = false;
        self.vert_curve = None;
        self.vert_accum = 0.0;
        self.vert_t = 0.0;
        self.vert_last_frac = 0.0;
        self.vert_anim_start = None;
    }

    fn cancel_horizontal(&mut self) {
        self.horiz_animating = false;
        self.horiz_curve = None;
        self.horiz_accum = 0.0;
        self.horiz_t = 0.0;
        self.horiz_last_frac = 0.0;
        self.horiz_anim_start = None;
    }
}

/// Spawn the injector thread and return a `Sender` for the hook layer to feed.
///
/// Returns `None` if smooth scrolling is disabled in `cfg`.
pub fn start(cfg: &Config) -> Option<Sender<WheelInput>> {
    if !cfg.scroll.enabled || !cfg.scroll.smooth {
        return None;
    }
    let s = &cfg.scroll;
    let (tx, rx) = mpsc::channel();
    let _injector = ScrollInjector {
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
        horiz_animating: false,
        horiz_t: 0.0,
        horiz_anim_start: None,
        horiz_accum: 0.0,
        horiz_curve: None,
        horiz_total_dist: 0.0,
        horiz_last_frac: 0.0,
    };
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
