use super::*;

use serde_json::json;

/// Equal entry/exit stamps must form a real HTTP window without losing the boundary print.
#[test]
fn collapsed_fill_slice_builds_outward_bounds_on_both_routes() {
    use crate::market::trade_replay::{ReplayIntent, ReplayWindow, tick_plan};
    use std::io::{Read, Write};
    let agent = ureq::Agent::new_with_defaults();
    for route in [TradeRoute::BitgetSpotFills, TradeRoute::BitgetMixFills] {
        let plan = tick_plan(
            ReplayWindow {
                from_ms: 5_000,
                to_ms: 5_000,
                open_ms: 5_000,
                close_ms: 5_000,
                margin_ms: 0,
                long_position_ms: 60_000,
                over_budget: false,
            },
            route,
            None,
            ReplayIntent::Chart,
        );
        assert_eq!(plan.slices, vec![(5_000, 5_000)]);
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(std::time::Duration::from_secs(5)))
                .unwrap();
            let mut request = Vec::new();
            let mut buf = [0; 1024];
            while !request.windows(4).any(|part| part == b"\r\n\r\n") {
                let count = stream.read(&mut buf).unwrap();
                assert!(count > 0);
                request.extend_from_slice(&buf[..count]);
            }
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}")
                .unwrap();
            String::from_utf8(request).unwrap()
        });
        let (from, to) = plan.slices[0];
        fills_request(
            &agent,
            route,
            "SYNUSDT",
            from,
            to,
            Some(TradeCursor::LessThanId(101)),
        )
        .uri(url)
        .call()
        .unwrap();
        let query = server.join().unwrap();
        assert!(query.contains("startTime=4999"), "{query}");
        assert!(query.contains("endTime=5001"), "{query}");
        assert!(query.contains("idLessThan=101"), "{query}");
    }
}

/// Status-first classification must not hide unknown symbols or turn bad bounds into outages.
#[test]
fn fills_classification_preserves_symbols_throttles_and_service_errors() {
    assert_eq!(
        classify_fills(400, &json!({"code":"40034"})),
        Err(FetchError::UnknownSymbol)
    );
    for status in [200, 400] {
        assert!(
            matches!(classify_fills(status, &json!({"code":"40017", "msg":"invalid startTime"})), Err(FetchError::RequestFault(text)) if text.contains("invalid startTime"))
        );
    }
    assert!(matches!(
        classify_fills(400, &json!({})),
        Err(FetchError::RequestFault(_))
    ));
    assert!(matches!(
        classify_fills(500, &json!({"code":"40034"})),
        Err(FetchError::Transient(_))
    ));
    for code in ["40010", "40725", "45001"] {
        assert!(matches!(
            classify_fills(400, &json!({"code": code})),
            Err(FetchError::Transient(_))
        ));
    }
    for status in [418, 429] {
        assert!(matches!(
            classify_fills(status, &json!({"code":"40034"})),
            Err(FetchError::Throttled { .. })
        ));
    }
}

/// Completing on equality drops older fill IDs at the entry millisecond. Only crossing the
/// bound or exhausting a short page proves that every boundary sibling was fetched.
#[test]
fn full_boundary_pages_continue_until_entry_siblings_are_exhausted() {
    let row =
        |id, stamp| json!({"tradeId": id, "ts": stamp, "price": "10", "size": "1", "side": "buy"});
    let first = parse_fills(&json!({"data": [row("102", "5000")]}), 1, 5_000).unwrap();
    assert_eq!(first.next, Some(TradeCursor::LessThanId(102)));
    let sibling = parse_fills(&json!({"data": [row("101", "5000")]}), 1, 5_000).unwrap();
    assert_eq!(sibling.next, Some(TradeCursor::LessThanId(101)));
    let crossed = parse_fills(&json!({"data": [row("100", "4999")]}), 1, 5_000).unwrap();
    assert_eq!(crossed.next, None);
    let exhausted = parse_fills(&json!({"data": [row("101", "5000")]}), 2, 5_000).unwrap();
    assert_eq!(exhausted.next, None);
}

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
