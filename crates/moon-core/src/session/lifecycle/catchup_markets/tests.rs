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

#[test]
fn a_not_found_is_searched_again_under_the_same_inputs() {
    let mut markets = CoinMarkets::default();
    assert_eq!(markets.resolve(|| 1_u64, "NEW", || None), None);
    let answer = markets.resolve(|| 1_u64, "NEW", || Some("NEWUSDT".to_string()));
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
