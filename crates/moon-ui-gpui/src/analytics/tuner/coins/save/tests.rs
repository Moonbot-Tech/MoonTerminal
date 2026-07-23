use super::apply_delta;

fn t(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| (*s).to_string()).collect()
}

/// One added coin is ONE change to the string, and everything else keeps the position the
/// strategy had it in.
#[test]
fn adding_one_coin_changes_one_thing() {
    assert_eq!(
        apply_delta("AAVE,BTC,ETH,ZRX", &t(&["AKE"]), &[]),
        "AKE,AAVE,BTC,ETH,ZRX"
    );
}

/// THE POINT OF THE WHOLE MODEL: a strategy keeps what it holds.
///
/// Two selected copies, one with a list and one without. The edit is "add AKE". Writing
/// the working set (the union) gave the empty copy the other's entire list; the delta
/// gives each of them exactly one new coin.
#[test]
fn a_strategy_keeps_its_own_list() {
    let added = t(&["AKE"]);
    assert_eq!(apply_delta("AAVE,BTC,ETH", &added, &[]), "AKE,AAVE,BTC,ETH");
    assert_eq!(apply_delta("", &added, &[]), "AKE");
}

/// Unticking removes exactly that coin, from every strategy, and disturbs nothing else.
#[test]
fn unticking_removes_only_that_coin() {
    assert_eq!(
        apply_delta("AAVE,BTC,ETH,ZRX", &[], &t(&["BTC"])),
        "AAVE,ETH,ZRX"
    );
}

/// A coin the CORE added since the panel last read is in neither delta, so it survives —
/// the lost update is impossible rather than merely reported.
#[test]
fn a_coin_the_core_added_survives() {
    let got = apply_delta("AAVE,DOGE,ETH", &t(&["AKE"]), &t(&["BTC"]));
    assert_eq!(
        got, "AKE,AAVE,DOGE,ETH",
        "DOGE was never mentioned by the edit"
    );
}

/// Several entries fold to one coin. Ticking keeps them all, as written; unticking takes
/// them all, because the box is per COIN and leaving `BTC_0626` would still list it.
#[test]
fn every_spelling_follows_the_coin() {
    assert_eq!(
        apply_delta("BTC,BTC_0626,BTC_0925,ETH", &[], &[]),
        "BTC,BTC_0626,BTC_0925,ETH"
    );
    assert_eq!(
        apply_delta("BTC,BTC_0626,BTC_0925,ETH", &[], &t(&["BTC"])),
        "ETH"
    );
}

/// The strategy's own spacing survives: Moonbot writes `A,B,C`, and re-spacing the whole
/// field changed every line of the confirmation for a one-coin edit.
#[test]
fn the_strategys_own_separator_is_kept() {
    assert_eq!(apply_delta("A,B", &t(&["C"]), &[]), "C,A,B");
    assert_eq!(apply_delta("A, B", &t(&["C"]), &[]), "C, A, B");
}

/// Matching folds case and contract suffix exactly as the tick boxes do, so a strategy
/// spelling a coin `mith` is neither duplicated nor missed.
#[test]
fn matching_folds_case_and_contract_suffix() {
    assert_eq!(
        apply_delta("mith,btc_1230", &t(&["MITH"]), &[]),
        "mith,btc_1230"
    );
    assert_eq!(apply_delta("mith,btc_1230", &[], &t(&["BTC"])), "mith");
}

/// An empty edit must leave the list byte-for-byte alone — nothing is "normalised".
///
/// Including the JSON-array shape: `split_coin_list` accepts it, and no separator
/// heuristic reproduces brackets and quotes. The first version of this test used commas
/// only, and the invariant it claimed to guard was false for every other spelling.
#[test]
fn an_empty_edit_writes_the_list_back_unchanged() {
    for stored in [
        "ZRX,AAVE,BTC",
        "ZRX, AAVE, BTC",
        "ZRX;AAVE;BTC",
        "ZRX AAVE BTC",
        "[\"ZRX\",\"AAVE\"]",
        "",
    ] {
        assert_eq!(
            apply_delta(stored, &[], &[]),
            stored,
            "stored as {stored:?}"
        );
    }
}

/// The field's OWN convention survives an edit, whichever of the accepted separators it
/// uses. `split_coin_list` takes commas, semicolons and whitespace alike, so assuming
/// commas rewrote every line of a field that used another — the same "one coin edited,
/// the whole field changed" this module exists to avoid.
#[test]
fn every_accepted_separator_is_preserved() {
    let add = t(&["AKE"]);
    assert_eq!(apply_delta("A,B", &add, &[]), "AKE,A,B");
    assert_eq!(apply_delta("A, B", &add, &[]), "AKE, A, B");
    assert_eq!(apply_delta("A;B", &add, &[]), "AKE;A;B");
    assert_eq!(apply_delta("A; B", &add, &[]), "AKE; A; B");
    assert_eq!(apply_delta("A B", &add, &[]), "AKE A B");
}

/// A single-entry list has no separator to copy, so the first one added picks the
/// Moonbot-ish default rather than gluing the two together.
#[test]
fn a_one_entry_list_gets_a_sane_separator() {
    assert_eq!(apply_delta("BTC", &t(&["AKE"]), &[]), "AKE, BTC");
    assert_eq!(apply_delta("", &t(&["AKE"]), &[]), "AKE");
}
