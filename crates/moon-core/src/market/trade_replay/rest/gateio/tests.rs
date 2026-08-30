use super::*;

fn fixture(name: &str) -> Value {
    let text = match name {
        "spot" => include_str!("fixtures/spot_klines.json"),
        "futures" => include_str!("fixtures/futures_klines.json"),
        "spot_unknown" => include_str!("fixtures/spot_unknown_symbol.json"),
        "futures_unknown" => include_str!("fixtures/futures_unknown_symbol.json"),
        _ => unreachable!("only recorded Gate fixtures are used"),
    };
    serde_json::from_str(text).expect("recorded Gate fixture is JSON")
}

/// `rest/gateio.rs:parse_spot_row` reading the familiar OHLC positions swaps open and close, so a
/// replayed Gate candle keeps plausible wicks but reverses its body for the user.
#[test]
fn gate_spot_keeps_its_non_ohlc_vendor_cell_order() {
    let body = fixture("spot");
    let raw_rows = body.as_array().expect("recorded spot response is an array");
    let bars = parse_spot_klines(&body).expect("spot fixture parses");

    for (raw, bar) in raw_rows.iter().zip(&bars) {
        assert!(bar.high >= bar.open.max(bar.close));
        assert!(bar.low <= bar.open.min(bar.close));
        let quote_volume = raw[1]
            .as_str()
            .expect("recorded quote volume")
            .parse::<f32>()
            .expect("recorded quote volume is finite decimal text");
        assert_ne!(bar.open, quote_volume, "cell 1 is quote volume, never open");
    }
}

/// `rest/gateio.rs:parse_spot_row` dropping the seconds-to-milliseconds conversion or its closed
/// flag admits an unfinished minute or files it at 1970, corrupting the replay timeline.
#[test]
fn gate_spot_scales_seconds_and_rejects_an_open_window() {
    let body = fixture("spot");
    let mut forming = body.clone();
    forming[0][7] = Value::String("false".to_string());
    let closed = parse_spot_klines(&body).expect("closed fixture parses");
    let filtered = parse_spot_klines(&forming).expect("modified fixture parses");

    assert_eq!(closed[0].t_open_ms, 1_787_508_780_000.0);
    assert_eq!(filtered.len() + 1, closed.len());
    assert!(
        !filtered
            .iter()
            .any(|bar| bar.t_open_ms == closed[0].t_open_ms)
    );
}

/// `rest/gateio.rs:parse_futures_row` reading `v` as volume treats a contract count as base volume,
/// so the replay volume histogram falsely reports an exchange-specific quantity as an asset amount.
#[test]
fn gate_futures_keeps_unknown_base_volume_at_zero() {
    let body = fixture("futures");
    let bars = parse_futures_klines(&body).expect("futures fixture parses");
    let raw_count = body[0]["v"].as_f64().expect("recorded contract count");

    assert_eq!(bars[0].t_open_ms, 1_787_508_780_000.0);
    assert_eq!(bars[0].open, 77_361.4);
    assert_eq!(bars[0].volume, 0.0);
    assert!(
        raw_count > 0.0,
        "the zero is deliberate, not a missing fixture value"
    );
}

/// `rest/gateio.rs:classify` treating Gate's known 400 labels as transient gives users a retry for
/// a market that does not exist instead of the permanent missing-symbol verdict.
#[test]
fn gate_unknown_symbol_labels_are_permanent() {
    assert_eq!(
        classify(400, &fixture("spot_unknown")),
        Err(FetchError::UnknownSymbol)
    );
    assert_eq!(
        classify(400, &fixture("futures_unknown")),
        Err(FetchError::UnknownSymbol)
    );
}
