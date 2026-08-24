use super::*;

/// The selector is tested on a string, never through the process-wide switch: every other test in
/// the binary shares that switch, and setting it here would make them race.
#[test]
fn an_entry_matches_any_spelling_of_the_market() {
    // Bybit's `1000BONKPERP` is the coin `1kBONKPERP`, so the market name and the token spell the
    // multiplier differently. A selector of `BONK` has to find it through either.
    assert!(follows_setting("BONK", &["1000BONKPERP", "1kBONKPERP"]));
    assert!(follows_setting("bonk", &["1000BONKUSDT", "1000BONK"]));
    assert!(!follows_setting("BONK", &["PEPEUSDT", "PEPE"]));
}

/// A list is the point: the coins worth reading are named up front and compared side by side.
#[test]
fn a_list_follows_every_entry() {
    let setting = "BONK, PEPE ,1000SATS";
    assert!(follows_setting(setting, &["PEPEUSDT", "PEPE"]));
    assert!(follows_setting(setting, &["1000SATSUSDT", "1000SATS"]));
    assert!(!follows_setting(setting, &["BTCUSDT", "BTC"]));
}

/// An empty selector follows nothing.
///
/// Breakage: treating it as "everything" — the way the ORDER selector treats `"1"` — would dump
/// every market of every core into the file the moment the key was added and left blank.
#[test]
fn an_empty_selector_follows_nothing() {
    assert!(!follows_setting("", &["BTCUSDT", "BTC"]));
    assert!(!follows_setting("  ,  ", &["BTCUSDT", "BTC"]));
}

/// A core is swept once, and a changed selector starts a fresh sweep.
///
/// Breakage: remembering across selectors means asking about a second coin prints nothing, because
/// the sweep has "already run"; forgetting per call means the reconciliation tick re-scans every
/// market universe several times a second, forever.
#[test]
fn a_core_is_finished_once_per_selector() {
    let mut swept = Swept {
        selector: String::new(),
        cores: std::collections::HashSet::new(),
    };
    assert!(!swept.done("BONK", 1));
    swept.finish("BONK", 1);
    assert!(swept.done("BONK", 1));
    // Every core is asked separately: how the OTHER core spells the coin is the whole comparison.
    assert!(!swept.done("BONK", 2));
    // A new selector is a new question, so the first core comes back.
    assert!(!swept.done("PEPE", 1));
    assert!(!swept.done("PEPE", 2));
}

/// A finish under a stale selector must not mark the core done under the current one.
///
/// The sweep reads its rows with the book of selectors unlocked, so an edit can land in between;
/// crediting the new selector with the old one's work is how a coin gets silently skipped.
#[test]
fn a_finish_from_the_previous_selector_is_dropped() {
    let mut swept = Swept {
        selector: String::new(),
        cores: std::collections::HashSet::new(),
    };
    assert!(!swept.done("BONK", 1));
    swept.finish("PEPE", 1);
    assert!(!swept.done("BONK", 1));
}
