use super::*;

/// The margin every test window is built with — the five minutes the module shipped with, so
/// the numbers in the assertions below stay what they were.
const MARGIN_MS: i64 = 5 * MINUTE_MS;

/// Tracks fake-server intervals to verify that published snapshots never cover unfetched time.
#[derive(Default)]
struct CoverageObserver {
    fetched: std::rc::Rc<std::cell::RefCell<Vec<(i64, i64)>>>,
    snapshots: Vec<Coverage>,
    paces: usize,
}

impl worker::TickObserver for CoverageObserver {
    /// A fake permit never contacts a venue.
    fn claim(&mut self, _: &str) -> Result<(), u32> {
        Ok(())
    }

    /// Count sends to ensure extended budgets do not bypass pacing.
    fn pace(&mut self, _: &str) {
        self.paces += 1;
    }

    /// A snapshot must be exhaustive and may only extend its predecessor.
    fn progress(&mut self, ticks: &[Tick], covered: &Coverage) {
        assert_fetched(&self.fetched.borrow(), covered);
        if let Some(last) = self.snapshots.last() {
            assert!(covered.covers(last), "progress cannot shrink");
        }
        assert!(ticks.iter().any(|t| covered.contains_ms(t.time_ms as i64)));
        self.snapshots.push(covered.clone());
    }
}

/// Every stretch of `covered` must be paid for; see [`assert_fetched_span`].
fn assert_fetched(fetched: &[(i64, i64)], covered: &Coverage) {
    assert!(!covered.is_empty(), "a snapshot covers something");
    for &span in covered.spans() {
        assert_fetched_span(fetched, span);
    }
}

/// Merge the fake server's paid-for intervals independently of production coverage helpers.
fn assert_fetched_span(fetched: &[(i64, i64)], covered: (i64, i64)) {
    let mut intervals = fetched.to_vec();
    intervals.sort_unstable();
    let mut next = covered.0;
    for (from, to) in intervals {
        if to < next {
            continue;
        }
        assert!(from <= next, "unfetched gap at {next} in {covered:?}");
        if to >= covered.1 {
            return;
        }
        next = to + 1;
    }
    panic!("coverage {covered:?} extends beyond fetched intervals");
}

/// Emit dense pages in either direction with overshoots that must be clipped by the paginator.
fn dense_page(
    route: venue_caps::TradeRoute,
    from: i64,
    to: i64,
    cursor: Option<rest::TradeCursor>,
    width: i64,
    fetched: &std::cell::RefCell<Vec<(i64, i64)>>,
) -> Result<rest::TradePage, rest::FetchError> {
    let backward = matches!(
        route,
        venue_caps::TradeRoute::OkxHistoryTrades | venue_caps::TradeRoute::BitgetMixFills
    );
    let (lo, hi) = if backward {
        let hi = match cursor {
            Some(rest::TradeCursor::LessThanId(id)) => id as i64 - 1,
            None => to,
            _ => panic!("wrong backward cursor"),
        };
        ((hi - width + 1).max(from), hi)
    } else {
        let lo = match cursor {
            Some(rest::TradeCursor::FromId(id)) => id as i64,
            None => from,
            _ => panic!("wrong forward cursor"),
        };
        (lo, (lo + width - 1).min(to))
    };
    fetched.borrow_mut().push((lo, hi));
    let next = if backward {
        (lo > from).then_some(rest::TradeCursor::LessThanId(lo as u64))
    } else {
        (hi < to).then_some(rest::TradeCursor::FromId((hi + 1) as u64))
    };
    Ok(rest::TradePage {
        ticks: [lo, hi, from - 1, to + 1]
            .map(|time_ms| Tick {
                time_ms: time_ms as f64,
                price: 10.0,
                qty: 1.0,
                side: crate::feed::Side::Buy,
            })
            .to_vec(),
        next,
    })
}

/// Restoring chronological focus tiles or normal limits inside trade tiles loses entry/exit
/// on a dense venue. Both paging directions must finish the trade before spending on margins.
#[test]
fn trade_first_paging_protects_position_from_soft_page_and_deadline_stops() {
    use venue_caps::TradeRoute::*;
    for route in [OkxHistoryTrades, BitgetMixFills, BinanceUsdMAggTrades] {
        for soft_deadline in [false, true] {
            // Up to the longest position still walked as one stretch: past
            // `long_position_ms()` the plan tiles the two ends only, which the plan's own test
            // covers. With the threshold under `TICK_SLICE_MS` no short position spans two
            // trade tiles any more, so the multi-tile trade prefix this loop once ran (21 min,
            // three tiles, under the hour-long threshold) is unreachable here by construction.
            for duration in [0, 120_000, long_position_ms() - MINUTE_MS] {
                let window =
                    replay_window_ms(100_000_000, 100_000_000 + duration, MARGIN_MS).unwrap();
                let plan = tick_plan(window, route, None, ReplayIntent::Chart);
                let mut observer = CoverageObserver::default();
                let fetched = observer.fetched.clone();
                let calls = std::cell::Cell::new(0);
                let verdict = worker::paginate_ticks(
                    route,
                    &plan,
                    1,
                    if soft_deadline { 100 } else { 1 },
                    || false,
                    |trade| soft_deadline && calls.get() >= 1 && !trade,
                    &mut observer,
                    |from, to, cursor| {
                        assert!(
                            from >= window.open_ms && to <= window.close_ms,
                            "context before protected trade"
                        );
                        if calls.get() == 0 {
                            match route {
                                OkxHistoryTrades | BitgetMixFills => {
                                    assert_eq!(to, window.close_ms)
                                }
                                _ => assert_eq!(from, window.open_ms),
                            }
                        }
                        calls.set(calls.get() + 1);
                        dense_page(route, from, to, cursor, 30_000, &fetched)
                    },
                );
                let worker::TickVerdict::Ready(harvest) = verdict else {
                    panic!("trade must be served")
                };
                assert_eq!(
                    harvest.covered,
                    Coverage::one((window.open_ms, window.close_ms))
                );
                assert!(!harvest.complete, "optional margins were not fetched");
                assert_fetched(&fetched.borrow(), &harvest.covered);
                assert!(
                    harvest
                        .ticks
                        .iter()
                        .all(|t| t.time_ms >= window.open_ms as f64
                            && t.time_ms <= window.close_ms as f64),
                    "overshoot leaked into harvest"
                );
                assert_eq!(observer.paces, calls.get());
                assert_eq!(observer.snapshots.last(), Some(&harvest.covered));
            }
        }
    }
}

/// Removing hard bounds makes a busy trade occupy the only replay worker indefinitely;
/// cancellation and venue refusal must retain precedence over trade protection.
#[test]
fn trade_first_paging_still_obeys_hard_stops_cancellation_and_venue_errors() {
    use venue_caps::TradeRoute::*;
    for route in [OkxHistoryTrades, BinanceUsdMAggTrades] {
        for stop in ["pages", "deadline", "cancel", "venue"] {
            let window = replay_window_ms(100_000_000, 100_120_000, MARGIN_MS).unwrap();
            let plan = tick_plan(window, route, None, ReplayIntent::Chart);
            let mut observer = CoverageObserver::default();
            let fetched = observer.fetched.clone();
            let calls = std::cell::Cell::new(0);
            let verdict = worker::paginate_ticks(
                route,
                &plan,
                1,
                1,
                || stop == "cancel" && calls.get() == 2,
                |_| stop == "deadline" && calls.get() == 2,
                &mut observer,
                |from, to, cursor| {
                    if stop == "venue" && calls.get() == 2 {
                        return Err(rest::FetchError::Transient("fixture refusal".into()));
                    }
                    calls.set(calls.get() + 1);
                    dense_page(route, from, to, cursor, 1, &fetched)
                },
            );
            if stop == "cancel" {
                assert!(matches!(
                    verdict,
                    worker::TickVerdict::Abandoned(worker::TickAbandon::Cancelled)
                ));
                continue;
            }
            let worker::TickVerdict::Ready(harvest) = verdict else {
                panic!("paid-for pages must survive")
            };
            assert_eq!(calls.get(), if stop == "pages" { 240 } else { 2 });
            assert!(!harvest.complete);
            assert_eq!(harvest.venue_refused, stop == "venue");
            assert_fetched(&fetched.borrow(), &harvest.covered);
            assert!(!harvest.covered.contains((window.open_ms, window.close_ms)));
        }
    }
}

/// Reordering completed tiles across a gap or joining far-side partial pages hides candles
/// over unfetched time; verify every prefix against independent fake-server coverage.
#[test]
fn trade_first_paging_keeps_progress_contiguous_across_both_margins_and_retention() {
    use venue_caps::TradeRoute::*;
    for route in [OkxHistoryTrades, BitgetMixFills, BinanceUsdMAggTrades] {
        let window = replay_window_ms(100_000_000, 100_120_000, MARGIN_MS).unwrap();
        for earliest in [
            None,
            Some(99_850_000),
            Some(100_060_000),
            Some(100_180_000),
            Some(100_420_001),
        ] {
            let plan = tick_plan(window, route, earliest, ReplayIntent::Chart);
            let mut observer = CoverageObserver::default();
            let fetched = observer.fetched.clone();
            let verdict = worker::paginate_ticks(
                route,
                &plan,
                1,
                100,
                || false,
                |_| false,
                &mut observer,
                |from, to, cursor| dense_page(route, from, to, cursor, 30_000, &fetched),
            );
            if earliest == Some(100_420_001) {
                assert!(matches!(
                    verdict,
                    worker::TickVerdict::Abandoned(worker::TickAbandon::Empty)
                ));
                assert!(fetched.borrow().is_empty());
                continue;
            }
            let worker::TickVerdict::Ready(harvest) = verdict else {
                panic!("retained focus must finish")
            };
            assert!(harvest.complete);
            assert_eq!(
                harvest.covered,
                Coverage::one((earliest.unwrap_or(99_700_000).max(99_700_000), 100_420_000))
            );
            assert_fetched(&fetched.borrow(), &harvest.covered);
            assert_eq!(observer.snapshots.last(), Some(&harvest.covered));
        }
    }
}

/// Unioning an interrupted far-side margin unconditionally fabricates a gap in the final
/// harvest, even if progressive publications were correct. Stop inside each margin to catch it.
#[test]
fn trade_first_paging_partial_margins_never_expand_past_fetched_coverage() {
    use venue_caps::TradeRoute::*;
    for route in [OkxHistoryTrades, BitgetMixFills, BinanceUsdMAggTrades] {
        for budget in [6, 16] {
            let window = replay_window_ms(100_000_000, 100_120_000, MARGIN_MS).unwrap();
            let plan = tick_plan(window, route, None, ReplayIntent::Chart);
            let mut observer = CoverageObserver::default();
            let fetched = observer.fetched.clone();
            let verdict = worker::paginate_ticks(
                route,
                &plan,
                1,
                budget,
                || false,
                |_| false,
                &mut observer,
                |from, to, cursor| dense_page(route, from, to, cursor, 30_000, &fetched),
            );
            let worker::TickVerdict::Ready(harvest) = verdict else {
                panic!("trade must remain visible")
            };
            assert!(!harvest.complete);
            assert_eq!(observer.paces, budget);
            assert_fetched(&fetched.borrow(), &harvest.covered);
            assert!(harvest.covered.contains((window.open_ms, window.close_ms)));
            assert_eq!(observer.snapshots.last(), Some(&harvest.covered));
        }
    }
}

/// Wide candle context must never expand either native or REST tick requests past five minutes.
#[test]
fn detailed_tick_window_excludes_wide_candle_context() {
    let window = replay_window_ms(100_000_000, 100_060_000, MARGIN_MS).expect("one-minute trade");
    let narrow = window.tick_window();
    assert_eq!((narrow.from_ms, narrow.to_ms), (99_700_000, 100_360_000));
    let plan = tick_plan(
        window,
        venue_caps::TradeRoute::BinanceUsdMAggTrades,
        None,
        ReplayIntent::Chart,
    );
    assert_eq!(plan.slices.iter().map(|s| s.0).min(), Some(99_700_000));
    assert_eq!(plan.slices.iter().map(|s| s.1).max(), Some(100_360_000));
    assert!(
        plan.slices
            .iter()
            .all(|s| s.0 >= narrow.from_ms && s.1 <= narrow.to_ms)
    );
    assert_eq!(
        plan.focus_len,
        plan.slices.len(),
        "no distant context tiles may be requested"
    );
}

/// Progressive coverage changes must upload candles again even though the request ID is stable.
#[test]
fn growing_tick_coverage_invalidates_candle_upload() {
    let mut series = bars_only_series();
    series.source = TradeReplaySource::Ticks;
    series.covered = Coverage::one((0, MINUTE_MS - 1));
    let mut out = ChartHistoryBuffers::default();
    let first = series.read_into(
        0.0,
        0.0,
        (3 * MINUTE_MS) as f32,
        Some(&candle_params(0)),
        &mut out,
    );
    series.covered = Coverage::one((0, 2 * MINUTE_MS - 1));
    let next = series.read_into(
        0.0,
        0.0,
        (3 * MINUTE_MS) as f32,
        Some(&candle_params(first.candles_revision)),
        &mut out,
    );
    assert!(
        next.candles_changed,
        "new tick chunks must remove newly covered candles"
    );
    assert_ne!(next.candles_revision, first.candles_revision);
}

const MINUTE_MS: i64 = 60_000;

/// Wide prefetch still uploads all rows, but off-screen extrema must not affect the visible Y fit.
#[test]
fn replay_price_window_is_independent_of_prefetch_for_ticks_and_cached_candles() {
    let mut series = bars_only_series();
    let mut out = ChartHistoryBuffers::default();
    let params = candle_params(0);
    let first = series.read_with_price_window(
        0.0,
        0.0,
        180_000.0,
        Some((60_000.0, 119_999.0)),
        Some(&params),
        &mut out,
    );
    assert_eq!(out.candles.len(), 3);
    assert_eq!(first.tick_price_range, Some((92.0, 110.0)));
    let repeat = series.read_with_price_window(
        0.0,
        0.0,
        180_000.0,
        Some((60_000.0, 119_999.0)),
        Some(&candle_params(first.candles_revision)),
        &mut out,
    );
    assert!(out.candles.is_empty());
    assert_eq!(repeat.tick_price_range, Some((92.0, 110.0)));
    let partial = series.read_with_price_window(
        0.0,
        0.0,
        180_000.0,
        Some((90_000.0, 119_999.0)),
        Some(&candle_params(first.candles_revision)),
        &mut out,
    );
    assert_eq!(partial.tick_price_range, Some((92.0, 110.0)));
    let mut coarse_params = candle_params(0);
    coarse_params.tf_ms = 180_000;
    let coarse = series.read_with_price_window(
        0.0,
        0.0,
        180_000.0,
        Some((90_000.0, 119_999.0)),
        Some(&coarse_params),
        &mut out,
    );
    assert_eq!(coarse.tick_price_range, Some((90.0, 110.0)));
    series.ticks = [
        (10_000.0, 1.0),
        (70_000.0, 100.0),
        (80_000.0, 102.0),
        (150_000.0, 1000.0),
    ]
    .map(|(time_ms, price)| Tick {
        time_ms,
        price,
        qty: 1.0,
        side: crate::feed::Side::Buy,
    })
    .to_vec();
    let ticks = series.read_with_price_window(
        0.0,
        0.0,
        180_000.0,
        Some((60_000.0, 119_999.0)),
        None,
        &mut out,
    );
    assert_eq!(out.ticks.len(), 4);
    assert_eq!(ticks.tick_price_range, Some((100.0, 102.0)));
}

fn candle(t_open_ms: i64, low: f32, high: f32, close: f32) -> ChartCandle {
    ChartCandle {
        t_open_ms: t_open_ms as f64,
        open: close,
        high,
        low,
        close,
        volume: 1.0,
        quote_volume: 0.0,
    }
}

fn bars_only_series() -> TradeReplaySeries {
    TradeReplaySeries {
        source: TradeReplaySource::Klines1m,
        venue: crate::venue::venue(2).expect("known test venue"),
        window: ReplayWindow {
            from_ms: 0,
            to_ms: 2 * MINUTE_MS,
            open_ms: 0,
            close_ms: 2 * MINUTE_MS,
            margin_ms: MARGIN_MS,
            long_position_ms: 5 * 60_000,
            over_budget: false,
        },
        tf_ms: MINUTE_MS,
        candles: vec![
            candle(0, 90.0, 101.0, 100.0),
            candle(MINUTE_MS, 92.0, 110.0, 108.0),
            candle(2 * MINUTE_MS, 95.0, 105.0, 104.0),
        ],
        ticks: Vec::new(),
        identity: 42,
        tick_status: TickStatus::Pending,
        side_slots: Vec::new(),
        bucket_ms: 0,
        partial: false,
        covered: Coverage::none(),
    }
}

fn candle_params(shipped_revision: u64) -> CandleReadParams {
    CandleReadParams {
        tf_ms: MINUTE_MS,
        trades_from_rel_ms: 0.0,
        trades_limit: 100,
        shipped_revision,
    }
}

/// `market/trade_replay/mod.rs:TradeReplaySeries::read_into` must clear each buffer, reset the
/// combo, retain a candle-derived Y range on a repeat read, and ship fresh bars; dropping any of
/// those branches duplicates or hides the dedicated trade chart.
#[test]
fn replay_read_protocol_keeps_bars_visible_without_stale_rows() {
    let series = bars_only_series();
    let mut out = ChartHistoryBuffers::default();
    let first = series.read_into(
        0.0,
        0.0,
        (2 * MINUTE_MS) as f32,
        Some(&candle_params(0)),
        &mut out,
    );

    assert!(
        first.combo_reset,
        "a frozen series must reset its complete answer"
    );
    assert!(
        first.combo_capacity >= 1,
        "the renderer needs a non-zero point-ring capacity"
    );
    assert!(
        first.candles_changed,
        "a fresh pane with revision zero must receive its bars"
    );
    assert_eq!(
        out.candles.len(),
        3,
        "the three supplied one-minute bars must reach a fresh pane"
    );
    assert_eq!(
        first.tick_price_range,
        Some((90.0, 110.0)),
        "the Y range is the independent low/high envelope of the supplied bars"
    );

    let repeat = series.read_into(
        0.0,
        0.0,
        (2 * MINUTE_MS) as f32,
        Some(&candle_params(first.candles_revision)),
        &mut out,
    );

    assert!(
        repeat.combo_reset,
        "a repeat frozen read must still reset the caller coverage"
    );
    assert!(
        !repeat.candles_changed && out.candles.is_empty(),
        "an already-shipped revision emits no stale bars after clearing the destination"
    );
    assert_eq!(
        repeat.tick_price_range,
        Some((90.0, 110.0)),
        "bars suppressed by revision matching still define the visible Y range"
    );

    for (epoch_ms, from_rel_ms, to_rel_ms) in [
        (f64::NAN, 0.0, 1.0),
        (0.0, f32::NAN, 1.0),
        (0.0, 0.0, f32::INFINITY),
    ] {
        let invalid = series.read_into(
            epoch_ms,
            from_rel_ms,
            to_rel_ms,
            Some(&candle_params(0)),
            &mut out,
        );
        assert!(
            out.ticks.is_empty() && out.candles.is_empty() && invalid.tick_price_range.is_none(),
            "non-finite chart bounds must produce an empty answer instead of a saturated window"
        );
    }
}

/// `market/trade_replay/mod.rs:TradeReplaySeries::read_into` must derive a candle Y range after
/// a revision-matched reread; dropping that fallback puts a bars-only replay off screen.
#[test]
fn replay_repeat_keeps_candle_range_after_bars_are_already_shipped() {
    let series = bars_only_series();
    let mut out = ChartHistoryBuffers::default();
    let first = series.read_into(
        0.0,
        0.0,
        (2 * MINUTE_MS) as f32,
        Some(&candle_params(0)),
        &mut out,
    );

    let repeat = series.read_into(
        0.0,
        0.0,
        (2 * MINUTE_MS) as f32,
        Some(&candle_params(first.candles_revision)),
        &mut out,
    );

    assert_eq!(
        repeat.tick_price_range,
        Some((90.0, 110.0)),
        "the original bar envelope remains available when no candle rows are re-emitted"
    );
}

/// `market/trade_replay/mod.rs:bar_inside` must reject only wholly contained bars; relaxing it
/// to an overlap drops the right edge candle and leaves a blank gutter beside the tick trace.
#[test]
fn replay_ticks_keep_both_straddling_edge_candles() {
    let mut series = bars_only_series();
    series.source = TradeReplaySource::Ticks;
    series.covered = Coverage::one((MINUTE_MS / 2, 5 * MINUTE_MS / 2));
    let mut out = ChartHistoryBuffers::default();

    series.read_into(
        0.0,
        0.0,
        (2 * MINUTE_MS) as f32,
        Some(&candle_params(0)),
        &mut out,
    );

    assert_eq!(
        out.candles
            .iter()
            .map(|candle| candle.t_open_ms as i64)
            .collect::<Vec<_>>(),
        vec![0, 2 * MINUTE_MS],
        "only the wholly covered middle candle may step aside; both straddling edge candles close the tick trace"
    );
}

/// `market/trade_replay/mod.rs:TradeReplaySeries::read_into` must leave a `covered: None`
/// Klines1m series whole; applying the hide with its window span blanks the fallback chart while ticks load.
#[test]
fn replay_bars_only_series_keeps_every_candle_without_tick_coverage() {
    let series = bars_only_series();
    let mut out = ChartHistoryBuffers::default();

    series.read_into(
        0.0,
        0.0,
        (2 * MINUTE_MS) as f32,
        Some(&candle_params(0)),
        &mut out,
    );

    assert_eq!(
        out.candles
            .iter()
            .map(|candle| candle.t_open_ms as i64)
            .collect::<Vec<_>>(),
        vec![0, MINUTE_MS, 2 * MINUTE_MS],
        "a bars-only fallback must keep every one-minute candle instead of rendering an empty chart"
    );
}

/// `market/trade_replay/mod.rs:cache_covers` must enforce both edges and its one-bar allowance;
/// widening the allowance or dropping the right-edge check silently reuses incomplete exit bars.
#[test]
fn cache_coverage_rejects_prefixes_and_oversized_holes() {
    let window = ReplayWindow {
        from_ms: 0,
        to_ms: 5 * MINUTE_MS,
        open_ms: 0,
        close_ms: 5 * MINUTE_MS,
        margin_ms: MARGIN_MS,
        long_position_ms: 5 * 60_000,
        over_budget: false,
    };
    let exact = [0, 1, 2, 3, 4, 5]
        .into_iter()
        .map(|minute| candle(minute * MINUTE_MS, 1.0, 2.0, 1.5))
        .collect::<Vec<_>>();
    let prefix = [0, 1, 2]
        .into_iter()
        .map(|minute| candle(minute * MINUTE_MS, 1.0, 2.0, 1.5))
        .collect::<Vec<_>>();
    let oversized_hole = [0, 1, 3, 4, 5]
        .into_iter()
        .map(|minute| candle(minute * MINUTE_MS, 1.0, 2.0, 1.5))
        .collect::<Vec<_>>();

    assert!(
        cache_covers(&exact, window, MINUTE_MS, 0),
        "every requested bar covers the window"
    );
    assert!(
        !cache_covers(&prefix, window, MINUTE_MS, 0),
        "a left-hand prefix cannot cover the trade exit"
    );
    assert!(
        !cache_covers(&oversized_hole, window, MINUTE_MS, 0),
        "a two-minute opening gap exceeds the one-minute allowance"
    );
    assert!(
        cache_covers(&exact, window, MINUTE_MS, 0),
        "adjacent bars are separated by exactly the allowed one-bar opening interval"
    );
    assert!(
        !cache_covers(&[], window, MINUTE_MS, 0),
        "no rows never cover a window"
    );
}

/// `market/trade_replay/mod.rs:replay_window` must frame a same-second trade on its context
/// floors, reject reversed or non-positive stamps, and retain a pre-epoch trade at the Unix epoch.
#[test]
fn replay_window_accepts_same_second_stamps_and_rejects_invalid_inputs() {
    let same_second =
        replay_window_ms(100_000_000, 100_000_000, MARGIN_MS).expect("same-second trade");
    assert_eq!(
        (same_second.from_ms, same_second.to_ms),
        (78_400_000, 107_200_000),
        "a same-second trade needs the six-hour lead and two-hour trail floors"
    );
    assert!(
        !same_second.over_budget,
        "the floor-only same-second window stays inside the replay budget"
    );
    assert_eq!(
        replay_window_ms(101_000, 100_000, MARGIN_MS),
        None,
        "replay_window accepting an exit before its open would request an impossible chart"
    );
    assert_eq!(
        replay_window_ms(0, 100_000, MARGIN_MS),
        None,
        "replay_window accepting a non-positive open would send an invalid venue request"
    );

    let pre_epoch = replay_window_ms(1_000, 2_000, MARGIN_MS).expect("short positive trade");
    assert_eq!(
        pre_epoch.from_ms, 0,
        "replay_window must not send a negative start time to a venue"
    );
    assert!(
        pre_epoch.to_ms >= 2_000,
        "replay_window moving a pre-epoch edge must still retain the trade exit"
    );
}

/// `market/trade_replay/mod.rs:replay_window` must keep proportional context when it exceeds
/// each floor; replacing `pad_ms.max(LEAD_FLOOR_MS)` with addition doubles long replay requests.
#[test]
fn replay_window_uses_maximum_floors_and_proportional_context() {
    let open_s = 200_000;
    let open_ms = open_s * 1_000;
    for (held_hours, lead_hours, trail_hours) in [(0, 6, 2), (4, 6, 2), (16, 8, 8), (32, 16, 16)] {
        let close_s = open_s + held_hours * 60 * 60;
        let window =
            replay_window_ms(open_s * 1_000, close_s * 1_000, MARGIN_MS).expect("valid trade");
        assert_eq!(
            open_ms - window.from_ms,
            lead_hours * 60 * MINUTE_MS,
            "the lead must retain six hours or half the holding time, whichever is greater"
        );
        assert_eq!(
            window.to_ms - close_s * 1_000,
            trail_hours * 60 * MINUTE_MS,
            "the trail must retain two hours or half the holding time, whichever is greater"
        );
        assert_eq!(
            window.span_ms(),
            (held_hours + lead_hours + trail_hours) * 60 * MINUTE_MS,
            "context floors must not be added on top of proportional padding"
        );
    }
}

/// `market/trade_replay/mod.rs:replay_window` must trim only context; restoring its centred
/// MAX_SPAN_MS clip hides an eight-day trade's entry and exit outside the replay picture.
#[test]
fn replay_window_keeps_trade_and_floors_when_trimming_the_budget() {
    let open_s = 1_000_000;
    let close_s = open_s + 8 * 24 * 60 * 60;
    let open_ms = open_s * 1_000;
    let close_ms = close_s * 1_000;
    let long = replay_window_ms(open_s * 1_000, close_s * 1_000, MARGIN_MS)
        .expect("valid eight-day trade");

    assert!(
        long.from_ms <= open_ms,
        "replay_window restoring a centred MAX_SPAN_MS clip would hide the entry outside its chart"
    );
    assert!(
        long.to_ms >= close_ms,
        "replay_window restoring a centred MAX_SPAN_MS clip would hide the exit outside its chart"
    );
    assert!(
        open_ms - long.from_ms >= 6 * 60 * MINUTE_MS,
        "replay_window trimming past LEAD_FLOOR_MS would remove required context before the entry"
    );
    assert!(
        long.to_ms - close_ms >= 2 * 60 * MINUTE_MS,
        "replay_window trimming past TRAIL_FLOOR_MS would remove required context after the exit"
    );
    assert!(
        long.over_budget,
        "replay_window retaining floors beyond MAX_SPAN_MS must label the wider request over_budget"
    );

    let threshold_ms = 7 * 24 * 60 * MINUTE_MS - 8 * 60 * MINUTE_MS;
    let just_under_s = threshold_ms / 1_000 - 60;
    let just_over_s = threshold_ms / 1_000 + 60;
    let just_under = replay_window_ms(open_s * 1_000, (open_s + just_under_s) * 1_000, MARGIN_MS)
        .expect("valid under-budget trade");
    let just_over = replay_window_ms(open_s * 1_000, (open_s + just_over_s) * 1_000, MARGIN_MS)
        .expect("valid over-budget trade");

    assert!(
        !just_under.over_budget,
        "replay_window marking a floor-preserving window over_budget below MAX_SPAN_MS misstates request cost"
    );
    assert!(
        just_over.over_budget,
        "replay_window discarding floors above MAX_SPAN_MS would hide that the request exceeds its budget"
    );
}

/// `market/trade_replay/mod.rs:TradeReplaySeries::read_into` dropping the source identity salt,
/// or salting `Klines1m`, makes a tick upgrade leave exchange candles on screen or makes every
/// existing replay look changed to the chart.
#[test]
fn tick_and_kline_replays_keep_distinct_revisions_without_changing_kline_revision() {
    let kline = bars_only_series();
    let mut ticks = kline.clone();
    ticks.source = TradeReplaySource::Ticks;
    ticks.ticks = vec![crate::feed::types::Tick {
        time_ms: MINUTE_MS as f64,
        price: 101.0,
        qty: 2.0,
        side: crate::feed::types::Side::Buy,
    }];

    let mut kline_out = ChartHistoryBuffers::default();
    let kline_read = kline.read_into(
        0.0,
        0.0,
        (2 * MINUTE_MS) as f32,
        Some(&candle_params(0)),
        &mut kline_out,
    );
    let mut tick_out = ChartHistoryBuffers::default();
    let tick_read = ticks.read_into(
        0.0,
        0.0,
        (2 * MINUTE_MS) as f32,
        Some(&candle_params(0)),
        &mut tick_out,
    );

    let unchanged_kline_revision = replay_revision(kline.identity, MINUTE_MS, 0, 2);
    assert_eq!(
        kline_read.revision, unchanged_kline_revision,
        "Klines1m keeps the established replay revision for the same identity and window"
    );
    assert_ne!(
        tick_read.revision, kline_read.revision,
        "a tick upgrade must force its aggregated candles to replace already shipped klines"
    );
}

/// `market/trade_replay/mod.rs:time_slices` representing an unlimited query span as a saturating
/// integer can make pagination step backwards forever, hanging the sole replay worker and every
/// later trade-detail window.
#[test]
fn time_slices_keeps_unbounded_windows_whole_and_bounded_windows_gap_free() {
    let window = ReplayWindow {
        from_ms: 1_000,
        to_ms: 7_200_999,
        open_ms: 1_000,
        close_ms: 7_200_999,
        margin_ms: MARGIN_MS,
        long_position_ms: 5 * 60_000,
        over_budget: false,
    };

    assert_eq!(
        time_slices(window, None),
        vec![(window.from_ms, window.to_ms)],
        "an unlimited route issues one request for precisely its requested window"
    );

    let span_ms = 3_600_000;
    let slices = time_slices(window, Some(span_ms));
    assert_eq!(
        slices.first().copied(),
        Some((window.from_ms, window.from_ms + span_ms - 1)),
        "the first bounded request starts at the requested left edge and consumes one legal span"
    );
    assert_eq!(
        slices.last().copied().map(|(_, end)| end),
        Some(window.to_ms),
        "the final bounded request reaches the requested right edge"
    );
    assert!(
        slices
            .iter()
            .all(|(start, end)| end >= start && end - start < span_ms),
        "each slice stays strictly within the documented exclusive maximum span"
    );
    assert!(
        slices.windows(2).all(|pair| pair[0].1 + 1 == pair[1].0),
        "adjacent requests neither leave a market-data gap nor re-fetch a boundary millisecond"
    );
}

/// `market/trade_replay/mod.rs:tick_plan` sorting tiles by clock time instead of focus-first
/// spends the budget on lead context and makes a partial replay omit the trade itself.
#[test]
fn tick_plan_prioritizes_focus_and_keeps_every_prefix_contiguous_after_clipping() {
    let window = replay_window_ms(100_000_000, 100_000_000, MARGIN_MS)
        .expect("a same-second scalp has floor context");
    let earliest_ms = window.from_ms + 20 * MINUTE_MS;
    let plan = tick_plan(
        window,
        venue_caps::TradeRoute::BinanceUsdMAggTrades,
        Some(earliest_ms),
        ReplayIntent::Chart,
    );
    let focus = window.focus();

    assert!(
        plan.focus_len > 0,
        "the retained focus must have at least one slice"
    );
    assert!(
        plan.slices[0].0 <= window.open_ms && window.open_ms <= plan.slices[0].1,
        "the eight-hour scalp window's entry belongs to the very first fetched slice"
    );
    let focus_slices = &plan.slices[..plan.focus_len];
    assert_eq!(
        focus_slices.iter().map(|slice| slice.0).min(),
        Some(focus.0),
        "the focus prefix begins at the independently derived focus edge"
    );
    assert_eq!(
        focus_slices.iter().map(|slice| slice.1).max(),
        Some(focus.1),
        "the focus prefix reaches the independently derived focus edge"
    );
    assert!(
        plan.slices.iter().all(|(from, _)| *from >= earliest_ms),
        "retention clipping must exclude tiles older than the route can answer"
    );
    for prefix_len in 1..=plan.slices.len() {
        let mut prefix = plan.slices[..prefix_len].to_vec();
        prefix.sort_unstable();
        assert!(
            prefix.windows(2).all(|pair| pair[0].1 + 1 == pair[1].0),
            "prefix {prefix_len} must form one gap-free covered interval rather than a comb"
        );
    }
}

/// `market/trade_replay/mod.rs:fit_ticks` dropping its terminal stride or thinning an already
/// fitting input can overflow the GPU ring or alter raw trade points without need.
#[test]
fn fit_ticks_obeys_every_budget_boundary_and_preserves_raw_inputs_that_fit() {
    let raw = (0..10)
        .map(|index| crate::feed::types::Tick {
            time_ms: (index * 100) as f64,
            price: 100.0 + index as f32,
            qty: 1.0,
            side: crate::feed::types::Side::Buy,
        })
        .collect::<Vec<_>>();

    for budget in [0, 1, 2, 3] {
        let (result, _) = fit_ticks(raw.clone(), budget);
        assert!(
            result.len() <= budget,
            "a ten-row input must never exceed requested budget {budget}"
        );
    }
    for budget in [10, 11] {
        let (result, bucket_ms) = fit_ticks(raw.clone(), budget);
        assert_eq!(
            result
                .iter()
                .map(|tick| (tick.time_ms, tick.price, tick.qty))
                .collect::<Vec<_>>(),
            raw.iter()
                .map(|tick| (tick.time_ms, tick.price, tick.qty))
                .collect::<Vec<_>>(),
            "a raw vector that fits budget {budget} stays unchanged"
        );
        assert_eq!(bucket_ms, 0, "an already fitting vector reports raw ticks");
    }
    let (empty, bucket_ms) = fit_ticks(Vec::new(), 0);
    assert!(empty.is_empty(), "empty input stays empty at zero budget");
    assert_eq!(bucket_ms, 0, "empty input already fits and remains raw");
}

/// `market/trade_replay/mod.rs:tick_identity_salt` including tick-status payload makes identical
/// candle rows re-upload merely because a tick attempt changed from pending to failed.
#[test]
fn kline_tick_statuses_keep_the_same_chart_revision_while_ticks_change_it() {
    let pending = bars_only_series();
    let mut failed = pending.clone();
    failed.tick_status = TickStatus::Failed;
    let mut tick_upgrade = pending.clone();
    tick_upgrade.source = TradeReplaySource::Ticks;
    tick_upgrade.tick_status = TickStatus::Served;
    tick_upgrade.ticks = vec![crate::feed::types::Tick {
        time_ms: MINUTE_MS as f64,
        price: 101.0,
        qty: 1.0,
        side: crate::feed::types::Side::Buy,
    }];

    let read_revision = |series: &TradeReplaySeries| {
        let mut out = ChartHistoryBuffers::default();
        series
            .read_into(
                0.0,
                0.0,
                (2 * MINUTE_MS) as f32,
                Some(&candle_params(0)),
                &mut out,
            )
            .revision
    };

    assert_eq!(
        read_revision(&pending),
        read_revision(&failed),
        "pending and failed candle fallbacks carry identical rows and therefore one revision"
    );
    assert_ne!(
        read_revision(&pending),
        read_revision(&tick_upgrade),
        "a tick upgrade must have its own revision so the pane uploads its new points"
    );
}

/// A position held up to `long_position_ms()` keeps one focus; past it the focus is two
/// neighbourhoods — the whole margin on both sides of each end, so the run-up before the entry
/// and the tail past the exit are the margin, as on a short position — clamped into the window
/// like the whole one.
#[test]
fn focus_spans_split_only_a_long_position() {
    // A margin well under the position's length, so the two ends of a long one stay apart:
    // neighbourhoods that reach each other fold into one stretch, which the end of this test
    // pins.
    let margin_ms: i64 = long_position_ms() / 5;
    let short =
        replay_window_ms(100_000_000, 100_000_000 + long_position_ms(), margin_ms).expect("window");
    assert_eq!(short.focus_spans(), Coverage::one(short.focus()));

    let open_ms = 100_000_000;
    let close_ms = open_ms + long_position_ms() + 1;
    let long = replay_window_ms(open_ms, close_ms, margin_ms).expect("window");
    let spans = long.focus_spans();
    assert_eq!(
        spans.spans(),
        &[
            (open_ms - margin_ms, open_ms + margin_ms),
            (close_ms - margin_ms, close_ms + margin_ms),
        ]
    );
    let (left, right) = long.focus();
    assert!(left >= long.from_ms && right <= long.to_ms);
    assert!(spans.hull().is_some_and(|(a, b)| a >= left && b <= right));
    // Zero margin: the position alone, and a long one only its two end stamps.
    let bare = replay_window_ms(open_ms, close_ms, 0).expect("window");
    assert_eq!(bare.focus(), (open_ms, close_ms));
    assert_eq!(
        bare.focus_spans().spans(),
        &[(open_ms, open_ms), (close_ms, close_ms)]
    );
    // A margin whose two neighbourhoods reach each other folds the two ends into one stretch —
    // the whole focus, the margin before the entry and past the exit as on a short one.
    let length = close_ms - open_ms;
    let wide = replay_window_ms(open_ms, close_ms, length).expect("window");
    assert_eq!(
        wide.focus_spans(),
        Coverage::one((open_ms - length, close_ms + length))
    );
    assert_eq!(wide.focus_spans(), Coverage::one(wide.focus()));
}

/// A long position's plan tiles only the two neighbourhoods: the trade parts of both first (the
/// exit's first on a backward route, the entry's on a forward one), then the outside margins,
/// and nothing between them.
#[test]
fn tick_plan_of_a_long_position_tiles_the_entry_and_the_exit_only() {
    use venue_caps::TradeRoute::*;
    let open_ms = 100_000_000;
    let close_ms = open_ms + 8 * 60 * MINUTE_MS;
    let window = replay_window_ms(open_ms, close_ms, MARGIN_MS).expect("window");
    for route in [BinanceUsdMAggTrades, OkxHistoryTrades] {
        let plan = tick_plan(window, route, None, ReplayIntent::Chart);
        let backward = route == OkxHistoryTrades;
        let entry_trade = (open_ms, open_ms + MARGIN_MS);
        let exit_trade = (close_ms - MARGIN_MS, close_ms);
        let lead = (open_ms - MARGIN_MS, open_ms - 1);
        let trail = (close_ms + 1, close_ms + MARGIN_MS);
        let expected = if backward {
            vec![exit_trade, entry_trade, lead, trail]
        } else {
            vec![entry_trade, exit_trade, trail, lead]
        };
        assert_eq!(plan.slices, expected, "{route:?}");
        assert_eq!(plan.trade_len, 2);
        assert_eq!(plan.focus_len, 4);
        assert!(
            plan.slices
                .iter()
                .all(|&s| s.1 <= open_ms + MARGIN_MS || s.0 >= close_ms - MARGIN_MS),
            "no tile inside the middle of the position"
        );
    }
}

/// A model's request walks the entry's lead before the exit's trail on a forward route — the
/// run-up is what its entry model reads — and the lead is still walked away from the trade, so
/// every completed prefix stays one stretch. A backward route already walks the lead first.
/// The model's trade tile reaches [`MODEL_PAD_MS`] outside the position — the run-up and the
/// tail are walked under the trade budget, never as an optional margin; a chart's is the
/// position alone.
#[test]
fn tick_plan_of_a_model_request_walks_the_lead_before_the_trail() {
    use venue_caps::TradeRoute::*;
    let open_ms = 100_000_000;
    let close_ms = open_ms + MINUTE_MS;
    let window = replay_window_ms(open_ms, close_ms, MARGIN_MS).expect("window");
    let (focus_from, focus_to) = window.focus();
    let padded = (open_ms - MODEL_PAD_MS, close_ms + MODEL_PAD_MS);
    let lead = (focus_from, padded.0 - 1);
    let trail = (padded.1 + 1, focus_to);
    let forward = tick_plan(window, BinanceUsdMAggTrades, None, ReplayIntent::Model);
    assert_eq!(forward.slices, vec![padded, lead, trail]);
    assert_eq!((forward.trade_len, forward.focus_len), (1, 3));
    let trade = (open_ms, close_ms);
    let chart = tick_plan(window, BinanceUsdMAggTrades, None, ReplayIntent::Chart);
    assert_eq!(
        chart.slices,
        vec![trade, (close_ms + 1, focus_to), (focus_from, open_ms - 1)],
        "the chart keeps its order and its bare trade tile"
    );
    let backward = tick_plan(window, OkxHistoryTrades, None, ReplayIntent::Model);
    assert_eq!(
        backward.slices,
        vec![padded, lead, trail],
        "a backward route walks the lead first for either intent, the model's tile padded"
    );
    assert_eq!(
        tick_plan(window, OkxHistoryTrades, None, ReplayIntent::Chart).slices,
        vec![trade, (focus_from, open_ms - 1), (close_ms + 1, focus_to)],
        "and the chart's bare"
    );
}

/// Walking a long position's plan proves two stretches, never a hull over the unwalked hours
/// between them; each snapshot is paid for and the middle stays out of coverage.
#[test]
fn paginating_a_long_position_covers_two_stretches_and_never_the_middle() {
    use venue_caps::TradeRoute::*;
    let open_ms = 100_000_000;
    let close_ms = open_ms + 8 * 60 * MINUTE_MS;
    let window = replay_window_ms(open_ms, close_ms, MARGIN_MS).expect("window");
    for route in [BinanceUsdMAggTrades, OkxHistoryTrades] {
        let plan = tick_plan(window, route, None, ReplayIntent::Chart);
        let mut observer = CoverageObserver::default();
        let fetched = observer.fetched.clone();
        let verdict = worker::paginate_ticks(
            route,
            &plan,
            usize::MAX,
            usize::MAX,
            || false,
            |_| false,
            &mut observer,
            |from, to, cursor| dense_page(route, from, to, cursor, 30_000, &fetched),
        );
        let worker::TickVerdict::Ready(harvest) = verdict else {
            panic!("served")
        };
        assert!(harvest.complete);
        assert_eq!(
            harvest.covered.spans(),
            &[
                (open_ms - MARGIN_MS, open_ms + MARGIN_MS),
                (close_ms - MARGIN_MS, close_ms + MARGIN_MS),
            ],
            "{route:?}"
        );
        assert_fetched(&fetched.borrow(), &harvest.covered);
        assert!(!harvest.covered.contains_ms(open_ms + 60 * MINUTE_MS));
        assert_eq!(observer.snapshots.last(), Some(&harvest.covered));
    }
}

/// Bars inside EACH covered stretch step aside; bars between the two stretches stay drawn, since
/// nothing proved them exhaustively told by ticks.
#[test]
fn replay_with_split_coverage_keeps_the_middle_bars() {
    let mut series = bars_only_series();
    series.source = TradeReplaySource::Ticks;
    series.covered = Coverage::from_spans([(0, MINUTE_MS - 1), (2 * MINUTE_MS, 3 * MINUTE_MS - 1)]);
    let mut out = ChartHistoryBuffers::default();
    series.read_into(
        0.0,
        0.0,
        (2 * MINUTE_MS) as f32,
        Some(&candle_params(0)),
        &mut out,
    );
    assert_eq!(
        out.candles
            .iter()
            .map(|candle| candle.t_open_ms as i64)
            .collect::<Vec<_>>(),
        vec![MINUTE_MS],
        "the first and third minutes are covered by ticks; the middle minute stays a bar"
    );
}
