//! Regression tests for native-window taskbar suppression lifetime.

use std::sync::{Arc, atomic::AtomicBool};

use gpui::{Bounds, WindowBounds, point, px, size};

use super::{TaskbarHideTask, window_bounds_for};

/// Removing the minimum-size cap lets native validation regrow fitted windows past small displays.
#[test]
fn native_minimum_respects_fitted_window_rectangle() {
    for (width, height, expected) in [
        (800.0, 600.0, size(px(800.0), px(560.0))),
        (800.0, 400.0, size(px(800.0), px(400.0))),
        (1200.0, 800.0, size(px(900.0), px(560.0))),
    ] {
        let bounds = Bounds {
            origin: point(px(0.0), px(0.0)),
            size: size(px(width), px(height)),
        };
        for (maximized, fullscreen) in [(false, false), (true, false), (false, true)] {
            let options = super::app_window_options(
                "test",
                window_bounds_for(maximized, fullscreen, bounds),
                None,
                Some(size(px(900.0), px(560.0))),
                super::APP_ID.to_string(),
                None,
                true,
            );
            assert_eq!(options.window_min_size, Some(expected));
        }
    }
}

/// `windowing.rs:TaskbarHideTask::drop` must cancel the exact background burst; removing the Drop
/// call makes this assertion fail and lets a released/replaced window keep issuing COM calls.
#[test]
fn dropping_taskbar_authority_cancels_its_worker() {
    let cancelled = Arc::new(AtomicBool::new(false));
    {
        let _task = TaskbarHideTask {
            cancelled: cancelled.clone(),
        };
    }
    assert!(cancelled.load(std::sync::atomic::Ordering::Acquire));
}

/// `windowing.rs:TaskbarHideTask::cancel` must be idempotent; replacing an activation burst calls
/// cancel before Drop and a non-idempotent transition could panic while the native window is live.
#[test]
fn taskbar_authority_can_be_cancelled_more_than_once() {
    let task = TaskbarHideTask {
        cancelled: Arc::new(AtomicBool::new(false)),
    };
    task.cancel();
    task.cancel();
    assert!(task.is_cancelled());
}

/// `windowing.rs:window_bounds_for` must test fullscreen before maximized; reversing that order
/// would reopen a macOS fullscreen window as an ordinary maximized window instead.
#[test]
fn window_bounds_for_preserves_every_saved_state_with_fullscreen_precedence() {
    let bounds = Bounds {
        origin: point(px(120.0), px(80.0)),
        size: size(px(1024.0), px(768.0)),
    };

    for (maximized, fullscreen, expected_state) in [
        (false, false, "windowed"),
        (true, false, "maximized"),
        (false, true, "fullscreen"),
        (true, true, "fullscreen"),
    ] {
        let (actual_state, actual_bounds) = match window_bounds_for(maximized, fullscreen, bounds) {
            WindowBounds::Windowed(actual_bounds) => ("windowed", actual_bounds),
            WindowBounds::Maximized(actual_bounds) => ("maximized", actual_bounds),
            WindowBounds::Fullscreen(actual_bounds) => ("fullscreen", actual_bounds),
        };
        assert_eq!(
            (actual_state, actual_bounds),
            (expected_state, bounds),
            "maximized={maximized}, fullscreen={fullscreen} must restore the independently specified state and rectangle"
        );
    }
}

/// Discarding saved state with an off-screen rectangle would reopen maximized tools as windowed.
#[test]
fn restored_window_fallback_keeps_native_state() {
    use moon_core::config::layout::{GeomRect, ScreenRect, first_run_window_rect};
    let work = ScreenRect {
        x: 0.0,
        y: 0.0,
        w: 1000.0,
        h: 800.0,
    };
    for (maximized, fullscreen, expected) in
        [(true, false, "maximized"), (false, true, "fullscreen")]
    {
        let saved = GeomRect {
            x: 5000,
            y: 5000,
            w: 2000,
            h: 1500,
            maximized,
            fullscreen,
            display_uuid: None,
        };
        let fitted = saved.restored_on(
            &[(0, 0, 1000, 800)],
            work,
            first_run_window_rect(work, 0.0, 0.0),
        );
        let bounds = Bounds {
            origin: point(px(fitted.x as f32), px(fitted.y as f32)),
            size: size(px(fitted.w as f32), px(fitted.h as f32)),
        };
        let (state, actual) = match super::restored_window_bounds(Some(saved), bounds) {
            WindowBounds::Windowed(b) => ("windowed", b),
            WindowBounds::Maximized(b) => ("maximized", b),
            WindowBounds::Fullscreen(b) => ("fullscreen", b),
        };
        assert_eq!(state, expected);
        assert_eq!(
            actual,
            Bounds {
                origin: point(px(125.0), px(100.0)),
                size: size(px(750.0), px(600.0))
            }
        );
    }
}
