//! Regression coverage for the toolbar's Live/Pause camera transition.

use super::ChartEngine;
use moon_core::config::ChartTheme;

/// Resuming Live must preserve a manually chosen time scale at both narrow and wide plot widths.
#[test]
fn pause_and_live_preserve_time_zoom_through_the_next_prepare() {
    for width in [320.0, 1400.0] {
        let now = 1_700_000_000_000.0;
        let mut engine = ChartEngine::new(now, ChartTheme::default());
        engine.open(42, "TESTUSDT");
        let ppm = {
            let mut container = engine.container.borrow_mut();
            let view = &mut container.panes_mut().first_mut().expect("main pane").view;
            view.ensure_default_window(width, 60.0, None);
            view.zoom_x_at(128.0, width, width * 0.5, now, false);
            view.px_per_ms
        };
        for cycle in 1..=3 {
            assert!(engine.set_follow(false, now + cycle as f64 * 1000.0));
            assert!(engine.set_follow(true, now + cycle as f64 * 1000.0 + 500.0));
            let mut container = engine.container.borrow_mut();
            let view = &mut container.panes_mut().first_mut().expect("main pane").view;
            view.ensure_default_window(width, 60.0, None);
            assert_eq!(view.px_per_ms, ppm, "Live reset the selected time zoom");
            assert_eq!(view.right_time_ms, now + cycle as f64 * 1000.0 + 500.0);
        }
    }
}

/// Catches `data_state/state.rs:apply_slot_geometry` collapsing the frame's two factors back into
/// one (the chart would zoom with the interface, or draw into a wrong-sized target), and
/// `engine.rs:chart_local_from_window_pos` crossing with the chart-design factor instead of the
/// window's (under UI zoom the pointer would land at `1 / zoom` of where it is).
#[test]
fn slot_geometry_keeps_device_density_while_the_pointer_crosses_with_the_windows_factor() {
    use gpui::{Bounds, GpuFrameInfo, point, px, size};
    use std::time::Instant;

    let now = 1_700_000_000_000.0;
    let mut engine = ChartEngine::new(now, ChartTheme::default());
    engine.open(42, "TESTUSDT");
    engine.data.borrow_mut().frame(GpuFrameInfo {
        now: Instant::now(),
        bounds: Bounds {
            origin: point(px(10.0), px(20.0)),
            size: size(px(200.0), px(100.0)),
        },
        scale_factor: 3.0,
        content_zoom: 1.5,
        presentable: true,
    });

    let (bounds, ppp, (w, h)) = engine.slot_geometry().expect("the frame installed a slot");
    assert_eq!(
        f32::from(bounds.origin.x),
        10.0,
        "slot bounds stay content space"
    );
    assert_eq!(
        (w, h),
        (600, 300),
        "the target is the slot in device pixels: bounds × 3.0"
    );
    assert_eq!(
        ppp, 2.0,
        "the chart's own factor excludes the zoom: 3.0 / 1.5"
    );
    assert_eq!(engine.slot_content_zoom(), 1.5);
    assert_eq!(engine.slot_scale_factor(), 3.0);

    let ((x, y), within) = engine
        .chart_local_from_window_pos(point(px(60.0), px(70.0)))
        .expect("the frame installed a slot");
    assert_eq!((x, y), (150.0, 150.0), "(60 − 10) × 3 and (70 − 20) × 3");
    assert!(within);
}
