//! Pins for the one-token edits of a coin list.

use super::{add, contains, edit, remove};

#[test]
fn add_to_empty() {
    assert_eq!(add("", "ADA"), "ADA");
    assert_eq!(add("   ", "ADA"), "ADA");
}

#[test]
fn add_appends() {
    assert_eq!(add("BTC,ETH", "ADA"), "BTC,ETH,ADA");
    assert_eq!(add("BTC,ETH,", "ADA"), "BTC,ETH,ADA");
}

#[test]
fn dedup_case_insensitive() {
    assert_eq!(add("BTC,ada", "ADA"), "BTC,ada");
    assert!(contains("BTC, ada , ETH", "ADA"));
    assert!(!contains("BTC,ETH", "ADA"));
}

/// Lifting a listed token must drop only that token so a checked menu row can un-blacklist a
/// coin without rewriting the rest of the list.
///
/// Mutation: make `remove` return the original string (the old add-only path). Pressing
/// a checked permanent-blacklist row would then re-send the same list and leave the coin banned.
#[test]
fn remove_drops_only_the_matched_token() {
    assert_eq!(remove("BTC,ETH,ADA", "ADA"), "BTC,ETH");
    assert_eq!(remove("ADA,BTC,ETH", "ADA"), "BTC,ETH");
    assert_eq!(remove("BTC,ADA,ETH", "ADA"), "BTC,ETH");
    assert_eq!(remove("BTC,ada,ETH", "ADA"), "BTC,ETH");
    assert_eq!(remove("ADA", "ADA"), "");
    assert_eq!(remove("  ADA  ", "ADA"), "");
    assert_eq!(remove("BTC,ETH", "ADA"), "BTC,ETH");
    assert_eq!(remove("", "ADA"), "");
}

/// Remaining entries keep their original order, inner spacing, and unrecognized spellings; only
/// the matched token is dropped.
///
/// Mutation: trim or rejoin the surviving list. A core whose blacklist held `BTC_RP` or padded
/// tokens would then be sent a rewritten string that is not "that one token removed".
#[test]
fn remove_preserves_remaining_entries_verbatim() {
    assert_eq!(
        remove(" BTC_RP, 1kBONKPERP ,ADA", "ADA"),
        " BTC_RP, 1kBONKPERP "
    );
    assert_eq!(remove("BTC, ada , ETH", "ADA"), "BTC, ETH");
    assert_eq!(remove("BTC,ADA,", "ADA"), "BTC,");
}

/// Adding then lifting a token restores the previous list; lifting a listed token then adding it
/// back appends, which is the same add path an unchecked row already uses.
///
/// Mutation: leave `edit(..., lift = true)` as `add`. The checked row would
/// keep the coin on the list and still pay a settings write.
#[test]
fn edit_is_a_toggle_in_both_directions() {
    assert_eq!(edit("", "ADA", false), "ADA");
    assert_eq!(edit("ADA", "ADA", true), "");

    let added = edit("BTC,ETH", "ADA", false);
    assert_eq!(added, "BTC,ETH,ADA");
    assert_eq!(edit(&added, "ADA", true), "BTC,ETH");

    let lifted = edit("BTC,ADA,ETH", "ADA", true);
    assert_eq!(lifted, "BTC,ETH");
    assert_eq!(edit(&lifted, "ADA", false), "BTC,ETH,ADA");
}
