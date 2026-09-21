//! CPU regressions for arrival opacity, repaint termination, and explicit clearing.

use super::{arrival_alpha, arrival_present_due};
use crate::chartdx::{ChartEngine, PaneRender};
use gpui::{Bounds, GpuFrameDecision, GpuFrameInfo, point, px, size};
use moon_core::config::ChartTheme;
use std::time::{Duration, Instant};

/// Changing the pulse count or replacing expiry with `None` loses the three flashes or core cue.
#[test]
fn arrival_alpha_keeps_three_pulses_then_a_steady_border() {
    assert_eq!(arrival_alpha(None, true), None);
    // Peaks and troughs are independently fixed at sixths of the specified 2.6-second window.
    for (seconds, expected) in [
        (0.0, 0.0),
        (2.6 / 6.0, 1.0),
        (2.6 / 3.0, 0.0),
        (1.3, 1.0),
        (2.6 * 2.0 / 3.0, 0.0),
        (2.6 * 5.0 / 6.0, 1.0),
    ] {
        let actual = arrival_alpha(Some(Duration::from_secs_f64(seconds)), true).unwrap();
        assert!((actual - expected).abs() < 1e-5, "at {seconds}: {actual}");
    }
    assert!(arrival_alpha(Some(Duration::from_millis(2599)), true).unwrap() < 0.004);
    for millis in [2600, 2601, 60_000, 86_400_000] {
        assert_eq!(
            arrival_alpha(Some(Duration::from_millis(millis)), true),
            Some(0.6)
        );
    }
}

/// Without the hold the pulses are identical and the stroke ends with them — the pre-hold picture.
#[test]
fn arrival_alpha_without_hold_ends_with_the_pulses() {
    assert_eq!(arrival_alpha(None, false), None);
    let peak = arrival_alpha(Some(Duration::from_secs_f64(2.6 / 6.0)), false).unwrap();
    assert!((peak - 1.0).abs() < 1e-5);
    for millis in [2600, 2601, 60_000] {
        assert_eq!(
            arrival_alpha(Some(Duration::from_millis(millis)), false),
            None
        );
    }
}

/// Removing the deadline guard causes endless idle presents; skipping the final tick freezes alpha.
#[test]
fn arrival_pacing_settles_once_even_inside_the_tick_interval() {
    let at = Instant::now();
    let before_end = at + Duration::from_millis(2599);
    let end = at + Duration::from_millis(2600);
    assert!(arrival_present_due(at, None, at));
    assert!(!arrival_present_due(
        at,
        Some(at),
        at + Duration::from_millis(99)
    ));
    assert!(arrival_present_due(
        at,
        Some(at),
        at + Duration::from_millis(100)
    ));
    assert!(arrival_present_due(at, Some(before_end), end));
    for millis in [2600, 2601, 2700, 60_000] {
        assert!(!arrival_present_due(
            at,
            Some(end),
            at + Duration::from_millis(millis)
        ));
    }
    // A tab first presented after the entire pulse window still needs its steady frame.
    let revealed = at + Duration::from_secs(60);
    assert!(arrival_present_due(at, None, revealed));
    assert!(!arrival_present_due(
        at,
        Some(revealed),
        revealed + Duration::from_secs(1)
    ));
}

/// Builds a CPU frame input; no GPU context, window, or live market session is started.
fn frame_info() -> GpuFrameInfo {
    GpuFrameInfo {
        now: Instant::now(),
        bounds: Bounds::new(point(px(0.0), px(0.0)), size(px(320.0), px(200.0))),
        scale_factor: 1.0,
        content_zoom: 1.0,
        presentable: true,
    }
}

/// Restoring expiry clearing or unbounded pacing loses the stroke or keeps a quiet chart busy.
#[test]
fn arrival_frame_retains_color_settles_and_clears() {
    let engine = ChartEngine::new(0.0, ChartTheme::default());
    let mut state = engine.state.borrow_mut();
    let mut pane = PaneRender::new();
    pane.active = true;
    pane.pane_bounds = [0.0, 0.0, 320.0, 200.0];
    state.panes.push(pane);
    let at = Instant::now() - Duration::from_secs(10);
    assert!(state.set_arrival_pulse(Some(at), [0.2, 0.4, 0.8, 0.5], true));
    assert!(matches!(
        state.frame(frame_info()),
        GpuFrameDecision::RequestPresent
    ));
    let rect = &state.panes[0].readout_rects[0];
    assert_eq!(rect.border, [0.2, 0.4, 0.8, 0.3]);
    assert_eq!(rect.dst, [0.0, 0.0, 320.0, 200.0]);
    assert_eq!(rect.m[0], 2.0);
    assert_eq!(state.arrival_pulse, Some(at));
    assert!(matches!(state.frame(frame_info()), GpuFrameDecision::Skip));

    // Recolouring the same settled arrival must rebuild the stroke without restarting the pulse.
    assert!(state.set_arrival_pulse(Some(at), [0.8, 0.2, 0.4, 1.0], true));
    assert_eq!(state.panes[0].readout_rects[0].border, [0.8, 0.2, 0.4, 0.6]);
    assert!(matches!(
        state.frame(frame_info()),
        GpuFrameDecision::RequestPresent
    ));
    assert!(matches!(state.frame(frame_info()), GpuFrameDecision::Skip));

    assert!(state.set_arrival_pulse(None, [0.8, 0.2, 0.4, 1.0], true));
    assert!(state.panes[0].readout_rects.is_empty());
    assert!(matches!(
        state.frame(frame_info()),
        GpuFrameDecision::RequestPresent
    ));
    assert!(matches!(state.frame(frame_info()), GpuFrameDecision::Skip));
}

/// Failing to reset the pacing stamp on replacement leaves the new core's arrival unanimated.
#[test]
fn arrival_replacement_restarts_pacing_and_unarmed_main_stays_idle() {
    let engine = ChartEngine::new(0.0, ChartTheme::default());
    let mut state = engine.state.borrow_mut();
    state.frame(frame_info());
    assert!(matches!(state.frame(frame_info()), GpuFrameDecision::Skip));
    assert_eq!(state.arrival_pulse, None);
    let old = Instant::now() - Duration::from_secs(10);
    state.set_arrival_pulse(Some(old), [1.0, 0.0, 0.0, 1.0], true);
    state.frame(frame_info());
    state.set_arrival_pulse(Some(Instant::now()), [0.0, 0.0, 1.0, 1.0], true);
    assert_eq!(state.last_arrival_present_at, None);
    state.needs_present = false;
    assert!(matches!(
        state.frame(frame_info()),
        GpuFrameDecision::RequestPresent
    ));
    assert_eq!(state.arrival_pulse_color, [0.0, 0.0, 1.0, 1.0]);
}

/// Letting a pulse's present enter `advance_camera` rebuilds every flashing chart's cached base.
#[test]
fn arrival_only_present_reuses_base_but_independent_updates_advance_camera() {
    let engine = ChartEngine::new(0.0, ChartTheme::default());
    let mut state = engine.state.borrow_mut();
    let mut pane = PaneRender::new();
    pane.active = true;
    pane.follow = true;
    pane.view.time_to_px = 1.0;
    pane.view.bounds = [0.0, 0.0, 320.0, 200.0];
    pane.pane_bounds = [0.0, 0.0, 320.0, 200.0];
    state.panes.push(pane);
    // Force the camera cap due in both cases; only the reason for the present differs.
    for independent_update in [false, true] {
        state.needs_present = independent_update;
        state.set_arrival_pulse(Some(Instant::now()), [0.2, 0.4, 0.8, 1.0], true);
        state.base_dirty = false;
        state.last_present_at = None;
        state.panes[0].gpu_prepare_dirty = false;
        state.panes[0].last_edge_px = 0;
        assert!(matches!(
            state.frame(frame_info()),
            GpuFrameDecision::RequestPresent
        ));
        assert_eq!(state.base_dirty, independent_update);
        assert_eq!(state.panes[0].gpu_prepare_dirty, independent_update);
        assert_eq!(state.panes[0].last_edge_px != 0, independent_update);
    }
    // The steady border must not suspend the normal live camera once its settling frame is done.
    state.set_arrival_pulse(
        Some(Instant::now() - Duration::from_secs(10)),
        [0.2, 0.4, 0.8, 1.0],
        true,
    );
    state.needs_present = false;
    state.base_dirty = false;
    state.frame(frame_info());
    assert!(!state.base_dirty);
    state.last_present_at = None;
    state.panes[0].last_edge_px = 0;
    assert!(matches!(
        state.frame(frame_info()),
        GpuFrameDecision::RequestPresent
    ));
    assert!(state.base_dirty);
}

/// Restoring the always-on stroke, or forgetting to clear the stamp, keeps a frame the reader
/// switched off — or leaves the canvas answering with a settling present on every reveal.
#[test]
fn arrival_without_hold_clears_at_the_deadline_and_hold_restyles_in_place() {
    let engine = ChartEngine::new(0.0, ChartTheme::default());
    let mut state = engine.state.borrow_mut();
    let mut pane = PaneRender::new();
    pane.active = true;
    pane.pane_bounds = [0.0, 0.0, 320.0, 200.0];
    state.panes.push(pane);
    // Mid-pulse: the frame is drawn exactly as with the hold.
    let at = Instant::now() - Duration::from_secs_f64(2.6 / 6.0);
    assert!(state.set_arrival_pulse(Some(at), [0.2, 0.4, 0.8, 1.0], false));
    assert!(matches!(
        state.frame(frame_info()),
        GpuFrameDecision::RequestPresent
    ));
    assert_eq!(state.panes[0].readout_rects.len(), 1);
    assert_eq!(state.arrival_pulse, Some(at));

    // The armed pulse ages past the deadline under the frame loop (the stamp is moved back rather
    // than re-armed, which the setter would filter): one present without the stroke, the stamp is
    // forgotten, then idle.
    let old = Instant::now() - Duration::from_secs(10);
    state.arrival_pulse = Some(old);
    assert!(matches!(
        state.frame(frame_info()),
        GpuFrameDecision::RequestPresent
    ));
    assert!(state.panes[0].readout_rects.is_empty());
    assert_eq!(state.arrival_pulse, None);
    assert_eq!(state.last_arrival_present_at, None);
    assert!(matches!(state.frame(frame_info()), GpuFrameDecision::Skip));

    // Switching the hold on for that same old arrival re-arms it straight into the steady stroke.
    assert!(state.set_arrival_pulse(Some(old), [0.2, 0.4, 0.8, 1.0], true));
    assert!(matches!(
        state.frame(frame_info()),
        GpuFrameDecision::RequestPresent
    ));
    assert_eq!(state.panes[0].readout_rects[0].border, [0.2, 0.4, 0.8, 0.6]);
    assert!(matches!(state.frame(frame_info()), GpuFrameDecision::Skip));

    // Switching it off again with the same stamp: the setter filters the expired stamp to a clear,
    // and the frame paints the cleared state once.
    assert!(state.set_arrival_pulse(Some(old), [0.2, 0.4, 0.8, 1.0], false));
    assert_eq!(state.arrival_pulse, None);
    assert!(state.panes[0].readout_rects.is_empty());
    assert!(matches!(
        state.frame(frame_info()),
        GpuFrameDecision::RequestPresent
    ));
    assert!(matches!(state.frame(frame_info()), GpuFrameDecision::Skip));
}

/// Dropping the expiry filter, or presenting a restyle of nothing armed, makes every look change
/// on a settled tab cost one window present per chart that shows no pixel for it.
#[test]
fn an_expired_arrival_without_hold_arms_nothing_and_presents_nothing() {
    let engine = ChartEngine::new(0.0, ChartTheme::default());
    let mut state = engine.state.borrow_mut();
    let mut pane = PaneRender::new();
    pane.active = true;
    pane.pane_bounds = [0.0, 0.0, 320.0, 200.0];
    state.panes.push(pane);
    state.frame(frame_info());
    assert!(matches!(state.frame(frame_info()), GpuFrameDecision::Skip));

    let old = Instant::now() - Duration::from_secs(10);
    assert!(!state.set_arrival_pulse(Some(old), [0.2, 0.4, 0.8, 1.0], false));
    assert_eq!(state.arrival_pulse, None);
    assert!(state.panes[0].readout_rects.is_empty());
    assert!(matches!(state.frame(frame_info()), GpuFrameDecision::Skip));
    // A colour change on that same settled, unheld chart is a restyle of nothing.
    assert!(!state.set_arrival_pulse(Some(old), [0.8, 0.2, 0.4, 1.0], false));
    assert!(matches!(state.frame(frame_info()), GpuFrameDecision::Skip));
    // The colour still lands, so the next hold or arrival draws with it.
    assert_eq!(state.arrival_pulse_color, [0.8, 0.2, 0.4, 1.0]);
}
