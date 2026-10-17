//! What the autoload asks the station for.

use super::*;

/// A trade `(exchange, market, open_ms, close_ms)` and its window, 30 s either side.
fn trade(market: &str, open_ms: i64, close_ms: i64) -> (String, String, ReplayWindow) {
    let window = moon_core::market::trade_replay::replay_window_ms(open_ms, close_ms, 30_000)
        .expect("a window");
    ("x".to_string(), market.to_string(), window)
}

/// The station is asked for each market once, for the windows of its rows merged, minus what the
/// file already holds; a market the file holds whole is not asked, and a market whose read did not
/// happen is asked whole.
#[test]
fn the_station_is_asked_only_for_what_the_file_lacks() {
    let rows = vec![
        trade("A", 100_000, 110_000),
        trade("A", 105_000, 150_000),
        trade("B", 100_000, 110_000),
        trade("C", 100_000, 110_000),
    ];
    let wants = lacking(
        &rows,
        |row| row.clone(),
        |_, market, from, to| match market {
            // A: the first 20 s held.
            "A" => Some(vec![(from, from + 19_999)]),
            // B: all of it held.
            "B" => Some(vec![(from, to)]),
            // C: the read did not happen.
            _ => None,
        },
    );
    assert_eq!(
        wants
            .iter()
            .map(|w| (w.market.as_str(), w.spans.clone()))
            .collect::<Vec<_>>(),
        vec![
            ("A", vec![(90_000, 180_000)]),
            ("C", vec![(70_000, 140_000)]),
        ]
    );
}
