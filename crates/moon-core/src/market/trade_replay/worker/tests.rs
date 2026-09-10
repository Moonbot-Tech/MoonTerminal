use std::cell::Cell;

use super::*;
use crate::feed::Side;
use crate::market::trade_replay::rest::{FetchError, TradePage};

/// Native data remains usable when context is rate-limited or the venue has no candle endpoint.
#[test]
fn native_ticks_survive_unavailable_candle_context() {
    for fallback in [
        TradeReplayOutcome::Failed(TradeReplayFailure::RateLimited { retry_in_s: 17 }),
        TradeReplayOutcome::Empty(TradeReplayEmpty::NoEndpoint {
            brand: crate::venue::Brand::Bybit,
        }),
    ] {
        let served = core_first(
            || Some(core_series(true)),
            || Served {
                outcome: fallback,
                tick_stage: None,
            },
        );
        let TradeReplayOutcome::Ready(series) = served.outcome else {
            panic!("usable native ticks hidden by a context failure")
        };
        assert_eq!(series.ticks.len(), 2);
        assert_eq!(series.source, TradeReplaySource::CoreTicks);
        assert_eq!(series.tick_status, TickStatus::ContextUnavailable);
        assert!(series.candles.is_empty());
    }
}

/// A late native result cannot erase a wider exchange interval already displayed to the user.
#[test]
fn late_core_upgrade_rejects_narrower_published_coverage() {
    let now = Instant::now();
    let probe = CoreUpgradeProbe::new(now);
    let shown = Some((0, 180_000));
    assert!(
        !probe.stop(false, now + Duration::from_millis(500), shown, || Some(
            core_series(true)
        ))
    );
    assert!(
        probe.ready.borrow().is_none(),
        "narrow core span must not stop REST or replace displayed points"
    );
    assert!(
        probe.stop(false, now + Duration::from_secs(1), shown, || Some(
            core_series(false)
        ))
    );
    assert_eq!(
        probe
            .ready
            .into_inner()
            .expect("covering replacement")
            .covered,
        shown
    );
}

/// Terminal public tick answers still observe a later native archive independently.
#[test]
fn delayed_native_archive_upgrades_no_route_and_retention_answers() {
    for status in [
        TickStatus::NoRoute,
        TickStatus::Failed,
        TickStatus::NoTrades,
        TickStatus::OutOfRetention {
            retention_ms: 1_000,
        },
    ] {
        let now = Instant::now();
        let mut bars = core_series(true);
        bars.source = TradeReplaySource::Klines1m;
        bars.tick_status = status;
        bars.ticks.clear();
        bars.candles.push(ChartCandle {
            t_open_ms: 0.0,
            open: 10.0,
            high: 12.0,
            low: 9.0,
            close: 11.0,
            volume: 5.0,
            quote_volume: 55.0,
        });
        let mut served = Served {
            outcome: TradeReplayOutcome::Ready(bars),
            tick_stage: None,
        };
        let mut wait = prepare_native_wait(&mut served, now)
            .expect("native follow-up must not require a REST tick job");
        assert!(
            matches!(served.outcome, TradeReplayOutcome::Ready(ref s) if s.tick_status == TickStatus::AwaitingCore)
        );
        assert!(wait.advance(None, now + Duration::from_secs(2)).is_none());
        let TradeReplayOutcome::Ready(series) = wait
            .advance(Some(core_series(true)), now + Duration::from_secs(3))
            .expect("late archive")
        else {
            panic!("native data lost")
        };
        assert_eq!(series.source, TradeReplaySource::CoreTicks);
        assert_eq!(series.tick_status, TickStatus::Served);
        assert_eq!(series.candles.len(), 1);
        let TradeReplayOutcome::Ready(expired) = wait
            .advance(None, now + Duration::from_secs(31))
            .expect("bounded follow-up")
        else {
            panic!("fallback lost")
        };
        assert_eq!(
            expired.tick_status, status,
            "timeout must restore the terminal reason, not leave a pending caption"
        );
    }
}

/// A retry must preserve wider cached points through native replacement and subsequent reopening.
#[test]
fn cached_retry_keeps_wider_tick_coverage() {
    let mut wide = core_series(false);
    wide.source = TradeReplaySource::Ticks;
    wide.window.to_ms = 600_000;
    wide.covered = Some((0, 360_000));
    wide.partial = true;
    let route = trade_route(wide.venue).expect("tick route");
    let key = OutcomeKey {
        venue: wide.venue,
        host: route.host(),
        market: "BTCUSDT".to_owned(),
        from_ms: wide.window.from_ms,
        to_ms: wide.window.to_ms,
    };
    let cache = Mutex::new(VecDeque::new());
    remember_store(
        &cache,
        key.clone(),
        Remembered::Ready {
            series: wide.clone(),
            ticks_settled: false,
        },
    );
    let Some(Remembered::Ready { mut series, .. }) = remember_lookup(&cache, &key, 42) else {
        panic!("retry cache missing")
    };
    let stage =
        stage_and_stamp(series.venue, series.window, &key, &mut series).expect("retry stage");
    assert_eq!(series.tick_status, TickStatus::Streaming);
    let narrow = core_series(true);
    assert!(!preserves_coverage(
        narrow.covered,
        stage.baseline.as_ref().and_then(|s| s.covered)
    ));
    let retained = retain_baseline(narrow, stage.baseline.as_ref());
    remember_store(
        &cache,
        key.clone(),
        Remembered::Ready {
            series: retained,
            ticks_settled: true,
        },
    );
    let Some(Remembered::Ready {
        series: reopened, ..
    }) = remember_lookup(&cache, &key, 43)
    else {
        panic!("settled cache missing")
    };
    assert_eq!(reopened.covered, wide.covered);
    assert_eq!(reopened.source, TradeReplaySource::Ticks);
}

/// Frozen native answer for testing the production source-selection path without I/O.
fn core_series(partial: bool) -> TradeReplaySeries {
    TradeReplaySeries {
        source: TradeReplaySource::CoreTicks,
        venue: crate::venue::venue(3).expect("Binance spot"),
        window: ReplayWindow {
            from_ms: 0,
            to_ms: 180_000,
            open_ms: 60_000,
            close_ms: 120_000,
            over_budget: false,
        },
        tf_ms: BAR_MS,
        candles: Vec::new(),
        ticks: vec![tick(60_000, 10.0), tick(120_000, 11.0)],
        identity: 42,
        tick_status: TickStatus::Served,
        bucket_ms: 0,
        partial,
        covered: Some(if partial {
            (60_000, 120_000)
        } else {
            (0, 180_000)
        }),
    }
}

/// Available core points still need the wide candle stage, but must avoid a redundant core scan.
#[test]
fn core_replay_preserves_wide_candle_stage() {
    let reads = Cell::new(0);
    let candles_requested = Cell::new(false);
    let served = core_first(
        || {
            reads.set(reads.get() + 1);
            Some(core_series(true))
        },
        || {
            candles_requested.set(true);
            let mut bars = core_series(true);
            bars.source = TradeReplaySource::Klines1m;
            Served {
                outcome: TradeReplayOutcome::Ready(bars),
                tick_stage: None,
            }
        },
    );
    assert!(candles_requested.get());
    assert_eq!(
        reads.get(),
        1,
        "a ready narrow core span needs no immediate rescan"
    );
    assert!(served.tick_stage.is_none());
    let TradeReplayOutcome::Ready(series) = served.outcome else {
        panic!("core answer lost")
    };
    assert_eq!(series.source, TradeReplaySource::CoreTicks);
    assert!(series.partial);
}

/// A newly arrived core archive upgrades candles and cancels their pending exchange tick stage.
#[test]
fn core_replay_rechecks_after_candles_and_keeps_context() {
    let reads = Cell::new(0);
    let mut bars = core_series(true);
    bars.source = TradeReplaySource::Klines1m;
    bars.ticks.clear();
    bars.covered = None;
    bars.tick_status = TickStatus::Pending;
    bars.candles.push(ChartCandle {
        t_open_ms: 0.0,
        open: 10.0,
        high: 12.0,
        low: 9.0,
        close: 11.0,
        volume: 5.0,
        quote_volume: 55.0,
    });
    let route = trade_route(bars.venue).expect("spot trade route");
    let stage = TickStage {
        baseline: None,
        route,
        key: OutcomeKey {
            venue: bars.venue,
            host: route.host(),
            market: "BTCUSDT".into(),
            from_ms: bars.window.from_ms,
            to_ms: bars.window.to_ms,
        },
        candles: bars.candles.clone(),
    };
    let served = core_first(
        || {
            reads.set(reads.get() + 1);
            (reads.get() == 2).then(|| core_series(true))
        },
        || Served {
            outcome: TradeReplayOutcome::Ready(bars),
            tick_stage: Some(stage),
        },
    );
    let TradeReplayOutcome::Ready(series) = served.outcome else {
        panic!("core upgrade lost")
    };
    assert_eq!(series.source, TradeReplaySource::CoreTicks);
    assert_eq!(series.covered, Some((60_000, 120_000)));
    assert_eq!(
        series.candles.len(),
        1,
        "context candles outside the core span must survive"
    );
    assert_eq!(series.tick_status, TickStatus::Served);
    assert!(served.tick_stage.is_none());
}

/// Unavailable core history must preserve the REST answer instead of reporting empty success.
#[test]
fn core_replay_missing_history_preserves_rest_failure() {
    let served = core_first(
        || None,
        || Served {
            outcome: TradeReplayOutcome::Failed(TradeReplayFailure::RateLimited { retry_in_s: 17 }),
            tick_stage: None,
        },
    );
    assert!(matches!(
        served.outcome,
        TradeReplayOutcome::Failed(TradeReplayFailure::RateLimited { retry_in_s: 17 })
    ));
}

/// Records the pagination seam without touching a real host gate.
#[derive(Default)]
struct FakeObserver {
    claims: usize,
    paces: usize,
    snapshots: Vec<(usize, (i64, i64))>,
}

impl TickObserver for FakeObserver {
    fn claim(&mut self, _host: &str) -> Result<(), u32> {
        self.claims += 1;
        Ok(())
    }

    fn pace(&mut self, _host: &str) {
        self.paces += 1;
    }

    /// Capture progress boundaries so tests can distinguish streaming from a final-only answer.
    fn progress(&mut self, ticks: &[Tick], covered: (i64, i64)) {
        self.snapshots.push((ticks.len(), covered));
    }
}

/// A core archive that arrives later must stop a REST walk without busy polling or sleeping.
#[test]
fn late_core_upgrade_is_detected_between_pages() {
    let now = Instant::now();
    let probe = CoreUpgradeProbe::new(now);
    assert!(!probe.stop(false, now, None, || panic!("early rescan")));
    assert!(
        probe.stop(false, now + Duration::from_millis(500), None, || Some(
            core_series(true)
        ))
    );
    assert!(probe.ready.into_inner().is_some());
}

/// Finished tiles must publish increasing snapshots before the whole plan has completed.
#[test]
fn paginator_publishes_contiguous_groups() {
    let plan = TickPlan {
        slices: vec![(100, 199), (200, 299)],
        focus_len: 1,
    };
    let mut observer = FakeObserver::default();
    let verdict = paginate_ticks(
        TradeRoute::BinanceUsdMAggTrades,
        &plan,
        100,
        10,
        || false,
        || false,
        &mut observer,
        |from, _, _| page(vec![tick(from + 5, 10.0)]),
    );
    assert!(matches!(verdict, TickVerdict::Ready(_)));
    assert_eq!(observer.snapshots, vec![(1, (100, 199)), (2, (100, 299))]);
}

/// A backwards page to the right cannot fill its unfetched gap next to the visible prefix.
#[test]
fn partial_page_progress_respects_pagination_direction() {
    let rows = [tick(250, 10.0), tick(299, 11.0)];
    assert_eq!(
        page_progress_span(
            Some((100, 199)),
            (200, 299),
            &rows,
            Some(rest::TradeCursor::LessThanId(42))
        ),
        Some((100, 199))
    );
    assert_eq!(
        page_progress_span(
            Some((100, 199)),
            (200, 299),
            &rows,
            Some(rest::TradeCursor::FromId(42))
        ),
        Some((100, 299))
    );
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
