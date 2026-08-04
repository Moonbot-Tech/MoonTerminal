//! Summary wording for the purge confirmation. Explicit imports: the parent re-exports `gpui::*`,
//! whose own `test` would shadow the built-in attribute.

use super::purge_summary_lines;

/// The caveat is the whole reason legacy rows are counted at all: they cannot be deleted, so a
/// confirmation that omits them promises a clean sweep the operation will not deliver.
#[test]
fn the_legacy_caveat_appears_whenever_there_are_legacy_rows() {
    let lines = purge_summary_lines(12, 4, 3);

    assert_eq!(lines.len(), 2, "count line plus the caveat: {lines:?}");
    assert!(
        lines[1].contains('3'),
        "the caveat must state how many rows remain: {lines:?}"
    );
}

/// Adding an unconditional warning would falsely claim that addressable rows must survive.
#[test]
fn no_legacy_rows_means_no_caveat() {
    let lines = purge_summary_lines(12, 4, 0);

    assert_eq!(lines.len(), 1, "nothing to warn about: {lines:?}");
}

/// Printing "0 trades" alongside a period count reads as a broken query rather than as a strategy
/// whose trades are already gone.
#[test]
fn zero_trades_is_stated_not_counted() {
    let zero = purge_summary_lines(0, 0, 0);
    let some = purge_summary_lines(5, 2, 0);

    assert_ne!(
        zero[0], some[0],
        "an empty purge gets its own sentence, not the count sentence with a zero"
    );
}

/// Both numbers are shown because the all-time total normally EXCEEDS the row's period count; a
/// summary carrying only one of them would read as the wrong number rather than as a wider scope.
#[test]
fn the_count_line_states_both_scopes() {
    let lines = purge_summary_lines(120, 37, 0);

    assert!(
        lines[0].contains("120") && lines[0].contains("37"),
        "{lines:?}"
    );
}
