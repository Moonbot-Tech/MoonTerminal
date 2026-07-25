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

/// A live view settled on `price` with a window of `price * pct`, driven through the same
/// per-frame path the chart uses, so the reference the badge divides by is whatever that path left
/// behind rather than a value the test poked in.
fn settled_view(price: f32, pct: f32, now: f64) -> ChartView {
    let mut view = ChartView::new(0.0);
    view.resume_live(now);
    for i in 0..4 {
        let half = price * pct * 0.5;
        view.update_y(
            now + i as f64 * 16.0,
            400.0,
            Some((price - half, price + half)),
            Some(price),
        );
    }
    view.set_scale_percent(pct);
    view.update_y(now + 100.0, 400.0, Some((price, price)), Some(price));
    view
}

/// Catches measuring the visible scale against the CENTRE OF THE VIEWPORT: a vertical drag moves
/// that centre without touching the zoom, so the badge restated a scale that had not changed —
/// drag a 1% chart down far enough and it read 2%.
#[test]
fn vertical_panning_does_not_change_the_reported_scale() {
    let now = 100_000.0;
    let mut view = settled_view(1000.0, 0.01, now);
    let before = view.visible_scale_percent().expect("scale before the pan");

    // Drag the chart by half its price: with the window at 1% of 1000 that is 500 price units,
    // over the `px_per_price` the settled view computed. Under the old formula the same window
    // would then read against 1500 instead of 1000 — 1% becoming 0.67%.
    let dy = 500.0 * view.px_per_price;
    view.pan_y_px(dy, now);
    view.update_y(now + 200.0, 400.0, Some((1000.0, 1000.0)), Some(1000.0));

    let after = view.visible_scale_percent().expect("scale after the pan");
    assert!(
        (after - before).abs() < 1e-3,
        "panning changed the reported scale: {before} → {after}"
    );
    // And the old formula really would have: the centre now sits at half the price.
    assert!(
        (view.render_center - 1500.0).abs() < 1.0,
        "the pan did not move the centre as intended: {}",
        view.render_center
    );
}

/// Catches a scale reference that keeps following the live price while the user holds a manual Y
/// view: the window is frozen, so the figure must be frozen with it.
#[test]
fn a_frozen_view_reports_a_frozen_scale_while_the_price_moves() {
    let now = 100_000.0;
    let mut view = settled_view(1000.0, 0.01, now);
    view.pan_y_px(1.0, now);
    let before = view.visible_scale_percent().expect("scale while frozen");

    for i in 1..20 {
        let price = 1000.0 + i as f32 * 50.0;
        view.update_y(now + 200.0 + i as f64 * 16.0, 400.0, None, Some(price));
    }

    let after = view
        .visible_scale_percent()
        .expect("scale after the price ran");
    assert!(
        (after - before).abs() < 1e-3,
        "the frozen view's scale followed the live price: {before} → {after}"
    );
}

/// Catches reporting a confident percentage for a chart that has no price at all: the badge must
/// hide instead of dividing the window by itself and announcing "100%".
#[test]
fn a_chart_without_a_price_reports_no_scale() {
    let view = ChartView::new(0.0);
    assert_eq!(view.scale_ref_price(), None);
    assert_eq!(view.visible_scale_percent(), None);
}

/// Catches a reference threshold tuned for major coins: instruments trade down to 1e-8, and a
/// guard like `> 1e-6` blanks the badge — or pins it to a fallback — for every micro-priced one.
#[test]
fn a_micro_priced_instrument_still_reports_its_scale() {
    let now = 100_000.0;
    let view = settled_view(1.2e-8, 0.10, now);
    let pct = view
        .visible_scale_percent()
        .expect("scale for a micro price");
    assert!(
        (pct - 10.0).abs() < 1.0,
        "micro-priced scale misreported: {pct}"
    );
}

/// Catches the fixed-percent frame path sizing the window off a different reference than the click
/// did: the range set by the step would be overwritten on the next frame, flashing the zoom.
#[test]
fn a_selected_step_survives_the_next_frame() {
    let now = 100_000.0;
    let mut view = settled_view(1000.0, 0.02, now);
    let after_click = view.render_range;
    view.update_y(now + 300.0, 400.0, Some((999.0, 1001.0)), Some(1000.0));
    let next_frame = view.render_range;
    assert!(
        (next_frame - after_click).abs() / after_click < 0.05,
        "the step's range was rewritten by the next frame: {after_click} → {next_frame}"
    );
}
