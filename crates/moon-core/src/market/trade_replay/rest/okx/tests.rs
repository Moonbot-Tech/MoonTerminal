use super::*;

/// Sending the inclusive end as an exclusive timestamp drops all exit-ms ticks; completing on
/// equality at the left edge also loses siblings when a same-ms scalp takes multiple pages.
#[test]
fn initial_anchor_includes_exit_and_id_pages_exhaust_same_millisecond() {
    let trades = [(103u64, 5_001i64), (102, 5_000), (101, 5_000), (100, 4_999)];
    let mut cursor = None;
    let mut retained = Vec::new();
    for _ in 0..3 {
        let (kind, after) = trade_anchor(5_000, cursor);
        let bound = after.parse::<u64>().unwrap();
        let &(id, ts) = trades
            .iter()
            .find(|(id, ts)| match kind {
                "2" => (*ts as u64) < bound,
                "1" => *id < bound,
                _ => panic!("invalid pagination type"),
            })
            .expect("fake exclusive-bound server has a matching row");
        let page = parse_history_trades(
            &serde_json::json!({"data": [{
                "tradeId": id.to_string(), "ts": ts.to_string(),
                "px": "10", "sz": "1", "side": "buy"
            }]}),
            1,
            5_000,
        )
        .unwrap();
        retained.extend(
            page.ticks
                .iter()
                .filter(|t| t.time_ms == 5_000.0)
                .map(|_| id),
        );
        cursor = page.next;
        if cursor.is_none() {
            break;
        }
    }
    assert_eq!(
        retained,
        vec![102, 101],
        "both exit-ms siblings must survive, without the later trade"
    );
    assert_eq!(
        cursor, None,
        "only an older timestamp proves the scalp fully fetched"
    );
}

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
