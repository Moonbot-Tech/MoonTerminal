use super::*;

const MINUTE_MS: i64 = 60_000;

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
        bucket_ms: 0,
        partial: false,
        covered: None,
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
    series.covered = Some((MINUTE_MS / 2, 5 * MINUTE_MS / 2));
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
    let same_second = replay_window(10_000, 10_000).expect("same-second trade");
    assert_eq!(
        (same_second.from_ms, same_second.to_ms),
        (6_400_000, 11_200_000),
        "a same-second trade needs the 60-minute lead and 20-minute trail floors"
    );
    assert!(
        !same_second.over_budget,
        "the floor-only same-second window stays inside the replay budget"
    );
    assert_eq!(
        replay_window(101, 100),
        None,
        "replay_window accepting an exit before its open would request an impossible chart"
    );
    assert_eq!(
        replay_window(0, 100),
        None,
        "replay_window accepting a non-positive open would send an invalid venue request"
    );

    let pre_epoch = replay_window(1, 2).expect("short positive trade");
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
    let ten_hour_open_s = 200_000;
    let ten_hour_close_s = ten_hour_open_s + 10 * 60 * 60;
    let ten_hour = replay_window(ten_hour_open_s, ten_hour_close_s).expect("valid ten-hour trade");
    let ten_hour_open_ms = ten_hour_open_s * 1_000;
    let ten_hour_close_ms = ten_hour_close_s * 1_000;

    assert_eq!(
        ten_hour_open_ms - ten_hour.from_ms,
        5 * 60 * MINUTE_MS,
        "replay_window changing pad_ms.max(LEAD_FLOOR_MS) to addition would inflate a ten-hour trade beyond its five-hour lead"
    );
    assert_eq!(
        ten_hour.to_ms - ten_hour_close_ms,
        5 * 60 * MINUTE_MS,
        "replay_window changing pad_ms.max(TRAIL_FLOOR_MS) to addition would inflate a ten-hour trade beyond its five-hour trail"
    );

    let short_open_s = 10_000;
    let short_close_s = short_open_s + 60;
    let short = replay_window(short_open_s, short_close_s).expect("valid short trade");
    let short_open_ms = short_open_s * 1_000;
    let short_close_ms = short_close_s * 1_000;

    assert_eq!(
        short_open_ms - short.from_ms,
        60 * MINUTE_MS,
        "replay_window removing LEAD_FLOOR_MS would leave a one-minute trade without 60 minutes of lead"
    );
    assert_eq!(
        short.to_ms - short_close_ms,
        20 * MINUTE_MS,
        "replay_window removing TRAIL_FLOOR_MS would leave a one-minute trade without 20 minutes of trail"
    );
    assert_eq!(
        short.span_ms(),
        80 * MINUTE_MS + 60 * 1_000,
        "replay_window summing floors with padding would make a one-minute trade wider than its stated floors"
    );

    let two_hour = replay_window(100_000, 100_000 + 2 * 60 * 60).expect("valid two-hour trade");
    let four_hour = replay_window(100_000, 100_000 + 4 * 60 * 60).expect("valid four-hour trade");
    let two_hour_open_ms = 100_000 * 1_000;
    let four_hour_open_ms = 100_000 * 1_000;

    assert_eq!(
        two_hour_open_ms - two_hour.from_ms,
        60 * MINUTE_MS,
        "replay_window adding LEAD_FLOOR_MS would inflate proportional context for a two-hour trade"
    );
    assert_eq!(
        four_hour_open_ms - four_hour.from_ms,
        2 * 60 * MINUTE_MS,
        "replay_window adding LEAD_FLOOR_MS would inflate proportional context for a four-hour trade"
    );
    assert_eq!(
        four_hour_open_ms - four_hour.from_ms,
        2 * (two_hour_open_ms - two_hour.from_ms),
        "replay_window bypassing proportional padding would stop longer trades from receiving double the lead"
    );
    assert_eq!(
        four_hour.to_ms - (100_000 + 4 * 60 * 60) * 1_000,
        2 * 60 * MINUTE_MS,
        "replay_window replacing proportional padding with TRAIL_FLOOR_MS would cap a four-hour trade at 20 minutes of trail"
    );
}

/// `market/trade_replay/mod.rs:replay_window` must trim only context; restoring its centred
/// MAX_SPAN_MS clip hides an eight-day trade's entry and exit outside the replay picture.
#[test]
fn replay_window_keeps_trade_and_floors_when_trimming_the_budget() {
    let open_s = 1_000_000;
    let close_s = open_s + 8 * 24 * 60 * 60;
    let open_ms = open_s * 1_000;
    let close_ms = close_s * 1_000;
    let long = replay_window(open_s, close_s).expect("valid eight-day trade");

    assert!(
        long.from_ms <= open_ms,
        "replay_window restoring a centred MAX_SPAN_MS clip would hide the entry outside its chart"
    );
    assert!(
        long.to_ms >= close_ms,
        "replay_window restoring a centred MAX_SPAN_MS clip would hide the exit outside its chart"
    );
    assert!(
        open_ms - long.from_ms >= 60 * MINUTE_MS,
        "replay_window trimming past LEAD_FLOOR_MS would remove required context before the entry"
    );
    assert!(
        long.to_ms - close_ms >= 20 * MINUTE_MS,
        "replay_window trimming past TRAIL_FLOOR_MS would remove required context after the exit"
    );
    assert!(
        long.over_budget,
        "replay_window retaining floors beyond MAX_SPAN_MS must label the wider request over_budget"
    );

    let threshold_ms = 7 * 24 * 60 * MINUTE_MS - 80 * MINUTE_MS;
    let just_under_s = threshold_ms / 1_000 - 60;
    let just_over_s = threshold_ms / 1_000 + 60;
    let just_under =
        replay_window(open_s, open_s + just_under_s).expect("valid under-budget trade");
    let just_over = replay_window(open_s, open_s + just_over_s).expect("valid over-budget trade");

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
    let window = replay_window(100_000, 100_000).expect("a same-second scalp has floor context");
    let earliest_ms = window.from_ms + 20 * MINUTE_MS;
    let plan = tick_plan(window, Some(60 * MINUTE_MS), Some(earliest_ms));
    let focus = window.focus();

    assert!(
        plan.focus_len > 0,
        "the retained focus must have at least one slice"
    );
    assert!(
        plan.slices[0].0 <= window.open_ms && window.open_ms <= plan.slices[0].1,
        "the 80-minute scalp's entry belongs to the very first fetched slice"
    );
    let focus_slices = &plan.slices[..plan.focus_len];
    assert_eq!(
        focus_slices.first().map(|slice| slice.0),
        Some(focus.0),
        "the focus prefix begins at the independently derived focus edge"
    );
    assert_eq!(
        focus_slices.last().map(|slice| slice.1),
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
