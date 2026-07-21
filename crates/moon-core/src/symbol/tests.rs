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
