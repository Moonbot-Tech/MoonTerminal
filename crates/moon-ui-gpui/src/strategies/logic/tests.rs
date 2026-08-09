//! Unit tests for the folder-count accumulator.
//!
//! The oracle is [`naive_folder_counts`], a straightforward scan of every strategy for each
//! folder. Its algorithm and traversal order are independent of the accumulator, so agreement
//! between them checks the result rather than restating the implementation.

use moon_core::feed::StrategyRow;

use super::{FolderCounts, visible_strategy_keys};
use crate::strategies::filter::{PreparedFilter, StrategyFilter};
use crate::strategies::tree::ops::path_segments;

/// Counts strategies at or below `prefix` by rescanning the whole list as a test-only oracle.
fn naive_folder_counts(
    strategies: &[StrategyRow],
    filter: &PreparedFilter,
    prefix: &[String],
) -> (usize, usize) {
    let mut active = 0;
    let mut total = 0;
    for r in strategies {
        if !filter.counts(r) {
            continue;
        }
        let parts: Vec<&str> = path_segments(&r.folder_path).collect();
        if parts.len() >= prefix.len()
            && prefix
                .iter()
                .zip(parts.iter())
                .all(|(a, b)| a.as_str() == *b)
        {
            total += 1;
            if r.checked {
                active += 1;
            }
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

/// Builds a prepared filter with independently chosen values for each filter dimension.
fn filter(search: &str, kind: Option<u8>, dir: Option<bool>, only_active: bool) -> PreparedFilter {
    StrategyFilter {
        search: search.to_string(),
        kind,
        dir,
        only_active,
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

/// Applying search or active-only gates in `FolderCounts::add` would shrink captions while filtering.
#[test]
fn counting_ignores_the_search_text_and_only_active() {
    // The accumulator applies its own gate, so this pins which filters that gate honours: a
    // search matching nothing, plus `only_active`, must leave every folder caption untouched.
    let rows = corpus();
    let plain = accumulate(&rows, &filter("", None, None, false));
    let noisy = accumulate(&rows, &filter("no-such-name", None, None, true));
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
    assert!(include_str!("../tree/mod.rs").contains("self.start_stop_plan"));
}
