//! Regression coverage for the Screener retained-range delta projection.

use moonproto::state::DerivedDeltaSnapshot;

use super::ScreenerRow;

/// Sharing a market must not share mutable member fields or copy market strings for each core.
#[test]
fn shared_market_rows_keep_account_values_independent() {
    use super::SharedScreenerRow;
    use std::sync::Arc;
    let market = Arc::new(ScreenerRow {
        provider: 3,
        market: "XALPHAUSDT".into(),
        coin: "XALPHA".into(),
        vol_24h: 123.0,
        ..Default::default()
    });
    let first = SharedScreenerRow::of_market(market.clone(), 5);
    let mut second = SharedScreenerRow::of_market(market.clone(), 7);
    second.core_pnl = -2.0;
    second.session = Some(4.0);
    second.pos_size = 6.0;
    second.leverage_x = 8;
    second.isolated = Some(true);
    second.orders = 10;
    assert!(Arc::ptr_eq(&first.market_data, &second.market_data));
    assert_eq!(
        (first.provider, first.core, second.provider, second.core),
        (3, 5, 3, 7)
    );
    assert_eq!(
        (
            first.market.as_str(),
            second.market.as_str(),
            second.vol_24h
        ),
        ("XALPHAUSDT", "XALPHAUSDT", 123.0)
    );
    assert_eq!(
        (
            first.core_pnl,
            first.session,
            first.pos_size,
            first.leverage_x,
            first.isolated,
            first.orders
        ),
        (0.0, None, 0.0, 0, None, 0)
    );
    assert_eq!(
        (
            second.core_pnl,
            second.session,
            second.pos_size,
            second.leverage_x,
            second.isolated,
            second.orders
        ),
        (-2.0, Some(4.0), 6.0, 8, Some(true), 10)
    );
}

/// Removing or swapping any assignment in `screener.rs:ScreenerRow::apply_range_deltas` must fail
/// the distinct-value assertion; otherwise a Screener column displays zero or another period's
/// movement instead of its matching retained-range delta.
#[test]
fn every_screener_delta_uses_its_matching_range_period() {
    let mut row = ScreenerRow::default();
    row.apply_range_deltas(Some(DerivedDeltaSnapshot {
        one_minute: 1.0,
        fifteen_minutes: 15.0,
        one_hour: 60.0,
        three_hours: 180.0,
        twenty_four_hours: 1_440.0,
        seventy_two_hours: 4_320.0,
        ..DerivedDeltaSnapshot::default()
    }));

    assert_eq!(
        [
            row.d_24h, row.d_3h, row.d_1h, row.d_15m, row.d_1m, row.d_72h
        ],
        [1_440.0, 180.0, 60.0, 15.0, 1.0, 4_320.0]
    );
}
