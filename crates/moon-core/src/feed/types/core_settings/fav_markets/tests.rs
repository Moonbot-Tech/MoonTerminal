use super::*;

/// The marked-markets field is ONE comma-separated string, and an empty one names nothing.
///
/// Breakage this pins: a bare `split(',')` that answers `[""]` for an empty field, which reads as a
/// list holding one nameless market — and would draw a row for it.
#[test]
fn an_empty_fav_field_names_nothing() {
    assert!(fav_markets_list("").is_empty());
    assert!(fav_markets_list("  ").is_empty());
    assert_eq!(fav_markets_list("BTCUSDT"), vec!["BTCUSDT"]);
    // Separator noise is the separator's, not the ticker's.
    assert_eq!(
        fav_markets_list(" BTCUSDT , ETHUSDT ,"),
        vec!["BTCUSDT", "ETHUSDT"]
    );
}

/// Membership is case-insensitive, the rule every other symbol comparison here uses.
#[test]
fn a_market_is_found_whatever_case_the_core_wrote() {
    assert!(fav_markets_has("BTCUSDT,ETHUSDT", "btcusdt"));
    assert!(!fav_markets_has("BTCUSDT,ETHUSDT", "SOLUSDT"));
    assert!(!fav_markets_has("", "BTCUSDT"));
}

/// Setting a market is ABSOLUTE and touches one name, leaving the others as the core spelled them.
///
/// Breakage this pins: rebuilding the list from a normalized copy. The core wrote those names —
/// respelling one while changing another is an edit nobody asked for, and it travels back over the
/// wire as though the trader had made it.
#[test]
fn setting_one_market_leaves_the_other_names_verbatim() {
    assert_eq!(
        fav_markets_set("btcUSDT,ETHUSDT", "SOLUSDT", true),
        "btcUSDT,ETHUSDT,SOLUSDT"
    );
    assert_eq!(
        fav_markets_set("btcUSDT,ETHUSDT,SOLUSDT", "solusdt", false),
        "btcUSDT,ETHUSDT",
        "the same market in another case is the same market"
    );
    assert_eq!(fav_markets_set("", "BTCUSDT", true), "BTCUSDT");
    // Already in that state: nothing to say, and nothing added twice.
    assert_eq!(fav_markets_set("BTCUSDT", "btcusdt", true), "BTCUSDT");
    assert_eq!(fav_markets_set("BTCUSDT", "ETHUSDT", false), "BTCUSDT");
    // A blank name would grow a separator per press and never be found again.
    assert_eq!(fav_markets_set("BTCUSDT", "  ", true), "BTCUSDT");
}
