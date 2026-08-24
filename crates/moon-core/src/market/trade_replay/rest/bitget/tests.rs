use super::*;

fn fixture(name: &str) -> Value {
    let text = match name {
        "spot" => include_str!("fixtures/spot_klines.json"),
        "futures" => include_str!("fixtures/futures_klines.json"),
        "spot_unknown" => include_str!("fixtures/spot_unknown_symbol.json"),
        "futures_unknown" => include_str!("fixtures/futures_unknown_symbol.json"),
        _ => unreachable!("only recorded BitGet fixtures are used"),
    };
    serde_json::from_str(text).expect("recorded BitGet fixture is JSON")
}

/// `rest/bitget.rs:parse_row` moving volume from cell 5 to a quote-volume cell inflates or changes
/// both BitGet market histories, so a user sees the wrong replayed base-volume bars.
#[test]
fn bitget_reads_base_volume_from_cell_five_on_both_row_shapes() {
    for name in ["spot", "futures"] {
        let body = fixture(name);
        let bars = parse_klines(&body).expect("recorded fixture parses");
        let raw_base = body["data"][0][5]
            .as_str()
            .expect("recorded base volume")
            .parse::<f32>()
            .expect("recorded base volume is finite decimal text");
        assert_eq!(
            bars[0].volume, raw_base,
            "{name} keeps its base-volume field"
        );
    }
}

/// `rest/bitget.rs:classify` losing either documented 400 code turns a permanently unknown market
/// into a retryable error, giving the user a retry button that cannot succeed.
#[test]
fn bitget_unknown_symbol_codes_are_permanent() {
    assert_eq!(
        classify(400, &fixture("spot_unknown")),
        Err(FetchError::UnknownSymbol)
    );
    assert_eq!(
        classify(400, &fixture("futures_unknown")),
        Err(FetchError::UnknownSymbol)
    );
}
