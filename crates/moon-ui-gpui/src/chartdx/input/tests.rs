//! Regression checks for the flick that lets a live chart leave the live edge.

use super::*;

const PLOT_W: f32 = 1000.0;

/// A held drag on a plain clock, so consecutive bursts of moves continue in time.
struct Drag {
    hold: LiveHold,
    now_ms: f64,
    ppp: f32,
}

impl Drag {
    fn new(ppp: f32) -> Self {
        Self {
            hold: LiveHold::default(),
            now_ms: 1_000_000.0,
            ppp,
        }
    }

    /// `steps` equal moves of `(dx, dy)`, `interval_ms` apart; reports whether the hold broke.
    fn moves(&mut self, steps: usize, dx: f32, dy: f32, interval_ms: f64) -> bool {
        for _ in 0..steps {
            self.hold.step(dx, dy, self.now_ms, self.ppp);
            self.now_ms += interval_ms;
        }
        self.hold.breaks(PLOT_W, self.ppp)
    }
}

/// Letting any travel break live — the old 20%-of-width rule — must fail: a slow drag is a price pan
/// and time has to stay on the live edge however far it goes.
#[test]
fn a_slow_drag_never_leaves_live() {
    // 1 px/ms for two seconds: 2000 px, far past any radius, at half the break speed.
    assert!(!Drag::new(1.0).moves(250, 8.0, 0.0, 8.0));
}

/// Dropping the speed test in `LiveHold::step` must fail the slow case above; breaking before the
/// travel clears the pull-back radius must fail here.
#[test]
fn a_flick_leaves_live_once_it_clears_the_pull_back() {
    let mut drag = Drag::new(1.0);
    // 3 px/ms is a flick, but after two moves the travel is still inside the rejoin radius.
    assert!(!drag.moves(2, 24.0, 0.0, 8.0));
    assert!(drag.moves(4, 24.0, 0.0, 8.0));
    assert!(drag.hold.travel >= ChartView::live_rejoin_px(PLOT_W) + LIVE_BREAK_HYSTERESIS_PX);
}

/// Timing a move against the interval it arrived in, rather than a window of at least
/// `LIVE_BREAK_WINDOW_MS`, must fail: a coarse event after a pause carries a lot of distance in
/// almost no measured time.
#[test]
fn one_coarse_event_after_a_pause_is_not_a_flick() {
    let mut hold = LiveHold::default();
    hold.step(1.0, 0.0, 1_000_000.0, 1.0);
    hold.step(90.0, 0.0, 1_000_001.0, 1.0);
    hold.step(1.0, 0.0, 1_000_400.0, 1.0);
    assert!(!hold.flicked);
}

/// Counting a fast VERTICAL drag as a flick must fail: that is a price pan with some sideways
/// wobble, and it must not unhook time.
#[test]
fn a_fast_vertical_drag_is_not_a_flick() {
    // 3.75 px/ms sideways, but twice that vertically.
    assert!(!Drag::new(1.0).moves(20, 30.0, 60.0, 8.0));
}

/// A flick toward the FUTURE has nowhere to go and must not break live.
#[test]
fn a_flick_toward_the_future_does_not_break_live() {
    assert!(!Drag::new(1.0).moves(20, -30.0, 0.0, 8.0));
}

/// Speed is in LOGICAL pixels: on a 2x display the same hand motion covers twice the device pixels,
/// and dropping the `ppp` scale would make every drag there twice as eager to break.
#[test]
fn the_break_speed_scales_with_the_display() {
    // 2.5 device px/ms is a flick at 1x and a slow drag at 2x.
    let mut at_1x = Drag::new(1.0);
    let mut at_2x = Drag::new(2.0);
    at_1x.moves(20, 20.0, 0.0, 8.0);
    at_2x.moves(20, 20.0, 0.0, 8.0);
    assert!(at_1x.hold.flicked);
    assert!(!at_2x.hold.flicked);
}

/// Passing the plain floor from either wheel branch or the hotkey loses the three-second view.
#[test]
fn super_zoom_wheel_and_hotkey_share_the_plot_floor() {
    use crate::chartdx::pane::ContainerKind;
    let mut container = Container::new(ContainerKind::Main);
    container.open_manual(42, "TESTUSDT", now_unix_ms());
    let mut input = ChartInput {
        hovered_pane: Some(0),
        pane_rects: vec![(
            0,
            Rect {
                x: 0.0,
                y: 0.0,
                w: 900.0,
                h: 500.0,
            },
        )],
        ..Default::default()
    };
    let (width, _) = input.plot_metrics_for(Some(0), 900.0, 1.0, &container);
    for precise in [false, true] {
        container
            .view_mut(0)
            .unwrap()
            .ensure_default_window(width, 60.0, None);
        for _ in 0..20 {
            assert!(input.wheel(
                if precise { 300.0 } else { 3.0 },
                precise,
                WheelMode::SuperZoom,
                true,
                &mut container,
                900.0,
                1.0
            ));
        }
        assert!((container.view_mut(0).unwrap().visible_x(width).1 - 3000.0).abs() < 0.1);
        assert!(input.wheel(
            -3.0,
            false,
            WheelMode::Zoom,
            true,
            &mut container,
            900.0,
            1.0
        ));
        assert!((container.view_mut(0).unwrap().visible_x(width).1 - 6000.0).abs() < 0.1);
        assert!(input.super_zoom(true, &mut container, 900.0, 1.0));
        assert!((container.view_mut(0).unwrap().visible_x(width).1 - 3000.0).abs() < 0.1);
        assert!(input.super_zoom(false, &mut container, 900.0, 1.0));
        assert!((container.view_mut(0).unwrap().visible_x(width).1 - 6000.0).abs() < 0.1);
    }
    input.orderbook_only = true;
    assert!(!input.super_zoom(true, &mut container, 900.0, 1.0));
}

/// A time-axis wheel zoom must publish its settled width so the next chart can open at it. A pan
/// must not, because it never changes how much history is on screen.
#[test]
fn time_zoom_records_the_window_and_a_pan_does_not() {
    use crate::chartdx::pane::ContainerKind;
    use moon_chart::view::{record_zoom_window, remembered_zoom_window};
    let mut container = Container::new(ContainerKind::Main);
    container.open_manual(42, "TESTUSDT", now_unix_ms());
    let mut input = ChartInput {
        hovered_pane: Some(0),
        pane_rects: vec![(
            0,
            Rect {
                x: 0.0,
                y: 0.0,
                w: 900.0,
                h: 500.0,
            },
        )],
        ..Default::default()
    };
    let width = 900.0;
    container
        .view_mut(0)
        .unwrap()
        .ensure_default_window(width, 60.0, None);
    record_zoom_window(99_000.0);
    assert!(input.wheel(
        -60.0,
        true,
        WheelMode::Pan,
        true,
        &mut container,
        900.0,
        1.0
    ));
    assert_eq!(remembered_zoom_window(), Some(99_000.0));
    assert!(input.wheel(
        3.0,
        false,
        WheelMode::Zoom,
        true,
        &mut container,
        900.0,
        1.0
    ));
    let recorded = remembered_zoom_window().expect("wheel zoom must remember a width");
    assert!(
        (recorded - 99_000.0).abs() > 1.0,
        "wheel zoom left the sentinel width in place ({recorded})"
    );
    let (plot_w, _) = input.plot_metrics_for(Some(0), 900.0, 1.0, &container);
    let view_window = container.view_mut(0).unwrap().visible_x(plot_w).1;
    assert!(
        (recorded - view_window).abs() < 1.0,
        "remembered {recorded} ms does not match the view window {view_window} ms"
    );
}

/// Reusing ordinary-wheel accumulated lines makes the first super-zoom notch fire too early.
#[test]
fn super_zoom_starts_a_fresh_wheel_accumulator() {
    use crate::chartdx::pane::ContainerKind;
    let mut container = Container::new(ContainerKind::Main);
    container.open_manual(42, "TESTUSDT", now_unix_ms());
    let mut input = ChartInput {
        hovered_pane: Some(0),
        ..Default::default()
    };
    assert!(!input.wheel(
        2.0,
        false,
        WheelMode::Zoom,
        true,
        &mut container,
        900.0,
        1.0
    ));
    assert!(!input.wheel(
        1.0,
        false,
        WheelMode::SuperZoom,
        true,
        &mut container,
        900.0,
        1.0
    ));
    assert!(input.wheel(
        2.0,
        false,
        WheelMode::SuperZoom,
        true,
        &mut container,
        900.0,
        1.0
    ));
    let view = container.view_mut(0).unwrap();
    view.set_manual_persistent();
    let before = view.px_per_ms;
    let anchor = view.right_time_ms;
    assert!(input.wheel(
        -3.0,
        false,
        WheelMode::Pan,
        true,
        &mut container,
        900.0,
        1.0
    ));
    let view = container.view_mut(0).unwrap();
    assert_eq!(view.px_per_ms, before);
    assert!(view.right_time_ms < anchor);
}

/// An unmodified wheel notch at mid-plot must keep a live chart on the live edge. The same
/// notch as Ctrl+wheel must let go.
///
/// Breakage: `x_zoom` mapping `WheelMode::Zoom` to anything but `XZoom::Plain`, or mapping
/// `WheelMode::CtrlZoom` to Plain, either flips the header to Pause on one plain notch or
/// pins Live when the user zoomed into an old move.
#[test]
fn plain_wheel_keeps_live_and_ctrl_wheel_lets_go() {
    use crate::chartdx::pane::ContainerKind;

    let rect = Rect {
        x: 0.0,
        y: 0.0,
        w: 900.0,
        h: 500.0,
    };
    let mut input = ChartInput {
        hovered_pane: Some(0),
        pane_rects: vec![(0, rect)],
        ..Default::default()
    };
    let mut container = Container::new(ContainerKind::Main);
    container.open_manual(42, "TESTUSDT", now_unix_ms());
    let plot = input.areas_of(&rect, 1.0, moon_chart::PRICE_AXIS_W).plot;
    input.last_ptr = (plot.x + plot.w * 0.5, plot.y + plot.h * 0.5);
    let (plot_w, cursor_x) = input.plot_metrics_for(Some(0), 900.0, 1.0, &container);
    assert!(
        (cursor_x - plot_w * 0.5).abs() < 1.0,
        "the pointer is not at mid-plot ({cursor_x} vs {})",
        plot_w * 0.5
    );

    let now = now_unix_ms();
    {
        let view = container.view_mut(0).unwrap();
        view.ensure_default_window(plot_w, 60.0, None);
        view.resume_live(now);
    }
    let before = container.view_mut(0).unwrap().visible_x(plot_w).1;
    assert!(input.wheel(
        3.0,
        false,
        WheelMode::Zoom,
        true,
        &mut container,
        900.0,
        1.0
    ));
    let view = container.view_mut(0).unwrap();
    let after = view.visible_x(plot_w).1;
    assert!(
        (after - before / 2.0).abs() < 1.0,
        "plain wheel did not halve the window: {before} -> {after}"
    );
    assert!(view.follow, "plain wheel dropped Live");
    assert!(
        (view.right_time_ms - now_unix_ms()).abs() < 2_000.0,
        "plain wheel moved the live edge to {}",
        view.right_time_ms
    );

    let mut ctrl_input = ChartInput {
        hovered_pane: Some(0),
        pane_rects: vec![(0, rect)],
        last_ptr: input.last_ptr,
        ..Default::default()
    };
    let mut ctrl = Container::new(ContainerKind::Main);
    ctrl.open_manual(42, "TESTUSDT", now_unix_ms());
    {
        let view = ctrl.view_mut(0).unwrap();
        view.ensure_default_window(plot_w, 60.0, None);
        view.resume_live(now_unix_ms());
    }
    assert!(ctrl_input.wheel(3.0, false, WheelMode::CtrlZoom, true, &mut ctrl, 900.0, 1.0));
    let view = ctrl.view_mut(0).unwrap();
    assert!(!view.follow, "ctrl wheel at mid-plot kept Live");
    assert!(
        view.right_time_ms < now_unix_ms() - 1_000.0,
        "ctrl wheel left the anchor at {}",
        view.right_time_ms
    );
}
/// Measured Geist Mono axis labels must fit without leaving navigation on the old 56px gutter.
#[test]
fn measured_price_axis_width_is_shared_and_settles() {
    std::thread::Builder::new()
        .stack_size(16 * 1024 * 1024)
        .spawn(measured_price_axis_geometry)
        .unwrap()
        .join()
        .unwrap();
}

/// Drive real frame/sync with an empty market, comparing pending text with applied input geometry.
fn measured_price_axis_geometry() {
    use gpui::{Bounds, GpuFrameDecision, GpuFrameInfo, point, px, size};
    use moon_core::market::{MarketDataSource, MarketStore};
    crate::test_locale::warm();
    let _window = crate::test_support::SharedWindowGuard::take();
    let mut engine =
        crate::chartdx::ChartEngine::new(0.0, moon_core::config::ChartTheme::default());
    engine.open(1, "TINYUSDT");
    engine.set_historical(true);
    engine.set_follow(false, 1_000.0);
    engine.set_scene_visible(true);
    engine.set_market_source(Some(MarketDataSource::new(MarketStore::shared(0.0))));
    let mut data = engine.data.borrow_mut();
    data.orderbook_enabled = false;
    let frame = || GpuFrameInfo {
        now: std::time::Instant::now(),
        bounds: Bounds::new(point(px(0.0), px(0.0)), size(px(900.0), px(400.0))),
        scale_factor: 1.0,
        content_zoom: 1.0,
        presentable: true,
    };
    data.frame(frame());
    assert!(!data.view_dirty);
    // Bundled GeistMono-Regular.ttf: ASCII advance600/1000em at11.5px. Seven ordinary chars fit.
    assert!(!data.set_price_axis_width(48.3 + 4.0));
    assert_eq!(data.pending_price_axis_width, None);
    assert!(!data.view_dirty);
    // The actual font measures "12.34 sat" at62.1px: its ink began9.41px outside the old canvas.
    assert!(data.set_price_axis_width(62.1 + 4.0));
    assert_eq!(data.pending_price_axis_width, Some(67.0));
    let rect = moon_chart::view::Rect {
        x: 0.0,
        y: 0.0,
        w: 900.0,
        h: 400.0,
    };
    let input = super::ChartInput {
        pane_rects: vec![(0, rect)],
        last_ptr: (97.0, 50.0),
        price_axis_pos: crate::persistence::chart_persist::PriceAxisPos::Left,
        ..Default::default()
    };
    assert_eq!(engine.state.borrow().panes[0].view.bounds[0], 56.0);
    assert_eq!(
        input.plot_metrics_for(Some(0), rect.w, 1.0, &engine.container.borrow()),
        (844.0, 41.0),
        "pending text must not publish next frame geometry to input"
    );
    data.frame(frame());
    assert_eq!(data.pending_price_axis_width, None);
    assert_eq!(engine.state.borrow().panes[0].view.bounds[0], 67.0);
    let width = engine.container.borrow().pane(0).unwrap().price_axis_width;
    assert_eq!(width, 67.0);
    let plot = input.areas_of(&rect, 1.0, width).plot;
    assert!(
        plot.x - 4.0 - 62.1 >= rect.x,
        "the complete label stays inside the canvas"
    );
    assert_eq!(
        input.plot_metrics_for(Some(0), rect.w, 1.0, &engine.container.borrow()),
        (833.0, 30.0)
    );
    assert!(!data.set_price_axis_width(62.1 + 4.0));
    assert_eq!(data.pending_price_axis_width, None);
    assert!(
        !data.view_dirty,
        "an unchanged measurement owns no new geometry work"
    );
    assert!(matches!(data.frame(frame()), GpuFrameDecision::Skip));
}
