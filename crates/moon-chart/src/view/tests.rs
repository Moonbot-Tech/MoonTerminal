//! Regression coverage for chart view initialization, navigation, and scale behavior.

use super::{ChartView, MANUAL_HOLD_MS};

/// Return how much live history remains left of the future margin after default initialization.
///
/// Args:
///     width: Plot width in logical pixels.
///     present_hz: Display refresh rate used for phase cleaning.
///
/// Returns:
///     Historical duration in milliseconds from the left edge through the live timestamp.
fn default_history_ms(width: f32, present_hz: f32) -> f32 {
    let mut view = ChartView::new(0.0);
    view.ensure_default_window(width, present_hz, None);
    view.resume_live(0.0);
    let (left, _) = view.visible_x(width);
    -left
}

/// Changing `view.rs:DEFAULT_WINDOW_MS` to six viewport hours must fail: the future margin would
/// leave the user with less than six hours of live history.
#[test]
fn default_live_window_keeps_at_least_six_hours_of_history() {
    const SIX_HOURS_MS: f32 = 6.0 * 60.0 * 60.0 * 1000.0;
    const MAX_PHASE_OVERSHOOT_MS: f32 = 2.0 * 60.0 * 1000.0;
    for (width, present_hz) in [
        (1000.0, 60.0),
        (1280.0, 120.0),
        (1920.0, 144.0),
        (2560.0, 60.0),
        (40.300_01, 30.0),
    ] {
        let actual = default_history_ms(width, present_hz);
        assert!(
            actual >= SIX_HOURS_MS,
            "width={width}, hz={present_hz}: history {actual} ms is shorter than six hours"
        );
        assert!(
            actual <= SIX_HOURS_MS + MAX_PHASE_OVERSHOOT_MS,
            "width={width}, hz={present_hz}: phase overshoot is unexpectedly large ({actual} ms)"
        );
    }
}

/// Restoring the half-pixel shrink threshold in `view.rs:ensure_default_window` must fail;
/// otherwise a default-mode resize can silently reduce live history below six hours.
#[test]
fn a_subpixel_default_resize_keeps_at_least_six_hours_of_history() {
    const SIX_HOURS_MS: f32 = 6.0 * 60.0 * 60.0 * 1000.0;
    let mut view = ChartView::new(0.0);
    view.ensure_default_window(500.0, 30.0, None);
    view.resume_live(0.0);

    let resized_width = 499.51;
    view.ensure_default_window(resized_width, 30.0, None);
    let (left, _) = view.visible_x(resized_width);

    assert!(
        -left >= SIX_HOURS_MS,
        "subpixel resize left only {} ms of history",
        -left
    );
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

/// Replacing `view.rs:zoom_x_at`'s width-based ceiling with the widened default scale must fail;
/// otherwise users can no longer zoom from the six-hour overview down to the 30-second floor.
#[test]
fn zoom_in_is_clamped_to_min_window_30s() {
    let now = 100_000.0;
    let width = 1000.0;
    let mut view = ChartView::new(0.0);
    view.ensure_default_window(width, 60.0, None);

    for _ in 0..20 {
        view.zoom_x_at(2.0, width, width * 0.5, now);
    }

    let (_, window_ms) = view.visible_x(width);
    assert!(
        (window_ms - 30_000.0).abs() < 1.0,
        "minimum window was {window_ms} ms"
    );
    assert!(view.follow);
}

/// Removing the `default_ppm` initialization branch in `view.rs:ensure_default_window` must fail;
/// otherwise a saved Shift+middle-click scale is overwritten by the six-hour built-in default.
#[test]
fn a_saved_x_scale_wins_during_initialization_and_explicit_reset() {
    let width = 1200.0;
    let saved_ppm = width / 180_000.0;
    let mut view = ChartView::new(0.0);

    view.ensure_default_window(width, 60.0, Some(saved_ppm));
    assert!((view.px_per_ms - saved_ppm).abs() < 1e-9);

    view.zoom_x_at(2.0, width, width * 0.5, 100_000.0);
    assert!((view.px_per_ms - saved_ppm).abs() >= 1e-9);
    view.reset_default_window_on_next_prepare();
    view.ensure_default_window(width, 60.0, Some(saved_ppm));
    assert!((view.px_per_ms - saved_ppm).abs() < 1e-9);
}

/// Making `view.rs:reset_default_window_on_next_prepare` a no-op must fail; otherwise reset leaves
/// a manually zoomed chart in place instead of restoring the built-in six-hour-history default.
#[test]
fn explicit_reset_without_a_saved_scale_restores_six_hours_of_history() {
    const SIX_HOURS_MS: f32 = 6.0 * 60.0 * 60.0 * 1000.0;
    let width = 1000.0;
    let mut view = ChartView::new(0.0);
    view.ensure_default_window(width, 60.0, None);
    view.zoom_x_at(2.0, width, width * 0.5, 0.0);

    view.reset_default_window_on_next_prepare();
    view.ensure_default_window(width, 60.0, None);
    view.resume_live(0.0);
    let (left, _) = view.visible_x(width);

    assert!(-left >= SIX_HOURS_MS, "reset left only {} ms", -left);
}

/// Removing the manual-scale guard in `view.rs:ensure_default_window` must fail; otherwise resizing
/// after a wheel zoom silently replaces the user's chosen X scale with the six-hour default.
#[test]
fn a_manual_zoom_survives_later_default_window_preparation() {
    let mut view = ChartView::new(0.0);
    view.ensure_default_window(1000.0, 60.0, None);
    view.zoom_x_at(2.0, 1000.0, 500.0, 100_000.0);
    let manual_ppm = view.px_per_ms;

    view.ensure_default_window(1600.0, 120.0, None);

    assert!((view.px_per_ms - manual_ppm).abs() < 1e-9);
}

/// Resetting `right_time_ms` inside `view.rs:ensure_default_window` must fail; otherwise preparing
/// a panned chart jumps the user back to the live edge before the normal hold timer expires.
#[test]
fn a_manual_pan_position_survives_default_window_preparation() {
    let now = 100_000_000.0;
    let mut view = ChartView::new(0.0);
    view.ensure_default_window(1000.0, 60.0, None);
    view.resume_live(now);
    view.pan_x_px(100.0, now, 1000.0);
    let panned_right = view.right_time_ms;

    view.ensure_default_window(1600.0, 120.0, None);

    assert_eq!(view.right_time_ms, panned_right);
    assert!(!view.follow);
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

/// Raising `view.rs:MIN_PX_PER_MS` above the maximum-window scale must fail these assertions;
/// otherwise deep zoom makes the camera and live edge drift away from the order-book boundary.
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

    // Zoom out until clamped (min ppm = area / MAX_WINDOW_MS, about 4e-8 < 1e-6).
    for _ in 0..40 {
        view.zoom_x_at(0.5, area, area * 0.5, now);
    }
    let lo = area / super::MAX_WINDOW_MS;
    assert!(
        (view.px_per_ms - lo).abs() <= lo * 1e-3,
        "ppm {} did not reach clamp {}",
        view.px_per_ms,
        lo
    );
    assert!(
        view.px_per_ms < 1e-6,
        "test must cover values below the historical floor"
    );
    assert!(view.follow, "live zoom must preserve follow");

    // The old floor does NOT cap the window: it equals area/ppm (365 days), not area/1e-6.
    view.follow_edge(now, now);
    let (left, window_ms) = view.visible_x(area);
    let expected_window = area / view.px_per_ms;
    assert!(
        (window_ms - expected_window).abs() <= expected_window * 1e-3,
        "window {} != area/ppm {}",
        window_ms,
        expected_window
    );
    // The live edge (now) stays at the (1-margin) width anchor and does not drift left.
    let x_now = ((now - view.epoch_ms) as f32 - left) * view.px_per_ms;
    let expected_x = area * (1.0 - view.right_margin_frac);
    assert!(
        (x_now - expected_x).abs() <= area * 0.01,
        "live edge is at {} px, expected {} px",
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
        "left edge drifted at the zoom limit: {} -> {}",
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
        "panning changed the reported scale: {before} -> {after}"
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
        "the frozen view's scale followed the live price: {before} -> {after}"
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

/// Catches floors tuned for major coins: instruments trade down to 1e-8, where an absolute guard
/// like `1e-6` is eighty times the whole price. Every step the toolbar ships is checked, because
/// the tighter ones fail first: 5% and 2% of 1.2e-8 fall under guards that 10% clears.
#[test]
fn every_toolbar_step_reports_its_scale_on_a_micro_priced_instrument() {
    let now = 100_000.0;
    for step in [0.50, 0.20, 0.10, 0.05, 0.02] {
        let view = settled_view(1.2e-8, step, now);
        let pct = view
            .visible_scale_percent()
            .unwrap_or_else(|| panic!("no scale reported for step {step}"));
        let expected = step * 100.0;
        assert!(
            (pct - expected).abs() < expected * 0.1,
            "step {step}: reported {pct}%, expected about {expected}%"
        );
    }
}

/// Catches a comparison-lock follower measuring the ANCHOR's window against its own price: the
/// window handed over belongs to the anchor, so the percentage must be read against the anchor too
/// — otherwise the same lock badges 1% or 20% depending on what the follower showed beforehand.
#[test]
fn a_locked_follower_reports_the_anchors_scale() {
    let now = 100_000.0;
    let mut follower = settled_view(3000.0, 0.05, now);
    follower.set_y_window(60_000.0, 600.0);
    let pct = follower.visible_scale_percent().expect("locked scale");
    assert!((pct - 1.0).abs() < 0.05, "locked follower reported {pct}%");
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
        "the step's range was rewritten by the next frame: {after_click} -> {next_frame}"
    );
}
