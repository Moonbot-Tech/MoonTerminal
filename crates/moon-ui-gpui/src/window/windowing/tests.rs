//! Regression tests for native-window taskbar suppression lifetime.

use std::sync::{Arc, atomic::AtomicBool};

use gpui::{Bounds, WindowBounds, point, px, size};

use super::{TaskbarHideTask, window_bounds_for};

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
