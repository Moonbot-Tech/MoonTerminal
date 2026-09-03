use super::*;

/// `rest/hyperliquid.rs:parse_row` treating string price fields as numbers drops recorded candles,
/// leaving a user with an empty Hyperliquid replay despite a valid market response.
#[test]
fn hyperliquid_parses_string_prices_at_numeric_millisecond_times() {
    let body: Value = serde_json::from_str(include_str!("fixtures/perp_klines.json"))
        .expect("recorded Hyperliquid fixture is JSON");
    let bars = parse_klines(&body).expect("recorded fixture parses");

    assert_eq!(bars[0].t_open_ms, 1_787_505_300_000.0);
    assert_eq!(bars[0].open, 77_317.0);
    assert_eq!(bars[0].volume, 28.83589);
    assert!(
        bars.iter()
            .all(|bar| bar.low <= bar.open && bar.high >= bar.close)
    );
}

/// `rest/hyperliquid.rs:parse_row` assigning an invented vendor cell instead of OHLC4 changes a
/// venue with no turnover field into a false measured-money value on the chart.
#[test]
fn hyperliquid_estimates_quote_turnover_from_the_recorded_ohlc_and_base_volume() {
    let body: Value = serde_json::from_str(include_str!("fixtures/perp_klines.json"))
        .expect("recorded fixture is JSON");
    let bars = parse_klines(&body).expect("recorded fixture parses");
    let raw = &body[0];
    let value = |key: &str| {
        raw[key]
            .as_str()
            .expect("recorded numeric string")
            .parse::<f32>()
            .expect("finite numeric string")
    };
    assert_eq!(
        bars[0].quote_volume,
        crate::market::candles::estimate_quote_volume(
            value("v"),
            value("o"),
            value("h"),
            value("l"),
            value("c")
        )
    );
}

/// `rest/hyperliquid.rs:classify` calling an HTTP 500 an unknown symbol permanently caches an
/// outage as a missing market instead of allowing the user to retry when the service recovers.
#[test]
fn hyperliquid_failures_are_transient_even_for_unknown_coins() {
    assert_eq!(classify(200), Ok(()));
    assert!(matches!(classify(500), Err(FetchError::Transient(_))));
    assert!(!matches!(classify(500), Err(FetchError::UnknownSymbol)));
}
