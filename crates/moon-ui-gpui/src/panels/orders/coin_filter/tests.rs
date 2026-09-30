use super::*;

#[test]
fn empty_or_blank_query_keeps_every_row() {
    assert!(matches_coin("BTC", ""));
    assert!(matches_coin("BTC", "   "));
}

#[test]
fn substring_matches_in_any_case() {
    assert!(matches_coin("SOL", "sol"));
    assert!(matches_coin("1000SOL", "Sol"));
    assert!(matches_coin("ETH", " eth "));
}

#[test]
fn a_non_matching_or_longer_query_drops_the_row() {
    assert!(!matches_coin("BTC", "eth"));
    assert!(!matches_coin("BT", "btc"));
}

#[test]
fn non_ascii_query_never_matches_an_ascii_token() {
    assert!(!matches_coin("BTC", "бтц"));
}
