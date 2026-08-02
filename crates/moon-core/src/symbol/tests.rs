use super::{coin_match_key, coin_of_market, strip_contract_suffix};

/// One key, both spellings: the report side and the list side must land on it.
/// The list side and the report side must land on the same keys, deduplicated.
#[test]
fn parse_list_dedups_by_key() {
    let l = super::parse_coin_list("BTC, btc_rp ;; ETH\nmith");
    assert_eq!(l.len(), 3, "BTC and btc_rp are one coin: {l:?}");
    assert!(l.contains("MITH"));
    assert!(super::parse_coin_list("  ").is_empty());
    // A JSON-array spelling must not produce bracketed pseudo-coins.
    let j = super::parse_coin_list("[\"BTC\",\"ETH\"]");
    assert_eq!(j.len(), 2, "{j:?}");
    assert!(j.contains("BTC") && j.contains("ETH"));
}

#[test]
fn match_key_folds_contract_and_case() {
    assert_eq!(coin_match_key(" btc_rp "), "BTC");
    assert_eq!(coin_match_key("BTC"), "BTC");
    assert_eq!(coin_match_key("1kFLOKI"), "1KFLOKI");
}

/// Ground truth from the report replica: 3858 of 4031 distinct coins carry no
/// underscore at all, the rest end in `_RP` or `_MMDD`; the strategy lists hold
/// base tokens.
#[test]
fn contract_suffix_only_known_tails() {
    assert_eq!(strip_contract_suffix("BTC_RP"), "BTC");
    assert_eq!(strip_contract_suffix("LTC_1230"), "LTC");
    // A coin without a contract stays as it is, unusual case included.
    assert_eq!(strip_contract_suffix("1kFLOKI"), "1kFLOKI");
    assert_eq!(strip_contract_suffix("ENA"), "ENA");
    // An unknown tail is part of the name, not a contract — keep it.
    assert_eq!(strip_contract_suffix("FOO_BAR"), "FOO_BAR");
    assert_eq!(
        strip_contract_suffix("FOO_2"),
        "FOO_2",
        "only a 4-digit MMDD"
    );
    assert_eq!(strip_contract_suffix("_RP"), "_RP");
}

#[test]
fn coin_plain() {
    assert_eq!(coin_of_market("ADAUSDT"), "ADA");
    assert_eq!(coin_of_market("BTCUSDT"), "BTC");
    assert_eq!(coin_of_market("VANRY_USDT"), "VANRY");
}

#[test]
fn coin_hip3_strips_dex() {
    assert_eq!(coin_of_market("xyz:BIRD"), "BIRD");
    assert_eq!(coin_of_market("xyz:BIRDUSDC"), "BIRD");
}

/// The chart label. A perpetual reads like a spot pair, because on a futures connection every
/// market is perpetual and a `-SWAP` in every label carries no information; a DATED contract keeps
/// its expiry, because two expiries of one pair are different instruments and a shared label would
/// make the two charts indistinguishable.
#[test]
fn pair_label_hides_perpetual_and_keeps_expiry() {
    assert_eq!(super::display_pair("BTCUSDT"), "BTC-USDT");
    assert_eq!(super::display_pair("BEAT-USDT-SWAP"), "BEAT-USDT");
    assert_eq!(super::display_pair("BTCUSDT-07AUG26"), "BTC-USDT-07AUG26");
    assert_eq!(super::display_pair("BNBUSD_260925"), "BNB-USD-260925");
    // No recognized quote: show the coin alone rather than invent a pair.
    assert_eq!(super::display_pair("KAITO"), "KAITO");
    assert_eq!(super::display_pair("xyz:BIRD"), "BIRD");
}

/// The quote drives the USD conversion, so an unrecognized one must stay empty rather than
/// guess: a wrong quote silently misprices every size label on that market.
#[test]
fn quote_is_canonical_uppercase_or_empty() {
    assert_eq!(super::resolve_quote("BEAT-USDT-SWAP"), "USDT");
    assert_eq!(super::resolve_quote("btcusdt"), "USDT");
    assert_eq!(super::resolve_quote("KAITO"), "");
    assert_eq!(super::resolve_quote("@206"), "");
}
