//! ScrollAnalyzer — per-axis tick analysis state machine.
//!
//! Ported from `ScrollAnalyzer.m`. For each raw wheel tick we:
//!  1. Detect direction changes (with None-guard: neither direction may be None).
//!  2. Track consecutive tick / swipe counters.
//!  3. Compute smoothed `timeBetweenTicks` using an exponential smoother.
//!  4. Classify ticks as first-in-swipe or consecutive.
//!
//! This is the **analysis** layer only — it produces [`ScrollAnalysis`]
//! which is consumed by the acceleration pipeline in `engine.rs`.

use std::time::Instant;

use crate::scroll::smoother::DoubleExponentialSmoother;

// ─── Constants ────────────────────────────────────────────────────────────────

/// Sentinel value for `time_between_ticks` when the tick is not consecutive
/// with the previous one (gap exceeded `interval_max`). Mirrors mac's `DBL_MAX`.
pub const TIME_BETWEEN_TICKS_NONE: f64 = f64::MAX;

/// Min time between consecutive ticks (seconds). Below this → clamp up.
/// mac: `consecutiveScrollTickIntervalMin = 0.001`
pub const TICK_INTERVAL_MIN: f64 = 0.001;

/// Max time between consecutive ticks (seconds). Above this → new swipe.
/// mac: `consecutiveScrollTickIntervalMax = 0.160`
pub const TICK_INTERVAL_MAX: f64 = 0.160;

/// Time used to initialise the time smoother on the first tick of a swipe.
const SWIPE_INIT_INTERVAL: f64 = TICK_INTERVAL_MAX;

// ─── Direction ────────────────────────────────────────────────────────────────

/// Direction of a wheel tick.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    None,
    Up,
    Down,
}

impl Direction {
    /// Classify a raw wheel delta into a direction.
    /// A delta of 0 is treated as `None` (no movement).
    pub fn from_delta(delta: i32) -> Self {
        if delta > 0 {
            Direction::Down
        } else if delta < 0 {
            Direction::Up
        } else {
            Direction::None
        }
    }
}

/// Returns true only when both directions are non-None and differ.
/// mac: `directionChanged(_previousDirection, direction)` — returns NO if
/// either argument is `kMFDirectionNone`.
fn direction_changed(prev: Direction, curr: Direction) -> bool {
    if prev == Direction::None || curr == Direction::None {
        return false;
    }
    prev != curr
}

// ─── ScrollAnalysis ───────────────────────────────────────────────────────────

/// Result of analyzing one wheel tick. Produced by [`ScrollAnalyzer::on_tick`].
/// Mirrors mac's `ScrollAnalysisResult`.
#[derive(Debug, Clone, Copy)]
pub struct ScrollAnalysis {
    /// Number of consecutive ticks in the current swipe sequence.
    pub tick_count: u32,
    /// Number of fast-flick bursts detected in the current swipe sequence.
    /// Passed to [`ScrollSpeedupCurve`] to compute the fast-scroll multiplier.
    pub swipe_count: f64,
    /// Whether the scroll direction changed relative to the previous tick.
    /// When true the animator must cancel any in-progress animation.
    pub direction_changed: bool,
    /// Smoothed seconds between the previous tick and the current one.
    /// `TIME_BETWEEN_TICKS_NONE` (= f64::MAX) when this is the first tick
    /// of a swipe — the acceleration pipeline substitutes `interval_max`.
    pub time_between_ticks: f64,
    /// Raw (unsmoothed) time between ticks, unclipped.
    /// Mac: `DEBUG_timeBetweenTicksRaw = secondsSinceLastTick` — unclipped.
    pub time_between_ticks_raw: f64,
    /// Number of ticks accumulated in the current consecutive swipe sequence.
    /// Used to compute tick-speed for the `consecutiveScrollSwipeMinTickSpeed` gate.
    pub ticks_in_swipe_sequence: u32,
    /// True when this tick started a new swipe sequence (gap > TICK_INTERVAL_MAX).
    pub is_new_swipe: bool,
}

// ─── ExponentialSmoother ─────────────────────────────────────────────────────

/// Simple exponential (single) smoother for low-latency time tracking.
/// Not the Holt-Winters double-exponential — that lives in `smoother.rs`.
#[derive(Debug, Clone)]
struct ExponentialSmoother {
    weight: f64,
    state: f64,
    initialized: bool,
}

impl ExponentialSmoother {
    fn new(weight: f64) -> Self {
        Self { weight, state: 0.0, initialized: false }
    }

    /// Reset to uninitialised state (next `add` will seed).
    fn reset(&mut self) {
        self.initialized = false;
    }

    /// Add a new sample, returning the smoothed value.
    /// First call seeds the filter; subsequent calls apply exponential smoothing.
    fn add(&mut self, value: f64) -> f64 {
        if !self.initialized {
            self.state = value;
            self.initialized = true;
        } else {
            self.state = self.weight * value + (1.0 - self.weight) * self.state;
        }
        self.state
    }
}

// ─── ScrollAnalyzer ───────────────────────────────────────────────────────────

/// Per-axis scroll analysis state machine.
/// Ported from `ScrollAnalyzer.m`. Stateless from the perspective of the
/// caller — all analysis state is held in static-lifetime variables here.
pub struct ScrollAnalyzer {
    /// Exponential smoother for time-between-ticks.
    time_smoother: ExponentialSmoother,
    /// Double-exponential smoother for velocity (currently unused — kept for parity).
    #[allow(dead_code)]
    velocity_smoother: DoubleExponentialSmoother,
    /// Last observed direction (None sentinel allowed).
    last_dir: Direction,
    /// Timestamp of the previous tick (for interval computation).
    last_tick_time: Option<Instant>,
    /// Rolling tick counter within the current swipe sequence.
    tick_counter: u32,
    /// Rolling swipe burst counter (incremented on each completed fast-flick).
    swipe_counter: u32,
    /// Counter of ticks in the current consecutive swipe sequence.
    /// Incremented each tick; reset to 0 when a new swipe begins.
    ticks_in_swipe_sequence: u32,
    /// Wall-clock time when the current swipe sequence started.
    /// Used to compute average tick-speed for the `consecutiveScrollSwipeMinTickSpeed` gate.
    swipe_sequence_start: Option<Instant>,
    /// The result of the most recent [`on_tick`], accessible via [`last_analysis`].
    last_result: Option<ScrollAnalysis>,

    // ── Config params (wired from ScrollConfig) ─────────────────────────────

    /// Swipe threshold in ticks. If `scrollSwipeThreshold_inTicks > tick_counter`,
    /// the swipe is reset instead of incremented. mac: `scrollConfig.scrollSwipeThreshold_inTicks`
    swipe_threshold: u32,
    /// Maximum interval (seconds) between last tick of previous swipe and first tick
    /// of current swipe. If exceeded, swipe is reset. mac: `consecutiveScrollSwipeMaxInterval`
    swipe_max_interval: f64,
    /// Minimum average tick speed (tick/s) for a swipe sequence to qualify for fast-scroll.
    /// mac: `consecutiveScrollSwipeMinTickSpeed`
    swipe_min_tick_speed: f64,
    /// Smoothness level (0=off, 1=low, 2=regular, 3=high).
    /// mac: `scrollConfig.u_smoothness`
    smoothness: u8,
    /// Whether precision mode is enabled.
    /// mac: `scrollConfig.u_precise`
    precise: bool,
}

     impl ScrollAnalyzer {

    /// Construct a new analyzer.
    ///
    /// `time_smoothing_weight` — exponential smoothing weight for `timeBetweenTicks`.
    /// Higher = more responsive but jittery; lower = smoother but more lag. Default 0.5.
    ///
    /// `velocity_a`, `velocity_y` — double-exponential params for implied velocity
    /// smoothing (currently unused but kept for mac parity).
    ///
    /// `swipe_threshold` — minimum ticks required before a swipe counter increments.
    /// `swipe_max_interval` — max gap (seconds) between swipe bursts.
    /// `swipe_min_tick_speed` — minimum tick speed for fast-scroll activation.
    /// `smoothness` — 0=off, 1=low, 2=regular, 3=high (mac `u_smoothness`).
    /// `precise` — whether precision mode is active (mac `u_precise`).
    pub fn new(
        time_smoothing_weight: f64,
        velocity_a: f64,
        velocity_y: f64,
        swipe_threshold: u32,
        swipe_max_interval: f64,
        swipe_min_tick_speed: f64,
        smoothness: u8,
        precise: bool,
    ) -> Self {
        Self {
            time_smoother: ExponentialSmoother::new(time_smoothing_weight),
            velocity_smoother: DoubleExponentialSmoother::with_seeds(velocity_a, velocity_y, 0.0, 0.0),
            last_dir: Direction::None,
            last_tick_time: None,
            tick_counter: 0,
            swipe_counter: 0,
            ticks_in_swipe_sequence: 0,
            swipe_sequence_start: None,
            last_result: None,
            swipe_threshold,
            swipe_max_interval,
            swipe_min_tick_speed,
            smoothness,
            precise,
        }
    }

    /// Returns true when the upcoming tick would be the first in a new swipe
    /// sequence — i.e., when the gap since the last tick exceeds `TICK_INTERVAL_MAX`,
    /// OR when the direction has changed from the previous tick.
    /// Mirrors `ScrollAnalyzer.peekIsFirstConsecutiveTickWithTickOccuringAt:` (Mac line 98-113).
    pub fn peek_is_first_consecutive_tick(&self, now: Instant, direction: Direction) -> bool {
        // mac checks direction_changed FIRST (line 103)
        if direction_changed(self.last_dir, direction) {
            return true;
        }
        match self.last_tick_time {
            Some(t) => {
                let dt = now.duration_since(t).as_secs_f64();
                dt >= TICK_INTERVAL_MAX
            }
            None => true,
        }
    }

    /// Analyze a raw wheel tick and its timestamp.
    /// Mirrors `ScrollAnalyzer.updateWithTickOccuringAt:`.
    ///
    /// Returns a [`ScrollAnalysis`] struct consumed by the acceleration pipeline.
    pub fn on_tick(&mut self, delta: i32, now: Instant) -> ScrollAnalysis {
        let dir = Direction::from_delta(delta);

        // ── Increment at TOP (mac line 145) ────────────────────────────────────
        self.ticks_in_swipe_sequence += 1;

        // ── Time gap since last tick ───────────────────────────────────────────
        let dt_raw = match self.last_tick_time {
            Some(t) => now.duration_since(t).as_secs_f64(),
            None => f64::MAX,
        };
        let dt_clamped = dt_raw.clamp(TICK_INTERVAL_MIN, TICK_INTERVAL_MAX);

        // ── Direction change detection (mac line 222 / resetState) ─────────────
        let dir_changed = direction_changed(self.last_dir, dir);

        // ── First-consecutive-tick branch (mac lines 147-186) ─────────────────
        let is_new_swipe = dt_raw > TICK_INTERVAL_MAX;
        if is_new_swipe {
            // Guard 1: scrollSwipeThreshold > tick_counter → reset (mac line 154)
            if self.swipe_threshold > self.tick_counter {
                self.swipe_counter = 0;
                self.ticks_in_swipe_sequence = 0;
                self.swipe_sequence_start = Some(now);
            }

            // Guard 2: interval > swipe_max_interval → reset (mac line 162)
            // interval = now - previousTickTime (but we don't store prev, use dt_raw)
            let interval_ok = dt_raw <= self.swipe_max_interval;
            if !interval_ok {
                self.swipe_counter = 0;
                self.ticks_in_swipe_sequence = 0;
                self.swipe_sequence_start = Some(now);
            }

            // Guard 3: tickSpeed < swipe_min_tick_speed → reset (mac line 170)
            let tick_speed = self.swipe_tick_speed(now);
            if tick_speed < self.swipe_min_tick_speed {
                self.swipe_counter = 0;
                self.ticks_in_swipe_sequence = 0;
                self.swipe_sequence_start = Some(now);
            }

            // Increment swipe counter (mac line 175)
            if self.swipe_threshold <= self.tick_counter && interval_ok && tick_speed >= self.swipe_min_tick_speed {
                self.swipe_counter += 1;
            }

            // updateTicks: reset tick_counter (mac line 189)
            self.tick_counter = 0;
        } else {
            // Not first consecutive tick (mac line 191-195)
            self.tick_counter += 1;
        }

        // ── Smoothing (mac lines 209-247) ────────────────────────────────────
        let dt_smoothed = if self.tick_counter == 0 {
            // First tick: reset smoother, seed with tick_max (mac line 220-241)
            self.time_smoother.reset();
            self.time_smoother.add(TICK_INTERVAL_MAX);
            // Time smoother init hack: only add tickMax when smoothness==high && !precise (mac line 239)
            if self.smoothness == 3 && !self.precise {
                self.time_smoother.add(TICK_INTERVAL_MAX);
            }
            TIME_BETWEEN_TICKS_NONE
        } else {
            self.time_smoother.add(dt_clamped)
        };

        // ── Direction change reset ──────────────────────────────────────────────
        if dir_changed {
            self.reset();
        }
        self.last_dir = dir;
        self.last_tick_time = Some(now);

        let result = ScrollAnalysis {
            tick_count: self.tick_counter,
            swipe_count: self.swipe_counter as f64,
            direction_changed: dir_changed,
            time_between_ticks: dt_smoothed,
            // Mac: DEBUG_timeBetweenTicksRaw = secondsSinceLastTick (unclipped).
            // Store unclipped dt_raw for debug parity.
            time_between_ticks_raw: dt_raw,
            ticks_in_swipe_sequence: self.ticks_in_swipe_sequence,
            is_new_swipe,
        };
        self.last_result = Some(result);
        result
    }

    /// Returns the [`ScrollAnalysis`] from the most recent [`on_tick`] call.
    ///
    /// Returns `None` if no tick has been analyzed yet.
    pub fn last_analysis(&self) -> Option<ScrollAnalysis> {
        self.last_result
    }

    /// Called by the injector tick loop to advance swipe sequence tracking.
    /// This is invoked once per physical tick (not per animation tick).
    /// Mirrors the `_ticksInCurrentConsecutiveSwipeSequence` update in `ScrollAnalyzer.m`.
    ///
    /// Returns the updated `ticks_in_swipe_sequence` count.
    pub fn advance_swipe_sequence(&mut self) {
        self.ticks_in_swipe_sequence += 1;
    }

    /// Called when a swipe sequence completes (enough ticks, enough speed,
    /// within the max interval). Increments the swipe counter.
    /// Mirrors `consecutiveScrollSwipeCounter++` in `ScrollAnalyzer.m`.
    pub fn complete_swipe(&mut self) {
        self.swipe_counter += 1;
    }

    /// Reset all internal state. Mirrors `[ScrollAnalyzer resetState]`.
    pub fn reset(&mut self) {
        self.time_smoother.reset();
        self.tick_counter = 0;
        self.swipe_counter = 0;
        self.ticks_in_swipe_sequence = 0;
        self.last_dir = Direction::None;
        self.last_tick_time = None;
        self.swipe_sequence_start = None;
        self.last_result = None;
    }

    /// Current swipe count (for reading by the acceleration pipeline).
    pub fn swipe_count(&self) -> u32 {
        self.swipe_counter
    }

    /// Current tick count within the swipe sequence.
    pub fn tick_count(&self) -> u32 {
        self.tick_counter
    }

    /// Average tick speed (ticks/sec) of the current swipe sequence.
    /// Used to gate fast-scroll activation via `consecutiveScrollSwipeMinTickSpeed`.
    /// Returns 0.0 if the swipe sequence hasn't started.
    pub fn swipe_tick_speed(&self, now: Instant) -> f64 {
        match self.swipe_sequence_start {
            Some(start) => {
                let elapsed = now.duration_since(start).as_secs_f64();
                if elapsed > 0.0 {
                    self.ticks_in_swipe_sequence as f64 / elapsed
                } else {
                    0.0
                }
            }
            None => 0.0,
        }
    }
}

// ─── WheelTracker ─────────────────────────────────────────────────────────────

/// Per-axis wheel tracker — combines [`ScrollAnalyzer`] (tick analysis) with
/// [`ScrollAxis`][`crate::scroll::engine::ScrollAxis`] (velocity / acceleration).
///
/// Wires the three-stage pipeline:
/// ```text
/// Raw wheel event
///   → ScrollAnalyzer::on_tick()   — tick/swipe counters, direction change
///   → ScrollAxis::on_wheel()       — velocity tracking
///   → injector tick loop
///       → ScrollAxis::tick()       — emit smoothed i32 delta
/// ```
///
/// `injector.rs` holds one `WheelTracker` per axis.
pub struct WheelTracker {
    /// Tick analysis state machine (mirrors `ScrollAnalyzer.m`).
    analyzer: ScrollAnalyzer,
    /// Scroll velocity + acceleration engine (mirrors `Scroll.m` heavy processing).
    /// Named `axis` to match the field that `engine.rs` tests access.
    #[allow(dead_code)]
    axis: crate::scroll::engine::ScrollAxis,
}

impl WheelTracker {
    /// Create a tracker for one axis.
    ///
    /// All params forwarded from config via `injector.rs`.
    pub fn new(
        drag_exponent: f64,
        drag_coefficient: f64,
        stop_speed: f64,
        gain: f64,
        step: f64,
        time_smoothing_weight: f64,
        velocity_a: f64,
        velocity_y: f64,
        // ── ScrollAnalyzer config params ─────────────────────────────────────────
        swipe_threshold: u32,
        swipe_max_interval: f64,
        swipe_min_tick_speed: f64,
        smoothness: u8,
        precise: bool,
        // ── ScrollAxis params ────────────────────────────────────────────────────
        accel_end: f64,
        tick_max: f64,
        base_ms_per_step: f64,
        accel_x_min: f64,
        accel_x_max: f64,
        accel_y_min: f64,
        accel_y_max: f64,
        accel_curvature: f64,
        // ── ScrollSpeedupCurve params ───────────────────────────────────────────
        fast_scroll_threshold: u32,
        fast_scroll_initial: f64,
        fast_scroll_exponential: f64,
    ) -> Self {
        let accel = crate::scroll::curve::BezierAccelCurve::new(
            accel_x_min, accel_x_max, accel_y_min, accel_y_max, accel_curvature,
        );
        let speedup = crate::scroll::curve::ScrollSpeedupCurve::new(
            fast_scroll_threshold, fast_scroll_initial, fast_scroll_exponential,
        );
        Self {
            analyzer: ScrollAnalyzer::new(
                time_smoothing_weight,
                velocity_a,
                velocity_y,
                swipe_threshold,
                swipe_max_interval,
                swipe_min_tick_speed,
                smoothness,
                precise,
            ),
            axis: crate::scroll::engine::ScrollAxis::new(
                drag_exponent,
                drag_coefficient,
                stop_speed,
                gain,
                step,
                accel,
                speedup,
                accel_end,
                tick_max,
                base_ms_per_step,
            ),
        }
    }

    /// Analyze a raw wheel tick and feed it to the velocity tracker.
    ///
    /// Call this from `mouse_proc` on `WM_MOUSEWHEEL` / `WM_MOUSEHWHEEL`.
    /// Returns the scroll analysis result for the acceleration pipeline.
    pub fn on_tick(&mut self, delta: i32, now: Instant) -> ScrollAnalysis {
        self.axis.on_wheel(delta, now);
        self.analyzer.on_tick(delta, now)
    }

    /// Access the underlying [`ScrollAnalysis`] analyzer (mutable).
    pub fn analyzer_mut(&mut self) -> &mut ScrollAnalyzer {
        &mut self.analyzer
    }

    /// Access the underlying [`ScrollAnalysis`] analyzer (immutable).
    pub fn analyzer(&self) -> &ScrollAnalyzer {
        &self.analyzer
    }

    /// Access the underlying scroll axis (mutable).
    #[allow(dead_code)]
    pub fn axis_mut(&mut self) -> &mut crate::scroll::engine::ScrollAxis {
        &mut self.axis
    }

    /// Access the underlying scroll axis (immutable).
    #[allow(dead_code)]
    pub fn axis(&self) -> &crate::scroll::engine::ScrollAxis {
        &self.axis
    }

    /// Reset all internal state.
    pub fn reset(&mut self) {
        self.analyzer.reset();
        self.axis.reset();
    }

    /// Whether the axis is currently active (has momentum to emit).
    pub fn is_active(&self) -> bool {
        self.axis.is_active()
    }

    /// Feed a raw wheel delta into the scroll axis (for velocity tracking).
    pub fn feed(&mut self, delta: i32, now: Instant) {
        self.axis.on_wheel(delta, now);
    }

    /// Advance the axis by one tick, returning the integer wheel delta to emit.
    pub fn tick(&mut self, now: Instant) -> i32 {
        self.axis.tick(now)
    }

    /// Add a value to the SubPixelAccumulator, returning the integer delta.
    pub fn subpixel_add(&mut self, value: f64) -> i32 {
        self.axis.subpixel_add(value)
    }

    /// Drag coefficient `a`.
    pub fn config_drag_coefficient(&self) -> f64 {
        self.axis.config_drag_coefficient()
    }

    /// Drag exponent `b`.
    pub fn config_drag_exponent(&self) -> f64 {
        self.axis.config_drag_exponent()
    }

    /// Stop speed.
    pub fn config_stop_speed(&self) -> f64 {
        self.axis.config_stop_speed()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx_eq(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }

    // ── ExponentialSmoother ──────────────────────────────────────────────────

    #[test]
    fn exponential_smoother_seeds_on_first_value() {
        let mut s = ExponentialSmoother::new(0.5);
        assert_eq!(s.add(10.0), 10.0);
    }

    #[test]
    fn exponential_smoother_smooths_subsequent_values() {
        let mut s = ExponentialSmoother::new(0.5);
        // weight=0.5: smoothed = 0.5*new + 0.5*old
        assert_eq!(s.add(10.0), 10.0);
        assert_eq!(s.add(20.0), 15.0);
        assert_eq!(s.add(20.0), 17.5);
    }

    #[test]
    fn exponential_smoother_resets() {
        let mut s = ExponentialSmoother::new(0.5);
        assert_eq!(s.add(10.0), 10.0);
        assert_eq!(s.add(20.0), 15.0);
        s.reset();
        assert_eq!(s.add(5.0), 5.0);
    }

    // ── ScrollAnalyzer::peek_is_first_consecutive_tick ───────────────────────

    #[test]
    fn peek_first_tick_is_always_true() {
        let a = ScrollAnalyzer::new(0.5, 1.0, 1.0, 2, 0.375, 16.0, 3, false);
        let t0 = Instant::now();
        assert!(a.peek_is_first_consecutive_tick(t0, Direction::Down));
    }

    #[test]
    fn peek_within_interval_is_false() {
        let mut a = ScrollAnalyzer::new(0.5, 1.0, 1.0, 2, 0.375, 16.0, 3, false);
        let t0 = Instant::now();
        let t1 = t0 + std::time::Duration::from_secs_f64(0.050); // 50ms < 160ms
        a.on_tick(120, t0);
        assert!(!a.peek_is_first_consecutive_tick(t1, Direction::Down));
    }

    #[test]
    fn peek_beyond_interval_is_true() {
        let mut a = ScrollAnalyzer::new(0.5, 1.0, 1.0, 2, 0.375, 16.0, 3, false);
        let t0 = Instant::now();
        let t1 = t0 + std::time::Duration::from_secs_f64(0.200); // 200ms >= 160ms
        a.on_tick(120, t0);
        assert!(a.peek_is_first_consecutive_tick(t1, Direction::Down));
    }

    // ── ScrollAnalyzer::on_tick ────────────────────────────────────────────

    #[test]
    fn first_tick_sets_none_time_between_ticks() {
        let mut a = ScrollAnalyzer::new(0.5, 1.0, 1.0, 2, 0.375, 16.0, 3, false);
        let t0 = Instant::now();
        let r = a.on_tick(120, t0);
        assert_eq!(r.tick_count, 0);
        // First tick: time_between_ticks is the sentinel value (mirrors mac's DBL_MAX)
        assert_eq!(r.time_between_ticks, TIME_BETWEEN_TICKS_NONE);
        assert!(r.is_new_swipe);
    }

    #[test]
    fn second_tick_smooths_time_between_ticks() {
        let mut a = ScrollAnalyzer::new(0.5, 1.0, 1.0, 2, 0.375, 16.0, 3, false);
        let t0 = Instant::now();
        let t1 = t0 + std::time::Duration::from_secs_f64(0.080); // 80ms
        a.on_tick(120, t0);
        let r = a.on_tick(120, t1);
        assert_eq!(r.tick_count, 1);
        assert!(!r.time_between_ticks.is_infinite());
        assert!(!r.is_new_swipe);
        // weight=0.5: smoothed = 0.5*80ms + 0.5*160ms = 120ms
        assert!(approx_eq(r.time_between_ticks, 0.120));
    }

    #[test]
    fn direction_change_resets_counters() {
        let mut a = ScrollAnalyzer::new(0.5, 1.0, 1.0, 2, 0.375, 16.0, 3, false);
        let t0 = Instant::now();
        let t1 = t0 + std::time::Duration::from_millis(50);
        let t2 = t0 + std::time::Duration::from_millis(100);
        a.on_tick(120, t0);  // tick_count=0
        a.on_tick(120, t1);  // tick_count=1
        let r = a.on_tick(-120, t2); // direction changed, gap=50ms < 160ms
        assert_eq!(r.tick_count, 0);
        assert!(r.direction_changed);
    }

    #[test]
    fn direction_change_none_guard() {
        let mut a = ScrollAnalyzer::new(0.5, 1.0, 1.0, 2, 0.375, 16.0, 3, false);
        let t0 = Instant::now();
        let t1 = t0 + std::time::Duration::from_millis(50);
        // First tick: last_dir=None → direction_changed must be false
        a.on_tick(120, t0);
        // Second tick same direction
        let r = a.on_tick(120, t1);
        assert!(!r.direction_changed);
    }

    #[test]
    fn is_new_swipe_on_long_gap() {
        let mut a = ScrollAnalyzer::new(0.5, 1.0, 1.0, 2, 0.375, 16.0, 3, false);
        let t0 = Instant::now();
        let t1 = t0 + std::time::Duration::from_millis(50);
        let t2 = t0 + std::time::Duration::from_secs_f64(0.300); // 300ms > 160ms
        a.on_tick(120, t0);
        a.on_tick(120, t1);
        let r = a.on_tick(120, t2);
        assert!(r.is_new_swipe);
        assert_eq!(r.tick_count, 0);
    }

    #[test]
    fn advance_swipe_sequence_increments() {
        let mut a = ScrollAnalyzer::new(0.5, 1.0, 1.0, 2, 0.375, 16.0, 3, false);
        let t0 = Instant::now();
        let t1 = t0 + std::time::Duration::from_millis(50);
        let t2 = t0 + std::time::Duration::from_millis(100);
        // First tick: is_new_swipe, ticks_in_swipe_sequence = 0
        a.on_tick(120, t0);
        assert_eq!(a.ticks_in_swipe_sequence, 0);
        // Second tick: consecutive, ticks_in_swipe_sequence += 1 → 1
        a.on_tick(120, t1);
        assert_eq!(a.ticks_in_swipe_sequence, 1);
        // Third tick: consecutive, ticks_in_swipe_sequence += 1 → 2
        a.on_tick(120, t2);
        assert_eq!(a.ticks_in_swipe_sequence, 2);
    }
    #[test]
    fn reset_clears_all_state() {
        let mut a = ScrollAnalyzer::new(0.5, 1.0, 1.0, 2, 0.375, 16.0, 3, false);
        let t0 = Instant::now();
        let t1 = t0 + std::time::Duration::from_millis(50);
        a.on_tick(120, t0);  // tick 0: tick_counter=0
        a.on_tick(120, t1);  // tick 1: tick_counter=1 (consecutive)
        a.complete_swipe();
        assert_eq!(a.swipe_counter, 1);
        assert_eq!(a.tick_counter, 1);
        a.reset();
        assert_eq!(a.swipe_counter, 0);
        assert_eq!(a.tick_counter, 0);
        assert_eq!(a.ticks_in_swipe_sequence, 0);
        assert_eq!(a.last_dir, Direction::None);
    }

    #[test]
    fn complete_swipe_increments_counter() {
        let mut a = ScrollAnalyzer::new(0.5, 1.0, 1.0, 2, 0.375, 16.0, 3, false);
        let t0 = Instant::now();
        let t1 = t0 + std::time::Duration::from_millis(50);
        let t2 = t0 + std::time::Duration::from_millis(100);
        a.on_tick(120, t0);  // tick 0
        a.on_tick(120, t1);  // tick 1 (consecutive, swipe_sequence += 1)
        a.on_tick(120, t2);  // tick 2 (consecutive, swipe_sequence += 1)
        a.complete_swipe();
        assert_eq!(a.swipe_counter, 1);
        a.complete_swipe();
        assert_eq!(a.swipe_counter, 2);
    }

    #[test]
    fn swipe_tick_speed_computes_average() {
        let mut a = ScrollAnalyzer::new(0.5, 1.0, 1.0, 2, 0.375, 16.0, 3, false);
        let t0 = Instant::now();
        // Three ticks within one swipe (gaps < 160ms):
        // tick 0: is_new_swipe, ticks_in_swipe_sequence=0
        // tick 1 (t0+50ms): consecutive, ticks_in_swipe_sequence=1
        // tick 2 (t0+100ms): consecutive, ticks_in_swipe_sequence=2
        let t1 = t0 + std::time::Duration::from_millis(50);
        let t2 = t0 + std::time::Duration::from_millis(100);
        let t3 = t0 + std::time::Duration::from_millis(500);
        let t4 = t0 + std::time::Duration::from_secs_f64(1.0);
        a.on_tick(120, t0);
        a.on_tick(120, t1); // consecutive → ticks_in_swipe_sequence += 1 → 1
        a.on_tick(120, t2); // consecutive → ticks_in_swipe_sequence += 1 → 2
        // At t2: 2 ticks in 100ms → 20 tick/s
        // After 400ms more (t3): still 2 ticks in 500ms → 4 tick/s
        let speed = a.swipe_tick_speed(t3);
        assert!(approx_eq(speed, 4.0));
        // After 900ms more (t4): 2 ticks in 1000ms → 2 tick/s
        let speed2 = a.swipe_tick_speed(t4);
        assert!(approx_eq(speed2, 2.0));
    }

    #[test]
    fn swipe_tick_speed_zero_before_start() {
        let a = ScrollAnalyzer::new(0.5, 1.0, 1.0, 2, 0.375, 16.0, 3, false);
        let t0 = Instant::now();
        assert!(approx_eq(a.swipe_tick_speed(t0), 0.0));
    }

    // ── direction_changed ───────────────────────────────────────────────────

    #[test]
    fn direction_changed_false_when_either_is_none() {
        assert!(!direction_changed(Direction::None, Direction::Down));
        assert!(!direction_changed(Direction::Up, Direction::None));
        assert!(!direction_changed(Direction::None, Direction::None));
    }

    #[test]
    fn direction_changed_true_when_both_non_none_and_differ() {
        assert!(direction_changed(Direction::Up, Direction::Down));
        assert!(direction_changed(Direction::Down, Direction::Up));
    }

    #[test]
    fn direction_changed_false_when_same() {
        assert!(!direction_changed(Direction::Up, Direction::Up));
        assert!(!direction_changed(Direction::Down, Direction::Down));
    }

    // ── Direction::from_delta ───────────────────────────────────────────────

    #[test]
    fn direction_from_delta_positive_is_down() {
        assert_eq!(Direction::from_delta(1), Direction::Down);
        assert_eq!(Direction::from_delta(120), Direction::Down);
    }

    #[test]
    fn direction_from_delta_negative_is_up() {
        assert_eq!(Direction::from_delta(-1), Direction::Up);
        assert_eq!(Direction::from_delta(-120), Direction::Up);
    }

    #[test]
    fn direction_from_delta_zero_is_none() {
        assert_eq!(Direction::from_delta(0), Direction::None);
    }
}
