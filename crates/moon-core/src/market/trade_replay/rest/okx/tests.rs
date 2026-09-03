use super::*;

fn fixture(name: &str) -> Value {
    let text = match name {
        "spot" => include_str!("fixtures/spot_klines.json"),
        "swap" => include_str!("fixtures/swap_klines.json"),
        "unknown" => include_str!("fixtures/unknown_symbol.json"),
        _ => unreachable!("only recorded OKX fixtures are used"),
    };
    serde_json::from_str(text).expect("recorded OKX fixture is JSON")
}

/// `rest/okx.rs:SWAP_VOLUME_CELL` changing from cell 6 to cell 5 turns swap contract counts into
/// base volume, making replayed perpetual candles show a hundredfold volume error in the shared cache.
#[test]
fn okx_uses_the_market_specific_base_volume_cell() {
    let spot = parse_klines(&fixture("spot"), SPOT_VOLUME_CELL).expect("spot fixture parses");
    let swap = parse_klines(&fixture("swap"), SWAP_VOLUME_CELL).expect("swap fixture parses");

    assert!((spot[0].volume - 10.030_456_94).abs() < 0.000_01);
    assert!((swap[0].volume - 144.84).abs() < 0.001);
    assert!(
        swap[0].volume < 1_000.0,
        "the recorded 14,484 contract count is not base-asset volume"
    );
}

/// `rest/okx.rs:parse_row` reading either market's base-volume cell for turnover makes the band
/// report contracts or base units rather than OKX's `volCcyQuote` money value.
#[test]
fn okx_reads_quote_turnover_from_cell_seven_for_spot_and_swap() {
    for (name, cell) in [("spot", SPOT_VOLUME_CELL), ("swap", SWAP_VOLUME_CELL)] {
        let body = fixture(name);
        let bars = parse_klines(&body, cell).expect("recorded fixture parses");
        let expected = body["data"][0][7]
            .as_str()
            .expect("quote turnover cell")
            .parse::<f32>()
            .expect("finite quote turnover");
        assert_eq!(bars[0].quote_volume, expected, "{name} uses volCcyQuote");
    }
}

/// `rest/okx.rs:classify` accepting every 2xx response as success stores an unknown instrument as
/// an authoritative empty replay window instead of showing the user a permanent missing-market verdict.
#[test]
fn okx_http_success_still_requires_a_success_envelope_code() {
    assert_eq!(
        classify(200, &fixture("unknown")),
        Err(FetchError::UnknownSymbol)
    );
    assert_eq!(classify(200, &fixture("spot")), Ok(()));
    assert!(matches!(
        classify(200, &serde_json::json!({"data": []})),
        Err(FetchError::Transient(_))
    ));
}
