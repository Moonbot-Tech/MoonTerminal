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
    let shown = Coverage::one((0, 180_000));
    assert!(
        !probe.stop(false, now + Duration::from_millis(500), &shown, || Some(
            core_series(true)
        ))
    );
    assert!(
        probe.ready.borrow().is_none(),
        "narrow core span must not stop REST or replace displayed points"
    );
    assert!(
        probe.stop(false, now + Duration::from_secs(1), &shown, || Some(
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

/// A model's request never arms the follow-up: its short answer keeps the stage's own terminal
/// status and the sender drops with it, so a batch pays no thirty seconds per row.
#[test]
fn model_request_skips_the_native_follow_up_and_keeps_the_terminal_status() {
    let (reply, _rx) = std::sync::mpsc::channel();
    let mut request = tile_request(reply);
    let now = Instant::now();
    let mut bars = core_series(true);
    bars.source = TradeReplaySource::Klines1m;
    bars.tick_status = TickStatus::Failed;
    bars.ticks.clear();
    let mut served = Served {
        outcome: TradeReplayOutcome::Ready(bars.clone()),
        tick_stage: None,
    };
    assert!(
        arm_native_wait(&request, &mut served, now).is_some(),
        "a chart's short answer waits for the archive"
    );
    request.intent = ReplayIntent::Model;
    let mut served = Served {
        outcome: TradeReplayOutcome::Ready(bars),
        tick_stage: None,
    };
    assert!(arm_native_wait(&request, &mut served, now).is_none());
    assert!(
        matches!(served.outcome, TradeReplayOutcome::Ready(ref s) if s.tick_status == TickStatus::Failed),
        "the terminal status must not be rewritten to a waiting caption"
    );
}

/// A retry must preserve wider cached points through native replacement and subsequent reopening.
#[test]
fn cached_retry_keeps_wider_tick_coverage() {
    let mut wide = core_series(false);
    wide.source = TradeReplaySource::Ticks;
    wide.window.to_ms = 600_000;
    wide.covered = Coverage::one((0, 360_000));
    wide.partial = true;
    let route = trade_route(wide.venue).expect("tick route");
    let key = OutcomeKey {
        venue: wide.venue,
        host: route.host(),
        market: "BTCUSDT".to_owned(),
        from_ms: wide.window.from_ms,
        to_ms: wide.window.to_ms,
        margin_ms: wide.window.margin_ms,
        long_position_ms: wide.window.long_position_ms,
    };
    let cache = Mutex::new(VecDeque::new());
    remember_store(
        &cache,
        ReplayIntent::Chart,
        key.clone(),
        Remembered::Ready {
            series: wide.clone(),
            ticks_settled: false,
        },
    );
    let Some(Remembered::Ready { mut series, .. }) = remember_lookup(&cache, &key, 42) else {
        panic!("retry cache missing")
    };
    let stage = stage_and_stamp(series.venue, &key, &mut series);
    assert_eq!(series.tick_status, TickStatus::Streaming);
    let narrow = core_series(true);
    assert!(!preserves_coverage(
        &narrow.covered,
        &stage
            .baseline
            .as_ref()
            .map(|s| s.covered.clone())
            .unwrap_or_default()
    ));
    let retained = retain_baseline(narrow, stage.baseline.as_ref());
    remember_store(
        &cache,
        ReplayIntent::Chart,
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
            margin_ms: 5 * 60_000,
            long_position_ms: 5 * 60_000,
            over_budget: false,
        },
        tf_ms: BAR_MS,
        candles: Vec::new(),
        ticks: vec![tick(60_000, 10.0), tick(120_000, 11.0)],
        identity: 42,
        tick_status: TickStatus::Served,
        side_slots: Vec::new(),
        bucket_ms: 0,
        partial,
        covered: Coverage::one(if partial {
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
    bars.covered = Coverage::none();
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
        route: Some(route),
        key: OutcomeKey {
            venue: bars.venue,
            host: route.host(),
            market: "BTCUSDT".into(),
            from_ms: bars.window.from_ms,
            to_ms: bars.window.to_ms,
            margin_ms: bars.window.margin_ms,
            long_position_ms: bars.window.long_position_ms,
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
    assert_eq!(series.covered, Coverage::one((60_000, 120_000)));
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
    snapshots: Vec<(usize, Vec<(i64, i64)>)>,
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
    fn progress(&mut self, ticks: &[Tick], covered: &Coverage) {
        self.snapshots.push((ticks.len(), covered.spans().to_vec()));
    }
}

/// A core archive that arrives later must stop a REST walk without busy polling or sleeping.
#[test]
fn late_core_upgrade_is_detected_between_pages() {
    let now = Instant::now();
    let probe = CoreUpgradeProbe::new(now);
    let none = Coverage::none();
    assert!(!probe.stop(false, now, &none, || panic!("early rescan")));
    assert!(
        probe.stop(false, now + Duration::from_millis(500), &none, || Some(
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
        trade_len: 0,
        focus_len: 1,
    };
    let mut observer = FakeObserver::default();
    let verdict = paginate_ticks(
        TradeRoute::BinanceUsdMAggTrades,
        &plan,
        100,
        10,
        || false,
        |_| false,
        &mut observer,
        |from, _, _| page(vec![tick(from + 5, 10.0)]),
    );
    assert!(matches!(verdict, TickVerdict::Ready(_)));
    assert_eq!(
        observer.snapshots,
        vec![(1, vec![(100, 199)]), (2, vec![(100, 299)])]
    );
}

/// A partly walked tile proves only the stretch its cursor direction actually walked: a backward
/// page to the right of the prefix is its own suffix, never the gap beside the prefix; a forward
/// one abuts the prefix and extends it; an undocumented order proves nothing beside a completed
/// stretch and only the rows' own extent with none.
#[test]
fn partial_page_progress_respects_pagination_direction() {
    let rows = [tick(250, 10.0), tick(299, 11.0)];
    let prefix = Coverage::one((100, 199));
    assert_eq!(
        walked_part(
            &prefix,
            (200, 299),
            &rows,
            Some(rest::TradeCursor::LessThanId(42))
        ),
        Some((250, 299))
    );
    let mut backward = prefix.clone();
    backward.add((250, 299));
    assert_eq!(backward.spans(), &[(100, 199), (250, 299)]);
    assert_eq!(
        walked_part(
            &prefix,
            (200, 299),
            &rows,
            Some(rest::TradeCursor::FromId(42))
        ),
        Some((200, 299))
    );
    let mut forward = prefix.clone();
    forward.add((200, 299));
    assert_eq!(forward.spans(), &[(100, 299)]);
    assert_eq!(
        walked_part(&prefix, (200, 299), &rows, Some(rest::TradeCursor::Page(2))),
        None
    );
    assert_eq!(
        walked_part(
            &Coverage::none(),
            (200, 299),
            &rows,
            Some(rest::TradeCursor::Page(2))
        ),
        Some((250, 299))
    );
    assert_eq!(walked_part(&prefix, (200, 299), &[], None), None);
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
        margin_ms: 5 * 60_000,
        long_position_ms: 5 * 60_000,
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
        trade_len: 0,
        focus_len: 2,
    };
    let mut observer = FakeObserver::default();

    let verdict = paginate_ticks(
        TradeRoute::BinanceUsdMAggTrades,
        &plan,
        40_000,
        10,
        || false,
        |_| false,
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
        Coverage::one((100, 299)),
        "only the two completed focus slices are covered after the budget stops the walk"
    );
    assert!(
        !harvest.complete,
        "skipping the non-focus slice is a partial, not a complete, harvest"
    );
    assert_eq!(
        observer.claims, 1,
        "a stage checks the host's refusal history once"
    );
}

/// `market/trade_replay/worker.rs:paginate_ticks` returning `Abandoned(Deadline)` after any
/// fetched page discards usable ticks and replaces the user's trade with candles.
#[test]
fn deadline_with_a_non_empty_harvest_is_ready_not_abandoned() {
    let plan = TickPlan {
        slices: vec![(100, 199), (0, 99)],
        trade_len: 0,
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
        |_| fetched.get(),
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
    assert_eq!(harvest.covered, Coverage::one((100, 199)));
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
        trade_len: 0,
        focus_len: 1,
    };
    let mut observer = FakeObserver::default();
    let verdict = paginate_ticks(
        TradeRoute::BinanceUsdMAggTrades,
        &plan,
        40_000,
        10,
        || false,
        |_| false,
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

/// A request on Binance spot — a route with no retention, so the window's epoch-near stamps are
/// inside it — over a one-minute trade at the middle of a one-hour window, whose focus (the
/// trade plus the five-minute margins) is `1_500_000..=2_160_000`.
fn tile_request(reply: Sender<TradeReplayOutcome>) -> TradeReplayRequest {
    let history =
        crate::market::source::MarketDataSource::new(crate::market::MarketStore::shared(0.0));
    let venue = crate::venue::venue(3).expect("Binance spot");
    TradeReplayRequest {
        address: crate::market::source::ReplayAddress {
            history,
            venue,
            exchange_key: "3:00000000".into(),
            cache: None,
        },
        market: "BTCUSDT".into(),
        window: ReplayWindow {
            from_ms: 0,
            to_ms: 3_600_000,
            open_ms: 1_800_000,
            close_ms: 1_860_000,
            margin_ms: 5 * 60_000,
            long_position_ms: 5 * 60_000,
            over_budget: false,
        },
        identity: 7,
        tick_value: super::super::venue_caps::TickValue::Base,
        ticks: true,
        intent: ReplayIntent::Chart,
        cancel: Arc::new(AtomicBool::new(false)),
        reply,
    }
}

fn tile_stage(request: &TradeReplayRequest) -> TickStage {
    let route = trade_route(request.address.venue).expect("Binance spot trades route");
    TickStage {
        baseline: None,
        route: Some(route),
        key: OutcomeKey {
            venue: request.address.venue,
            host: route.host(),
            market: request.market.clone(),
            from_ms: request.window.from_ms,
            to_ms: request.window.to_ms,
            margin_ms: request.window.margin_ms,
            long_position_ms: request.window.long_position_ms,
        },
        candles: Vec::new(),
    }
}

/// A focus the tile store already holds whole is served from it — the real `serve_ticks`, with
/// no page fetched: the residual is empty, so the paginator is never entered.
#[test]
fn a_focus_held_by_the_tiles_is_served_without_a_walk() {
    let (reply, _rx) = mpsc::channel();
    let request = tile_request(reply);
    let stage = tile_stage(&request);
    let key = (request.address.exchange_key.clone(), request.market.clone());
    let tiles = Mutex::new(TickTileStore::default());
    // A wider neighbouring window's harvest: covers the focus and more.
    tiles.lock().unwrap().insert(
        key,
        1_400_000,
        2_200_000,
        vec![
            tick(1_450_000, 9.0),
            tick(1_700_000, 10.0),
            tick(1_830_000, 11.0),
            tick(2_190_000, 12.0),
        ],
        TileSource::Venue,
    );
    let (series, retry) = serve_ticks(&rest::agent(), &ReplayGate::new(), &request, &stage, &tiles)
        .expect("served from the tiles");
    assert!(!retry);
    assert_eq!(series.source, TradeReplaySource::Ticks);
    assert_eq!(series.tick_status, TickStatus::Served);
    assert_eq!(
        series.covered,
        Coverage::one((1_500_000, 2_160_000)),
        "clipped to the focus"
    );
    assert_eq!(
        series
            .ticks
            .iter()
            .map(|t| t.time_ms as i64)
            .collect::<Vec<_>>(),
        vec![1_700_000, 1_830_000]
    );
    assert_eq!(
        series.side_slots.len(),
        2,
        "the band is summed from the served prints"
    );
}

/// A venue with no public trade route is served from a captured core tile — and prints
/// `NoRoute`, as before, when the tiles hold nothing for the focus.
#[test]
fn a_route_less_stage_serves_captured_tiles_or_prints_no_route() {
    let (reply, _rx) = mpsc::channel();
    let request = tile_request(reply);
    let mut stage = tile_stage(&request);
    stage.route = None;
    let key = (request.address.exchange_key.clone(), request.market.clone());
    let tiles = Mutex::new(TickTileStore::default());
    let outcome = serve_ticks(&rest::agent(), &ReplayGate::new(), &request, &stage, &tiles);
    assert!(
        matches!(outcome, Err(Some(TickStatus::NoRoute))),
        "{outcome:?}"
    );
    tiles.lock().unwrap().insert(
        key,
        1_700_000,
        1_900_000,
        vec![tick(1_750_000, 9.0), tick(1_850_000, 10.0)],
        TileSource::Core,
    );
    let (series, retry) = serve_ticks(&rest::agent(), &ReplayGate::new(), &request, &stage, &tiles)
        .expect("served from the captured tile");
    assert!(
        retry,
        "a later capture may widen the tiles, so a reopen re-decides"
    );
    assert_eq!(series.covered, Coverage::one((1_700_000, 1_900_000)));
    assert_eq!(series.ticks.len(), 2);
    assert!(series.partial);
}

/// A window older than its route's retention — every Binance futures trade closed more than
/// 48 h ago — is still served from what the tiles hold for its focus: the venue is not asked,
/// the store is. `OutOfRetention` is what it prints only when the tiles hold nothing. The stage
/// used to answer the retention before the tiles were read, and a trade whose tape the tuner had
/// fetched showed candles alone on its chart (2026-09-23).
#[test]
fn a_window_past_the_routes_retention_is_served_from_the_tiles() {
    let (reply, _rx) = mpsc::channel();
    let mut request = tile_request(reply);
    request.address.venue = crate::venue::venue(4).expect("Binance futures");
    let stage = tile_stage(&request);
    assert_eq!(stage.route, Some(TradeRoute::BinanceUsdMAggTrades));
    let key = (request.address.exchange_key.clone(), request.market.clone());
    let tiles = Mutex::new(TickTileStore::default());
    let outcome = serve_ticks(&rest::agent(), &ReplayGate::new(), &request, &stage, &tiles);
    assert!(
        matches!(outcome, Err(Some(TickStatus::OutOfRetention { .. }))),
        "{outcome:?}"
    );
    tiles.lock().unwrap().insert(
        key,
        1_400_000,
        2_200_000,
        vec![tick(1_700_000, 10.0), tick(1_830_000, 11.0)],
        TileSource::Venue,
    );
    let (series, _) = serve_ticks(&rest::agent(), &ReplayGate::new(), &request, &stage, &tiles)
        .expect("served from the tiles past the retention");
    assert_eq!(series.covered, Coverage::one((1_500_000, 2_160_000)));
    assert_eq!(series.ticks.len(), 2);
}

/// A window older than its route's retention still earns its tick stage: the stage is what
/// reads the tiles (see the test above), and only its venue walk is bounded by the retention.
#[test]
fn a_window_past_the_routes_retention_still_queues_its_tick_stage() {
    let (reply, _rx) = mpsc::channel();
    let mut request = tile_request(reply);
    request.address.venue = crate::venue::venue(4).expect("Binance futures");
    let key = tile_stage(&request).key;
    let stage = tick_stage_for(request.address.venue, &key, &[]);
    assert_eq!(stage.route, Some(TradeRoute::BinanceUsdMAggTrades));
}

/// An empty run the tiles cover whole is the authoritative "no trades", not a retryable failure.
#[test]
fn an_empty_covered_focus_is_no_trades() {
    let (reply, _rx) = mpsc::channel();
    let request = tile_request(reply);
    let stage = tile_stage(&request);
    let key = (request.address.exchange_key.clone(), request.market.clone());
    let tiles = Mutex::new(TickTileStore::default());
    tiles
        .lock()
        .unwrap()
        .insert(key, 1_400_000, 2_200_000, Vec::new(), TileSource::Venue);
    let outcome = serve_ticks(&rest::agent(), &ReplayGate::new(), &request, &stage, &tiles);
    assert!(
        matches!(outcome, Err(Some(TickStatus::NoTrades))),
        "{outcome:?}"
    );
}

/// A close-time capture copies what the trade's window would ask for as ticks: the whole focus
/// on a short position, only the two neighbourhoods on a long one, each the margin on both sides
/// of its end — and before the settle pass, nothing past the exit, since the trail has not
/// printed yet.
#[test]
fn capture_spans_follow_the_windows_focus_and_stop_at_the_exit_before_settling() {
    let history =
        crate::market::source::MarketDataSource::new(crate::market::MarketStore::shared(0.0));
    let venue = crate::venue::venue(3).expect("Binance spot");
    let margin = 20 * 60_000;
    let capture = |open_ms: i64, close_ms: i64| CaptureRequest {
        address: crate::market::source::ReplayAddress {
            history: history.clone(),
            venue,
            exchange_key: "3:00000000".into(),
            cache: None,
        },
        market: "BTCUSDT".into(),
        open_ms,
        close_ms,
        margin_ms: margin,
        long_position_ms: 5 * 60_000,
    };
    let short = capture(100_000_000, 100_120_000);
    assert_eq!(
        capture_spans(&short, false).spans(),
        &[(100_000_000 - margin, 100_120_000)]
    );
    assert_eq!(
        capture_spans(&short, true).spans(),
        &[(100_000_000 - margin, 100_120_000 + margin)]
    );
    let close_ms = 100_000_000 + 8 * 60 * 60_000;
    let long = capture(100_000_000, close_ms);
    assert_eq!(
        capture_spans(&long, false).spans(),
        &[
            (100_000_000 - margin, 100_000_000 + margin),
            (close_ms - margin, close_ms),
        ]
    );
    assert_eq!(
        capture_spans(&long, true).spans(),
        &[
            (100_000_000 - margin, 100_000_000 + margin),
            (close_ms - margin, close_ms + margin),
        ]
    );
    // Zero is the position alone, on both passes — and nothing to settle: no trail, no second
    // pass, or the first pass would schedule itself again forever.
    let mut bare = capture(100_000_000, 100_120_000);
    bare.margin_ms = 0;
    assert_eq!(
        capture_spans(&bare, true).spans(),
        &[(100_000_000, 100_120_000)]
    );
    assert!(settle_plan(&bare).is_none());
    // With a margin, the settle pass is due when the trail has printed: the whole margin past
    // the exit, on a short position and a long one alike.
    let slack = CAPTURE_SETTLE_SLACK.as_millis() as i64;
    let (spans, due_ms) = settle_plan(&short).expect("a trail to settle");
    assert_eq!(spans, capture_spans(&short, true));
    assert_eq!(due_ms, 100_120_000 + margin + slack);
    let (_, due_ms) = settle_plan(&long).expect("a trail to settle");
    assert_eq!(due_ms, close_ms + margin + slack);
}

/// `worker.rs:file_core_into_tiles` answering a tiles reader from the ring instead of filing it
/// leaves the model's held query empty and the walk's residual whole: the ring's stretch is
/// paid to the venue again, or — when the ring answered in place of the walk — never fetched
/// at all, and the row stays missing on every fetch (19 of 925 rows on 2026-09-21).
#[test]
fn a_tiles_reader_gets_the_ring_as_core_tiles_the_walk_no_longer_asks_for() {
    let history =
        crate::market::source::MarketDataSource::new(crate::market::MarketStore::shared(0.0));
    let venue = crate::venue::venue(3).expect("Binance spot");
    let address = ReplayAddress {
        history,
        venue,
        exchange_key: "3:00000000".into(),
        cache: None,
    };
    let key: TileKey = (address.exchange_key.clone(), "BTCUSDT".into());
    let (open_ms, close_ms) = (100_000_000, 100_120_000);
    let window = super::super::replay_window_ms(open_ms, close_ms, 60_000).expect("window");
    let focus = window.focus_spans();
    let (focus_from, focus_to) = focus.hull().expect("one focus");
    let tiles = Mutex::new(TickTileStore::default());
    let asked = std::cell::RefCell::new(Vec::new());
    // The ring holds the position and a little after it, not the lead before the entry.
    let ring = (open_ms - 5_000, close_ms + 30_000);
    let filed = file_core_into_tiles(&address, "BTCUSDT", &focus, &tiles, |span| {
        asked.borrow_mut().push(span);
        let from = span.0.max(ring.0);
        let to = span.1.min(ring.1);
        (from <= to).then(|| crate::market::source::CoreReplayTicks {
            ticks: vec![tick(from, 1.0), tick(to, 2.0)],
            covered: (from, to),
        })
    });
    assert_eq!(
        asked.borrow().as_slice(),
        focus.spans(),
        "one read per focus span"
    );
    assert_eq!(filed, vec![(ring.0.max(focus_from), ring.1.min(focus_to))]);

    // What the walk still owes the venue is exactly what the ring did not hold.
    let route = TradeRoute::BinanceSpotAggTrades;
    let plan = tick_plan(window, route, None, ReplayIntent::Model);
    let residual = residual_plan(&plan, &lock_tiles(&tiles), &key);
    let residual_ms: i64 = residual.slices.iter().map(|(a, b)| b - a + 1).sum();
    let plan_ms: i64 = plan.slices.iter().map(|(a, b)| b - a + 1).sum();
    assert!(
        residual_ms < plan_ms,
        "the ring's stretch left the residual"
    );
    assert!(
        residual
            .slices
            .iter()
            .all(|&(a, b)| b < ring.0 || a > ring.1),
        "nothing inside the filed stretch is asked again: {:?}",
        residual.slices
    );
    // And the tiles say who answered.
    let store = lock_tiles(&tiles);
    let held = held_coverage(&store, &key, &focus, Coverage::none());
    assert!(held.contains((ring.0.max(focus_from), ring.1.min(focus_to))));
    drop(store);

    // A chart keeps the ring as an answer; a model never takes it in place of the walk.
    assert!(!ReplayIntent::Chart.files_core());
    assert!(ReplayIntent::Model.files_core());
}

/// The real pager over the real venue, by hand: `MOON_TICKS_PROBE=GateFuturesTrades,GSTOCKBSC_USDT,<from_ms>,<to_ms>`
/// walks that one slice and prints what came back — rows, distinct prints, the largest holes —
/// so a hole or a duplicate in the store can be told apart from one the pager makes today.
#[test]
#[ignore = "asks the venue over the network; run by hand"]
fn probe_one_slice_against_the_venue() {
    let Ok(spec) = std::env::var("MOON_TICKS_PROBE") else {
        eprintln!("MOON_TICKS_PROBE is not set; nothing to do");
        return;
    };
    let parts: Vec<&str> = spec.split(',').collect();
    let [route, market, from_ms, to_ms] = parts[..] else {
        panic!("MOON_TICKS_PROBE=<route>,<market>,<from_ms>,<to_ms>");
    };
    let route = match route {
        "GateFuturesTrades" => TradeRoute::GateFuturesTrades,
        "GateSpotTrades" => TradeRoute::GateSpotTrades,
        "OkxHistoryTrades" => TradeRoute::OkxHistoryTrades,
        "BinanceUsdMAggTrades" => TradeRoute::BinanceUsdMAggTrades,
        other => panic!("unknown route {other}"),
    };
    let (from_ms, to_ms): (i64, i64) = (from_ms.parse().unwrap(), to_ms.parse().unwrap());
    let plan = TickPlan {
        slices: vec![(from_ms, to_ms)],
        trade_len: 1,
        focus_len: 1,
    };
    let agent = rest::agent();
    let mut observer = FakeObserver::default();
    let verdict = paginate_ticks(
        route,
        &plan,
        TICK_BUDGET,
        TICK_PAGE_BUDGET,
        || false,
        |_| false,
        &mut observer,
        |from, to, cursor| {
            let page = rest::fetch_trades(&agent, route, market, from, to, cursor);
            if let Ok(page) = &page {
                let (lo, hi) = page.ticks.iter().fold((i64::MAX, i64::MIN), |(lo, hi), t| {
                    (lo.min(t.time_ms as i64), hi.max(t.time_ms as i64))
                });
                eprintln!(
                    "PROBE page cursor={cursor:?} rows={} t=+{}..+{} ms next={:?}",
                    page.ticks.len(),
                    lo.saturating_sub(from_ms),
                    hi.saturating_sub(from_ms),
                    page.next
                );
            }
            page
        },
    );
    let TickVerdict::Ready(harvest) = verdict else {
        panic!("abandoned: {verdict:?}");
    };
    let mut keys: Vec<(i64, u32, u32, u8)> = harvest
        .ticks
        .iter()
        .map(|t| {
            (
                t.time_ms as i64,
                t.price.to_bits(),
                t.qty.to_bits(),
                t.side as u8,
            )
        })
        .collect();
    let rows = keys.len();
    keys.sort_unstable();
    keys.dedup();
    let mut times: Vec<i64> = keys.iter().map(|k| k.0).collect();
    times.dedup();
    let mut gaps: Vec<(i64, i64)> = times
        .windows(2)
        .map(|w| (w[1] - w[0], w[0] - from_ms))
        .collect();
    gaps.sort_unstable_by(|a, b| b.cmp(a));
    eprintln!(
        "PROBE {market}: pages={} rows={rows} distinct={} covered={} complete={} stop={:?}\nPROBE largest gaps (ms, at +ms): {:?}",
        observer.paces,
        keys.len(),
        harvest.covered,
        harvest.complete,
        harvest.stop,
        &gaps[..gaps.len().min(5)]
    );
}

/// `worker.rs:file_core_into_tiles` answering a tiles reader from the ring instead of filing it
/// leaves the model's held query empty and the walk's residual whole: the ring's stretch is
/// paid to the venue again, or — when the ring answered in place of the walk — never fetched
/// at all, and the row stays missing on every fetch (19 of 925 rows on 2026-09-21).
#[test]
fn a_tiles_reader_gets_the_ring_as_core_tiles_the_walk_no_longer_asks_for() {
    let history =
        crate::market::source::MarketDataSource::new(crate::market::MarketStore::shared(0.0));
    let venue = crate::venue::venue(3).expect("Binance spot");
    let address = ReplayAddress {
        history,
        venue,
        exchange_key: "3:00000000".into(),
        cache: None,
    };
    let key: TileKey = (address.exchange_key.clone(), "BTCUSDT".into());
    let (open_ms, close_ms) = (100_000_000, 100_120_000);
    let window = super::super::replay_window_ms(open_ms, close_ms, 60_000).expect("window");
    let focus = window.focus_spans();
    let (focus_from, focus_to) = focus.hull().expect("one focus");
    let tiles = Mutex::new(TickTileStore::default());
    let asked = std::cell::RefCell::new(Vec::new());
    // The ring holds the position and a little after it, not the lead before the entry.
    let ring = (open_ms - 5_000, close_ms + 30_000);
    let filed = file_core_into_tiles(&address, "BTCUSDT", &focus, &tiles, |span| {
        asked.borrow_mut().push(span);
        let from = span.0.max(ring.0);
        let to = span.1.min(ring.1);
        (from <= to).then(|| crate::market::source::CoreReplayTicks {
            ticks: vec![tick(from, 1.0), tick(to, 2.0)],
            covered: (from, to),
        })
    });
    assert_eq!(
        asked.borrow().as_slice(),
        focus.spans(),
        "one read per focus span"
    );
    assert_eq!(filed, vec![(ring.0.max(focus_from), ring.1.min(focus_to))]);

    // What the walk still owes the venue is exactly what the ring did not hold.
    let route = TradeRoute::BinanceSpotAggTrades;
    let plan = tick_plan(window, route, None, ReplayIntent::Model);
    let residual = residual_plan(&plan, &lock_tiles(&tiles), &key);
    let residual_ms: i64 = residual.slices.iter().map(|(a, b)| b - a + 1).sum();
    let plan_ms: i64 = plan.slices.iter().map(|(a, b)| b - a + 1).sum();
    assert!(
        residual_ms < plan_ms,
        "the ring's stretch left the residual"
    );
    assert!(
        residual
            .slices
            .iter()
            .all(|&(a, b)| b < ring.0 || a > ring.1),
        "nothing inside the filed stretch is asked again: {:?}",
        residual.slices
    );
    // And the tiles say who answered.
    let store = lock_tiles(&tiles);
    let held = held_coverage(&store, &key, &focus, Coverage::none());
    assert!(held.contains((ring.0.max(focus_from), ring.1.min(focus_to))));
    drop(store);

    // A chart keeps the ring as an answer; a model never takes it in place of the walk.
    assert!(!ReplayIntent::Chart.files_core());
    assert!(ReplayIntent::Model.files_core());
}
