use super::{ChartView, MANUAL_HOLD_MS};

fn default_window_sec(width: f32, present_hz: f32) -> f32 {
    let px_per_ms = ChartView::phase_clean_default_px_per_ms(width, present_hz);
    width / px_per_ms / 1000.0
}

#[test]
fn default_time_window_snaps_to_phase_clean_values_around_60s() {
    let cases = [
        (1000.0, 66.66667),
        (1280.0, 64.0),
        (1920.0, 64.0),
        (2560.0, 42.66667),
    ];
    for (width, expected) in cases {
        let actual = default_window_sec(width, 60.0);
        assert!(
            (actual - expected).abs() < 0.01,
            "width={width}: got {actual}, expected {expected}"
        );
    }
}

#[test]
fn x_pan_detaches_immediately_even_inside_live_snap_zone() {
    let now = 100_000.0;
    let mut view = ChartView::new(0.0);
    view.ensure_default_window(1000.0, 60.0, None);
    view.resume_live(now);

    view.pan_x_px(1.0, now, 1000.0);

    assert!(!view.follow);
    assert!(view.right_time_ms < now);
    assert!(view.snap_to_live_if_near(now, 1000.0));
    assert!(view.follow);
}

#[test]
fn zoom_in_is_clamped_to_min_window_30s() {
    let now = 100_000.0;
    let mut view = ChartView::new(0.0);
    view.ensure_default_window(1000.0, 60.0, None);
    let default_px_per_ms = view.px_per_ms;

    // One zoom-in step (×2) from the default (60 s) is allowed down to 30 s = 2× px/ms (Item 7).
    view.zoom_x_at(2.0, 1000.0, 500.0, now);
    assert!((view.px_per_ms - default_px_per_ms * 2.0).abs() < 1e-9);
    assert!(view.follow);

    // Further zoom-in is capped at 30 s and cannot go deeper.
    view.zoom_x_at(2.0, 1000.0, 500.0, now);
    assert!((view.px_per_ms - default_px_per_ms * 2.0).abs() < 1e-9);
}

#[test]
fn pan_then_hold_auto_returns_to_live() {
    let now = 100_000.0;
    let mut view = ChartView::new(0.0);
    view.ensure_default_window(1000.0, 60.0, None);
    view.resume_live(now);

    view.pan_x_px(50.0, now, 1000.0);
    assert!(!view.follow);
    // Live does not resume within the hold window.
    assert!(!view.tick_auto_live(now + 1000.0));
    assert!(!view.follow);
    // Live resumes automatically after the hold expires.
    assert!(view.tick_auto_live(now + MANUAL_HOLD_MS + 1.0));
    assert!(view.follow);
}

#[test]
fn deep_zoom_out_keeps_live_edge_anchored() {
    // Regression: at maximum zoom-out, the chart drifted left of the order book. Below the
    // historical 1e-6 ppm floor (a 365-day window), visible_x capped the window but the camera
    // did not, moving the live edge away from its (1-margin) anchor. The guard must be below min ppm.
    let area = 1300.0_f32;
    let now = 7_200_000.0; // 2 hours since the epoch.
    let mut view = ChartView::new(0.0);
    view.ensure_default_window(area, 60.0, None);
    view.resume_live(now);

    // Zoom out until clamped (min ppm = area / MAX_WINDOW_MS ≈ 4e-8 < 1e-6).
    for _ in 0..40 {
        view.zoom_x_at(0.5, area, area * 0.5, now);
    }
    let lo = area / super::MAX_WINDOW_MS;
    assert!(
        (view.px_per_ms - lo).abs() <= lo * 1e-3,
        "ppm {} не дошёл до клампа {}",
        view.px_per_ms,
        lo
    );
    assert!(
        view.px_per_ms < 1e-6,
        "тест должен покрывать зону ниже старого флора"
    );
    assert!(view.follow, "зум в live не должен срывать follow");

    // The old floor does NOT cap the window: it equals area/ppm (365 days), not area/1e-6.
    view.follow_edge(now, now);
    let (left, window_ms) = view.visible_x(area);
    let expected_window = area / view.px_per_ms;
    assert!(
        (window_ms - expected_window).abs() <= expected_window * 1e-3,
        "окно {} != area/ppm {}",
        window_ms,
        expected_window
    );
    // The live edge (now) stays at the (1-margin) width anchor and does not drift left.
    let x_now = ((now - view.epoch_ms) as f32 - left) * view.px_per_ms;
    let expected_x = area * (1.0 - view.right_margin_frac);
    assert!(
        (x_now - expected_x).abs() <= area * 0.01,
        "live-край на {} px, ожидался {} px",
        x_now,
        expected_x
    );

    // Repeated zoom attempts at the limit do not shift the view (no leftward drift).
    let (left_before, _) = view.visible_x(area);
    for _ in 0..5 {
        view.zoom_x_at(0.5, area, area * 0.3, now);
    }
    let (left_after, _) = view.visible_x(area);
    assert!(
        (left_after - left_before).abs() <= window_ms * 1e-3,
        "дрейф левого края на упоре зума: {} → {}",
        left_before,
        left_after
    );
}

#[test]
fn explicit_follow_off_has_no_auto_return() {
    let now = 100_000.0;
    let mut view = ChartView::new(0.0);
    view.resume_live(now);
    // Explicitly disabling Live does not schedule a delayed return.
    view.set_manual_persistent();
    assert!(!view.follow);
    assert!(view.auto_live_deadline_ms().is_none());
    assert!(!view.tick_auto_live(now + 10_000.0));
    assert!(!view.follow);
}
