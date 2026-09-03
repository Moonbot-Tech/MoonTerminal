use std::cell::Cell;

use super::*;
use crate::feed::Side;
use crate::market::trade_replay::rest::{FetchError, TradePage};

/// Records the pagination seam without touching a real host gate.
#[derive(Default)]
struct FakeObserver {
    claims: usize,
    paces: usize,
}

impl TickObserver for FakeObserver {
    fn claim(&mut self, _host: &str) -> Result<(), u32> {
        self.claims += 1;
        Ok(())
    }

    fn pace(&mut self, _host: &str) {
        self.paces += 1;
    }
}

/// Builds one real-looking trade row for a deterministic fake page.
fn tick(time_ms: i64, price: f32) -> Tick {
    Tick {
        time_ms: time_ms as f64,
        price,
        qty: 1.0,
        side: Side::Buy,
    }
}

/// Returns a completed fake page with the supplied rows.
fn page(ticks: Vec<Tick>) -> Result<TradePage, FetchError> {
    Ok(TradePage { ticks, next: None })
}

/// `market/trade_replay/worker.rs:rows_for_cache` returning rows for both arms would file
/// tick-derived data as shared settled klines for every core.
#[test]
fn ticks_are_never_eligible_for_the_shared_kline_cache() {
    let rows = [ChartCandle {
        t_open_ms: 10_000.0,
        open: 10.0,
        high: 12.0,
        low: 9.0,
        close: 11.0,
        volume: 5.0,
        quote_volume: 55.0,
    }];

    assert!(
        rows_for_cache(TradeReplaySource::Ticks, &rows).is_empty(),
        "a tick series must never write a candle-shaped row into shared klines.sqlite"
    );
    assert_eq!(
        rows_for_cache(TradeReplaySource::Klines1m, &rows),
        &rows,
        "genuine exchange klines remain cacheable"
    );
}

/// `market/trade_replay/worker.rs:inside_retention` judging `window.from_ms` instead of the
/// focus rejects a still-retained trade merely because optional lead context is older.
#[test]
fn retention_is_decided_from_the_trade_focus_not_padded_context() {
    const HOUR_MS: i64 = 3_600_000;
    let now_ms = 1_000 * HOUR_MS;
    let window = ReplayWindow {
        from_ms: now_ms - 60 * HOUR_MS,
        to_ms: now_ms - 10 * HOUR_MS,
        open_ms: now_ms - 47 * HOUR_MS,
        close_ms: now_ms - 46 * HOUR_MS,
        over_budget: false,
    };

    assert!(
        inside_retention(TradeRoute::BinanceUsdMAggTrades, window, now_ms),
        "the focus begins inside Binance USD-M's 48-hour retention despite older optional lead"
    );
}

/// `market/trade_replay/worker.rs:paginate_ticks` restoring an over-budget abandonment would
/// throw away a non-empty focus harvest and fall back to candles.
#[test]
fn budget_stops_after_whole_focus_slices_and_serves_their_harvest() {
    let plan = TickPlan {
        slices: vec![(100, 199), (200, 299), (0, 99)],
        focus_len: 2,
    };
    let mut observer = FakeObserver::default();

    let verdict = paginate_ticks(
        TradeRoute::BinanceUsdMAggTrades,
        &plan,
        40_000,
        10,
        || false,
        || false,
        &mut observer,
        |from_ms, _, _| {
            page(
                (0..30_000)
                    .map(|n| tick(from_ms + (n % 100), n as f32))
                    .collect(),
            )
        },
    );

    let TickVerdict::Ready(harvest) = verdict else {
        panic!("a non-empty harvest stopped by the tick budget must be served")
    };
    assert_eq!(
        harvest.ticks.len(),
        60_000,
        "the 40,000 budget must not truncate either 30,000-row focus slice"
    );
    assert_eq!(
        harvest.covered,
        (100, 299),
        "only the two completed focus slices are covered after the budget stops the walk"
    );
    assert!(
        !harvest.complete,
        "skipping the non-focus slice is a partial, not a complete, harvest"
    );
    assert_eq!(observer.claims, 1, "a stage takes one host permit");
}

/// `market/trade_replay/worker.rs:paginate_ticks` returning `Abandoned(Deadline)` after any
/// fetched page discards usable ticks and replaces the user's trade with candles.
#[test]
fn deadline_with_a_non_empty_harvest_is_ready_not_abandoned() {
    let plan = TickPlan {
        slices: vec![(100, 199), (0, 99)],
        focus_len: 1,
    };
    let fetched = Cell::new(false);
    let mut observer = FakeObserver::default();

    let verdict = paginate_ticks(
        TradeRoute::BinanceUsdMAggTrades,
        &plan,
        40_000,
        10,
        || false,
        || fetched.get(),
        &mut observer,
        |from_ms, _, _| {
            fetched.set(true);
            page(vec![tick(from_ms + 5, 10.0)])
        },
    );

    let TickVerdict::Ready(harvest) = verdict else {
        panic!("a deadline after a fetched focus page must preserve the non-empty harvest")
    };
    assert_eq!(
        harvest
            .ticks
            .iter()
            .map(|tick| (tick.time_ms as i64, tick.price, tick.qty))
            .collect::<Vec<_>>(),
        vec![(105, 10.0, 1.0)],
        "the deadline preserves the fetched tick's time, price, and quantity"
    );
    assert_eq!(harvest.covered, (100, 199));
    assert!(
        !harvest.complete,
        "the deadline leaves remaining slices unwalked"
    );
}

/// `market/trade_replay/worker.rs:paginate_ticks` dropping its per-page retain lets adjacent
/// slices duplicate overshot exchange trades and spend the tick budget twice.
#[test]
fn each_fetched_page_is_clipped_to_its_own_slice_before_collection() {
    let plan = TickPlan {
        slices: vec![(100, 199), (200, 299)],
        focus_len: 1,
    };
    let mut observer = FakeObserver::default();
    let verdict = paginate_ticks(
        TradeRoute::BinanceUsdMAggTrades,
        &plan,
        40_000,
        10,
        || false,
        || false,
        &mut observer,
        |from_ms, _, _| match from_ms {
            100 => page(vec![
                tick(99, 1.0),
                tick(100, 2.0),
                tick(199, 3.0),
                tick(200, 4.0),
            ]),
            200 => page(vec![
                tick(199, 5.0),
                tick(200, 6.0),
                tick(299, 7.0),
                tick(300, 8.0),
            ]),
            _ => unreachable!("the plan contains only two slices"),
        },
    );

    let TickVerdict::Ready(harvest) = verdict else {
        panic!("the fake pages contain in-slice ticks")
    };
    assert_eq!(
        harvest
            .ticks
            .iter()
            .map(|tick| tick.time_ms as i64)
            .collect::<Vec<_>>(),
        vec![100, 199, 200, 299],
        "only rows inside their own requested slice may enter the aggregate harvest"
    );
}
