//! Regression tests for exact-core durable marker geometry.

use moon_core::db::ChartTradeRecord;

use super::*;

/// Removing the exact-core filter admits record 3, while emitting only entry or only exit changes
/// the independently enumerated marker tuples and makes a historical round trip incomplete.
#[test]
fn durable_markers_are_an_exact_core_entry_exit_union() {
    let records = vec![
        ChartTradeRecord {
            record_id: 1,
            core_uid: 7,
            coin: "BTCUSDT".to_string(),
            buy_date: 101,
            close_date: 109,
            buy_price: 10.0,
            sell_price: 12.0,
            quantity: 2.0,
            is_short: false,
        },
        ChartTradeRecord {
            record_id: 2,
            core_uid: 7,
            coin: "BTCUSDT".to_string(),
            buy_date: 111,
            close_date: 119,
            buy_price: 20.0,
            sell_price: 18.0,
            quantity: 3.0,
            is_short: true,
        },
        ChartTradeRecord {
            record_id: 3,
            core_uid: 8,
            coin: "BTCUSDT".to_string(),
            buy_date: 121,
            close_date: 129,
            buy_price: 30.0,
            sell_price: 31.0,
            quantity: 4.0,
            is_short: false,
        },
    ];
    let markers = build_trade_history_markers(
        &records,
        7,
        100_000.0,
        2.0,
        [0.1, 0.2, 0.3, 1.0],
        [0.4, 0.5, 0.6, 1.0],
    );
    let actual = markers
        .iter()
        .map(|marker| (marker.t_rel, marker.price, marker.size, marker.shape))
        .collect::<Vec<_>>();
    assert_eq!(
        actual,
        vec![
            (1_000.0, 10.0, 14.0, MARKER_SHAPE_CROSS),
            (9_000.0, 12.0, 14.0, MARKER_SHAPE_KNOT),
            (11_000.0, 20.0, 10.0, MARKER_SHAPE_CROSS),
            (19_000.0, 18.0, 10.0, MARKER_SHAPE_KNOT),
        ]
    );
}
