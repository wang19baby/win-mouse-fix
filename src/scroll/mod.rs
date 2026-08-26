//! Smooth scrolling engine.
//!
//! Pipeline (mirrors mac-mouse-fix's `Core/Smoothing` + `Core/Scroll`):
//!
//! ```text
//! raw wheel event (hooks.rs)
//!   -> ScrollAxis::on_wheel(delta, now)   estimate & smooth velocity
//!   -> ScrollAxis::tick(now)              emit decaying wheel delta (inertia)
//!   -> SubPixelAccumulator                keep fractional remainder
//!   -> SendInput (injector.rs)            synthesize continuous scroll
//! ```
//!
//! Windows low-level hooks (`WH_MOUSE_LL`) can only *observe & modify* an
//! event, not freely synthesize a continuous stream. So the hook swallows the
//! raw wheel event and a dedicated injector thread replays the smoothed stream
//! via `SendInput`.

pub mod engine;
pub mod injector;
pub mod smoother;
pub mod subpixel;
