//! Regression and scale probes for background market recording.

use moonproto::MoonTime;
use moonproto::state::TradeHistoryRow;

/// Build a synthetic retained print without using account or production data.
fn row(index: usize) -> TradeHistoryRow {
    TradeHistoryRow {
        time: MoonTime::from_unix_millis(index as i64 * 100),
        price: 100.0 + (index % 100) as f32 * 0.01,
        qty: if index.is_multiple_of(2) { 1.0 } else { -1.0 },
    }
}

/// Measure full-ring ownership and conversion against the recorder's current aggregation.
#[test]
#[ignore = "synthetic recorder first-pass timing probe"]
fn recorder_first_pass_scale_probe() {
    let retained: Vec<_> = (0..100_000).map(row).collect();
    let mut before = Vec::new();
    let mut after = Vec::new();
    for _ in 0..7 {
        let start = std::time::Instant::now();
        let rows = retained.to_vec();
        let mut ticks = Vec::new();
        super::super::rows_to_ticks(&rows, &mut ticks);
        let mut candles = Vec::new();
        crate::market::candles::aggregate_trades(&ticks, 300_000, &mut candles);
        std::hint::black_box(&candles);
        before.push(start.elapsed().as_secs_f64() * 1_000.0);

        let start = std::time::Instant::now();
        let mut candles = Vec::new();
        for row in &retained {
            let tick = crate::feed::Tick {
                time_ms: row.unix_millis() as f64,
                price: row.price,
                qty: row.quantity(),
                side: if row.is_buy() {
                    crate::feed::Side::Buy
                } else {
                    crate::feed::Side::Sell
                },
            };
            crate::market::candles::aggregate_trade(&tick, 300_000, &mut candles);
        }
        std::hint::black_box(&candles);
        after.push(start.elapsed().as_secs_f64() * 1_000.0);
    }
    before.sort_by(f64::total_cmp);
    after.sort_by(f64::total_cmp);
    println!(
        "[bench] recorder_first_pass rows=100000 before_ms={:.6} after_ms={:.6}",
        before[3], after[3]
    );
}

/// Releasing a chart must remove its own registry entries without a provider-wide scan.
#[test]
fn chart_release_forgets_candle_and_deep_request_state() {
    let source = crate::market::MarketDataSource::new(crate::market::MarketStore::shared(0.0));
    let now = std::time::Instant::now();
    {
        let inner = source.inner.read().expect("market source");
        inner.candle_subs.lock().expect("candle subs").insert(
            1,
            std::collections::HashMap::from([(
                "COIN_A".to_string(),
                super::super::CandleSubState {
                    kind_min: 1,
                    last_want: now,
                    subscribed: false,
                },
            )]),
        );
        inner
            .deep_req_gate
            .lock()
            .expect("deep gate")
            .insert((1, "COIN_A".to_string(), 1), now);
    }

    source.release_chart_history(1, "COIN_A");

    let inner = source.inner.read().expect("market source");
    assert!(inner.candle_subs.lock().expect("candle subs").is_empty());
    assert!(inner.deep_req_gate.lock().expect("deep gate").is_empty());
}

/// Borrowed-row aggregation must produce exactly the former owned-copy candles.
#[test]
fn recorder_aggregation_matches_the_owned_copy_path() {
    let mut rows: Vec<_> = (0..1_000).map(row).collect();
    rows.push(row(500));
    let mut ticks = Vec::new();
    super::super::rows_to_ticks(&rows, &mut ticks);
    let mut expected = Vec::new();
    crate::market::candles::aggregate_trades(&ticks, 300_000, &mut expected);
    let mut actual = Vec::new();
    for row in &rows {
        super::aggregate_recorder_trade(row, 300_000, &mut actual);
    }
    assert_eq!(actual, expected);
}
