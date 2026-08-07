use super::follows_setting;

/// The selector's whole point is narrowing to ONE order on ONE core: the run that found this bug had
/// 22 cores and a hundred churning orders each, and a market-only filter buried the one being
/// watched. Dropping the `/` split would silently widen it back to every core.
#[test]
fn a_core_qualified_selector_narrows_to_that_core() {
    assert!(follows_setting("GateF/BTC", "GateF", "BTC_USDT"));
    assert!(!follows_setting("GateF/BTC", "BB1", "BTC_USDT"));
    assert!(!follows_setting("GateF/BTC", "GateF", "KGENUSDT"));
}

/// Both halves are substrings, and case must not matter: the core is written `GateF` in one place
/// and `gatef` in another, and a market is `BTC_USDT` here, `BTCUSDT` there.
#[test]
fn both_halves_match_case_insensitively_as_substrings() {
    assert!(follows_setting("gatef/btc", "GateF", "BTC_USDT"));
    assert!(follows_setting("BTC", "anything", "BTCUSDT"));
    assert!(follows_setting("KGEN", "BB1", "KGENUSDT"));
    assert!(!follows_setting("KGEN", "BB1", "BTCUSDT"));
}

/// `1` is the "everything" switch, and an empty half means "any" on its side — following one core's
/// entire order flow is `Core/`, which is what to reach for when the market name is not yet known.
#[test]
fn one_follows_everything_and_an_empty_half_follows_its_whole_side() {
    assert!(follows_setting("1", "any core", "any market"));
    assert!(follows_setting("GateF/", "GateF", "anything at all"));
    assert!(!follows_setting("GateF/", "BB1", "anything at all"));
    assert!(follows_setting("/BTC", "any core", "BTC_USDT"));
}
