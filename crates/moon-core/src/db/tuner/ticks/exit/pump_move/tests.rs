//! PumpsDetection's pump move on a synthetic tape.

use super::*;
use crate::db::tuner::ticks::exit::line::walk;
use crate::db::tuner::ticks::exit::tests::{deal, fill, params, tape};

/// Half-to-even ties fire at the half-second only for an even next integer.
#[test]
fn rounded_timer_starts_at_the_sell_phase_and_zero_is_off() {
    for (timer, delay) in [(2.0, 2_501), (3.0, 3_500), (2.3, 2_501)] {
        let p = ExitParams {
            pump_move_timer_s: timer,
            ..params()
        };
        let rule = PumpMove::new(&p, fill(), Side { long: true }, 1_000);
        assert_eq!(rule.due(1_000 + delay - 1), None);
        assert_eq!(rule.due(1_000 + delay), Some(1_000 + delay));
    }
    let p = params();
    assert_eq!(
        PumpMove::new(&p, fill(), Side { long: true }, 1_000).due(i64::MAX),
        None
    );
}

/// A peak on the losing side must not move a long below its buy or a short above its buy.
#[test]
fn pump_move_clamps_the_peak_distance_and_mirrors_shorts() {
    for (long, peak, ask, expected) in [
        (true, 98.0, 99.0, 100.0),
        (true, 110.0, 105.0, 109.9),
        (true, 110.0, 112.0, 111.88),
        (false, 102.0, 101.0, 100.0),
        (false, 90.0, 95.0, 90.1),
        (false, 90.0, 88.0, 88.12),
    ] {
        let p = ExitParams {
            pump_move_timer_s: 2.0,
            pump_move_pct: 1.0,
            ..params()
        };
        let ticks = tape(&[
            (0, peak),
            (2_500, ask),
            (3_000, if long { 150.0 } else { 50.0 }),
        ]);
        let w = super::super::line::walk_held(
            &deal(!long),
            &ticks,
            fill(),
            if long { 120.0 } else { 80.0 },
            &p,
            Some(10_000),
        );
        let price = w.points.last().unwrap().price;
        assert!(
            (price - expected).abs() < 1e-9,
            "{long}: {price} != {expected}"
        );
    }
}

/// When the proxy window is empty the last observable ASK still anchors the move.
#[test]
fn pump_move_reads_the_last_print_before_due_outside_the_peak_window() {
    let p = ExitParams {
        pump_move_timer_s: 2.0,
        pump_move_pct: 1.0,
        sell_delay_ms: 1_000.0,
        model: crate::db::tuner::ticks::ModelSettings {
            pump_peak_lookback_ms: 0,
            ..Default::default()
        },
        ..params()
    };
    let ticks = tape(&[(500, 112.0), (4_000, 150.0)]);
    let w = super::super::line::walk_held(&deal(false), &ticks, fill(), 120.0, &p, Some(10_000));
    assert!((w.points.last().unwrap().price - 111.88).abs() < 1e-9);
}

// ---- PumpMove ------------------------------------------------------------------------------

/// `PumpMoveTimer` after the take, once: to `PumpMovePersent` of the peak-to-buy distance short
/// of the pump's peak — which printed before the buy. Peak 110, buy 100, 1 %: 109.9.
#[test]
fn the_pump_move_goes_once_to_the_peak_less_its_share() {
    let p = ExitParams {
        pump_move_timer_s: 2.0,
        pump_move_pct: 1.0,
        ..params()
    };
    let mut d = deal(false);
    d.kind = "PumpsDetection".into();
    d.tick = Some(0.01);
    let ticks = tape(&[
        (-3_000, 104.0),
        (-50, 110.0),
        (500, 101.0),
        (1_500, 102.0),
        (2_600, 101.5),
        (6_000, 101.0),
    ]);
    let w = walk(&d, &ticks, fill(), 104.0, &p);
    let points: Vec<(i64, f64)> = w.points.iter().map(|pt| (pt.t_ms, pt.price)).collect();
    assert_eq!(points.len(), 2, "{points:?}");
    assert_eq!(points[1].0, 2_501);
    assert!((points[1].1 - 109.9).abs() < 1e-9, "{points:?}");
}
