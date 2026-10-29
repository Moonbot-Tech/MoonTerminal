use super::CoinMarkets;

#[test]
fn a_coin_is_searched_once_while_the_inputs_hold() {
    let mut markets = CoinMarkets::default();
    for _ in 0..3 {
        let answer = markets.resolve(|| 7_u64, "ACE", || Some("ACEUSDT".to_string()));
        assert_eq!(answer.as_deref(), Some("ACEUSDT"));
    }
    assert_eq!(markets.searched, 1);
}

/// A refusal is kept under the version it was read at, and the market that appears with a newer
/// version is found: caching "not found" without the version in the inputs would hide it.
#[test]
fn a_not_found_holds_until_the_catalog_version_moves() {
    let mut markets = CoinMarkets::default();
    assert_eq!(markets.resolve(|| 1_u64, "NEW", || None), None);
    assert_eq!(markets.resolve(|| 1_u64, "NEW", || panic!("kept")), None);
    let answer = markets.resolve(|| 2_u64, "NEW", || Some("NEWUSDT".to_string()));
    assert_eq!(answer.as_deref(), Some("NEWUSDT"));
    assert_eq!(markets.searched, 2);
}

#[test]
fn a_market_found_under_older_inputs_is_searched_again() {
    let mut markets = CoinMarkets::default();
    markets.resolve(|| 1_u64, "ACE", || Some("ACEUSDT".to_string()));
    let answer = markets.resolve(|| 2_u64, "ACE", || Some("ACEUSDC".to_string()));
    assert_eq!(answer.as_deref(), Some("ACEUSDC"));
    assert_eq!(markets.searched, 2);
}
