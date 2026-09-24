use serde_json::json;

use super::*;

/// `rest/mod.rs:split_no_fill` reading a zero only as a JSON number, or only as text, lets one
/// venue's spelling of "nothing traded" back into the hole check as a malformed row — and the
/// page it sits on is refused with a host backoff, the outage of 2026-09-21 on Gate futures.
#[test]
fn split_no_fill_reads_every_spelling_of_zero_and_keeps_the_rest() {
    let rows = vec![
        json!({"q": 0}),
        json!({"q": "0"}),
        json!({"q": "0.000"}),
        json!({"q": -0.0}),
        json!({"q": "1.5"}),
        json!({"q": 2}),
        // No quantity at all, or text that is not a number: not a zero, so it stays for the
        // row parser to reject — a malformed row must still count as one.
        json!({"p": "1"}),
        json!({"q": "n/a"}),
    ];
    let (fills, no_fill) = split_no_fill(&rows, "q");
    assert_eq!(no_fill, 4);
    assert_eq!(fills.len(), 4);
    assert_eq!(fills[0]["q"], json!("1.5"));
    assert_eq!(fills[1]["q"], json!(2));
}

/// `rest/binance.rs:parse_agg_trades` refusing a page over a zero-quantity row sends the host
/// into a backoff for a row that holds no fill — the same trap Gate futures sprang, on the one
/// route whose docs never mention a zero either. Binance stands in for the routes that share
/// the split; the Gate futures fixture pins the venue it was recorded on.
#[test]
fn binance_skips_a_zero_quantity_row_and_still_refuses_a_malformed_one() {
    let row = |id: u64, qty: &str| json!({"a": id, "T": 5_000, "p": "10", "q": qty, "m": false});
    let page =
        binance::parse_agg_trades(&json!([row(1, "1"), row(2, "0"), row(3, "2")]), 5_000, 10)
            .expect("a zero-quantity row is no fill, not a refusal");
    assert_eq!(page.ticks.len(), 2);
    let malformed = binance::parse_agg_trades(&json!([row(1, "1"), row(2, "abc")]), 5_000, 10);
    assert!(
        matches!(malformed, Err(FetchError::Transient(ref text)) if text.contains("1 unparseable")),
        "{malformed:?}"
    );
    // Nothing but zero rows on a full page: an empty page whose cursor still moves on.
    let dead = json!([row(1, "0"), row(2, "0")]);
    let page = binance::parse_agg_trades(&dead, 5_000, 2).expect("a dead page parses");
    assert!(page.ticks.is_empty());
    assert!(
        page.next.is_some(),
        "the venue's row count, not the fill count, fills a page"
    );
}
