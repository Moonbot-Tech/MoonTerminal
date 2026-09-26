//! The trailing stop on synthetic tapes. Every tape quotes a spread by a taker sell (the bid)
//! and a taker buy (the ask) at the same moment; the ticker arrives every
//! [`crate::db::tuner::ticks::exit::stops::TICKER_PERIOD_MS`] = 2 150 ms after the fill at 0.

use super::trailing_level;
use crate::db::tuner::ticks::exit::ExitParams;
use crate::db::tuner::ticks::exit::line::walk;
use crate::db::tuner::ticks::exit::tests::{deal, fill, params};
use crate::db::tuner::ticks::verify::stated_peak;
use crate::db::tuner::ticks::{EntryParams, ExitKind, verify};
use crate::feed::types::{Side as TickSide, Tick};

fn print(t_ms: i64, price: f64, side: TickSide) -> Tick {
    Tick {
        time_ms: t_ms as f64,
        price: price as f32,
        qty: 1.0,
        side,
    }
}

/// A spread quoted at `t_ms`: the bid by a taker sell, the ask by a taker buy.
fn quote(t_ms: i64, bid: f64, ask: f64) -> [Tick; 2] {
    [
        print(t_ms, bid, TickSide::Sell),
        print(t_ms, ask, TickSide::Buy),
    ]
}

fn tape(quotes: &[(i64, f64, f64)]) -> Vec<Tick> {
    quotes
        .iter()
        .flat_map(|&(t, bid, ask)| quote(t, bid, ask))
        .collect()
}

/// A 1 % trailing, a take far away, no stop.
fn trailing() -> ExitParams {
    ExitParams {
        sell_price_pct: 50.0,
        trailing_pct: -1.0,
        ..params()
    }
}

fn far_take() -> f64 {
    150.0
}

/// The middle 100.1 at the first arrival (2 150) starts the peak; 102.1 at the second (4 300)
/// raises it; 101.0 at the third (6 450) is under 102.1 · 0.99 = 101.079 and sells — at the
/// arrival, at the middle.
#[test]
fn the_line_follows_the_peak_of_the_middle_and_sells_on_it() {
    let ticks = tape(&[
        (100, 100.0, 100.2),
        (3_000, 102.0, 102.2),
        (5_000, 100.9, 101.1),
        (9_000, 100.0, 100.2),
    ]);
    let w = walk(&deal(false), &ticks, fill(), far_take(), &trailing());
    assert_eq!((w.exit.kind, w.exit.t_ms), (ExitKind::Stop, 6_450), "{w:?}");
    assert!((w.exit.price - 101.0).abs() < 1e-4, "{w:?}");
}

/// `TrailingEMA` 4: a step moves the peak a fifth of the way, 100.1 → 100.5, so the line stays at
/// 99.495 and the same fall does not reach it.
#[test]
fn the_trailing_ema_moves_the_peak_a_share_of_the_way() {
    let ticks = tape(&[
        (100, 100.0, 100.2),
        (3_000, 102.0, 102.2),
        (5_000, 100.9, 101.1),
        (9_000, 100.9, 101.1),
    ]);
    let smoothed = ExitParams {
        trailing_ema: 4.0,
        ..trailing()
    };
    let w = walk(&deal(false), &ticks, fill(), far_take(), &smoothed);
    assert_eq!(w.exit.kind, ExitKind::OpenAtWindowEnd, "{w:?}");
}

/// `UseTakeProfit` 2 % with a 1 % trailing: no line until the middle passes 103; then the line
/// stands no lower than 102 and sells only while the middle is still above 102.
#[test]
fn the_take_profit_holds_the_line_back_and_floors_the_sale() {
    let tp = ExitParams {
        trailing_take_profit_pct: Some(2.0),
        ..trailing()
    };
    // Up to 102.5 and back to 100: the line never appeared.
    let short_of_it = tape(&[
        (100, 102.4, 102.6),
        (3_000, 99.9, 100.1),
        (9_000, 99.9, 100.1),
    ]);
    let w = walk(&deal(false), &short_of_it, fill(), far_take(), &tp);
    assert_eq!(w.exit.kind, ExitKind::OpenAtWindowEnd, "{w:?}");
    // Past 103, then 102.1: under 103.2 · 0.99 = 102.168 and above 102 — sold.
    let sold = tape(&[
        (100, 103.1, 103.3),
        (3_000, 102.0, 102.2),
        (9_000, 102.0, 102.2),
    ]);
    let w = walk(&deal(false), &sold, fill(), far_take(), &tp);
    assert_eq!((w.exit.kind, w.exit.t_ms), (ExitKind::Stop, 4_300), "{w:?}");
    // Past 103, then straight to 101.9 in one arrival: below the take profit, nothing sells.
    let through = tape(&[
        (100, 103.1, 103.3),
        (3_000, 101.8, 102.0),
        (9_000, 101.8, 102.0),
    ]);
    let w = walk(&deal(false), &through, fill(), far_take(), &tp);
    assert_eq!(w.exit.kind, ExitKind::OpenAtWindowEnd, "{w:?}");
}

/// The peak followed inside `StopLossDelay` restarts at the middle of the delay's end: a spike
/// to 105 before it does not leave the line at 103.95.
#[test]
fn the_peak_restarts_where_the_delay_ends() {
    let delayed = ExitParams {
        stop_loss_delay_s: 5.0,
        ..trailing()
    };
    let ticks = tape(&[
        (100, 104.9, 105.1),
        (3_000, 100.9, 101.1),
        (7_000, 100.4, 100.6),
        (12_000, 100.4, 100.6),
    ]);
    let w = walk(&deal(false), &ticks, fill(), far_take(), &delayed);
    assert_eq!(w.exit.kind, ExitKind::OpenAtWindowEnd, "{w:?}");
    // Without the delay the spike is the peak, and 101 is under its line.
    let w = walk(&deal(false), &ticks, fill(), far_take(), &trailing());
    assert_eq!(w.exit.kind, ExitKind::Stop, "{w:?}");
}

/// A short's peak is the lowest middle and its line 1 % above it.
#[test]
fn a_short_trails_the_lowest_middle() {
    let ticks = tape(&[
        (100, 99.8, 100.0),
        (3_000, 97.8, 98.0),
        (5_000, 98.9, 99.1),
        (9_000, 98.9, 99.1),
    ]);
    // 97.9 · 1.01 = 98.879; the middle 99.0 is above it at 6 450.
    let w = walk(&deal(true), &ticks, fill(), 50.0, &trailing());
    assert_eq!((w.exit.kind, w.exit.t_ms), (ExitKind::Stop, 6_450), "{w:?}");
}

/// The line under a peak, floored at the take profit's level off the buy — a short's by division.
#[test]
fn the_trailing_level_is_the_line_under_the_peak_floored_at_the_take() {
    let p = trailing();
    assert!((trailing_level(102.1, 100.0, &p, true) - 101.079).abs() < 1e-9);
    assert!((trailing_level(97.9, 100.0, &p, false) - 98.879).abs() < 1e-9);
    let tp = ExitParams {
        trailing_take_profit_pct: Some(2.0),
        ..trailing()
    };
    assert!((trailing_level(102.5, 100.0, &tp, true) - 102.0).abs() < 1e-9);
    // A short's line 97.5 · 1.01 = 98.475 stands above its take 100/1.02 = 98.04: the take caps it;
    // 97.0 · 1.01 = 97.97 is already below the take and stays.
    assert!((trailing_level(97.5, 100.0, &tp, false) - 100.0 / 1.02).abs() < 1e-9);
    assert!((trailing_level(97.0, 100.0, &tp, false) - 97.97).abs() < 1e-9);
}

/// A fact the trailing closed: the verdict cuts its archived line at the panic sell — the first
/// move past the fact's own line under the peak its reason printed — and times the model's
/// trailing against that move. Without the cut the panic sell's move would count as a line move
/// the model never made.
#[test]
fn verify_cuts_a_trailing_fact_at_its_panic_sell() {
    assert_eq!(
        stated_peak(
            "TrailingStop AutoActivated on price drop: ASK = 101.10 LastPrice = 101.00; PeakPrice = 102.10; allowed drop"
        ),
        Some(102.1)
    );
    assert_eq!(
        stated_peak("TrailingStop AutoActivated … PeakPrice = ;"),
        None
    );
    let ticks = tape(&[
        (100, 100.0, 100.2),
        (3_000, 102.0, 102.2),
        (5_000, 100.9, 101.1),
        (9_000, 100.0, 100.2),
    ]);
    let mut d = deal(false);
    d.sell_reason = "TrailingStop AutoActivated on price drop: ASK = 101.10 LastPrice = 101.00; \
                     PeakPrice = 102.10; allowed drop level: -30.0% ; spread: 0.5%"
        .into();
    d.close_ms = 6_900;
    d.sell_price = 100.5;
    // The take as placed, then the panic sell at 100.5.
    let archived = [(0, 150.0), (6_500, 100.5)];
    let v = verify(
        &d,
        &ticks,
        &EntryParams::Fact,
        &trailing(),
        None,
        Some(&archived),
    );
    assert_eq!(v.exit_kind, Some(ExitKind::Stop), "{v:?}");
    assert_eq!(v.exit, Some(true), "{v:?}");
}

/// A book stop past its level on the same arrival as the trailing line goes first: the exit is
/// the stop's, at the bid the book stop reads, not the trailing's middle.
#[test]
fn the_stop_goes_first_on_the_same_arrival() {
    let both = ExitParams {
        stop_loss_pct: -1.0,
        fast_stop_loss: false,
        stop_loss_ema: 7.0,
        ..trailing()
    };
    let ticks = tape(&[
        (100, 100.0, 100.2),
        (3_000, 98.0, 98.4),
        (9_000, 98.0, 98.4),
    ]);
    let w = walk(&deal(false), &ticks, fill(), far_take(), &both);
    assert_eq!((w.exit.kind, w.exit.t_ms), (ExitKind::Stop, 4_300), "{w:?}");
    assert!((w.exit.price - 98.0).abs() < 1e-4, "the bid, {w:?}");
}
