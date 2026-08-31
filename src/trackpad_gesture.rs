//! Trackpad gesture recognition — extracted pure logic so iOS vs Android
//! branching can be unit-tested without a browser.
//!
//! The phone-side JS in `assets/trackpad.html` is the source of truth for
//! what gesture fires. This module mirrors the *classification* logic
//! (swipe direction, finger count → gesture name) in Rust so the policy
//! can be verified in `cargo test` without spinning up Playwright.
//!
//! The actual JS detection runs at runtime on `navigator.userAgent`. For
//! these tests we pass the resolved platform in directly so we can cover
//! both branches.

/// Which OS the phone is running. Drives the 3→4 finger shift that
/// avoids Android's three-finger screenshot gesture.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    Android,
    Ios,
    Desktop,
}

/// Minimum finger count that triggers a navigation swipe.
///   * iOS / Desktop : 3 fingers
///   * Android       : 4 fingers (3-finger swipe is the system screenshot)
pub fn min_gesture_fingers(platform: Platform) -> u8 {
    match platform {
        Platform::Android => 4,
        Platform::Ios | Platform::Desktop => 3,
    }
}

/// What to do when the user lifts all fingers after a gesture without
/// swiping (i.e. a tap). For 2 fingers that's right-click on every
/// platform; for the gesture-finger count and above it's taskview.
pub fn tap_action(platform: Platform, finger_count: u8) -> TapAction {
    match finger_count {
        1 => TapAction::LeftClick,
        2 => TapAction::RightClick,
        n if n == min_gesture_fingers(platform) => TapAction::TaskView,
        n if n >= min_gesture_fingers(platform) + 1 => TapAction::ShowDesktop,
        _ => TapAction::Ignore,
    }
}

/// What swipe direction was detected, in the gesture coordinate space.
/// `Up` = finger(s) moved toward the top of the phone screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SwipeAxis {
    /// Dominant axis was horizontal. `Left` / `Right` is the direction.
    Horizontal { left: bool },
    /// Dominant axis was vertical. `Up` / `Down` is the direction.
    Vertical { up: bool },
    /// Both axes exceeded DIAG_MIN and neither dominated (within DIAG_RATIO).
    Diagonal { up: bool, left: bool },
}

/// Classify a swipe from the centroid delta and decide which named
/// gesture to emit. Returns `None` if the gesture is below the
/// minimum-finger threshold (treated as scroll/zoom on 2 fingers, no
/// navigation gesture).
///
/// `committed_count` is the locked finger count from the gesture start.
/// `dx`, `dy` are the centroid displacement from start to current.
pub fn classify_swipe(
    platform: Platform,
    committed_count: u8,
    dx: f32,
    dy: f32,
    swipe_px: f32,
    diag_min: f32,
    diag_ratio: f32,
) -> Option<GestureName> {
    if committed_count < min_gesture_fingers(platform) {
        return None;
    }
    if dx.abs() < swipe_px && dy.abs() < swipe_px {
        return None;
    }
    let axis = classify_axis(dx, dy, diag_min, diag_ratio);
    let is_snap = committed_count >= min_gesture_fingers(platform) + 1;
    Some(match (is_snap, axis) {
        // ── snap (max-finger): window snap to edges / corners
        (true, SwipeAxis::Horizontal { left }) => gesture(if left {
            GestureName::SnapLeft
        } else {
            GestureName::SnapRight
        }),
        (true, SwipeAxis::Vertical { up }) => gesture(if up {
            GestureName::SnapUp
        } else {
            GestureName::SnapDown
        }),
        (true, SwipeAxis::Diagonal { up, left }) => {
            let base = if up { GestureName::SnapUp } else { GestureName::SnapDown };
            gesture(if left {
                GestureName::SnapUpLeft
            } else if matches!(base, GestureName::SnapUp) {
                GestureName::SnapUpRight
            } else if left {
                GestureName::SnapDownLeft
            } else {
                GestureName::SnapDownRight
            })
        }
        // ── navigation (min-finger): virtual desktop / taskview
        (false, SwipeAxis::Horizontal { left }) => gesture(if left {
            GestureName::DesktopLeft
        } else {
            GestureName::DesktopRight
        }),
        (false, SwipeAxis::Vertical { up }) => gesture(if up {
            GestureName::TaskView
        } else {
            GestureName::ShowDesktop
        }),
        (false, SwipeAxis::Diagonal { up, left }) => gesture(if up {
            if left { GestureName::UpLeft } else { GestureName::UpRight }
        } else if left {
            GestureName::DownLeft
        } else {
            GestureName::DownRight
        }),
    })
}

fn gesture(name: GestureName) -> GestureName {
    name
}

fn classify_axis(dx: f32, dy: f32, diag_min: f32, diag_ratio: f32) -> SwipeAxis {
    let ax = dx.abs();
    let ay = dy.abs();
    let diag = ax > diag_min
        && ay > diag_min
        && ax < ay * diag_ratio
        && ay < ax * diag_ratio;
    if diag {
        SwipeAxis::Diagonal {
            up: dy < 0.0,
            left: dx < 0.0,
        }
    } else if ax >= ay {
        SwipeAxis::Horizontal { left: dx < 0.0 }
    } else {
        SwipeAxis::Vertical { up: dy < 0.0 }
    }
}

/// What the receiving end should do when the gesture name is emitted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GestureName {
    DesktopLeft,
    DesktopRight,
    TaskView,
    ShowDesktop,
    UpLeft,
    UpRight,
    DownLeft,
    DownRight,
    SnapUp,
    SnapDown,
    SnapLeft,
    SnapRight,
    SnapUpLeft,
    SnapUpRight,
    SnapDownLeft,
    SnapDownRight,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TapAction {
    LeftClick,
    RightClick,
    TaskView,
    ShowDesktop,
    Ignore,
}

#[cfg(test)]
mod tests {
    use super::*;

    const SWIPE_PX: f32 = 60.0;
    const DIAG_MIN: f32 = 40.0;
    const DIAG_RATIO: f32 = 1.5;

    #[test]
    fn android_uses_four_fingers_to_avoid_system_screenshot() {
        // Three-finger swipe on Android is the system screenshot gesture.
        // Our app must NOT emit a navigation gesture on 3 fingers there —
        // otherwise the OS handler fires first and the user gets a screenshot
        // they didn't ask for.
        assert_eq!(min_gesture_fingers(Platform::Android), 4);
    }

    #[test]
    fn ios_keeps_three_finger_navigation() {
        assert_eq!(min_gesture_fingers(Platform::Ios), 3);
    }

    #[test]
    fn desktop_falls_back_to_three_finger() {
        // Desktop browsers don't have the OS gesture conflict; the original
        // trackpad UX (3-finger nav) wins.
        assert_eq!(min_gesture_fingers(Platform::Desktop), 3);
    }

    // ─── swipe classification: iOS 3-finger / Android 4-finger ─────

    #[test]
    fn ios_three_finger_swipe_up_fires_taskview() {
        let g = classify_swipe(
            Platform::Ios, 3, 0.0, -100.0, SWIPE_PX, DIAG_MIN, DIAG_RATIO,
        );
        assert_eq!(g, Some(GestureName::TaskView));
    }

    #[test]
    fn ios_three_finger_swipe_down_fires_showdesktop() {
        let g = classify_swipe(
            Platform::Ios, 3, 0.0, 100.0, SWIPE_PX, DIAG_MIN, DIAG_RATIO,
        );
        assert_eq!(g, Some(GestureName::ShowDesktop));
    }

    #[test]
    fn ios_three_finger_swipe_left_fires_desktop_left() {
        let g = classify_swipe(
            Platform::Ios, 3, -100.0, 0.0, SWIPE_PX, DIAG_MIN, DIAG_RATIO,
        );
        assert_eq!(g, Some(GestureName::DesktopLeft));
    }

    #[test]
    fn ios_three_finger_swipe_right_fires_desktop_right() {
        let g = classify_swipe(
            Platform::Ios, 3, 100.0, 0.0, SWIPE_PX, DIAG_MIN, DIAG_RATIO,
        );
        assert_eq!(g, Some(GestureName::DesktopRight));
    }

    #[test]
    fn ios_four_finger_swipe_fires_snap_not_navigation() {
        // Four-finger on iOS is the "window snap" tier, not desktop nav.
        let g = classify_swipe(
            Platform::Ios, 4, 0.0, -100.0, SWIPE_PX, DIAG_MIN, DIAG_RATIO,
        );
        assert_eq!(g, Some(GestureName::SnapUp));
    }

    #[test]
    fn android_three_finger_swipe_is_silently_ignored() {
        // The whole reason this branch exists: Android would screenshot on
        // 3 fingers, so we must NOT emit a gesture — return None and let
        // the OS handle it (or let the user move on with no trackpad side
        // effect).
        let g = classify_swipe(
            Platform::Android, 3, 0.0, -100.0, SWIPE_PX, DIAG_MIN, DIAG_RATIO,
        );
        assert_eq!(g, None);
    }

    #[test]
    fn android_four_finger_swipe_up_fires_taskview() {
        // On Android, the "navigation" tier starts at 4 fingers.
        let g = classify_swipe(
            Platform::Android, 4, 0.0, -100.0, SWIPE_PX, DIAG_MIN, DIAG_RATIO,
        );
        assert_eq!(g, Some(GestureName::TaskView));
    }

    #[test]
    fn android_four_finger_swipe_left_fires_desktop_left() {
        let g = classify_swipe(
            Platform::Android, 4, -100.0, 0.0, SWIPE_PX, DIAG_MIN, DIAG_RATIO,
        );
        assert_eq!(g, Some(GestureName::DesktopLeft));
    }

    #[test]
    fn android_five_finger_swipe_fires_snap() {
        let g = classify_swipe(
            Platform::Android, 5, 0.0, -100.0, SWIPE_PX, DIAG_MIN, DIAG_RATIO,
        );
        assert_eq!(g, Some(GestureName::SnapUp));
    }

    #[test]
    fn android_six_finger_swipe_also_snap() {
        // Any count above max-tier should still snap (graceful overflow).
        let g = classify_swipe(
            Platform::Android, 6, 0.0, 100.0, SWIPE_PX, DIAG_MIN, DIAG_RATIO,
        );
        assert_eq!(g, Some(GestureName::SnapDown));
    }

    // ─── tap classification ────────────────────────────────────────

    #[test]
    fn one_finger_tap_is_left_click_on_every_platform() {
        assert_eq!(tap_action(Platform::Ios, 1), TapAction::LeftClick);
        assert_eq!(tap_action(Platform::Android, 1), TapAction::LeftClick);
        assert_eq!(tap_action(Platform::Desktop, 1), TapAction::LeftClick);
    }

    #[test]
    fn two_finger_tap_is_right_click_on_every_platform() {
        assert_eq!(tap_action(Platform::Ios, 2), TapAction::RightClick);
        assert_eq!(tap_action(Platform::Android, 2), TapAction::RightClick);
    }

    #[test]
    fn three_finger_tap_on_ios_fires_taskview() {
        assert_eq!(tap_action(Platform::Ios, 3), TapAction::TaskView);
    }

    #[test]
    fn three_finger_tap_on_android_is_ignored() {
        // Same Android screenshot-avoidance policy: 3 fingers must not emit.
        assert_eq!(tap_action(Platform::Android, 3), TapAction::Ignore);
    }

    #[test]
    fn four_finger_tap_on_android_fires_taskview() {
        assert_eq!(tap_action(Platform::Android, 4), TapAction::TaskView);
    }

    #[test]
    fn four_finger_tap_on_ios_fires_showdesktop() {
        assert_eq!(tap_action(Platform::Ios, 4), TapAction::ShowDesktop);
    }

    #[test]
    fn five_finger_tap_on_android_fires_showdesktop() {
        assert_eq!(tap_action(Platform::Android, 5), TapAction::ShowDesktop);
    }

    // ─── sub-threshold swipes should not emit anything ─────────────

    #[test]
    fn swipe_below_threshold_returns_none() {
        // Tiny jitter shouldn't fire a snap/nav.
        let g = classify_swipe(
            Platform::Ios, 3, 5.0, -5.0, SWIPE_PX, DIAG_MIN, DIAG_RATIO,
        );
        assert_eq!(g, None);
    }

    #[test]
    fn single_axis_motion_below_swipe_px_returns_none() {
        // dx dominates but only barely — within noise band.
        let g = classify_swipe(
            Platform::Ios, 3, 30.0, 5.0, SWIPE_PX, DIAG_MIN, DIAG_RATIO,
        );
        assert_eq!(g, None);
    }

    // ─── diagonal classification ───────────────────────────────────

    #[test]
    fn clear_diagonal_on_ios_three_finger_emits_named_corner() {
        // dx and dy both well above DIAG_MIN and within DIAG_RATIO → diagonal.
        let g = classify_swipe(
            Platform::Ios, 3, -80.0, -80.0, SWIPE_PX, DIAG_MIN, DIAG_RATIO,
        );
        assert_eq!(g, Some(GestureName::UpLeft));
    }

    #[test]
    fn clear_diagonal_on_android_four_finger_emits_named_corner() {
        let g = classify_swipe(
            Platform::Android, 4, -80.0, -80.0, SWIPE_PX, DIAG_MIN, DIAG_RATIO,
        );
        assert_eq!(g, Some(GestureName::UpLeft));
    }

    #[test]
    fn nearly_horizontal_swipe_does_not_count_as_diagonal() {
        // dx=100, dy=5 → ax >> ay, horizontal axis.
        let g = classify_swipe(
            Platform::Ios, 3, -100.0, 5.0, SWIPE_PX, DIAG_MIN, DIAG_RATIO,
        );
        assert_eq!(g, Some(GestureName::DesktopLeft));
    }

    // ─── symmetry: mirrored swipes give mirrored gestures ──────────

    #[test]
    fn mirrored_horizontal_swipes_give_mirrored_gestures() {
        let left = classify_swipe(
            Platform::Ios, 3, -100.0, 0.0, SWIPE_PX, DIAG_MIN, DIAG_RATIO,
        );
        let right = classify_swipe(
            Platform::Ios, 3, 100.0, 0.0, SWIPE_PX, DIAG_MIN, DIAG_RATIO,
        );
        assert_eq!(left, Some(GestureName::DesktopLeft));
        assert_eq!(right, Some(GestureName::DesktopRight));
    }

    #[test]
    fn mirrored_vertical_swipes_give_mirrored_gestures() {
        let up = classify_swipe(
            Platform::Ios, 3, 0.0, -100.0, SWIPE_PX, DIAG_MIN, DIAG_RATIO,
        );
        let down = classify_swipe(
            Platform::Ios, 3, 0.0, 100.0, SWIPE_PX, DIAG_MIN, DIAG_RATIO,
        );
        assert_eq!(up, Some(GestureName::TaskView));
        assert_eq!(down, Some(GestureName::ShowDesktop));
    }

    // ─── regression: iOS 3-finger MUST still classify as nav ───────

    #[test]
    fn regression_ios_3_finger_does_not_snap() {
        // Earlier change raised the iOS threshold to 4 — that broke iOS UX.
        // 3-finger swipe on iOS must remain a navigation gesture.
        let g = classify_swipe(
            Platform::Ios, 3, 0.0, -100.0, SWIPE_PX, DIAG_MIN, DIAG_RATIO,
        );
        assert!(matches!(g, Some(GestureName::TaskView)));
        assert!(!matches!(g, Some(GestureName::SnapUp)));
    }
}
