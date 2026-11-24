//! Pins for the one-token edits of a coin list.

use super::{
    add, contains, edit, remove, strategy_add, strategy_contains, strategy_edit, strategy_entries,
    strategy_remove,
};

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

/// MoonBot's strategy `CoinsBlackList` takes names "separated by a space, comma+space or comma
/// without space" (moonbot.pro filters-tab-parameters), and `symbol::parse_coin_list` already
/// reads `ANC GALA` as two entries. Lifting GALA from that list must therefore change the text;
/// adding ANC must not duplicate it (#1448).
///
/// Mutation: route the strategy functions through the comma-only [`contains`] / [`remove`] /
/// [`add`] again. The coin menu then reports GALA as not listed, its lift sends nothing, and an
/// add writes `ANC GALA,ANC`.
#[test]
fn probe_space_separated_strategy_list() {
    let text = "ANC GALA";
    assert!(crate::symbol::parse_coin_list(text).contains("GALA"));
    assert!(
        strategy_contains(text, "GALA"),
        "contains misses a space-separated entry"
    );
    assert_eq!(
        strategy_remove(text, "GALA"),
        "ANC",
        "lift leaves GALA on the list"
    );
    assert_eq!(strategy_add(text, "ANC"), text, "add duplicates ANC");
    assert_eq!(strategy_edit(text, "gala", true), "ANC");
}

/// The core-wide list is comma-only by MoonBot's own rule, so a space inside it is part of one
/// entry the core does not associate with either coin.
///
/// Mutation: widen [`contains`] to the strategy separator set. The menu would then show a coin
/// as banned core-wide that the core trades, and a lift would rewrite the user's entry.
#[test]
fn core_wide_list_stays_comma_only() {
    assert!(!contains("ANC GALA", "GALA"));
    assert_eq!(remove("ANC GALA", "GALA"), "ANC GALA");
    assert_eq!(add("ANC GALA", "GALA"), "ANC GALA,GALA");
}

/// Every entry of a strategy list is found whatever separates it, and found only whole.
///
/// Mutation: split on commas and spaces only. A `;`- or newline-separated field, which the
/// Strategies counters read as several coins, would again hide all but one of them.
#[test]
fn strategy_contains_reads_every_separator() {
    for text in [
        "ANC GALA",
        "ANC,GALA",
        "ANC, GALA",
        "ANC;GALA",
        "ANC\nGALA",
        "ANC\tgala",
    ] {
        assert!(strategy_contains(text, "GALA"), "{text:?}");
        assert!(strategy_contains(text, "anc"), "{text:?}");
    }
    assert!(!strategy_contains("GALAUSDT BTC", "GALA"));
    assert!(!strategy_contains("", "GALA"));
    assert!(!strategy_contains("ANC GALA", ""));
}

/// A lift drops the coin and one separator run beside it, nothing else: the other entries, the
/// separators between them, and the text before the first entry and after the last stay as
/// written.
///
/// Mutation: rejoin the survivors with one separator. `A, B;C D` would then come back re-spelled
/// for a one-coin edit, and the confirmation would show every line changed.
#[test]
fn strategy_remove_keeps_every_other_byte() {
    assert_eq!(strategy_remove("ANC GALA", "ANC"), "GALA");
    assert_eq!(strategy_remove("A, B;C D", "B"), "A;C D");
    assert_eq!(strategy_remove("A, B;C D", "A"), "B;C D");
    assert_eq!(strategy_remove("A, B;C D", "D"), "A, B;C");
    assert_eq!(strategy_remove(" A  B ,", "B"), " A ,");
    assert_eq!(strategy_remove("[\"A\",\"B\"]", "B"), "[\"A\"]");
    assert_eq!(strategy_remove("GALA gala", "GALA"), "");
    assert_eq!(strategy_remove("  GALA  ", "GALA"), "");
    assert_eq!(strategy_remove(" X  Y ", "GALA"), " X  Y ");
    assert_eq!(strategy_remove("", "GALA"), "");
}

/// An add follows the field's own separator once it has one, and MoonBot's comma before that.
///
/// Mutation: always append with `,`. A space-separated field would come back as `A B,C`, a
/// spelling the user never wrote.
#[test]
fn strategy_add_follows_the_fields_separator() {
    assert_eq!(strategy_add("A B", "C"), "A B C");
    assert_eq!(strategy_add("A, B", "C"), "A, B, C");
    assert_eq!(strategy_add("A;B;", "C"), "A;B;C");
    assert_eq!(strategy_add("A B ", "C"), "A B C");
    assert_eq!(strategy_add("A", "C"), "A,C");
    assert_eq!(strategy_add("A,", "C"), "A,C");
    assert_eq!(strategy_add("", "C"), "C");
    assert_eq!(strategy_add("   ", "C"), "C");
    assert_eq!(strategy_add("a b", "B"), "a b");
}

/// The ranges are byte offsets into the text, so a caller can paint or cut the entry in place.
///
/// Mutation: return offsets relative to the trimmed text. Every highlight would then sit one
/// separator off the coin it names.
#[test]
fn strategy_entries_are_byte_ranges_of_the_text() {
    let text = "  ANC ; GALA,BTC ";
    let entries: Vec<&str> = strategy_entries(text).map(|r| &text[r]).collect();
    assert_eq!(entries, ["ANC", "GALA", "BTC"]);
}
