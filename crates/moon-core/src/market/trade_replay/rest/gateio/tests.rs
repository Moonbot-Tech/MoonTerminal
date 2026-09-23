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

/// `rest/gateio.rs:parse_spot_row` or `parse_futures_row` reading a base or contract cell for
/// quote turnover makes the chart's money band silently disagree with recorded Gate turnover.
#[test]
fn gate_routes_keep_quote_turnover_separate_from_base_or_contract_units() {
    let spot_body = fixture("spot");
    let spot = parse_spot_klines(&spot_body).expect("spot fixture parses");
    let spot_quote = spot_body[0][1]
        .as_str()
        .expect("spot quote cell")
        .parse::<f32>()
        .expect("finite spot quote");
    assert_eq!(spot[0].quote_volume, spot_quote);

    let futures_body = fixture("futures");
    let futures = parse_futures_klines(&futures_body).expect("futures fixture parses");
    let futures_quote = futures_body[0]["sum"]
        .as_str()
        .expect("futures turnover cell")
        .parse::<f32>()
        .expect("finite futures quote");
    assert_eq!(futures[0].quote_volume, futures_quote);
    assert_eq!(
        futures[0].volume, 0.0,
        "a contract count is never invented as base volume"
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

fn spot_trade(price: &str, amount: &str, side: &str, time_ms: &str) -> serde_json::Value {
    serde_json::json!({
        "price": price,
        "amount": amount,
        "side": side,
        "create_time_ms": time_ms,
        "create_time": 1
    })
}

fn full_spot_page(rows: usize) -> serde_json::Value {
    let row = spot_trade("1", "1", "buy", "1000");
    serde_json::Value::Array(vec![row; rows])
}

/// `rest/gateio.rs:parse_spot_trade_row` preferring `create_time` (seconds) over
/// `create_time_ms` files the print a thousandfold early, and swapping the side arms paints
/// every sell as a buy.
#[test]
fn gate_spot_trade_prefers_the_millisecond_string() {
    let body = serde_json::json!([
        spot_trade("10.5", "2", "sell", "1700000001500"),
        spot_trade("9", "1", "buy", "1700000001600")
    ]);
    let page = parse_spot_trades(&body, 3, None).expect("both rows parse");

    assert_eq!(page.ticks.len(), 2);
    assert_eq!(page.ticks[0].side, Side::Sell);
    assert_eq!(page.ticks[0].price, 10.5);
    assert_eq!(page.ticks[0].qty, 2.0);
    assert_eq!(page.ticks[0].time_ms, 1_700_000_001_500.0);
    assert_eq!(page.ticks[1].side, Side::Buy);
    assert_eq!(page.ticks[1].price, 9.0);
    assert_eq!(page.ticks[1].qty, 1.0);
    assert_eq!(page.ticks[1].time_ms, 1_700_000_001_600.0);
    assert_eq!(
        page.next, None,
        "two rows against a limit of three is a final page"
    );
}

/// `rest/gateio.rs:parse_spot_trades` dropping its raw-row guard returns the rows that parsed
/// and paginates onward, so the chart draws a hole as a complete tape.
#[test]
fn gate_spot_trade_page_rejects_an_unparseable_row() {
    let body = serde_json::json!([
        spot_trade("10", "1", "buy", "1000"),
        {"amount": "1", "side": "sell", "create_time_ms": "2000"}
    ]);

    let parsed = parse_spot_trades(&body, 10, None);
    assert!(
        parsed.is_err(),
        "one malformed sibling must fail the page, got {parsed:?}"
    );
}

/// `rest/gateio.rs:parse_spot_trades` treating an exactly-full page as finished, or stopping one
/// page before Gate's `limit*(page-1) <= 100000` boundary, truncates the tape or asks for a
/// page the venue will refuse.
#[test]
fn gate_spot_trade_page_stops_on_the_documented_row_cap() {
    // 1_000 is Gate spot's real page size. The last legal page is 101, because
    // `1000 * (101 - 1) == 100_000` and `1000 * (102 - 1)` is past the cap.
    let max_rows = 1_000;
    let full = full_spot_page(max_rows);
    let opened = parse_spot_trades(&full, max_rows, None).expect("first page parses");
    assert_eq!(opened.next, Some(TradeCursor::Page(2)));

    let short = parse_spot_trades(
        &full_spot_page(max_rows - 1),
        max_rows,
        Some(TradeCursor::Page(4)),
    )
    .expect("short page parses");
    assert_eq!(short.next, None);

    let at_cap =
        parse_spot_trades(&full, max_rows, Some(TradeCursor::Page(100))).expect("page 100");
    assert_eq!(at_cap.next, Some(TradeCursor::Page(101)));

    let past_cap =
        parse_spot_trades(&full, max_rows, Some(TradeCursor::Page(101))).expect("page 101");
    assert_eq!(past_cap.next, None);

    let ignored =
        parse_spot_trades(&full, max_rows, Some(TradeCursor::Offset(50))).expect("offset");
    assert_eq!(
        ignored.next,
        Some(TradeCursor::Page(2)),
        "a futures offset is not a spot page number"
    );
}

/// `rest/gateio.rs:parse_futures_trade_row` reading the sign backwards, or keeping the signed
/// size as quantity, draws every Gate futures print on the wrong side or with a negative size.
#[test]
fn gate_futures_trade_side_is_the_sign_of_size() {
    let body = serde_json::json!([
        {"price": "10", "size": -4, "create_time_ms": 5_000},
        {"price": 9, "size": "2.5", "create_time_ms": 6_000}
    ]);
    let page = parse_futures_trades(&body, 10, None).expect("both rows parse");

    assert_eq!(page.ticks.len(), 2);
    assert_eq!(page.ticks[0].side, Side::Sell);
    assert_eq!(page.ticks[0].qty, 4.0);
    assert_eq!(page.ticks[0].price, 10.0);
    assert_eq!(page.ticks[0].time_ms, 5_000.0);
    assert_eq!(page.ticks[1].side, Side::Buy);
    assert_eq!(page.ticks[1].qty, 2.5);
    assert_eq!(page.ticks[1].price, 9.0);
    assert_eq!(page.ticks[1].time_ms, 6_000.0);
}

/// `rest/gateio.rs:parse_futures_trade_row` forgetting to scale `create_time` leaves a
/// second-resolution stamp on the millisecond timeline, so the print lands near 1970.
#[test]
fn gate_futures_trade_falls_back_to_second_timestamps() {
    let body = serde_json::json!([
        {"price": "8", "size": 1, "create_time": 1_700_000_000}
    ]);
    let page = parse_futures_trades(&body, 10, None).expect("seconds-only row parses");

    assert_eq!(page.ticks.len(), 1);
    assert_eq!(page.ticks[0].time_ms, 1_700_000_000_000.0);
    assert_eq!(page.ticks[0].side, Side::Buy);
    assert_eq!(page.ticks[0].qty, 1.0);
}

/// `rest/gateio.rs:parse_futures_trades` treating an exactly-full page as complete, or advancing
/// the offset by the requested limit rather than the rows returned, either stops a silently
/// truncated page or skips the next slice.
#[test]
fn gate_futures_trade_full_page_continues_by_row_count() {
    let row = serde_json::json!({"price": "3", "size": 1, "create_time_ms": 10});
    let one = serde_json::json!([row]);
    let exact = parse_futures_trades(&one, 1, Some(TradeCursor::Offset(10))).expect("exact page");
    assert_eq!(exact.next, Some(TradeCursor::Offset(11)));
    assert_eq!(exact.ticks.len(), 1);

    let two = serde_json::json!([
        {"price": "3", "size": 1, "create_time_ms": 10},
        {"price": "4", "size": 2, "create_time_ms": 11}
    ]);
    let over = parse_futures_trades(&two, 1, Some(TradeCursor::Offset(10))).expect("over-full");
    assert_eq!(over.next, Some(TradeCursor::Offset(12)));

    let short = parse_futures_trades(&one, 2, Some(TradeCursor::Offset(10))).expect("short page");
    assert_eq!(short.next, None);
}
