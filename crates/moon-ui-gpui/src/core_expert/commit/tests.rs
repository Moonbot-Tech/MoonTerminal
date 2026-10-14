//! An unstaged coin lift must not become a parameter OK writes to every selected core.

use std::collections::BTreeSet;

use super::{BlacklistLift, apply_blacklist_lift, blacklist_lift};

/// Nothing staged: both sides lose the coin and the caller must not stage the field.
///
/// Staging it would send the anchor's blacklist text to every selected core on OK.
#[test]
fn an_unstaged_lift_rewrites_both_sides_and_does_not_stage() {
    let base = "XALPHA, WBTC";
    let draft = "XALPHA, WBTC";
    let lift = blacklist_lift(false, base, draft, "XALPHA");
    assert_eq!(
        lift,
        BlacklistLift {
            base_text: " WBTC".to_string(),
            draft_text: " WBTC".to_string(),
            rewrite_base: true,
        }
    );
    assert!(
        !lift
            .base_text
            .split(',')
            .any(|token| token.trim().eq_ignore_ascii_case("XALPHA"))
    );
    assert!(
        lift.draft_text
            .split(',')
            .any(|token| token.trim() == "WBTC")
    );
}

/// A staged blacklist stays the only field the caller rewrites, and the base is left alone.
///
/// Rewriting the base as well would make the field look unchanged and drop a parameter the
/// user had already staged.
#[test]
fn a_staged_lift_rewrites_only_the_draft() {
    let base = "XALPHA, WBTC";
    let draft = "XALPHA, ETH";
    let lift = blacklist_lift(true, base, draft, "XALPHA");
    assert_eq!(lift.base_text, base);
    assert_eq!(lift.draft_text, " ETH");
    assert!(!lift.rewrite_base);
    assert!(
        lift.draft_text
            .split(',')
            .any(|token| token.trim() == "ETH")
    );
}

/// An unstaged lift must leave `general.blacklist_text` out of the staged keys.
///
/// Putting that key in the set would make OK write the anchor's blacklist onto every selected
/// core.
#[test]
fn an_unstaged_lift_keeps_blacklist_text_out_of_the_set() {
    let mut base = "XALPHA, WBTC".to_string();
    let mut draft = "XALPHA, WBTC".to_string();
    let mut staged = BTreeSet::new();
    apply_blacklist_lift(&mut base, &mut draft, &mut staged, "XALPHA");
    assert!(
        !staged.contains("general.blacklist_text"),
        "an unstaged lift must not stage general.blacklist_text"
    );
    assert_eq!(base, " WBTC");
    assert_eq!(draft, base);
}

/// A staged lift must keep `general.blacklist_text` staged and leave the base text alone.
///
/// Dropping the key would make OK skip a parameter the user had already staged. Rewriting the
/// base as well would make the field look unchanged.
#[test]
fn a_staged_lift_keeps_blacklist_text_staged_and_the_base() {
    let mut base = "XALPHA, WBTC".to_string();
    let mut draft = "XALPHA, ETH".to_string();
    let mut staged = BTreeSet::from(["general.take_profit_pct", "general.blacklist_text"]);
    apply_blacklist_lift(&mut base, &mut draft, &mut staged, "XALPHA");
    assert!(staged.contains("general.blacklist_text"));
    assert!(
        staged.contains("general.take_profit_pct"),
        "a lift must not drop a field staged beside the blacklist"
    );
    assert_eq!(base, "XALPHA, WBTC");
    assert_eq!(draft, " ETH");
}
