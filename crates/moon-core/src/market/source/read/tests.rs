use super::*;
use moonproto::MoonTime;
use moonproto::state::TradeHistoryRow;

/// Synthetic retained print; negative quantities retain the wire sell-side encoding.
fn trade(ms: i64, index: usize) -> TradeHistoryRow {
    TradeHistoryRow {
        time: MoonTime::from_unix_millis(ms),
        price: 100.0 + (index % 100) as f32 * 0.01,
        qty: if index.is_multiple_of(2) { 1.0 } else { -1.0 },
    }
}

/// Frozen pinned copy_time_range loop over retained append order, including its reservation.
/// The protocol keeps its writer private, so this oracle models its immutable read view.
fn old_rows(
    retained: &[TradeHistoryRow],
    capacity: usize,
    from: i64,
    to: i64,
) -> Vec<TradeHistoryRow> {
    let mut rows = Vec::with_capacity(capacity);
    let start = retained
        .iter()
        .position(|row| row.unix_millis() >= from)
        .unwrap_or(retained.len());
    if from < to {
        for row in &retained[start..] {
            let time = row.unix_millis();
            if time >= from && time < to {
                rows.push(*row);
            }
        }
    }
    rows
}

/// Compare every wire field because equal timestamps alone would miss ordering or side changes.
fn row_values(rows: &[TradeHistoryRow]) -> Vec<(i64, u32, u32)> {
    rows.iter()
        .map(|row| (row.unix_millis(), row.price.to_bits(), row.qty.to_bits()))
        .collect()
}

/// Boundary inclusivity, wrapped retention and late resends must match the old thumbnail read.
#[test]
fn detects_trade_rows_match_frozen_window_read() {
    let mut retained = std::collections::VecDeque::with_capacity(8);
    for (index, ms) in [0, 100, 200, 300, 400, 500, 600, 700, 800, 450, 200, 900]
        .into_iter()
        .enumerate()
    {
        if retained.len() == 8 {
            retained.pop_front();
        }
        retained.push_back(trade(ms, index));
        for (from, to) in [(200, 800), (450, 900), (0, 1000), (900, 900), (950, 1000)] {
            let expected = old_rows(&retained.iter().copied().collect::<Vec<_>>(), 8, from, to);
            let (first, second) = retained.as_slices();
            let actual = detect_trade_rows(first.iter().chain(second.iter()), from, to);
            assert_eq!(
                row_values(&actual),
                row_values(&expected),
                "{index}: {from}..{to}"
            );
        }
    }
    let selected = detect_trade_rows(retained.iter(), 450, 800);
    assert_eq!(
        selected
            .iter()
            .map(|row| row.unix_millis())
            .collect::<Vec<_>>(),
        vec![500, 600, 700, 450]
    );
}

/// Time the old reservation/read model and production collection plus conversion/aggregation.
/// Provider cursor lookup, locks and kline-cache waits are deliberately outside this CPU probe.
#[test]
#[ignore = "synthetic retained-print collection and aggregation timing"]
fn detects_trade_ring_scale_probe() {
    let retained: Vec<_> = (0..100_000)
        .map(|index| trade(index as i64 * 864, index))
        .collect();
    for from in [0, 82_800_000] {
        let mut times = Vec::new();
        let mut values = Vec::new();
        for old in [true, false] {
            let start = std::time::Instant::now();
            for _ in 0..48 {
                let rows = if old {
                    old_rows(&retained, 131_072, from, 86_400_000)
                } else {
                    let start = retained
                        .iter()
                        .position(|row| row.unix_millis() >= from)
                        .unwrap_or(retained.len());
                    detect_trade_rows(retained[start..].iter(), from, 86_400_000)
                };
                let mut ticks = Vec::new();
                rows_to_ticks(&rows, &mut ticks);
                let mut candles = Vec::new();
                crate::market::candles::aggregate_trades(&ticks, 300_000, &mut candles);
                std::hint::black_box(&candles);
                values.push(candles);
            }
            times.push(start.elapsed().as_secs_f64() * 1000.0);
        }
        for index in 0..48 {
            assert_eq!(values[index], values[index + 48]);
        }
        eprintln!(
            "detects_ring_model from_ms={from} old_48_cpu_ms={:.6} new_48_background_cpu_ms={:.6}",
            times[0], times[1]
        );
    }
}
