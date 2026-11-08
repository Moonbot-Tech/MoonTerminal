//! The post-read filter and the reason classes, against texts a real replica holds.

use super::*;

/// Every text of the class table checked on a year of a replica lands in its class.
#[test]
fn reasons_fold_into_their_classes() {
    let cases = [
        ("Auto Price Down", Reason::AutoPriceDown),
        ("StopLoss", Reason::Stoploss),
        ("StopLoss by Fast", Reason::Stoploss),
        ("Sell Price", Reason::SellPrice),
        ("TrailingStop 0.5%", Reason::TrailingStop),
        ("Manual PanicSell", Reason::Manual),
        ("Global PanicSell", Reason::Manual),
        ("Funding", Reason::Funding),
        ("LIQUIDATION", Reason::Liquidation),
        ("SellFromAssets", Reason::Other),
        ("Sell by FilterCheck: Delta", Reason::Other),
    ];
    for (text, class) in cases {
        assert_eq!(Reason::classify(text), class, "{text}");
    }
}

/// Regression target: parsing an id above 2^63 as `i64`, which the report stores negative.
#[test]
fn a_strategy_id_above_two_to_the_63_keeps_its_bits() {
    let filter = TradeFilter {
        strategy_ids: vec!["18446744073709551615".into(), "0".into()],
        ..Default::default()
    };
    assert_eq!(strategy_ids(&filter).unwrap(), vec![-1, 0]);
    let bad = TradeFilter {
        strategy_ids: vec!["-5".into()],
        ..Default::default()
    };
    assert!(strategy_ids(&bad).is_err());
}

/// `%` and `_` behave as in SQL, on characters rather than bytes.
#[test]
fn coin_like_matches_like_sql() {
    let p = |s: &str| s.chars().collect::<Vec<_>>();
    assert!(like(&p("JCT%"), "JCTUSDT"));
    assert!(like(&p("%USDT"), "JCTUSDT"));
    assert!(like(&p("J_T"), "JCT"));
    assert!(!like(&p("J_T"), "JCCT"));
    assert!(like(&p("%"), ""));
    assert!(!like(&p("A%B"), "AC"));
}

/// Regression target: a coin filter comparing raw names, which loses `JCT_RP` and `JCT_0925`.
#[test]
fn a_coin_filter_finds_contract_tails() {
    let filter = TradeFilter {
        coins: vec!["jct".into()],
        ..Default::default()
    };
    let residual = Residual::new(&filter, &[]).unwrap();
    assert!(residual.keeps(&Trade::sample(1, 7, "JCT_RP", 1, 1.0)));
    assert!(!residual.keeps(&Trade::sample(1, 7, "ACE", 1, 1.0)));
}

/// An unvalued trade never satisfies a USDT bound: unknown is not zero.
#[test]
fn an_unvalued_trade_fails_a_usdt_bound() {
    let filter = TradeFilter {
        profit_usdt_max: Some(10.0),
        ..Default::default()
    };
    let residual = Residual::new(&filter, &[]).unwrap();
    let mut trade = Trade::sample(1, 7, "ACE", 1, 1.0);
    assert!(residual.keeps(&trade));
    trade.profit_usdt = None;
    assert!(!residual.keeps(&trade));
}

/// The name at the time of the trade matches too: a renamed strategy keeps its history.
#[test]
fn a_strategy_name_matches_the_name_at_trade() {
    let filter = TradeFilter {
        strategy_name: Some("MainShot".into()),
        ..Default::default()
    };
    let residual = Residual::new(&filter, &[]).unwrap();
    let mut trade = Trade::sample(1, 7, "ACE", 1, 1.0);
    assert!(!residual.keeps(&trade));
    trade.name_at_trade = Some(std::sync::Arc::from("MainShotL"));
    assert!(residual.keeps(&trade));
}

/// Both stored shapes of `channelname` give the strategy name.
#[test]
fn the_name_at_trade_is_read_from_both_channel_shapes() {
    use super::super::trade::name_in_channel;
    assert_eq!(
        name_in_channel("Bot (strategy <MainShotL>)"),
        Some("MainShotL")
    );
    assert_eq!(name_in_channel("JoinKey: 17 <USDTPump>"), Some("USDTPump"));
    assert_eq!(name_in_channel("no name here"), None);
    assert_eq!(name_in_channel("<>"), None);
}
