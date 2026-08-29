//! Unit tests for the folder-count accumulator.
//!
//! The oracle is [`naive_folder_counts`], a straightforward scan of every strategy for each
//! folder. Its algorithm and traversal order are independent of the accumulator, so agreement
//! between them checks the result rather than restating the implementation.

use moon_core::feed::StrategyRow;

use super::{FolderCounts, subtree_check_targets, subtree_folder_paths, visible_strategy_keys};
use crate::strategies::filter::{PreparedFilter, StrategyFilter};
use crate::strategies::tree::ops::path_segments;

/// Returns whether one row sits at or below a folder prefix, spelled out segment by segment.
///
/// The oracles' shared half. It stays a hand-written scan over [`path_segments`] rather than a call
/// to the production predicate, or the tests below would compare an expression with itself.
fn naive_under(row: &StrategyRow, prefix: &[String]) -> bool {
    let parts: Vec<&str> = path_segments(&row.folder_path).collect();
    parts.len() >= prefix.len()
        && prefix
            .iter()
            .zip(parts.iter())
            .all(|(a, b)| a.as_str() == *b)
}

/// Counts strategies at or below `prefix` by rescanning the whole list as a test-only oracle.
fn naive_folder_counts(
    strategies: &[StrategyRow],
    filter: &PreparedFilter,
    prefix: &[String],
) -> (usize, usize) {
    let mut active = 0;
    let mut total = 0;
    for r in strategies {
        if !filter.counts(r) || !naive_under(r, prefix) {
            continue;
        }
        total += 1;
        if r.checked {
            active += 1;
        }
    }
    (active, total)
}

/// Builds a strategy row with explicit values for every input used by folder counting.
fn row(id: u64, folder_path: &str, kind_ordinal: u8, is_short: bool, checked: bool) -> StrategyRow {
    StrategyRow {
        id,
        name: format!("s{id}"),
        kind: "Test".to_string(),
        kind_ordinal,
        folder_path: folder_path.to_string(),
        checked,
        is_short,
        fields: Vec::new(),
    }
}

/// Paths chosen to exercise every branch of `path_segments`: plain nesting, a slash with a
/// whitespace neighbour (ONE folder in MoonBot), doubled and edge separators, a backslash
/// separator, and a strategy sitting directly at a folder that also has children.
fn corpus() -> Vec<StrategyRow> {
    vec![
        row(1, "", 0, false, true),
        row(2, "a", 0, false, true),
        row(3, "a", 1, true, false),
        row(4, "a/b", 0, false, true),
        row(5, "a/b/c", 0, false, false),
        row(6, "a/b/c", 1, false, true),
        row(7, "EMA / ORGANIC", 0, false, true),
        row(8, "EMA / ORGANIC/deep", 0, true, true),
        row(9, "/a", 0, false, false),
        row(10, "a//b", 0, false, true),
        row(11, "a\\b", 1, true, true),
        row(12, "a/ b", 0, false, true),
        row(13, " /a", 0, false, true),
        row(14, "z", 0, false, false),
    ]
}

/// Every folder prefix the corpus can produce, plus prefixes that no strategy occupies.
fn prefixes() -> Vec<Vec<String>> {
    let mut out: Vec<Vec<String>> = vec![
        Vec::new(),
        vec!["a".into()],
        vec!["a".into(), "b".into()],
        vec!["a".into(), "b".into(), "c".into()],
        vec!["EMA / ORGANIC".into()],
        vec!["EMA / ORGANIC".into(), "deep".into()],
        vec!["z".into()],
        vec!["a/ b".into()],
        vec![" /a".into()],
        // Occupied by nothing: a UI-only folder created before its first strategy.
        vec!["ghost".into()],
        vec!["a".into(), "ghost".into()],
    ];
    out.sort();
    out
}

/// Feeds every row, exactly as `tree::moon::build` does: the kind/direction gate lives inside
/// `add`, so pre-filtering here would hide a regression in that gate.
fn accumulate(rows: &[StrategyRow], filter: &PreparedFilter) -> FolderCounts {
    let mut counts = FolderCounts::default();
    for r in rows {
        counts.add(r, filter);
    }
    counts
}

/// Builds a prepared filter with independently chosen values for each ROW-level dimension.
///
/// The exchange filter takes no parameter: it never reaches [`PreparedFilter`], because it selects
/// whole cores and the folder counts under test are per core.
fn filter(search: &str, kind: Option<u8>, dir: Option<bool>, active_only: bool) -> PreparedFilter {
    StrategyFilter {
        search: search.to_string(),
        kind,
        dir,
        exchange: None,
        active_only,
    }
    .prepare()
}

/// Changing `FolderCounts::add` prefix accumulation would corrupt folder count captions.
#[test]
fn the_accumulator_agrees_with_the_naive_scan_for_every_prefix() {
    let rows = corpus();
    // Cross the corpus with the filter combinations that counting actually honours.
    for (kind, dir) in [
        (None, None),
        (Some(0), None),
        (Some(1), None),
        (None, Some(true)),
        (None, Some(false)),
        (Some(0), Some(false)),
        (Some(1), Some(true)),
        (Some(7), None),
    ] {
        let f = filter("", kind, dir, false);
        let counts = accumulate(&rows, &f);
        for prefix in prefixes() {
            let want = naive_folder_counts(&rows, &f, &prefix);
            let got = if prefix.is_empty() {
                counts.root()
            } else {
                counts.for_path(&prefix.join("/"))
            };
            assert_eq!(
                got, want,
                "prefix {prefix:?} under kind={kind:?} dir={dir:?}"
            );
        }
    }
}

/// Applying the search gate in `FolderCounts::add` would shrink captions while filtering.
#[test]
fn counting_ignores_the_search_text() {
    // The accumulator applies its own gate, so a search matching nothing must leave every folder
    // caption untouched.
    let rows = corpus();
    let plain = accumulate(&rows, &filter("", None, None, false));
    let noisy = accumulate(&rows, &filter("no-such-name", None, None, false));
    assert_eq!(plain.root(), noisy.root());
    for prefix in prefixes() {
        let key = prefix.join("/");
        assert_eq!(plain.for_path(&key), noisy.for_path(&key), "prefix {key}");
    }
}

/// Returning a nonzero fallback from `FolderCounts::for_path` would mislabel empty UI folders.
#[test]
fn an_unoccupied_folder_counts_zero() {
    let counts = accumulate(&corpus(), &filter("", None, None, false));
    assert_eq!(counts.for_path("ghost"), (0, 0));
    assert_eq!(counts.for_path("a/ghost"), (0, 0));
}

/// Omitting an accepted row from `FolderCounts::root` would undercount the core caption.
#[test]
fn the_root_counts_every_filtered_strategy() {
    // The core's own caption reads `root()`, which must equal what the naive scan reports for the
    // empty prefix — the oracle, not a recount of the same predicate.
    let rows = corpus();
    let f = filter("", Some(0), None, false);
    assert_eq!(
        accumulate(&rows, &f).root(),
        naive_folder_counts(&rows, &f, &[])
    );
}

/// Counting only descendants in `FolderCounts::add` would omit direct rows from ancestor captions.
#[test]
fn a_strategy_at_a_folder_counts_in_that_folder_and_its_ancestors() {
    // Row 4 sits directly in `a/b`, which also has the child `a/b/c`.
    let rows = vec![
        row(4, "a/b", 0, false, true),
        row(5, "a/b/c", 0, false, true),
    ];
    let counts = accumulate(&rows, &filter("", None, None, false));
    assert_eq!(counts.for_path("a/b"), (2, 2));
    assert_eq!(counts.for_path("a/b/c"), (1, 1));
    assert_eq!(counts.for_path("a"), (2, 2));
    assert_eq!(counts.root(), (2, 2));
}

/// Adding active-only visibility to `FolderCounts::add` would reduce the core and folder totals
/// when unchecked rows are hidden, so captions would stop describing the configured strategy set.
#[test]
fn counting_ignores_active_only_visibility() {
    let rows = vec![
        row(1, "folder", 0, false, true),
        row(2, "folder", 0, false, false),
    ];
    let counts = accumulate(&rows, &filter("", None, None, true));
    assert_eq!(counts.root(), (1, 2));
    assert_eq!(counts.for_path("folder"), (1, 2));
}

/// Changing the joined count key independently of `path_segments` would attach counts to wrong folders.
#[test]
fn the_joined_key_round_trips_through_the_window_splitter() {
    // The accumulator keys folders by the joined prefix while the tree keys them by the same
    // join; both must re-split to the segments they were built from, or a folder's caption would
    // read another folder's numbers.
    for r in corpus() {
        let segs: Vec<&str> = path_segments(&r.folder_path).collect();
        let joined = segs.join("/");
        let resplit: Vec<&str> = path_segments(&joined).collect();
        assert_eq!(segs, resplit, "path {:?}", r.folder_path);
    }
}

/// `logic.rs:selected_keys` returning the retained set unchanged would let a hidden Classic
/// selection drive copy, delete, version, parameter, or edit actions on another core in Auto mode.
#[test]
fn hidden_classic_selection_cannot_drive_auto_actions() {
    let retained = vec![(11, 101), (22, 202), (11, 303)];

    assert_eq!(
        visible_strategy_keys(retained.clone(), Some(&[22])),
        vec![(22, 202)]
    );
    assert_eq!(retained, vec![(11, 101), (22, 202), (11, 303)]);
    assert_eq!(
        visible_strategy_keys(retained.clone(), None),
        retained,
        "Classic must restore the retained selection unchanged"
    );

    let logic = include_str!("../logic.rs");
    let selected_keys = logic
        .split("pub(super) fn selected_keys(")
        .nth(1)
        .and_then(|tail| tail.split("\n}").next())
        .expect("effective selection adapter must exist");
    assert!(selected_keys.contains("visible_strategy_keys"));

    let actions = include_str!("../actions.rs");
    assert!(actions.contains("strategy_action_authorized"));
    assert!(actions.contains("pub(super) fn start_stop_plan"));
    assert!(actions.contains("field_edit_plan_authorized"));
    // The buttons still dispatch a plan built during the frame that rendered them, rather than one
    // resolved at click time — it now comes from the pane cache. Only that link is asserted here;
    // `tree::pane_cache::tests` owns the rest of the chain, and this test is about authority.
    assert!(include_str!("../tree/mod.rs").contains("pane.plan.clone()"));
    assert!(actions.contains("fn apply_start_stop("));
}

/// Lists what one bulk checkbox covers by rescanning the whole list, as a test-only oracle.
///
/// Membership is spelled out here independently of the production predicate: visibility comes from
/// `matches` (search and active-only included, unlike the counters), and the folder test compares
/// segments rather than string prefixes.
fn naive_check_targets(
    strategies: &[StrategyRow],
    filter: &PreparedFilter,
    prefix: &[String],
) -> Vec<(u64, bool)> {
    strategies
        .iter()
        .filter(|r| filter.matches(r) && naive_under(r, prefix))
        .map(|r| (r.id, r.checked))
        .collect()
}

/// A folder's bulk checkbox stages exactly the rows the tree drew under it — no more, because a
/// click that reached a filtered-out strategy would start something the user cannot see, and no
/// fewer, because a missed row leaves the folder half-applied.
#[test]
fn a_bulk_check_covers_exactly_the_visible_subtree() {
    let rows = corpus();
    for f in [
        filter("", None, None, false),
        filter("s4", None, None, false),
        filter("", Some(0), None, false),
        filter("", None, Some(true), false),
        filter("", None, None, true),
    ] {
        for prefix in prefixes() {
            assert_eq!(
                subtree_check_targets(&rows, &prefix, &f),
                naive_check_targets(&rows, &f, &prefix),
                "prefix {prefix:?}"
            );
        }
    }
}

/// The core root spells its subtree as the empty path, which must cover every visible row —
/// including the strategies sitting outside any folder. Reading the empty path as "no folder
/// matched" instead would make the core checkbox stage nothing at all.
#[test]
fn the_core_root_covers_every_visible_row() {
    let rows = corpus();
    let f = filter("", None, None, false);
    let all: Vec<(u64, bool)> = rows.iter().map(|r| (r.id, r.checked)).collect();

    assert_eq!(subtree_check_targets(&rows, &[], &f), all);
}

/// Folder coverage is compared segment by segment. Matching the joined path as a string prefix
/// would make `test` swallow `testing`, and one click would stage a sibling folder's strategies.
#[test]
fn a_folder_never_swallows_its_longer_sibling() {
    let rows = vec![
        row(1, "test", 0, false, false),
        row(2, "testing", 0, false, false),
        row(3, "test/inner", 0, false, false),
    ];
    let f = filter("", None, None, false);

    assert_eq!(
        subtree_check_targets(&rows, &["test".to_string()], &f),
        vec![(1, false), (3, false)]
    );
}

/// A click on a folder acts on the whole subtree, and the tree draws nested folders as rows of
/// their own: leaving their boxes alone shows a checked parent above unchecked children whose
/// strategies it just staged — the exact mismatch reported against the first cut of the feature.
#[test]
fn a_bulk_click_carries_every_folder_row_below_it() {
    let rows = vec![
        row(1, "test", 0, false, false),
        row(2, "test/emulators", 0, false, false),
        row(3, "test/reals/deep", 0, false, false),
        row(4, "other/inner", 0, false, false),
    ];
    let f = filter("", None, None, false);

    assert_eq!(
        subtree_folder_paths(&rows, &["test".to_string()], &f),
        vec![
            "test/emulators".to_string(),
            "test/reals".to_string(),
            "test/reals/deep".to_string(),
        ],
        "every descendant folder, including an intermediate one holding no strategy of its own"
    );
    assert_eq!(
        subtree_folder_paths(&rows, &[], &f),
        vec![
            "other".to_string(),
            "other/inner".to_string(),
            "test".to_string(),
            "test/emulators".to_string(),
            "test/reals".to_string(),
            "test/reals/deep".to_string(),
        ],
        "the core root carries every folder in the core"
    );
}

/// The carried folders come from the same visible rows the strategies do, so a search that hides a
/// whole subfolder must leave that folder's box out of the click as well.
#[test]
fn a_hidden_subfolder_is_not_carried() {
    let rows = vec![
        row(1, "test/emulators", 0, false, false),
        row(2, "test/reals", 0, false, false),
    ];

    assert_eq!(
        subtree_folder_paths(&rows, &["test".to_string()], &filter("s1", None, None, false)),
        vec!["test/emulators".to_string()]
    );
}
