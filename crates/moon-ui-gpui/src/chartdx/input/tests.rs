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
    let (width, _) = input.plot_metrics_for(Some(0), 900.0, 1.0);
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
