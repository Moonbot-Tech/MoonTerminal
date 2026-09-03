use super::*;

use serde_json::json;

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

/// `rest/bitget.rs:parse_row` taking cell 5 for quote turnover makes both recorded row shapes
/// show base quantity in a money-denominated chart band.
#[test]
fn bitget_reads_quote_turnover_from_cell_six_on_both_row_shapes() {
    for name in ["spot", "futures"] {
        let body = fixture(name);
        let bars = parse_klines(&body).expect("recorded fixture parses");
        let expected = body["data"][0][6]
            .as_str()
            .expect("quote turnover cell")
            .parse::<f32>()
            .expect("finite quote turnover");
        assert_eq!(
            bars[0].quote_volume, expected,
            "{name} preserves its quote-turnover cell"
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

/// `rest/bitget.rs:parse_fills` treating a full, uncovered page with an unparseable oldest
/// `tradeId` as complete silently ships a truncated tick window as a complete chart.
#[test]
fn bitget_rejects_a_full_uncovered_page_without_a_cursor() {
    let body = json!({
        "data": [
            {"price": "100", "size": "1", "ts": "2000", "side": "buy", "tradeId": "101"},
            {"price": "99", "size": "1", "ts": "1000", "side": "sell", "tradeId": "not-an-id"}
        ]
    });

    assert!(
        parse_fills(&body, 2, 0).is_err(),
        "an uncovered full page cannot be complete when its continuation cursor is unknowable"
    );
}

/// `rest/bitget.rs:parse_fills` dropping its raw-row guard accepts a partly malformed page and
/// draws a gap-ridden tick series as complete market history.
#[test]
fn bitget_rejects_a_page_with_any_unparseable_row() {
    let body = json!({
        "data": [
            {"price": "100", "size": "1", "ts": "2000", "side": "buy", "tradeId": "101"},
            {"size": "1", "ts": "1000", "side": "sell", "tradeId": "100"}
        ]
    });

    assert!(
        parse_fills(&body, 2, 0).is_err(),
        "a response with a malformed row is incomplete even when another row parsed"
    );
}
