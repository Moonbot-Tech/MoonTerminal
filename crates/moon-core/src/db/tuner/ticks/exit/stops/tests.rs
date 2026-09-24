//! The Stops section on synthetic tapes: the fast stop, the book stop on its ticker proxy, and
//! how the verdict judges each.

use super::*;
use crate::db::tuner::ticks::exit::line::walk;
use crate::db::tuner::ticks::exit::tests::{deal, fill, params, tape, tick};
use crate::db::tuner::ticks::{EntryParams, verify};
use crate::feed::types::Side as TickSide;

// ---- StopLoss ---------------------------------------------------------------------------------

#[test]
fn the_stop_fires_on_the_print_after_its_delay() {
    let p = ExitParams {
        stop_loss_pct: -1.0,
        stop_loss_delay_s: 2.0,
        ..params()
    };
    // A print through the stop inside the delay does not fire; one after it does, at the
    // print's price (a market exit).
    let ticks = tape(&[(1_000, 98.5), (3_000, 98.7)]);
    let w = walk(&deal(false), &ticks, fill(), 101.0, &p);
    assert_eq!((w.exit.kind, w.exit.t_ms), (ExitKind::Stop, 3_000));
    assert!((w.exit.price - 98.7).abs() < 1e-4);
}

/// A short's stop is the buy DIVIDED by `1 + StopLoss/100`, not the long's product mirrored:
/// `-2.5 %` off 100 is 102.564, so a print at 102.53 is still inside it. The core prints it that
/// way on 11 099 short stops of the report (`StopLoss fixed: X`) against 25 for the mirror.
#[test]
fn a_short_stop_divides_the_buy() {
    let p = ExitParams {
        stop_loss_pct: -2.5,
        ..params()
    };
    let inside = tape(&[(1_000, 102.53), (2_000, 102.57)]);
    let w = walk(&deal(true), &inside, fill(), 99.0, &p);
    assert_eq!((w.exit.kind, w.exit.t_ms), (ExitKind::Stop, 2_000), "{w:?}");
    assert!((stop_level(100.0, -2.5, false) - 100.0 / 0.975).abs() < 1e-9);
    assert!((stop_level(100.0, -2.5, true) - 97.5).abs() < 1e-9);
    // A stop on the profit side of a short sits below the buy, by the same division.
    assert!((stop_level(100.0, 0.4, false) - 100.0 / 1.004).abs() < 1e-9);
    // A loss of 100 % or more leaves a short no finite price to stop at.
    assert_eq!(stop_level(100.0, -100.0, false), f64::INFINITY);
}

fn sold(t_ms: i64, price: f64) -> Tick {
    Tick {
        side: TickSide::Sell,
        ..tick(t_ms, price)
    }
}

/// A book-watching stop (`FastStopLoss` off) that reads the bare ticker price: `StopLossEMA`
/// neither 0 (which adds the series) nor 3, 5, 10 (which average).
fn bare_book_stop() -> ExitParams {
    ExitParams {
        stop_loss_pct: -1.0,
        fast_stop_loss: false,
        stop_loss_ema: 7.0,
        ..params()
    }
}

/// The book-watching stop (`FastStopLoss` off) reads the ticker's BID through the prints that
/// hit it — taker sells — on the ticker's clock, not every print through the level.
#[test]
fn the_book_stop_fires_on_a_sample_of_the_bid_not_on_a_print() {
    let book = bare_book_stop();
    // A taker BUY through the level says nothing about the BID; the taker sell at 98.8 does,
    // and the next arrival after it — 4.3 s — fires, at the proxy's price.
    let ticks = vec![
        tick(1_000, 98.5),
        sold(1_500, 99.5),
        sold(2_500, 98.8),
        tick(5_000, 100.0),
    ];
    let w = walk(&deal(false), &ticks, fill(), 101.0, &book);
    assert_eq!(
        (w.exit.kind, w.exit.t_ms),
        (ExitKind::Stop, 2 * TICKER_PERIOD_MS)
    );
    assert!((w.exit.price - 98.8).abs() < 1e-4);
    // The fast stop takes the first print through the level, whichever side it hit.
    let fast = ExitParams {
        fast_stop_loss: true,
        ..book.clone()
    };
    let w = walk(&deal(false), &ticks, fill(), 101.0, &fast);
    assert_eq!((w.exit.kind, w.exit.t_ms), (ExitKind::Stop, 1_000));
}

/// A sample due exactly at the tape's last print reads that print — the loop only reaches the
/// samples before a print, so the tape's end is where it must not be forgotten.
#[test]
fn the_book_stop_takes_the_sample_at_the_last_print() {
    let book = bare_book_stop();
    let w = walk(
        &deal(false),
        &[sold(TICKER_PERIOD_MS, 98.8)],
        fill(),
        101.0,
        &book,
    );
    assert_eq!(
        (w.exit.kind, w.exit.t_ms),
        (ExitKind::Stop, TICKER_PERIOD_MS)
    );
    // A sample the tape ends before is not taken: nothing is known past the last print.
    let w = walk(&deal(false), &[sold(1_500, 98.8)], fill(), 101.0, &book);
    assert_eq!(w.exit.kind, ExitKind::OpenAtWindowEnd);
}

/// `StopLossEMA` 3 averages the ticker's arrivals as the core does, `(avg·2 + bid)/3`, so a BID
/// just past the level fires only once the average is past it too.
#[test]
fn the_stop_ema_waits_for_the_average() {
    let ticks = vec![
        sold(1_500, 99.5),
        sold(2_500, 98.9),
        sold(9_000, 98.9),
        sold(13_000, 98.9),
    ];
    let bare = bare_book_stop();
    let w = walk(&deal(false), &ticks, fill(), 101.0, &bare);
    assert_eq!(
        (w.exit.kind, w.exit.t_ms),
        (ExitKind::Stop, 2 * TICKER_PERIOD_MS)
    );
    // Arrivals 99.5, then 98.9 from the second on: the average reads 99.5, 99.3, 99.167,
    // 99.078, 99.019, 98.979 — past 99 at the sixth. An EMA of 2/(N + 1) = 0.5 would have
    // crossed at the fourth.
    let smoothed = ExitParams {
        stop_loss_ema: 3.0,
        ..bare
    };
    let w = walk(&deal(false), &ticks, fill(), 101.0, &smoothed);
    assert_eq!(
        (w.exit.kind, w.exit.t_ms),
        (ExitKind::Stop, 6 * TICKER_PERIOD_MS)
    );
}

/// The core keeps the average from its start, so at the fill it remembers the prices before it:
/// the prints before the fill warm it, and can never fire it.
#[test]
fn the_stop_average_is_warm_at_the_fill() {
    let smoothed = ExitParams {
        stop_loss_ema: 3.0,
        ..bare_book_stop()
    };
    let after = [sold(1_500, 98.9), sold(30_000, 98.9)];
    let cold = walk(&deal(false), &after, fill(), 101.0, &smoothed);
    assert_eq!(
        (cold.exit.kind, cold.exit.t_ms),
        (ExitKind::Stop, TICKER_PERIOD_MS)
    );
    // A minute at 101 before the fill: the average starts there, seven arrivals of 98.9 leave
    // it at 101 · (2/3)^7 + 98.9 · (1 − (2/3)^7) = 99.02, and the eighth brings it to 98.98.
    let mut warm: Vec<Tick> = (0..30).map(|i| sold(-60_000 + i * 2_000, 101.0)).collect();
    warm.extend(after);
    let w = walk(&deal(false), &warm, fill(), 101.0, &smoothed);
    assert_eq!(w.exit.kind, ExitKind::Stop);
    assert_eq!(w.exit.t_ms, 8 * TICKER_PERIOD_MS, "{w:?}");
    // The prints before the fill, past the level, fire nothing before it.
    let early = vec![sold(-3_000, 98.0), tick(500, 100.0)];
    let w = walk(&deal(false), &early, fill(), 101.0, &bare_book_stop());
    assert!(w.exit.t_ms > 0, "{w:?}");
}

/// A short's book stop watches the bare ASK: `StopLossEMA` averages a long's BID only.
#[test]
fn a_short_book_stop_does_not_average() {
    let smoothed = ExitParams {
        stop_loss_ema: 3.0,
        ..bare_book_stop()
    };
    // A taker buy prints at the ASK: 100 before the fill, then 101.1 past a short's stop at 101
    // — which an average of the two, 100.37, would not be.
    let ticks = vec![tick(-1_000, 100.0), tick(1_500, 101.1), tick(5_000, 100.0)];
    let w = walk(&deal(true), &ticks, fill(), 99.0, &smoothed);
    assert_eq!(
        (w.exit.kind, w.exit.t_ms),
        (ExitKind::Stop, TICKER_PERIOD_MS)
    );
}

/// At `StopLossEMA` 0 the core's price series fires the stop too: a lone print past the level
/// is its tick's point, and fires when the tick closes; a spike among prints near the last
/// point is not the point, and waits for the ticker.
#[test]
fn a_stop_without_averaging_fires_on_the_series_point() {
    let series = ExitParams {
        stop_loss_ema: 0.0,
        ..bare_book_stop()
    };
    let lone = vec![tick(1_000, 98.5), tick(5_000, 100.0)];
    let w = walk(&deal(false), &lone, fill(), 101.0, &series);
    assert_eq!((w.exit.kind, w.exit.t_ms), (ExitKind::Stop, 1_250));
    assert!((w.exit.price - 98.5).abs() < 1e-4);
    // The same print after a point at 100 and beside 99.9 in its tick: the point is 99.9.
    let spike = vec![
        tick(900, 100.0),
        tick(1_000, 98.5),
        tick(1_100, 99.9),
        tick(5_000, 100.0),
    ];
    let w = walk(&deal(false), &spike, fill(), 101.0, &series);
    assert_ne!(w.exit.kind, ExitKind::Stop, "{w:?}");
    // At 7 the core reads the bare ticker price and no series.
    let w = walk(&deal(false), &lone, fill(), 101.0, &bare_book_stop());
    assert_ne!(w.exit.kind, ExitKind::Stop, "{w:?}");
}

/// A book stop's sale is a panic sell walked through the book: the verdict holds the model's
/// stop level against the one the core printed, and the moment against the close — never the
/// sale price. A stop the core fired and the model never did is a miss.
#[test]
fn verify_judges_a_book_stop_by_its_level_and_moment() {
    let book = ExitParams {
        stop_loss_pct: -1.0,
        fast_stop_loss: false,
        ..params()
    };
    let mut d = deal(false);
    d.sell_reason = "StopLoss AutoActivated on price drop: BID = 98.800 ASK: 98.900 \
                     (strategy <S>); StopLoss fixed: 99.000 AllowedDrop: BUY -15.0%"
        .into();
    d.sell_price = 97.0;
    d.close_ms = 4_050;
    let ticks = vec![sold(1_500, 99.5), sold(2_500, 98.8), tick(5_000, 100.0)];
    let v = verify(&d, &ticks, &EntryParams::Fact, &book, None, None);
    assert_eq!(v.exit_kind, Some(ExitKind::Stop));
    assert_eq!(v.exit, Some(true), "{v:?}");
    assert!(v.exit_dev_pct.is_some_and(|dev| dev.abs() < 1e-9));
    // The stored reason cut inside the level: still the book stop, judged by its moment.
    let full = d.sell_reason.clone();
    d.sell_reason = full[..full.find("99.000").expect("level") + 3].to_string();
    let v = verify(&d, &ticks, &EntryParams::Fact, &book, None, None);
    assert_eq!(v.exit, Some(true), "{v:?}");
    d.close_ms = 9_000;
    let v = verify(&d, &ticks, &EntryParams::Fact, &book, None, None);
    assert_eq!(v.exit, Some(false), "five seconds off the moment, {v:?}");
    d.close_ms = 4_050;
    d.sell_reason = full;
    // The core's level elsewhere: not this stop.
    d.sell_reason = d.sell_reason.replace("99.000", "98.000");
    let v = verify(&d, &ticks, &EntryParams::Fact, &book, None, None);
    assert_eq!(v.exit, Some(false), "{v:?}");
    // A tape whose BID never reaches the level: the core stopped, the model did not.
    let calm = vec![sold(1_500, 99.5), tick(5_000, 100.0)];
    let v = verify(&d, &calm, &EntryParams::Fact, &book, None, None);
    assert_eq!(v.exit, Some(false), "{v:?}");
}

/// A market stop's sale sweeps our size through the book: with no level on record — its reason
/// never carries one — the verdict holds the moment it fired, never the sweep's price.
#[test]
fn verify_judges_a_market_stop_without_a_level_by_its_moment() {
    let fast = ExitParams {
        stop_loss_pct: -1.0,
        fast_stop_loss: true,
        ..params()
    };
    let mut d = deal(false);
    d.sell_reason = "StopLoss Market Sell".into();
    // 1.4 % past the print that fired it: the sweep, which no print on the tape shows.
    d.sell_price = 97.5;
    d.close_ms = 3_100;
    let ticks = tape(&[(1_000, 99.5), (3_000, 98.9), (5_000, 99.0)]);
    let v = verify(&d, &ticks, &EntryParams::Fact, &fast, None, None);
    assert_eq!(v.exit_kind, Some(ExitKind::Stop));
    assert_eq!(v.exit, Some(true), "{v:?}");
    assert_eq!(v.exit_dev_pct, None);
    d.close_ms = 9_000;
    let v = verify(&d, &ticks, &EntryParams::Fact, &fast, None, None);
    assert_eq!(v.exit, Some(false), "six seconds off the moment, {v:?}");
}
