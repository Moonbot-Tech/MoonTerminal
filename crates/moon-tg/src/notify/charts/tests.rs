//! Pins for deal chart decisions and the picture's numbers.

use moon_core::feed::ArchivedOrderTrace;
use moon_core::telegram::notify::{ChartLedger, ChartRule};

use super::*;
use crate::notify::trades::{HOLD_SECS, decide_charts};

const NOW: i64 = 1_700_000_000;

fn trade(rec_id: i64, profit_usd: Option<f64>) -> ClosedTrade {
    ClosedTrade {
        core: 1,
        rec_id,
        close_utc: NOW - 5,
        coin: "ACE".to_string(),
        profit_usd,
        profit_native: Some(0.0001),
        ..ClosedTrade::default()
    }
}

fn rule(profit: Option<f64>, loss: Option<f64>) -> ChartRule {
    ChartRule {
        on: true,
        profit_at_least_usd: profit,
        loss_at_least_usd: loss,
    }
}

fn enabled() -> ChartLedger {
    ChartLedger {
        enabled_utc: Some(NOW - 1_000),
        ..ChartLedger::default()
    }
}

fn ids(trades: &[ClosedTrade]) -> Vec<i64> {
    trades.iter().map(|t| t.rec_id).collect()
}

/// LinKvo's example, profit at least 100 or a loss of at least 12: either threshold sends, a
/// result between them does not.
#[test]
fn either_threshold_sends_a_picture() {
    let rows = [
        trade(1, Some(150.0)),
        trade(2, Some(50.0)),
        trade(3, Some(-13.0)),
        trade(4, Some(-5.0)),
    ];
    let mut ledger = enabled();
    let due = decide_charts(
        &rule(Some(100.0), Some(12.0)),
        &mut ledger,
        &[1],
        &rows,
        NOW,
    );
    assert_eq!(ids(&due), vec![1, 3]);
    assert_eq!(
        ledger.seen[&1].len(),
        4,
        "the turned-down trades are not judged again"
    );
}

/// The picture's caption is in dollars, so a trade waits for its dollar value even with no
/// threshold set, and is drawn once it comes.
#[test]
fn a_picture_waits_for_the_dollar_value() {
    let mut ledger = enabled();
    let all = rule(None, None);
    assert!(decide_charts(&all, &mut ledger, &[1], &[trade(7, None)], NOW).is_empty());
    assert!(ledger.held[&1].contains_key(&7));
    let due = decide_charts(&all, &mut ledger, &[1], &[trade(7, Some(2.0))], NOW + 10);
    assert_eq!(ids(&due), vec![7]);
    assert!(ledger.held.is_empty());
}

/// A trade whose dollar value never comes gets no picture — the card rule would send it
/// unchecked, a picture would draw a number it does not have.
#[test]
fn a_trade_never_valued_gets_no_picture() {
    let mut ledger = enabled();
    let all = rule(None, None);
    let rows = [trade(8, None)];
    decide_charts(&all, &mut ledger, &[1], &rows, NOW);
    let due = decide_charts(&all, &mut ledger, &[1], &rows, NOW + HOLD_SECS);
    assert!(due.is_empty());
    assert!(ledger.seen[&1].contains_key(&8));
}

/// The chart rule keeps its own ledger: a card rule switched off does not stop the pictures.
#[test]
fn the_chart_rule_needs_no_card_rule() {
    let mut ledger = enabled();
    let due = decide_charts(
        &rule(None, None),
        &mut ledger,
        &[1],
        &[trade(9, Some(1.0))],
        NOW,
    );
    assert_eq!(ids(&due), vec![9]);
}

/// A core the chat cannot see sends no picture of its trades.
#[test]
fn an_invisible_core_sends_nothing() {
    let mut ledger = enabled();
    let due = decide_charts(
        &rule(None, None),
        &mut ledger,
        &[2],
        &[trade(9, Some(1.0))],
        NOW,
    );
    assert!(due.is_empty());
}

/// A core's day is a sum only while every trade in it has a dollar value, and holds the trades
/// of the trade's own day up to its close — not a later one, not another core's.
#[test]
fn the_day_counts_the_core_up_to_the_trade() {
    let closed = |rec_id, close_utc, profit| ClosedTrade {
        close_utc,
        ..trade(rec_id, profit)
    };
    let mut other = closed(3, 150, Some(4.0));
    other.core = 2;
    let trades = [
        closed(1, 50, Some(9.0)),
        closed(2, 120, Some(2.5)),
        closed(4, 130, Some(-1.0)),
        closed(5, 140, Some(7.0)),
        other,
    ];
    assert_eq!(day_sum(&trades, 1, 100, 130), Some(1.5));
    assert_eq!(day_sum(&trades, 2, 100, 200), Some(4.0));
    let unvalued = [closed(1, 120, Some(2.5)), closed(2, 125, None)];
    assert_eq!(day_sum(&unvalued, 1, 100, 130), None);
}

/// The day is the close's own day in the zone: a trade at 23:58 drawn after midnight still
/// counts in the day it closed.
#[test]
fn the_day_starts_at_the_closes_midnight() {
    let zone = chrono_tz::Europe::Moscow;
    // 2026-10-03 23:58 MSK.
    let close = 1_791_061_080;
    assert_eq!(midnight_utc(zone, close), Some(1_790_974_800));
    assert_eq!(midnight_utc(zone, close + 180), Some(1_791_061_200));
}

/// LinKvo 04.10: at least ten seconds before the entry and three after the exit; a long trade
/// shows a third of its length before it, but no more than the recorder keeps.
#[test]
fn the_window_frames_the_trade() {
    // The two-second LRCUSDT trade of 04.10, the recorder keeping its default 30 s.
    assert_eq!(window(1_000_000, 1_002_172, 30_000), (990_000, 1_005_172));
    // A minute: twenty seconds of run-up.
    assert_eq!(window(1_000_000, 1_060_000, 30_000), (980_000, 1_063_000));
    // Five minutes: the run-up stops at the recorder's 30 s; a 5-minute margin gives 100 s.
    assert_eq!(window(1_000_000, 1_300_000, 30_000).0, 970_000);
    assert_eq!(window(1_000_000, 1_300_000, 300_000).0, 900_000);
    // A margin below the floor still shows ten seconds.
    assert_eq!(window(1_000_000, 1_002_000, 5_000).0, 990_000);
}

/// The lines are the trade's own, not an ancestor's inherited through a join; the stop is the
/// exit's.
#[test]
fn the_lines_are_the_trades_own() {
    let trace = |own, kind, stop: Option<f64>, price: f64| ArchivedOrderTrace {
        own,
        kind,
        stop_price: stop,
        stop_time_ms: None,
        points: vec![(1_000.0, price), (2_000.0, price + 1.0)],
    };
    let traces = [
        trace(false, ArchivedLineKind::Entry, None, 50.0),
        trace(true, ArchivedLineKind::Entry, Some(9.0), 10.0),
        trace(true, ArchivedLineKind::Exit, Some(8.0), 20.0),
    ];
    let lines = Lines::of(&traces);
    assert_eq!(lines.entry, vec![(1_000, 10.0), (2_000, 11.0)]);
    assert_eq!(lines.exit, vec![(1_000, 20.0), (2_000, 21.0)]);
    assert_eq!(lines.stop, Some(8.0));
}
