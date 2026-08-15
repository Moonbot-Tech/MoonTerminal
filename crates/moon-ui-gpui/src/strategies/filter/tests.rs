//! Unit tests for the strategy-tree filter predicates.
//!
//! The oracle is a hand-written expectation table, never another production predicate: `matches`
//! on [`StrategyFilter`] delegates to [`PreparedFilter`], so comparing the two would compare a
//! function with itself.

use moon_core::feed::StrategyRow;

use super::StrategyFilter;

/// Builds a row carrying only the fields the filters read.
fn row(name: &str, kind_ordinal: u8, is_short: bool, checked: bool) -> StrategyRow {
    StrategyRow {
        id: 1,
        name: name.to_string(),
        kind: "Test".to_string(),
        kind_ordinal,
        folder_path: String::new(),
        checked,
        is_short,
        fields: Vec::new(),
    }
}

/// Builds stored filter state with independently chosen values for each filter dimension.
fn filter(search: &str, kind: Option<u8>, dir: Option<bool>, active_only: bool) -> StrategyFilter {
    StrategyFilter {
        search: search.to_string(),
        kind,
        dir,
        active_only,
    }
}

/// A `prepare` edit that stores an empty query would force-expand the tree with no search text.
#[test]
fn an_empty_search_matches_every_name() {
    let f = filter("", None, None, false).prepare();
    assert!(f.matches(&row("anything", 0, false, false)));
    assert!(!f.searching());
    assert!(f.query().is_none());
}

/// Removing `trim` in `prepare` would make whitespace hide every strategy as a live search.
#[test]
fn a_whitespace_only_search_is_not_a_search() {
    let f = filter("   ", None, None, false).prepare();
    assert!(f.matches(&row("anything", 0, false, false)));
    assert!(!f.searching());
}

/// Switching `PreparedFilter::matches` to ASCII-only lowering would break Cyrillic name searches.
#[test]
fn the_search_is_case_insensitive_across_scripts() {
    // Strategy names in this product are commonly Cyrillic, so the lowering must be full-Unicode
    // rather than ASCII-only.
    let f = filter("  СТРАТЕГИЯ  ", None, None, false).prepare();
    assert!(f.searching());
    assert_eq!(f.query(), Some("стратегия"));
    assert!(f.matches(&row("Моя Стратегия 7", 0, false, false)));
    assert!(f.matches(&row("МОЯ СТРАТЕГИЯ", 0, false, false)));
    assert!(!f.matches(&row("Another", 0, false, false)));

    let f = filter("EmA", None, None, false).prepare();
    assert!(f.matches(&row("Long ema fast", 0, false, false)));
}

/// `PreparedFilter::matches` ignoring `active_only` would leave unchecked strategies visible after
/// the user enabled the setting, while an unconditional checked gate would hide them by default.
#[test]
fn active_only_controls_unchecked_row_visibility() {
    let all = filter("", None, None, false).prepare();
    assert!(all.matches(&row("on", 0, false, true)));
    assert!(all.matches(&row("off", 0, false, false)));

    let active = filter("", None, None, true).prepare();
    assert!(active.matches(&row("on", 0, false, true)));
    assert!(!active.matches(&row("off", 0, false, false)));
}

/// Applying the query in `PreparedFilter::counts` would make folder captions shrink during search.
#[test]
fn the_search_never_affects_counts() {
    let f = filter("zzz", None, None, false).prepare();
    assert!(!f.matches(&row("abc", 0, false, true)));
    assert!(f.counts(&row("abc", 0, false, true)));
}

/// Omitting a kind or direction gate would show and count strategies outside the selected filter.
#[test]
fn kind_and_direction_gate_both_predicates() {
    let f = filter("", Some(3), Some(true), false).prepare();
    assert!(f.counts(&row("x", 3, true, false)));
    assert!(f.matches(&row("x", 3, true, false)));
    // Wrong kind.
    assert!(!f.counts(&row("x", 4, true, false)));
    assert!(!f.matches(&row("x", 4, true, false)));
    // Wrong direction.
    assert!(!f.counts(&row("x", 3, false, false)));
    assert!(!f.matches(&row("x", 3, false, false)));

    // `None` means "either".
    let f = filter("", None, None, false).prepare();
    assert!(f.counts(&row("x", 9, true, false)));
    assert!(f.counts(&row("x", 0, false, false)));
}

/// Skipping a condition in `StrategyFilter::matches` could leave a revealed row hidden by the tree.
#[test]
fn the_delegator_applies_the_whole_predicate() {
    // `StrategyFilter::matches` is the cold single-row path. Comparing it with `prepare().matches`
    // would compare a function with itself, so assert against the expectation table instead: it
    // catches a delegator rewritten to skip a condition.
    let f = filter("EMA", Some(2), Some(false), true);
    assert!(f.matches(&row("ema long", 2, false, true)));
    // Fails the search.
    assert!(!f.matches(&row("other", 2, false, true)));
    // Fails the direction.
    assert!(!f.matches(&row("ema long", 2, true, true)));
    // Fails the kind.
    assert!(!f.matches(&row("ema long", 5, false, true)));
    // Fails active-only after every other condition passes.
    assert!(!f.matches(&row("ema long", 2, false, false)));
}

/// Adding active-only to `PreparedFilter::counts` would make every core and folder caption shrink
/// when the preference hides unchecked rows, concealing the number of configured strategies.
#[test]
fn active_only_never_affects_counts() {
    let f = filter("", None, None, true).prepare();
    assert!(f.counts(&row("on", 0, false, true)));
    assert!(f.counts(&row("off", 0, false, false)));
}
