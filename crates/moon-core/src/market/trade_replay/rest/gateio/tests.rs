use super::*;

fn fixture(name: &str) -> Value {
    let text = match name {
        "spot" => include_str!("fixtures/spot_klines.json"),
        "futures" => include_str!("fixtures/futures_klines.json"),
        "spot_unknown" => include_str!("fixtures/spot_unknown_symbol.json"),
        "futures_unknown" => include_str!("fixtures/futures_unknown_symbol.json"),
        "spot_trades" => include_str!("fixtures/spot_trades.json"),
        "futures_trades" => include_str!("fixtures/futures_trades.json"),
        "futures_trades_zero_size" => include_str!("fixtures/futures_trades_zero_size.json"),
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

    let ignored = parse_spot_trades(
        &full,
        max_rows,
        Some(TradeCursor::Before {
            boundary_ms: 50,
            below_id: 50,
        }),
    )
    .expect("futures cursor");
    assert_eq!(
        ignored.next,
        Some(TradeCursor::Page(2)),
        "a futures cursor is not a spot page number"
    );
}

/// `rest/gateio.rs:parse_futures_trade_row` reading the sign backwards, or keeping the signed
/// size as quantity, draws every Gate futures print on the wrong side or with a negative size.
#[test]
fn gate_futures_trade_side_is_the_sign_of_size() {
    // Futures `create_time_ms` is fractional SECONDS (see the recorded case below).
    let body = serde_json::json!([
        {"price": "10", "size": -4, "create_time_ms": 5.0},
        {"price": 9, "size": "2.5", "create_time_ms": 6.0}
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

/// `rest/gateio.rs:parse_futures_trades` treating an exactly-full page as complete, or pinning
/// the next page to a row other than the oldest one it holds, either stops a silently
/// truncated page or skips the prints between the two rows.
#[test]
fn gate_futures_trade_full_page_continues_by_row_count() {
    let row = serde_json::json!({"price": "3", "size": 1, "create_time_ms": 10.0});
    let one = serde_json::json!([row]);
    let exact = parse_futures_trades(&one, 1, Some(TradeCursor::Offset(10))).expect("exact page");
    assert_eq!(exact.next, Some(TradeCursor::Offset(11)));
    assert_eq!(exact.ticks.len(), 1);

    // Newest first, as the venue answers.
    let two = serde_json::json!([
        {"price": "3", "size": 1, "create_time_ms": 10.0},
        {"price": "4", "size": 2, "create_time_ms": 11.0}
    ]);
    let over = parse_futures_trades(&two, 1, None).expect("over-full");
    assert_eq!(
        over.next,
        Some(TradeCursor::Before {
            boundary_ms: 10_000,
            below_id: 7
        })
    );

    let short = parse_futures_trades(&one, 2, None).expect("short page");
    assert_eq!(short.next, None);
}

/// `rest/gateio.rs:parse_futures_trade_row` reading `create_time_ms` as an integer of
/// milliseconds rejects every row Gate futures actually sends — the field is a fractional
/// NUMBER of SECONDS there (`1789726954.306`), unlike spot's millisecond STRING — so a whole page
/// comes back "unparseable", the walk files it as a venue refusal and the host backs off for
/// minutes on a request the venue answered in 300 ms. Recorded on 2026-09-21 from
/// `/futures/usdt/trades?contract=CATE_USDT`, the market from that day's refused batch.
#[test]
fn gate_futures_trades_carry_fractional_seconds_not_integer_milliseconds() {
    let body = fixture("futures_trades");
    let page = parse_futures_trades(&body, 1_000, None).expect("recorded futures page parses");

    assert_eq!(page.ticks.len(), 3, "every recorded row is a tick");
    assert_eq!(page.ticks[0].time_ms, 1_789_726_954_306.0);
    assert_eq!(page.ticks[0].price, 0.09058);
    assert_eq!(page.ticks[0].qty, 1.0);
    assert_eq!(page.ticks[0].side, Side::Sell);
    assert_eq!(page.ticks[1].side, Side::Buy);
    assert_eq!(
        page.next, None,
        "three rows against a cap of 1000 is a final page"
    );
}

/// `rest/gateio.rs:parse_spot_trade_row` is the OTHER shape of the same brand: a millisecond
/// string. Pinned beside the futures case so a "fix" that unifies the two parsers on one unit
/// breaks here rather than on a live chart.
#[test]
fn gate_spot_trades_carry_a_millisecond_string() {
    let body = fixture("spot_trades");
    let page = parse_spot_trades(&body, 1_000, None).expect("recorded spot page parses");

    assert_eq!(page.ticks.len(), 3);
    assert_eq!(page.ticks[0].time_ms, 1_789_726_954_886.294);
    assert_eq!(page.ticks[0].qty, 66.0);
    assert_eq!(page.ticks[0].side, Side::Sell);
    assert_eq!(page.ticks[2].side, Side::Buy);
}

/// `#[ignore]` on purpose: it asks the LIVE Gate futures endpoint, so it is a probe to run by
/// hand when the recorded fixture and the venue may have parted ways — the parser above is
/// pinned to a recording, and a recording cannot notice the venue changing its row shape. The
/// window is the recorded one; Gate keeps futures trades for months, so it stays answerable.
#[test]
#[ignore = "probe: asks the live Gate futures trades endpoint"]
fn gate_futures_trades_live_page_parses_end_to_end() {
    let agent = super::super::agent();
    // CATE: the fractional-seconds recording; UB: the market that prints `size: 0` rows
    // between its fills (both recorded 2026-09-21).
    for (market, from_ms, to_ms) in [
        ("CATE_USDT", 1_789_726_772_000, 1_789_726_957_000),
        ("UB_USDT", 1_789_899_637_000, 1_789_899_702_000),
    ] {
        let page = super::super::fetch_trades(
            &agent,
            TradeRoute::GateFuturesTrades,
            market,
            from_ms,
            to_ms,
            None,
        )
        .unwrap_or_else(|e| panic!("live Gate futures page for {market} parses: {e:?}"));

        assert!(
            !page.ticks.is_empty(),
            "{market}: the recorded window held prints"
        );
        for tick in &page.ticks {
            let t = tick.time_ms as i64;
            assert!(
                (from_ms..=to_ms).contains(&t),
                "{market}: tick at {t} ms sits inside the asked window {from_ms}..{to_ms}"
            );
        }
    }
}

/// `rest/gateio.rs:trade_window_seconds` truncating `to` asks Gate for prints up to the slice's
/// last WHOLE second: a print at `…954.306` is not returned for `to=…954` (probed live on both
/// routes), so the slice's final partial second goes unasked while the walk marks it covered.
#[test]
fn gate_trade_window_asks_one_second_past_the_slice_end() {
    assert_eq!(
        trade_window_seconds(1_789_726_772_454, 1_789_726_954_306),
        (1_789_726_772, 1_789_726_955)
    );
    // A second-aligned end still widens by one: whether Gate's `to` is inclusive at `.000`
    // is not something the walk should depend on.
    assert_eq!(
        trade_window_seconds(1_789_726_772_000, 1_789_726_954_000),
        (1_789_726_772, 1_789_726_955)
    );
}

/// `rest/gateio.rs:parse_futures_trades` counting a `size: 0` row as unparseable turns a page
/// the venue served in full into a `Transient` refusal, and the host backs off for minutes on
/// a market where every other row prints that way. Recorded on 2026-09-21 from
/// `/futures/usdt/trades?contract=UB_USDT`: rows with their own ids and a price but no size,
/// alternating with ordinary fills. Nothing was traded in such a row, so it is not a tick —
/// but it is the venue's well-formed answer, and the page stands.
#[test]
fn gate_futures_trades_skip_a_zero_size_row_without_refusing_the_page() {
    let body = fixture("futures_trades_zero_size");
    let page = parse_futures_trades(&body, 1_000, None).expect("a page with zero-size rows parses");

    assert_eq!(
        page.ticks.len(),
        3,
        "the three fills; the three zero-size rows are no fills"
    );
    assert_eq!(
        page.ticks.iter().map(|t| t.qty).collect::<Vec<_>>(),
        vec![1.0, 2.0, 8.0]
    );
    assert_eq!(page.next, None);
}

/// `rest/gateio.rs:parse_futures_trades` on a FULL page of nothing but `size: 0` rows: the
/// route never accepts a full page as complete, so the cursor must still move onto the page's
/// oldest row — a dead stretch on a small contract is walked through, not refused and not
/// mistaken for the end of the tape.
#[test]
fn gate_futures_page_of_only_zero_size_rows_is_empty_and_still_pages_on() {
    let mut body = fixture("futures_trades_zero_size");
    for row in body.as_array_mut().expect("array") {
        row["size"] = serde_json::json!(0);
    }
    let rows = body.as_array().expect("array");
    let oldest_id = rows
        .iter()
        .map(|r| r["id"].as_u64().unwrap())
        .min()
        .unwrap();
    let page = parse_futures_trades(
        &body,
        6,
        Some(TradeCursor::Before {
            boundary_ms: i64::MAX,
            below_id: u64::MAX,
        }),
    )
    .expect("a page of zero-size rows parses");
    assert!(page.ticks.is_empty());
    assert!(
        matches!(page.next, Some(TradeCursor::Before { below_id, .. }) if below_id == oldest_id)
    );
}

/// A full page hands back a `Before` cursor at its OLDEST row (time and id), and the next page
/// — asked up to that row's second, so it holds that second again — drops every row at or
/// above that id and keeps the rest. Three rows against a cap of three is a full page.
#[test]
fn gate_futures_full_page_pages_back_by_time_and_drops_the_rows_already_taken() {
    let body = fixture("futures_trades");
    let page = parse_futures_trades(&body, 3, None).expect("full page parses");
    assert_eq!(page.ticks.len(), 3);
    let Some(TradeCursor::Before {
        boundary_ms,
        below_id,
    }) = page.next
    else {
        panic!("a full page pages on: {:?}", page.next);
    };
    assert_eq!(boundary_ms, 1_789_726_950_929, "the oldest row's stamp");
    assert_eq!(below_id, 29202, "the oldest row's id");
    // The next page, as the venue answers `to=1789726951`: the two rows of that second again
    // (ids 29203, 29202 — already held) plus one older row.
    let next_body = serde_json::json!([
        {"id": 29203, "contract": "CATE_USDT", "create_time": 1789726950.929, "create_time_ms": 1789726950.929, "size": 1, "price": "0.09102"},
        {"id": 29202, "contract": "CATE_USDT", "create_time": 1789726950.929, "create_time_ms": 1789726950.929, "size": 1, "price": "0.09102"},
        {"id": 29201, "contract": "CATE_USDT", "create_time": 1789726948.100, "create_time_ms": 1789726948.100, "size": -2, "price": "0.09100"}
    ]);
    let next = parse_futures_trades(&next_body, 3, page.next).expect("next page parses");
    assert_eq!(
        next.ticks.len(),
        1,
        "only the row below the id already held"
    );
    assert_eq!(next.ticks[0].time_ms, 1_789_726_948_100.0);
    assert!(
        matches!(
            next.next,
            Some(TradeCursor::Before {
                below_id: 29201,
                boundary_ms: 1_789_726_948_100
            })
        ),
        "{:?}",
        next.next
    );
    // A full page whose every row is already held is a second denser than a page: the walk
    // drains that second by offset, from its start.
    let dense = serde_json::json!([
        {"id": 29203, "contract": "CATE_USDT", "create_time": 1789726950.929, "create_time_ms": 1789726950.929, "size": 1, "price": "0.09102"},
        {"id": 29202, "contract": "CATE_USDT", "create_time": 1789726950.929, "create_time_ms": 1789726950.929, "size": 1, "price": "0.09102"},
        {"id": 29204, "contract": "CATE_USDT", "create_time": 1789726954.306, "create_time_ms": 1789726954.306, "size": -1, "price": "0.09058"}
    ]);
    let drain = parse_futures_trades(&dense, 3, page.next).expect("parses");
    assert!(drain.ticks.is_empty());
    assert_eq!(
        drain.next,
        Some(TradeCursor::Within {
            second_s: 1_789_726_950,
            offset: 0,
            below_id: 29202,
            low_id: 29202
        })
    );
    // A full drain page moves the offset by the venue's row count and keeps only the new
    // rows; the boundary id follows the oldest new row.
    let drain_page = serde_json::json!([
        {"id": 29202, "contract": "CATE_USDT", "create_time": 1789726950.929, "create_time_ms": 1789726950.929, "size": 1, "price": "0.09102"},
        {"id": 29200, "contract": "CATE_USDT", "create_time": 1789726950.500, "create_time_ms": 1789726950.500, "size": 3, "price": "0.09101"},
        {"id": 29199, "contract": "CATE_USDT", "create_time": 1789726950.400, "create_time_ms": 1789726950.400, "size": 3, "price": "0.09101"}
    ]);
    let drained = parse_futures_trades(&drain_page, 3, drain.next).expect("parses");
    assert_eq!(drained.ticks.len(), 2);
    assert_eq!(
        drained.next,
        Some(TradeCursor::Within {
            second_s: 1_789_726_950,
            offset: 3,
            below_id: 29202,
            low_id: 29199
        }),
        "the boundary stays, the low id follows the drain"
    );
    // Oldest-first inside the second would not lose a row: the boundary does not follow.
    let ascending = serde_json::json!([
        {"id": 29195, "contract": "CATE_USDT", "create_time": 1789726950.050, "create_time_ms": 1789726950.050, "size": 1, "price": "0.09100"},
        {"id": 29196, "contract": "CATE_USDT", "create_time": 1789726950.060, "create_time_ms": 1789726950.060, "size": 1, "price": "0.09100"},
        {"id": 29197, "contract": "CATE_USDT", "create_time": 1789726950.070, "create_time_ms": 1789726950.070, "size": 1, "price": "0.09100"}
    ]);
    let asc = parse_futures_trades(&ascending, 3, drained.next).expect("parses");
    assert_eq!(asc.ticks.len(), 3);
    // A short drain page ends the second: back to time, up to the second's own start, so the
    // next `to` is the second itself and the rows of it are not asked a third time.
    let tail = serde_json::json!([
        {"id": 29198, "contract": "CATE_USDT", "create_time": 1789726950.100, "create_time_ms": 1789726950.100, "size": 1, "price": "0.09100"}
    ]);
    let ended = parse_futures_trades(&tail, 3, drained.next).expect("parses");
    assert_eq!(ended.ticks.len(), 1);
    assert_eq!(
        ended.next,
        Some(TradeCursor::Before {
            boundary_ms: 1_789_726_949_999,
            below_id: 29198
        })
    );
    assert_eq!(trade_window_seconds(0, 1_789_726_949_999).1, 1_789_726_950);
    // On a continuation page a row without an id is dropped, not taken again.
    let no_id = serde_json::json!([
        {"contract": "CATE_USDT", "create_time": 1789726948.100, "create_time_ms": 1789726948.100, "size": -2, "price": "0.09100"}
    ]);
    let short = parse_futures_trades(&no_id, 3, page.next).expect("parses");
    assert!(short.ticks.is_empty());
    assert_eq!(short.next, None);
    let first = parse_futures_trades(&no_id, 3, None).expect("parses");
    assert_eq!(first.ticks.len(), 1, "a first page keeps it");
}

/// `rest/gateio.rs:fetch_trades` sends the cursor's boundary as `to`, one second past the
/// boundary's own second, and never an `offset`.
#[test]
fn gate_futures_before_cursor_moves_to_onto_the_boundary_second() {
    // `trade_window_seconds` is the one rule for `to`; the cursor reuses it on its boundary.
    let (_, to) = trade_window_seconds(0, 1_789_726_950_929);
    assert_eq!(to, 1_789_726_951);
}
