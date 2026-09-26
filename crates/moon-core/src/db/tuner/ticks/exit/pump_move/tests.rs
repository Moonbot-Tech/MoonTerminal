//! PumpsDetection's pump move on a synthetic tape.

use super::*;
use crate::db::tuner::ticks::exit::line::walk;
use crate::db::tuner::ticks::exit::tests::{deal, fill, params, tape};

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
    assert_eq!(points[1].0, 2_000 + PUMP_MOVE_LAG_MS);
    assert!((points[1].1 - 109.9).abs() < 1e-9, "{points:?}");
}
