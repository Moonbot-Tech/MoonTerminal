//! The hole of a long position's tape: where it lies, and what the fact's stop bounds.

use super::*;
use crate::db::tuner::ticks::exit::line::{walk, walk_held};
use crate::db::tuner::ticks::{Deal, EntryParams, ExitKind, Fill, ModelSettings, simulate};
use crate::feed::types::{Side, Tick};

/// A long bought at 100 at the epoch, closed two hours later.
fn deal(short: bool) -> Deal {
    Deal {
        buy_ms: 0,
        close_ms: 2 * HOUR,
        buy_price: 100.0,
        is_short: short,
        ..crate::db::tuner::ticks::tests::deal()
    }
}

const HOUR: i64 = 3_600_000;

/// A two-hour long position held at its two ends: the hole runs between the runs.
#[test]
fn a_long_positions_hole_lies_between_its_held_ends() {
    let d = deal(false);
    let covered = Coverage::from_spans([(-30_000, 30_000), (2 * HOUR - 30_000, 2 * HOUR + 30_000)]);
    let gap = TapeGap::of(&d, &covered, &ExitParams::default(), None).expect("a hole");
    assert_eq!((gap.from_ms, gap.to_ms), (30_000, 2 * HOUR - 30_000));
    assert_eq!(gap.fact_line, None);
    assert_eq!(gap.fact_stop, None, "the default sell runs no stop");
}

/// A window held whole has no hole, nor has one missing an end.
#[test]
fn a_window_held_whole_has_no_hole() {
    let d = deal(false);
    let whole = Coverage::one((-30_000, 2 * HOUR + 30_000));
    assert_eq!(TapeGap::of(&d, &whole, &ExitParams::default(), None), None);
    let entry_only = Coverage::one((-30_000, 30_000));
    assert_eq!(
        TapeGap::of(&d, &entry_only, &ExitParams::default(), None),
        None
    );
}

/// Several holes read as one, from the run at the buy to the run at the close.
#[test]
fn a_middle_held_in_part_reads_as_one_hole() {
    let d = deal(false);
    let covered = Coverage::from_spans([
        (-30_000, 30_000),
        (HOUR, HOUR + 60_000),
        (2 * HOUR - 30_000, 2 * HOUR + 30_000),
    ]);
    let gap = TapeGap::of(&d, &covered, &ExitParams::default(), None).expect("a hole");
    assert_eq!((gap.from_ms, gap.to_ms), (30_000, 2 * HOUR - 30_000));
}

/// The fact's stop bounds the hole at its deepest level — a deeper ladder rung included — and
/// only while the fact proves it quiet through the hole.
#[test]
fn the_facts_stop_bounds_the_hole_at_its_deepest_level() {
    use crate::db::tuner::ticks::exit::StopStep;
    let mut d = deal(false);
    let covered = Coverage::from_spans([(-30_000, 30_000), (2 * HOUR - 30_000, 2 * HOUR + 30_000)]);
    let exit = ExitParams {
        stop_loss_pct: -2.0,
        second_stop: Some(StopStep {
            after_s: 60.0,
            switch_pct: 1.0,
            level_pct: -5.0,
        }),
        fast_stop_loss: false,
        stop_loss_ema: 3.0,
        ..ExitParams::default()
    };
    let gap = TapeGap::of(&d, &covered, &exit, Some(&[(0, 101.0)])).expect("a hole");
    let stop = gap.fact_stop.expect("a stop");
    assert!((stop.level - 95.0).abs() < 1e-9, "{stop:?}");
    assert!(!stop.fast);
    assert_eq!(gap.fact_level_at(HOUR), Some(101.0));
    assert_eq!(gap.fact_level_at(-1), None, "before the line's first point");

    // A short's deepest stop is the highest one.
    let mut short = d.clone();
    short.is_short = true;
    let stop = TapeGap::of(&short, &covered, &exit, None)
        .and_then(|g| g.fact_stop)
        .expect("a stop");
    assert!((stop.level - 100.0 / 0.95).abs() < 1e-9, "{stop:?}");

    // A stop the fact fired inside the hole proves nothing about it.
    d.stop_anchor = Some(crate::db::tuner::ticks::StopAnchor {
        entry_price: 100.0,
        entry_ms: 0,
        stop_pct: -2.0,
        delay_s: 0.0,
        fast: false,
        ema: 3.0,
        second: exit.second_stop,
        third: None,
        fired: Some((HOUR, 98.0)),
        quiet_until_ms: HOUR,
    });
    let gap = TapeGap::of(&d, &covered, &exit, None).expect("a hole");
    assert_eq!(gap.fact_stop, None);
}

#[test]
fn a_quicker_trigger_is_not_bounded_by_a_slower_one() {
    let book = GapStop {
        level: 98.0,
        fast: false,
        ema: 3.0,
    };
    assert!(
        !trigger_not_quicker(true, 0.0, &book),
        "any print beats the ticker"
    );
    assert!(!trigger_not_quicker(false, 0.0, &book), "another average");
    assert!(trigger_not_quicker(false, 3.0, &book));
    let fast = GapStop { fast: true, ..book };
    assert!(trigger_not_quicker(true, 0.0, &fast));
    assert!(trigger_not_quicker(false, 5.0, &fast));
}

// ---- the walk across the hole -------------------------------------------------------------

/// Where the entry end's prints stop and the exit end's begin.
const HOLE_FROM: i64 = 30_000;
const HOLE_TO: i64 = 2 * HOUR - 30_000;

fn print(t_ms: i64, price: f64) -> Tick {
    Tick {
        time_ms: t_ms as f64,
        price: price as f32,
        qty: 1.0,
        side: Side::Buy,
    }
}

/// A long bought at 100 and held two hours, its tape held at both ends only; the core's line
/// and stop as given.
fn holed(fact_line: Option<&[(i64, f64)]>, fact_stop: Option<GapStop>) -> Deal {
    Deal {
        gap: Some(TapeGap {
            from_ms: HOLE_FROM,
            to_ms: HOLE_TO,
            fact_line: fact_line.map(Arc::from),
            fact_stop,
        }),
        ..deal(false)
    }
}

/// Prints below the take at the entry end, one print at the take's 101 ten seconds before the
/// close.
fn two_ends() -> Vec<Tick> {
    vec![
        print(1_000, 100.3),
        print(20_000, 100.3),
        print(HOLE_TO + 5_000, 100.6),
        print(2 * HOUR - 10_000, 101.0),
    ]
}

fn fill() -> Fill {
    Fill {
        t_ms: 0,
        price: 100.0,
    }
}

/// A 1 % take and no latency.
fn sell() -> ExitParams {
    ExitParams {
        model: ModelSettings {
            latency_ms: 0.0,
            ..ModelSettings::default()
        },
        ..ExitParams::default()
    }
}

/// The spec's §3.1 seam: a line PriceDown took to its floor stood under the core's line through
/// two hours nobody holds, and the walk sold it on the first print past the hole. Where the price
/// crossed it — if it did — is on no record: the trade is not judged.
#[test]
fn a_line_stepped_under_the_facts_is_not_sold_on_the_seam() {
    let p = ExitParams {
        price_down_timer_s: 1.0,
        price_down_pct: 50.0,
        price_down_delay_s: 1.0,
        price_down_relative: true,
        price_down_allowed_drop_pct: 0.5,
        ..sell()
    };
    let d = holed(Some(&[(0, 101.0)]), None);
    let w = walk(&d, &two_ends(), fill(), 101.0, &p);
    assert_eq!(w.exit.kind, ExitKind::InGap, "{:?}", w.exit);
}

/// A sell no nearer the price than the core's line through the hole was not reached there: the
/// exit end judges it, the take filled by the print at it.
#[test]
fn a_line_no_nearer_than_the_facts_is_judged_on_the_exit_end() {
    let d = holed(Some(&[(0, 101.0), (HOUR, 100.8)]), None);
    let w = walk(&d, &two_ends(), fill(), 101.0, &sell());
    assert_eq!(
        (w.exit.kind, w.exit.t_ms),
        (ExitKind::Take, 2 * HOUR - 10_000),
        "{:?}",
        w.exit
    );
}

/// Without the core's line on record nothing proves a standing sell unreached.
#[test]
fn without_the_archive_a_standing_sell_is_not_judged() {
    let d = holed(None, None);
    let w = walk(&d, &two_ends(), fill(), 101.0, &sell());
    assert_eq!(w.exit.kind, ExitKind::InGap);
}

/// A stop nearer the price than the core's, or on a quicker trigger, may have fired in the hole;
/// a deeper one on the same trigger did not.
#[test]
fn a_stop_nearer_than_the_facts_is_not_judged_and_a_deeper_one_is() {
    let fact_stop = Some(GapStop {
        level: 97.0,
        fast: false,
        ema: 0.0,
    });
    let d = holed(Some(&[(0, 101.0)]), fact_stop);
    let stop = |pct: f64, fast: bool| ExitParams {
        stop_loss_pct: pct,
        fast_stop_loss: fast,
        ..sell()
    };
    let near = walk(&d, &two_ends(), fill(), 101.0, &stop(-1.0, false));
    assert_eq!(
        near.exit.kind,
        ExitKind::InGap,
        "99 stands above the core's 97"
    );
    let quick = walk(&d, &two_ends(), fill(), 101.0, &stop(-5.0, true));
    assert_eq!(quick.exit.kind, ExitKind::InGap, "a print beats the ticker");
    let deep = walk(&d, &two_ends(), fill(), 101.0, &stop(-5.0, false));
    assert_eq!(deep.exit.kind, ExitKind::Take, "{:?}", deep.exit);
    // No stop of the core's to lean on.
    let bare = holed(Some(&[(0, 101.0)]), None);
    let w = walk(&bare, &two_ends(), fill(), 101.0, &stop(-5.0, false));
    assert_eq!(w.exit.kind, ExitKind::InGap);
}

/// SellLevel follows the price's high: where it stood after the hole is a function of prints
/// nobody holds.
#[test]
fn a_rule_following_the_price_is_not_judged_across_the_hole() {
    let p = ExitParams {
        sell_level_delay_s: 1.0,
        sell_level_time_s: 10.0,
        sell_level_count: 5_000,
        // Half a per cent over the high: nothing at the entry end reaches it.
        sell_level_adjust_pct: 0.5,
        ..sell()
    };
    let d = holed(Some(&[(0, 101.0)]), None);
    let w = walk(&d, &two_ends(), fill(), 101.0, &p);
    assert_eq!(w.exit.kind, ExitKind::InGap);
}

/// The verdict holds the sell through the close and judges where the line stood there: the
/// timer steps through the hole need no print and no archive, and nothing is sold on the seam.
#[test]
fn the_verdict_steps_the_line_through_the_hole_on_its_timer() {
    let p = ExitParams {
        price_down_timer_s: 1.0,
        price_down_pct: 50.0,
        price_down_delay_s: 1.0,
        price_down_relative: true,
        price_down_allowed_drop_pct: 0.5,
        ..sell()
    };
    let d = holed(None, None);
    let w = walk_held(&d, &two_ends(), fill(), 101.0, &p, Some(d.close_ms));
    assert_ne!(w.exit.kind, ExitKind::InGap, "{:?}", w.exit);
    let last = w.points.last().expect("the line's levels");
    assert!((last.price - 100.5).abs() < 1e-9, "at the floor: {last:?}");
}

/// An entry the tape shows filling only past the hole's start filled inside it, or never: not
/// judged, and a search point leaving it so is refused like one left open.
#[test]
fn an_entry_filling_past_the_holes_start_is_not_judged() {
    use crate::db::tuner::ticks::MshotParams;
    let d = Deal {
        buy_ms: 0,
        ..holed(Some(&[(0, 101.0)]), None)
    };
    let ticks = vec![
        print(-10_000, 100.0),
        print(1_000, 100.0),
        print(HOLE_TO + 1_000, 98.5),
        print(2 * HOUR - 10_000, 101.0),
    ];
    let entry = EntryParams::MoonShot(MshotParams::default());
    let outcome = simulate(&d, &ticks, &entry, &sell(), None);
    assert_eq!(
        outcome.exit.map(|e| e.kind),
        Some(ExitKind::InGap),
        "{outcome:?}"
    );
    assert!(outcome.left_open());
    assert_eq!(outcome.profit_pct, None);
}

/// The core's level near a moment is the loosest one it stood at within the slack: the lowest
/// for a long, the highest for a short; nothing where the line is not on record.
#[test]
fn the_facts_level_near_a_moment_is_the_loosest_within_the_slack() {
    let line = [(0, 101.0), (60_000, 100.5), (120_000, 100.8)];
    assert_eq!(fact_level_near(&line, 59_500, 1_000, true), Some(100.5));
    assert_eq!(fact_level_near(&line, 58_500, 1_000, true), Some(101.0));
    assert_eq!(fact_level_near(&line, 119_500, 1_000, false), Some(100.8));
    assert_eq!(fact_level_near(&line, -2_000, 1_000, true), None);
}

/// A PriceDown step the model takes within a second of the core's archived one is that step: the
/// trade's own settings are judged on the exit end. Five seconds early the model's sell stood
/// under the core's for those seconds, and the trade is not judged.
#[test]
fn a_step_within_the_verdicts_slack_is_the_facts_own() {
    let archive: &[(i64, f64)] = &[(0, 101.0), (60_000, 100.5)];
    let d = holed(Some(archive), None);
    // One relative step of 50 % off 101 to the 100.5 floor.
    let step_at = |timer_s: f64| ExitParams {
        price_down_timer_s: timer_s,
        price_down_pct: 50.0,
        price_down_delay_s: 1.0,
        price_down_relative: true,
        price_down_allowed_drop_pct: 0.5,
        ..sell()
    };
    let near = walk(&d, &two_ends(), fill(), 101.0, &step_at(59.5));
    assert_eq!(
        (near.exit.kind, near.exit.t_ms),
        (ExitKind::Line, HOLE_TO + 5_000),
        "{:?}",
        near.exit
    );
    let early = walk(&d, &two_ends(), fill(), 101.0, &step_at(55.0));
    assert_eq!(early.exit.kind, ExitKind::InGap, "{:?}", early.exit);
}
