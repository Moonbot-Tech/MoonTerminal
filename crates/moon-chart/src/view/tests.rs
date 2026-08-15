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

/// Removing padding or computing the right anchor as the visible edge rather than the chart's
/// internal live anchor must place at least one independently chosen endpoint outside the window.
#[test]
fn historical_trade_interval_is_visible_with_padding() {
    let epoch = 1_700_000_000_000.0;
    let start = epoch + 3_600_000.0;
    let end = start + 900_000.0;
    let width = 1_200.0;
    let mut view = ChartView::new(epoch);

    assert!(view.show_time_range(start, end, width, 0.15));
    let (left_rel, window_ms) = view.visible_x(width);
    let left = epoch + left_rel as f64;
    let right = left + window_ms as f64;
    assert!(left < start, "left padding must precede the entry");
    assert!(right > end, "right padding must follow the exit");
    assert!(
        !view.follow,
        "history focus must remain stable until user navigation"
    );
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

/// Wall-clock and plot width shared by the navigation cases below; neither is special.
const NOW: f64 = 100_000_000.0;
const WIDTH: f32 = 1000.0;

/// A live view at the built-in default scale.
fn live_view(now: f64, width: f32) -> ChartView {
    let mut view = ChartView::new(0.0);
    view.ensure_default_window(width, 60.0, None);
    view.resume_live(now);
    view
}

/// A view panned as far forward as it goes, for the future-navigation cases below.
fn panned_to_the_future_limit(now: f64, width: f32) -> ChartView {
    let mut view = live_view(now, width);
    // Far more than the ceiling allows, so the clamp is what decides where it stops.
    view.pan_x_px(-100.0 * width, now, width);
    view
}

/// Where the live edge lands on the plot, in pixels from its left border; negative is off screen.
fn live_edge_px(view: &ChartView, now: f64, width: f32) -> f32 {
    let (left, _) = view.visible_x(width);
    ((now - view.epoch_ms) as f32 - left) * view.px_per_ms
}

/// Settles a live view on `price` with a `±half` band, through the same per-frame path the chart
/// uses, so what is asserted afterwards is whatever that path left behind.
fn settle_y(view: &mut ChartView, now: f64, band: (f32, f32), last: f32) {
    for i in 0..4 {
        view.update_y(now + i as f64 * 16.0, 400.0, Some(band), Some(last));
    }
}

/// Restoring the `min(now_ms)` clamp in `view.rs:pan_x_px` must fail: there would be nowhere to
/// draw a plan, which is the whole point of walking past the live edge.
///
/// The ceiling is stated as a POSITION — at the limit the live edge sits on the left border of the
/// plot — because that is the invariant a user can see, and it holds at every zoom. Asserted to
/// within half a pixel, which is what the geometry delivers; a tolerance in milliseconds would be
/// hundreds of pixels of slack at the default scale and would hide any later drift.
#[test]
fn panning_forward_stops_with_now_on_the_left_edge() {
    let view = panned_to_the_future_limit(NOW, WIDTH);

    let x_now = live_edge_px(&view, NOW, WIDTH);
    assert!(
        x_now.abs() <= 0.5,
        "the live edge stopped {x_now} px from the left border"
    );
}

/// Making `view.rs:clamp_future_anchor` a no-op must fail. (That it is CALLED every prepare is a
/// separate claim, and a source-level one, pinned by the `theme_contract` chart suite.)
///
/// The ceiling relates the anchor, the zoom and the plot width, and two of those move without any
/// pan: this reproduces a resize, which halves the window and therefore the ceiling while leaving
/// the anchor where it was. Unclamped, the live edge ends up 450 px off the left of the plot with
/// nothing but empty future on screen.
#[test]
fn a_narrowed_pane_cannot_push_the_live_edge_off_screen() {
    let mut view = panned_to_the_future_limit(NOW, WIDTH);

    let narrowed = WIDTH * 0.5;
    assert!(
        view.clamp_future_anchor(NOW, narrowed),
        "nothing was clamped"
    );

    let x_now = live_edge_px(&view, NOW, narrowed);
    assert!(
        x_now >= -0.5,
        "the live edge is {x_now} px off the left border after the resize"
    );
    assert!(
        !view.clamp_future_anchor(NOW, narrowed),
        "the clamp is not idempotent"
    );
}

/// Arming the timed hold for a forward pan must fail: `tick_auto_live` would yank the chart back to
/// the live edge three seconds after the user got to the empty part they went there to draw on.
#[test]
fn a_pan_into_the_future_holds_without_a_timer() {
    let mut view = panned_to_the_future_limit(NOW, WIDTH);

    assert_eq!(view.auto_live_deadline_ms(), None);
    assert!(!view.tick_auto_live(NOW + MANUAL_HOLD_MS * 100.0));
}

/// Latching the suppressed hold on HAVING BEEN ahead — rather than on being ahead now — must fail.
///
/// A forward wheel notch is 6% of the window and needs no mouse-up, so crossing the live edge by
/// accident is easy; if that switched the three-second return off until the pane died, an accident
/// would silently change how the chart behaves for the rest of the session.
#[test]
fn coming_back_from_the_future_re_arms_the_hold() {
    let mut view = panned_to_the_future_limit(NOW, WIDTH);
    assert_eq!(view.auto_live_deadline_ms(), None, "parked ahead, no timer");

    // Back into history, far enough to be nowhere near the rejoin zone.
    view.pan_x_px(WIDTH * 2.0, NOW, WIDTH);

    assert!(view.right_time_ms < NOW, "the pan did not reach history");
    assert!(view.tick_auto_live(NOW + MANUAL_HOLD_MS + 1.0));
}

/// Deriving persistent-manual from `manual_until == 0` must fail: a forward pan produces that same
/// state, so the two cannot be told apart and one of them gets the other's behaviour.
///
/// This also pins a case that was broken BEFORE any of this: the toolbar's Live-off is documented as
/// having no automatic return, but a later pan used to arm the three-second hold anyway and drag the
/// user back to live they had explicitly switched off.
#[test]
fn an_explicit_live_off_is_not_undone_by_a_pan() {
    let mut view = live_view(NOW, WIDTH);

    view.set_manual_persistent();
    view.pan_x_px(WIDTH * 0.2, NOW, WIDTH);

    assert!(view.right_time_ms < NOW, "the pan did not reach history");
    assert_eq!(view.auto_live_deadline_ms(), None);
    assert!(!view.tick_auto_live(NOW + MANUAL_HOLD_MS * 100.0));
}

/// Restoring the signed comparison in `view.rs:snap_to_live_if_near` must fail: `now - right` is
/// negative for EVERY future anchor, so releasing a drag one window ahead would read as "near live"
/// and snap the view back.
#[test]
fn releasing_a_drag_ahead_of_now_does_not_count_as_near_live() {
    let mut view = panned_to_the_future_limit(NOW, WIDTH);

    assert!(!view.snap_to_live_if_near(NOW, WIDTH));

    // A second ahead of the live edge is genuinely near it, and must still rejoin.
    view.right_time_ms = NOW + 1000.0;
    assert!(view.snap_to_live_if_near(NOW, WIDTH));
    assert!(view.follow);
}

/// Replacing the `right_before.max(now_ms)` bound in `view.rs:zoom_x_at` with the ceiling alone must
/// fail: a zoom would then SCROLL the chart.
///
/// Anywhere left of the right margin the cursor holds its own time while the window grows around it,
/// which walks the anchor forward — measured, one 0.5x notch with the cursor at the left edge moved
/// it 0.42 windows. Under the old `min(now_ms)` that was invisible; with the future open it
/// accumulates into a chart that drifts off the live edge on nothing but wheel input.
#[test]
fn zooming_out_does_not_walk_the_view_into_the_future() {
    let mut view = live_view(NOW, WIDTH);
    // Two windows back, so several zoom-out steps land in history before the widening window brings
    // the anchor into the rejoin zone and `snap_to_live_if_near` legitimately resumes live.
    view.pan_x_px(WIDTH * 2.0, NOW, WIDTH);
    assert!(view.right_time_ms < NOW, "the pan did not reach history");

    let mut manual_steps = 0;
    while !view.follow && manual_steps < 6 {
        view.zoom_x_at(0.5, WIDTH, 0.0, NOW);
        manual_steps += 1;
        assert!(
            view.right_time_ms <= NOW + 1.0,
            "zooming out walked the anchor {} ms past the live edge",
            view.right_time_ms - NOW
        );
    }
    assert!(
        manual_steps >= 2,
        "only {manual_steps} zoom step(s) ran while manual — the drift case is not exercised"
    );

    // The other half of the same bound: a view already parked ahead KEEPS its position through a
    // zoom. Restoring the old `.min(now_ms)` satisfies the loop above and fails here — one wheel
    // notch would drag the plan being drawn back to the live edge.
    let mut parked = panned_to_the_future_limit(NOW, WIDTH);
    parked.zoom_x_at(0.5, WIDTH, WIDTH * 0.5, NOW);
    assert!(
        parked.right_time_ms > NOW,
        "a zoom pulled the parked view {} ms back behind the live edge",
        NOW - parked.right_time_ms
    );
}

/// Dropping the `clamp_future_anchor` call from `view.rs:zoom_x_at` must fail: zooming in shortens
/// the window, so an anchor legal at the old scale can end up further ahead than any pan could reach
/// — asserted as the same visible invariant the pan case uses rather than by re-deriving the
/// formula, which would pass even if that formula were wrong in both places.
#[test]
fn zooming_in_cannot_escape_the_future_ceiling() {
    let mut view = panned_to_the_future_limit(NOW, WIDTH);

    for _ in 0..6 {
        view.zoom_x_at(2.0, WIDTH, WIDTH * 0.5, NOW);
        let x_now = live_edge_px(&view, NOW, WIDTH);
        assert!(
            x_now >= -0.5,
            "zooming in left the live edge {x_now} px off the left border"
        );
    }
}

/// Restoring the `.or(last_price)` centre fallback, or the unconditional seed range, in
/// `view.rs:update_y` must fail.
///
/// `None` is what [`super::fit_band`] hands over for a window that holds no data of its own — and
/// off the live edge that is not a chart waiting for its first trade, it is a chart the user panned
/// away from the data. Both the seed range (a thousandth of the price) and the centre fallback would
/// then act, and the branch that runs while X is manual applies them with neither lerp nor
/// hysteresis: measured, a settled range of 24 collapsed to 0.6 in a single frame.
#[test]
fn a_window_with_nothing_to_fit_leaves_the_price_scale_alone() {
    let mut view = live_view(NOW, WIDTH);
    settle_y(&mut view, NOW, (990.0, 1010.0), 1000.0);
    let (center, range) = (view.render_center, view.render_range);
    assert!(range > 10.0, "the settled range is degenerate: {range}");

    view.pan_x_px(-100.0 * WIDTH, NOW, WIDTH);
    // The price has run since; nothing of it is inside the window the user is drawing in.
    for i in 0..4 {
        view.update_y(NOW + 100.0 + i as f64 * 16.0, 400.0, None, Some(1200.0));
    }

    assert!(
        (view.render_range - range).abs() <= range * 1e-3,
        "the price window was rescaled: {range} -> {}",
        view.render_range
    );
    assert!(
        (view.render_center - center).abs() <= range * 1e-3,
        "the price window was recentred: {center} -> {}",
        view.render_center
    );
}

/// Restoring the plain union in `chartdx/data_state/market.rs` must fail here.
///
/// [`super::fit_band`] is the rule that made the case above REACHABLE: the caller used to union the
/// last price and the order-book band into one span, so a window ahead of the live edge arrived as a
/// zero-width `(last, last)` or the ±0.5% book band — never as `None` — and the fit dutifully sized
/// itself to a reference that is not on screen. The distinction is what the window CONTAINS versus
/// what merely has to stay visible, and only the caller knows which is which.
#[test]
fn a_reference_price_is_something_to_fit_only_while_live() {
    let data = Some((990.0, 1010.0));
    let reference = Some((1200.0, 1200.0));

    // The references are the fallback that lets a market with no trades yet show something.
    assert_eq!(super::fit_band(None, reference, true), reference);
    // Otherwise they are not in the window, and nothing is: keep the scale.
    assert_eq!(super::fit_band(None, reference, false), None);
    // With data present the two are unioned either way — an order line is drawn across the whole
    // window, so its price is on screen even when every trade has scrolled off.
    for use_reference in [true, false] {
        assert_eq!(
            super::fit_band(data, reference, use_reference),
            Some((990.0, 1200.0)),
            "use_reference={use_reference}"
        );
    }
    assert_eq!(super::fit_band(data, None, false), data);
    assert_eq!(super::fit_band(None, None, true), None);
}

/// Making the fallback depend on whether the view HAS a price must fail here.
///
/// A pane that has never had window data must go on fitting its reference: a chart opened while the
/// toolbar's Live is already off, or a market with an order book and no trades, has no scale to keep
/// and would otherwise sit on the constructor's zero centre. The tempting test — "does the view know
/// a price yet" — is wrong by exactly one frame: the first fit gives it one, the fallback switches
/// off, and the pane latches onto that first hairline band while the price runs away from it. So the
/// caller's test is a fact about the DATA, which this reproduces frame by frame; that prepare really
/// spells it that way is pinned by the `theme_contract` chart suite.
#[test]
fn a_pane_that_never_had_data_keeps_fitting_its_reference() {
    let mut view = live_view(NOW, WIDTH);
    view.set_manual_persistent();

    // Six frames in which nothing is ever drawn inside the window, while the price runs.
    let saw_window_data = false;
    let mut price = 0.0;
    for i in 0..6 {
        price = 1000.0 + i as f32 * 100.0;
        let band = super::fit_band(None, Some((price, price)), view.follow || !saw_window_data);
        view.update_y(NOW + i as f64 * 16.0, 400.0, band, Some(price));
    }

    assert!(
        (view.render_center - price).abs() <= price * 1e-3,
        "the pane stopped following its reference at {} while the price reached {price}",
        view.render_center
    );
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
    let (_, window_ms) = view.visible_x(area);
    let expected_window = area / view.px_per_ms;
    assert!(
        (window_ms - expected_window).abs() <= expected_window * 1e-3,
        "window {} != area/ppm {}",
        window_ms,
        expected_window
    );
    // The live edge (now) stays at the (1-margin) width anchor and does not drift left.
    let x_now = live_edge_px(&view, now, area);
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
    let half = price * pct * 0.5;
    settle_y(&mut view, now, (price - half, price + half), price);
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
