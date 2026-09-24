//! The SellShot section on synthetic tapes.

use super::*;
use crate::db::tuner::ticks::ExitKind;
use crate::db::tuner::ticks::exit::line::walk;
use crate::db::tuner::ticks::exit::tests::{deal, fill, params, tape};

// ---- SellShot ---------------------------------------------------------------------------------

#[test]
fn sell_shot_follows_the_market_inside_its_corridor() {
    // Distance 1 %, corridor 50 %: the line stays while 0.5-1.5 % off the reference. The
    // take at 101 is 1 % off 100; a rise to 100.8 leaves it 0.2 % off -> too close -> after
    // the raise wait (0) it moves away to 101.808.
    let p = ExitParams {
        ignore_sell_shot: false,
        sell_shot_distance_pct: 1.0,
        sell_shot_corridor_pct: 50.0,
        sell_shot_calc_interval_s: 0.1,
        sell_shot_allowed_up_pct: 10.0,
        sell_shot_allowed_down_pct: -1.0,
        ..params()
    };
    let ticks = tape(&[(500, 100.0), (1_000, 100.8), (1_500, 101.5)]);
    let w = walk(&deal(false), &ticks, fill(), 101.0, &p);
    assert!((w.points[1].price - 101.808).abs() < 1e-4, "{:?}", w.points);
    // 101.5 stays under the moved line, and the tape ends before the report's close.
    assert_eq!(w.exit.kind, ExitKind::OpenAtWindowEnd);
}

#[test]
fn sell_shot_is_capped_by_allowed_up() {
    let p = ExitParams {
        ignore_sell_shot: false,
        sell_shot_distance_pct: 1.0,
        sell_shot_corridor_pct: 50.0,
        sell_shot_calc_interval_s: 0.1,
        sell_shot_allowed_up_pct: 0.5,
        sell_shot_allowed_down_pct: -1.0,
        ..params()
    };
    let ticks = tape(&[(500, 100.0), (1_000, 100.8)]);
    let w = walk(&deal(false), &ticks, fill(), 101.0, &p);
    assert!((w.points[1].price - 100.5).abs() < 1e-4, "{:?}", w.points);
}
