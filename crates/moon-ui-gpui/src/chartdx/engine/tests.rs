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
            view.zoom_x_at(128.0, width, width * 0.5, now);
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
