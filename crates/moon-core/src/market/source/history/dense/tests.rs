use super::*;

/// A broad, stationary camera around the generated five-minute range.
fn viewport(force_reset: bool) -> Viewport {
    Viewport {
        window: (0.0, 400_000.0),
        force_reset,
        price_window: Some((0.0, 400_000.0)),
    }
}

/// Display every retained trade while deriving one-minute candles from the same input.
fn params(revision: u64) -> CandleReadParams {
    CandleReadParams {
        tf_ms: 60_000,
        trades_from_rel_ms: 0.0,
        trades_limit: CAPACITY,
        shipped_revision: revision,
        series_reset: false,
    }
}

/// The first fill, idle read, small tail and device-reset reread exercise different upload paths.
#[test]
fn dense_history_delivers_a_real_initial_fill_then_only_the_new_tail() {
    let mut history = DenseHistory::new(900_000, 1, 300_000);
    let mut cursor = ChartHistoryCursor::default();
    let mut out = ChartHistoryBuffers::default();
    let first = read(
        &history,
        1,
        600_000.0,
        viewport(false),
        Some(&params(0)),
        &mut cursor,
        &mut out,
    );
    assert!(first.combo_reset && first.candles_changed);
    assert_eq!(out.ticks.len(), CAPACITY);
    assert!(out.candles.len() >= 5);
    let first_bucket = &out.candles[0];
    let trades: Vec<_> = out
        .ticks
        .iter()
        .filter(|tick| tick.time_ms < 660_000.0)
        .collect();
    assert_eq!(first_bucket.open, trades.first().unwrap().price);
    assert_eq!(first_bucket.close, trades.last().unwrap().price);
    assert_eq!(
        first_bucket.high,
        trades
            .iter()
            .map(|tick| tick.price)
            .fold(f32::MIN, f32::max)
    );
    let previous_candles = out.candles.clone();
    out.clear();
    let idle = read(
        &history,
        1,
        600_000.0,
        viewport(false),
        Some(&params(first.candles_revision)),
        &mut cursor,
        &mut out,
    );
    assert!(!idle.combo_reset && !idle.candles_changed);
    assert!(out.ticks.is_empty() && out.candles.is_empty());
    assert_eq!(idle.tick_price_range, first.tick_price_range);

    history.advance(250, 100);
    out.clear();
    let tail = read(
        &history,
        1,
        600_000.0,
        viewport(false),
        Some(&params(first.candles_revision)),
        &mut cursor,
        &mut out,
    );
    assert!(!tail.combo_reset);
    assert_eq!(out.ticks.len(), 250);
    assert_eq!(tail.candles_patch_from, Some(previous_candles.len() - 1));
    assert_eq!(out.candles.len(), 1);
    assert_eq!(out.candles[0].close, history.last_price());

    out.clear();
    let reset = read(
        &history,
        1,
        600_000.0,
        viewport(true),
        Some(&params(tail.candles_revision)),
        &mut cursor,
        &mut out,
    );
    assert!(reset.combo_reset && reset.candles_changed);
    assert_eq!(out.ticks.len(), CAPACITY);
    assert_eq!(out.ticks.last().unwrap().price, history.last_price());
    assert_eq!(
        out.candles[0], previous_candles[0],
        "reset must retain the full candle before the clipped trade tail"
    );
    let before_wrap = cursor.candle_series.candles().to_vec();
    history.advance(CAPACITY + 1, 1_000);
    out.clear();
    let wrapped = read(
        &history,
        1,
        600_000.0,
        viewport(false),
        Some(&params(reset.candles_revision)),
        &mut cursor,
        &mut out,
    );
    assert!(wrapped.combo_reset);
    assert_eq!(
        out.candles[0], before_wrap[0],
        "a lapped trade cursor must not erase older candles"
    );
    assert_eq!(out.candles.last().unwrap().close, history.last_price());
}

/// A candle-only consumer must not steal trades from a simultaneous hybrid consumer.
#[test]
fn candle_and_hybrid_consumers_have_independent_cursors() {
    let history = DenseHistory::new(900_000, 1, 300_000);
    let mut candle_cursor = ChartHistoryCursor::default();
    let mut hybrid_cursor = ChartHistoryCursor::default();
    let mut candles = ChartHistoryBuffers::default();
    let mut hybrid = ChartHistoryBuffers::default();
    let mut candle_params = params(0);
    candle_params.trades_from_rel_ms = f32::INFINITY;
    read(
        &history,
        1,
        600_000.0,
        viewport(false),
        Some(&candle_params),
        &mut candle_cursor,
        &mut candles,
    );
    read(
        &history,
        1,
        600_000.0,
        viewport(false),
        Some(&params(0)),
        &mut hybrid_cursor,
        &mut hybrid,
    );
    assert!(candles.ticks.is_empty());
    assert_eq!(hybrid.ticks.len(), CAPACITY);
    assert_eq!(candles.candles, hybrid.candles);
}
