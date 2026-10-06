use super::super::super::exit::{ExitParams, UnmodelledRule};
use super::super::super::tests::deal;
use super::super::super::{EntryParams, MshotParams};
use super::super::verify;
use super::*;
use crate::feed::types::{Side, Tick};

/// A buy-side print at `t_ms`.
fn print(t_ms: i64, price: f64) -> Tick {
    Tick {
        time_ms: t_ms as f64,
        price: price as f32,
        qty: 1.0,
        side: Side::Buy,
    }
}

/// A tape of buy-side prints at `(t_ms, price)`.
fn tape(points: &[(i64, f64)]) -> Vec<Tick> {
    points.iter().map(|&(t, p)| print(t, p)).collect()
}

/// The default MoonShot — a 1 % / 0.5 % corridor, as the parent suite runs it.
fn mshot() -> MshotParams {
    MshotParams::default()
}

/// The fact's tape of the parent suite: the take off 99.0 stands at 99.99 when the core sells
/// at 100.0.
fn fact_tape() -> Vec<Tick> {
    tape(&[
        (9_000, 100.0),
        (10_000, 99.0),
        (20_000, 99.5),
        (25_000, 99.5),
    ])
}

#[test]
fn an_order_that_never_fills_is_unfilled_not_off() {
    let ticks = tape(&[(0, 100.0), (10_000, 99.5), (20_000, 100.0)]);
    let v = verify(
        &deal(),
        &ticks,
        &EntryParams::MoonShot(mshot()),
        &ExitParams::default(),
        None,
        None,
    );
    assert_eq!(v.entry_finding, EntryFinding::Unfilled);
    assert_eq!(v.entry, Some(false));
}

#[test]
fn a_fill_outside_the_corridor_is_off() {
    let mut d = deal();
    d.buy_price = 98.0;
    let ticks = tape(&[(0, 100.0), (10_000, 99.0)]);
    let v = verify(
        &d,
        &ticks,
        &EntryParams::MoonShot(mshot()),
        &ExitParams::default(),
        None,
        None,
    );
    assert_eq!(v.entry_finding, EntryFinding::Off);
}

#[test]
fn a_sell_delay_outliving_the_trade_is_no_level() {
    let late = ExitParams {
        sell_delay_ms: 30_000.0,
        ..ExitParams::default()
    };
    let v = verify(&deal(), &fact_tape(), &EntryParams::Fact, &late, None, None);
    assert_eq!(v.exit_finding, ExitFinding::Miss(ExitMiss::NoLevel));
    assert_eq!(v.exit, Some(false));
}

#[test]
fn an_archived_move_the_model_never_made_is_a_line_miss_at_that_move() {
    // The core placed the take at 99.99 and later moved it to 99.5; the model's line holds the
    // take: the level at the close matches, the line does not — at its second move.
    let archive = [(10_000, 99.99), (15_000, 99.5)];
    let v = verify(
        &deal(),
        &fact_tape(),
        &EntryParams::Fact,
        &ExitParams::default(),
        None,
        Some(&archive),
    );
    match v.exit_finding {
        ExitFinding::Miss(ExitMiss::Off(parts)) => {
            assert_eq!(parts.first_unmatched, Some(1));
            assert_eq!(parts.late_ms, None);
        }
        other => panic!("expected a line miss, got {other:?}"),
    }
    assert_eq!(v.exit, Some(false));
}

#[test]
fn a_rule_the_model_lacks_is_unjudged_under_its_name() {
    let shot = ExitParams {
        unmodelled: Some(UnmodelledRule::SellShot),
        ..ExitParams::default()
    };
    let v = verify(&deal(), &fact_tape(), &EntryParams::Fact, &shot, None, None);
    assert_eq!(
        v.exit_finding,
        ExitFinding::Unjudged(Unjudged::Rule(UnmodelledRule::SellShot))
    );
    assert_eq!(v.exit, None);
}

#[test]
fn the_fill_stamp_is_held_against_the_nearest_print_at_its_price() {
    // Fill stamped at 10 000 ms at 99.0; prints at that price 300 ms before and 2 s after, and a
    // print at another price 50 ms before that must not be taken.
    let ticks = tape(&[(9_650, 98.0), (9_700, 99.0), (12_000, 99.0)]);
    assert_eq!(fill_clock_ms(&deal(), &ticks, 0.05), Some(300));
}

#[test]
fn no_print_at_the_fill_price_inside_the_window_measures_nothing() {
    let ticks = tape(&[(9_700, 98.0), (10_000 + FILL_CLOCK_WINDOW_MS + 1, 99.0)]);
    assert_eq!(fill_clock_ms(&deal(), &ticks, 0.05), None);
}
