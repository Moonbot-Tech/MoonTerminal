//! The sell line's shared step on synthetic tapes: the price grid, the latency, the mirror,
//! the chain across a step, and the archive's clock.

use super::*;
use crate::db::tuner::ticks::exit::ExitModel;
use crate::db::tuner::ticks::exit::tests::{deal, fill, params, tape};
use crate::db::tuner::ticks::{EntryParams, ModelSettings, verify};

#[test]
fn the_placed_level_is_rounded_to_the_step() {
    // ARX, 2026-09-21, step 0.0001: take 0.197802 goes to the book as 0.1978; the 20 %
    // relative steps chain off the placed prices (0.1978 → 0.19714 → 0.1971 → 0.19658), and
    // the floor at +1 % (0.196445) is placed at 0.1964 — which is why the print AT 0.1964
    // sells.
    let p = ExitParams {
        price_down_timer_s: 3.0,
        price_down_pct: 20.0,
        price_down_delay_s: 10.0,
        price_down_relative: true,
        price_down_allowed_drop_pct: 1.0,
        ..params()
    };
    let mut d = deal(false);
    d.buy_price = 0.1945;
    d.tick = Some(0.0001);
    let fill = Fill {
        t_ms: 0,
        price: 0.1945,
    };
    let ticks = tape(&[
        (1_000, 0.1950),
        (4_000, 0.1950),
        (14_000, 0.1950),
        (24_000, 0.1950),
        (34_000, 0.1950),
        (40_000, 0.1964),
    ]);
    let w = walk(&d, &ticks, fill, 0.197802, &p);
    let levels: Vec<f64> = w.points.iter().map(|pt| pt.price).collect();
    let on_grid = |v: f64| (v / 0.0001).round() * 0.0001;
    assert_eq!(levels.len(), 4, "{levels:?}");
    for (level, want) in levels.iter().zip([0.1978, 0.1971, 0.1966, 0.1964]) {
        assert!((level - want).abs() < 1e-9, "{levels:?}");
        assert!(
            (level - on_grid(*level)).abs() < 1e-9,
            "off the grid: {level}"
        );
    }
    assert_eq!((w.exit.kind, w.exit.t_ms), (ExitKind::Line, 40_000));
    assert!(
        (w.exit.price - 0.1964).abs() < 1e-9,
        "sold at the placed level"
    );
}

#[test]
fn without_a_step_the_placed_level_is_the_exact_one() {
    let p = ExitParams {
        price_down_timer_s: 3.0,
        price_down_pct: 20.0,
        price_down_delay_s: 10.0,
        price_down_relative: true,
        price_down_allowed_drop_pct: 1.0,
        ..params()
    };
    let mut d = deal(false);
    d.buy_price = 0.1945;
    let fill = Fill {
        t_ms: 0,
        price: 0.1945,
    };
    let ticks = tape(&[(1_000, 0.1950), (40_000, 0.1964)]);
    let w = walk(&d, &ticks, fill, 0.197802, &p);
    assert_eq!(
        w.exit.kind,
        ExitKind::OpenAtWindowEnd,
        "0.196445 is above the tape's high"
    );
}

/// The take is an order like every move: on the book `latency_ms` after the core placed it. The
/// spike's own tail, printed in the milliseconds after the fill, cannot fill it.
#[test]
fn the_take_is_on_the_book_only_after_the_latency() {
    let p = ExitParams {
        model: ModelSettings {
            latency_ms: 100.0,
            ..ModelSettings::default()
        },
        ..ExitParams::default()
    };
    let ticks = tape(&[(9, 101.5), (150, 101.2)]);
    let w = walk(&deal(false), &ticks, fill(), 101.0, &p);
    assert_eq!((w.exit.kind, w.exit.t_ms), (ExitKind::Take, 150));
}

// ---- the mirror and the archive -------------------------------------------------------------

#[test]
fn a_short_mirrors_every_rule() {
    let p = ExitParams {
        price_down_timer_s: 1.0,
        price_down_pct: 50.0,
        price_down_delay_s: 1.0,
        price_down_allowed_drop_pct: 0.1,
        stop_loss_pct: -1.0,
        ..params()
    };
    // Take 99 (1 % below the buy at 100); one step -> 99.5 at t=1000; a print down to 99.4
    // before the next step crosses it.
    let ticks = tape(&[(1_500, 100.0), (1_900, 99.4)]);
    let w = walk(&deal(true), &ticks, fill(), 99.0, &p);
    assert!((w.points[1].price - 99.5).abs() < 1e-9, "{:?}", w.points);
    assert_eq!(w.exit.kind, ExitKind::Line);
    assert!((w.exit.price - 99.5).abs() < 1e-9);
    // The stop is ABOVE a short's buy.
    let ticks = tape(&[(500, 101.2)]);
    let w = walk(&deal(true), &ticks, fill(), 99.0, &p);
    assert_eq!(w.exit.kind, ExitKind::Stop);
}

#[test]
fn a_spike_through_the_old_level_fills_before_a_step_lands() {
    let p = ExitParams {
        price_down_timer_s: 1.0,
        price_down_pct: 50.0,
        model: ModelSettings {
            latency_ms: 100.0,
            ..ModelSettings::default()
        },
        ..params()
    };
    // The step is decided at t=1000 and reaches the book at t=1100; a print at 101 at t=1050
    // fills the old take at 101, not the new line at 100.5.
    let ticks = tape(&[(1_000, 100.0), (1_050, 101.0)]);
    let w = walk(&deal(false), &ticks, fill(), 101.0, &p);
    assert_eq!(w.exit.kind, ExitKind::Take);
    assert!((w.exit.price - 101.0).abs() < 1e-9);
}

#[test]
fn verify_holds_the_line_against_the_archived_points() {
    let p = ExitParams {
        price_down_timer_s: 1.0,
        price_down_pct: 50.0,
        price_down_delay_s: 1.0,
        price_down_allowed_drop_pct: 0.1,
        ..params()
    };
    let mut d = deal(false);
    d.sell_price = 100.25;
    let ticks = tape(&[(0, 100.0), (1_500, 100.0), (2_500, 100.3)]);
    // The archive's points are what the core did: placed at 101, 100.5 at 1 s, 100.25 at 2 s.
    let archived = [(0, 101.0), (1_000, 100.5), (2_000, 100.25)];
    let v = verify(&d, &ticks, &EntryParams::Fact, &p, None, Some(&archived));
    assert_eq!(v.exit_kind, Some(ExitKind::Line));
    assert_eq!(v.exit, Some(true), "{v:?}");
    assert_eq!(v.line_points, Some((3, 3)));
    // A line the core moved differently - a point the model never re-placed at - is a miss
    // even when the exit price agrees.
    let other = [(0, 101.0), (1_000, 100.7), (2_000, 100.25)];
    let v = verify(&d, &ticks, &EntryParams::Fact, &p, None, Some(&other));
    assert_eq!(v.exit, Some(false));
    assert_eq!(v.line_points, Some((2, 3)));
    // The same walk through `ExitModel`.
    let w = ExitModel::new(&p).walk(&d, &ticks, fill());
    assert_eq!(w.points.len(), 3);
}

// ---- the core's sell price across a step --------------------------------------------------------

/// Relative PriceDown on the grid, flat tape under the line: `(t, level)` of every point.
fn grid_walk(take: f64, step_lag_ms: f64, ticks: &[Tick]) -> Vec<(i64, f64)> {
    let p = ExitParams {
        price_down_timer_s: 1.0,
        price_down_pct: 10.0,
        price_down_delay_s: 1.0,
        price_down_relative: true,
        price_down_allowed_drop_pct: 0.1,
        ..params()
    };
    let mut d = deal(false);
    d.buy_price = 1_000.0;
    d.tick = Some(1.0);
    d.step_lag_ms = step_lag_ms;
    let fill = Fill {
        t_ms: 0,
        price: 1_000.0,
    };
    walk(&d, ticks, fill, take, &p)
        .points
        .iter()
        .map(|pt| (pt.t_ms, pt.price))
        .collect()
}

/// A step that moved the order chains the next one off the ORDER's price: 1096 → 1086.4, placed
/// 1086; then 1086 → 1077.4, placed 1077 — off the exact 1086.4 it would round to 1078. INDEX
/// 2026-09-22 (Gate, 10 % relative): all twelve archived levels land only this way.
#[test]
fn a_moved_order_chains_off_its_placed_price() {
    let ticks = tape(&[(500, 1_000.0), (1_500, 1_000.0), (2_500, 1_000.0)]);
    assert_eq!(
        grid_walk(1_096.0, 0.0, &ticks),
        vec![(0, 1_096.0), (1_000, 1_086.0), (2_000, 1_077.0)]
    );
}

/// A step that rounds back onto the order sends nothing and carries its exact value into the
/// next: 1004 → 1003.6 (still 1004), → 1003.24 (1003), → 1002.7 (still 1003), → 1002.43 (1002).
/// Off the placed price alone the line would never leave 1004; off the exact chain the second
/// move would come a step late. FATCOIN 2026-09-22 climbs one tick every other step this way.
#[test]
fn a_step_rounding_back_onto_the_order_carries_its_exact_value() {
    let ticks = tape(&[
        (500, 1_000.0),
        (1_500, 1_000.0),
        (2_500, 1_000.0),
        (3_500, 1_000.0),
        (4_500, 1_000.0),
    ]);
    assert_eq!(
        grid_walk(1_004.0, 0.0, &ticks),
        vec![(0, 1_004.0), (2_000, 1_003.0), (4_000, 1_002.0)]
    );
}

/// The core's own replace lag spaces the steps: the first is timed off the take by
/// `PriceDownTimer`, the next one after a step that moved the order off that step plus the delay
/// plus the lag — here every step moves the order.
#[test]
fn the_core_step_lag_spaces_the_price_down_steps() {
    let ticks = tape(&[
        (500, 1_000.0),
        (1_500, 1_000.0),
        (2_500, 1_000.0),
        (3_500, 1_000.0),
    ]);
    let times: Vec<i64> = grid_walk(1_096.0, 50.0, &ticks)
        .iter()
        .map(|&(t, _)| t)
        .collect();
    assert_eq!(times, vec![0, 1_000, 2_050, 3_100]);
}

/// A step rounding kept in place sent nothing, so the next one waits for no round trip: the
/// lag follows only the steps that moved the order (1004 stays at 1 s, 1003 at 2 s, 1003 stays
/// at 3.05 s, 1002 at 4.05 s).
#[test]
fn a_step_kept_in_place_adds_no_lag() {
    let ticks = tape(&[
        (500, 1_000.0),
        (1_500, 1_000.0),
        (2_500, 1_000.0),
        (3_500, 1_000.0),
        (4_500, 1_000.0),
    ]);
    assert_eq!(
        grid_walk(1_004.0, 50.0, &ticks),
        vec![(0, 1_004.0), (2_000, 1_003.0), (4_050, 1_002.0)]
    );
}

// ---- the verdict's clock ---------------------------------------------------------------------

/// PriceDown 50 % relative every second, 100 ms to the book: the take and its steps.
fn clock_params(take_pct: f64) -> ExitParams {
    ExitParams {
        sell_price_pct: take_pct,
        price_down_timer_s: 1.0,
        price_down_pct: 50.0,
        price_down_delay_s: 1.0,
        price_down_allowed_drop_pct: 0.1,
        model: ModelSettings {
            latency_ms: 100.0,
            ..ModelSettings::default()
        },
        ..params()
    }
}

/// The core stepped onto 100.5 at 900 ms and sold there 100 ms before the close — the archive
/// files the step and the fill as one point. The model's step to 100.5 is stamped 1 100 ms: late
/// by 200 ms, inside the point tolerance. On the model's own clock the fill found the take still
/// standing and the verdict had nothing to say; on the archive's the line stood at 100.5.
#[test]
fn the_level_at_the_fill_is_read_on_the_archive_clock() {
    let mut d = deal(false);
    d.sell_price = 100.5;
    d.close_ms = 1_000;
    let ticks = tape(&[(0, 100.0), (500, 100.0), (1_500, 100.0)]);
    let archived = [(0, 101.0), (900, 100.5)];
    let v = verify(
        &d,
        &ticks,
        &EntryParams::Fact,
        &clock_params(1.0),
        None,
        Some(&archived),
    );
    assert_eq!(v.exit_kind, Some(ExitKind::Line), "{v:?}");
    assert_eq!(v.exit, Some(true), "{v:?}");
}

/// A step the model took with no archived move to match: well before the fill it is a step the
/// core never took, and its level is the answer; inside the point tolerance of the fill it is
/// the model's timing, and the core's last level is.
#[test]
fn a_stray_step_counts_only_outside_the_point_tolerance_of_the_fill() {
    // Take 102; the model steps to 101 (1 100 ms), 100.5 (2 100 ms), 100.25 (3 100 ms). The
    // core stepped once, to 101, and filled a touch above it.
    let mut d = deal(false);
    d.sell_price = 101.02;
    let ticks = tape(&[
        (0, 100.0),
        (500, 100.0),
        (1_500, 100.0),
        (2_500, 100.0),
        (3_500, 100.0),
    ]);
    let p = clock_params(2.0);
    // Filled at 3 450 ms: the model's 100.5 at 2 100 ms is more than a second before it.
    d.close_ms = 3_600;
    let late = [(0, 102.0), (1_000, 101.0), (3_450, 101.02)];
    let v = verify(&d, &ticks, &EntryParams::Fact, &p, None, Some(&late));
    assert_eq!(v.exit, Some(false), "{v:?}");
    // Filled at 2 500 ms: the model's 100.5 came 400 ms before it.
    d.close_ms = 2_600;
    let soon = [(0, 102.0), (1_000, 101.0), (2_500, 101.02)];
    let v = verify(&d, &ticks, &EntryParams::Fact, &p, None, Some(&soon));
    assert_eq!(v.exit, Some(true), "{v:?}");
}

/// The fact's sell timers run from the take the core placed — the archive's first point, less
/// `SellDelay` — when that is later than the report's buy stamp.
#[test]
fn the_fact_sell_starts_at_the_archived_take() {
    let d = deal(false);
    let p = params();
    let start =
        |points: Option<&[(i64, f64)]>, p: &ExitParams| verify::fact_sell_start(&d, p, points).t_ms;
    assert_eq!(start(Some(&[(2_000, 101.0), (3_000, 100.5)]), &p), 2_000);
    let delayed = ExitParams {
        sell_delay_ms: 500.0,
        ..params()
    };
    assert_eq!(start(Some(&[(2_000, 101.0)]), &delayed), 1_500);
    assert_eq!(start(Some(&[(-7, 101.0)]), &p), 0, "never before the buy");
    assert_eq!(start(None, &p), 0);
}
