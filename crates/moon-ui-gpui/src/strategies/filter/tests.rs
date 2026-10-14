//! Unit tests for the strategy-tree filter predicates.
//!
//! The oracle is a hand-written expectation table, never another production predicate: `matches`
//! on [`StrategyFilter`] delegates to [`PreparedFilter`], so comparing the two would compare a
//! function with itself.

use moon_core::feed::StrategyRow;

use super::StrategyFilter;

/// Reverting folder matching would hide a matching ancestor's differently named strategies.
/// Splitting on every slash would also invent folders inside a valid spaced-slash name.
#[test]
fn folder_name_queries_keep_the_whole_subtree() {
    for (query, path, expected) in [
        ("FAST wave", "Root/Fast Wave/Child", true),
        ("fast missing", "Root/Fast Wave", false),
        ("slow, WAVE", "Root/Fast Wave", true),
        ("wave !fast", "Root/Fast Wave", false),
        ("wave !slow", "Root/Fast Wave/Slow Child", true),
        ("alpha beta", "Alpha/Beta", false),
        ("alpha beta", "Alpha / Beta/Child", true),
    ] {
        let f = filter(query, None, None, false).prepare();
        let mut strategy = row("Unrelated", 0, false, false);
        strategy.folder_path = path.into();
        assert_eq!(f.matches(&strategy), expected, "query={query}, path={path}");
    }
}

/// Expanding folders for exclusion-only queries would revive excluded strategies in innocent folders.
#[test]
fn strategy_names_and_exclusion_only_queries_keep_their_semantics() {
    for (query, name, path, expected) in [
        ("needle", "Needle Strategy", "Other", true),
        ("needle", "Other", "Other", false),
        ("!test", "Test Strategy", "Production", false),
        ("!test", "Live Strategy", "Test", true),
        ("needle !test", "Test Strategy", "Needle/Child", true),
    ] {
        let f = filter(query, None, None, false).prepare();
        let mut strategy = row(name, 0, false, false);
        strategy.folder_path = path.into();
        assert_eq!(f.matches(&strategy), expected, "query={query}, name={name}");
    }
}

/// A matching folder changes only search, so other tree filters must still narrow its rows.
#[test]
fn folder_matches_preserve_other_filter_dimensions() {
    let f = filter("Needle", Some(2), Some(false), true).prepare();
    for (kind, short, checked, expected) in [
        (2, false, true, true),
        (3, false, true, false),
        (2, true, true, false),
        (2, false, false, false),
    ] {
        let mut strategy = row("Other", kind, short, checked);
        strategy.folder_path = "Needle/Child".into();
        assert_eq!(f.matches(&strategy), expected);
    }
}

/// Empty cores survive the default and exchange-only views, but row filters require a match.
#[test]
fn empty_core_visibility_depends_on_row_filters_only() {
    for (search, kind, dir, active, narrows) in [
        ("", None, None, false, false),
        ("  ", None, None, false, false),
        ("needle", None, None, false, true),
        ("", Some(0), None, false, true),
        ("", None, Some(false), false, true),
        ("", None, None, true, true),
    ] {
        let mut state = filter(search, kind, dir, active);
        assert_eq!(state.prepare().narrows(), narrows);
        state.exchange = Some(moon_core::session::core_order::ExchangeSection::Unidentified);
        assert_eq!(state.prepare().narrows(), narrows);
        assert!(state.narrows());
    }
}

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

/// Builds stored filter state with independently chosen values for each ROW-level dimension.
///
/// The exchange filter takes no parameter: it selects whole cores rather than rows, so nothing
/// these tests assert about `matches` or `counts` can depend on it.
fn filter(search: &str, kind: Option<u8>, dir: Option<bool>, active_only: bool) -> StrategyFilter {
    StrategyFilter {
        search: search.to_string(),
        kind,
        dir,
        exchange: None,
        active_only,
        quick: None,
    }
}

/// A `prepare` edit that stores an empty query would force-expand the tree with no search text.
#[test]
fn an_empty_search_matches_every_name() {
    let f = filter("", None, None, false).prepare();
    assert!(f.matches(&row("anything", 0, false, false)));
    assert!(!f.searching());
    assert!(f.name_matches("anything"));
    assert!(f.name_matches(""));
}

/// Removing `trim` in `prepare` would make whitespace hide every strategy as a live search.
#[test]
fn a_whitespace_only_search_is_not_a_search() {
    let f = filter("   ", None, None, false).prepare();
    assert!(f.matches(&row("anything", 0, false, false)));
    assert!(!f.searching());
}

/// Switching `PreparedFilter::matches` (via `StrategyQuery`) to ASCII-only folding would break
/// Cyrillic name searches.
#[test]
fn the_search_is_case_insensitive_across_scripts() {
    // Strategy names in this product are commonly Cyrillic, so the fold must be full Unicode case
    // folding rather than ASCII-only lowering.
    let f = filter("  СТРАТЕГИЯ  ", None, None, false).prepare();
    assert!(f.searching());
    assert!(f.name_matches("Стратегия_1"));
    assert!(f.name_matches("стратегия"));
    assert!(!f.name_matches("Another"));
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

/// The quick filters: Disabled keeps unticked rows only; WithOrders keeps exactly the ids its
/// core was scoped to, and an unscoped predicate (selection prune, no core in hand) fails open
/// rather than hiding everything.
#[test]
fn quick_filters_keep_their_rows_and_fail_open_when_unscoped() {
    use super::QuickFilter;
    let quick = |q| StrategyFilter {
        quick: Some(q),
        ..Default::default()
    };
    let on = row("UNIT_01", 0, false, true);
    let off = row("UNIT_02", 0, false, false);

    let disabled = quick(QuickFilter::Disabled).prepare();
    assert!(!disabled.matches(&on));
    assert!(disabled.matches(&off));

    // Core-level: rows are never hidden by it, the build drops whole cores instead.
    let stopped = quick(QuickFilter::Stopped).prepare();
    assert!(stopped.matches(&on) && stopped.matches(&off));

    let unscoped = quick(QuickFilter::WithOrders).prepare();
    assert!(unscoped.matches(&on), "WithOrders unscoped must fail open");
    assert!(unscoped.narrows());

    let mut other = off.clone();
    other.id = 2;
    // No orders on the core: the orders filter keeps nothing on it.
    let cd = moon_core::session::store::CoreData::default();
    let scoped = quick(QuickFilter::WithOrders).prepare_for_core(&cd);
    assert!(!scoped.matches(&on) && !scoped.matches(&other));
}

/// The Stopped chip keeps a stopped core even when it holds no strategy (fix: it used to count as
/// a row filter and drop empty cores), and it rejects a running or unconfirmed core.
#[test]
fn the_stopped_chip_keeps_empty_stopped_cores_and_rejects_running_ones() {
    use super::QuickFilter;
    let quick = |q| StrategyFilter {
        quick: Some(q),
        ..Default::default()
    };
    let stopped = quick(QuickFilter::Stopped).prepare();
    assert!(
        !stopped.narrows_rows(),
        "Stopped must not drop an empty core"
    );
    assert!(stopped.core_matches_quick(Some(false)));
    assert!(!stopped.core_matches_quick(Some(true)));
    assert!(!stopped.core_matches_quick(None));

    for q in [QuickFilter::WithOrders, QuickFilter::Disabled] {
        let f = quick(q).prepare();
        assert!(f.narrows_rows(), "{q:?} narrows rows");
        assert!(f.core_matches_quick(Some(true)), "{q:?} is not core-level");
    }
    let none = StrategyFilter::default().prepare();
    assert!(!none.narrows_rows() && none.core_matches_quick(Some(true)));
}

/// A reveal must see that a per-core chip hides the target: scoped to the core, an unconfirmed
/// edit is hidden under WithOrders even though the unscoped predicate fails open on it.
#[test]
fn a_scoped_quick_chip_reports_the_reveal_target_as_hidden() {
    use super::QuickFilter;
    let f = StrategyFilter {
        quick: Some(QuickFilter::WithOrders),
        ..Default::default()
    };
    let mut cd = moon_core::session::store::CoreData::default();
    cd.strategy_edits.push(moon_core::feed::StrategyEditRow {
        id: 2,
        phase: moon_core::feed::StrategyEditPhase::Pending,
        submitted_at_ms: 0,
        fields: Vec::new(),
    });
    let mut target = row("UNIT_02", 0, false, true);
    target.id = 2;
    assert!(f.prepare().matches(&target), "unscoped fails open");
    assert!(!f.prepare_for_core(&cd).matches(&target), "scoped hides it");
}
