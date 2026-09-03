//! The strategy list's filter/sort pass and the memo that keeps it off the render path.
//!
//! Explicit imports, never `use super::*`: the parent re-exports `gpui::*`, whose own `test`
//! shadows the built-in attribute and makes `#[test]` expand recursively (CONTRIBUTING.md).

use moon_core::db::analytics::GroupStat;

use super::{
    SORT_NAME, StratListFilter, VisibleKey, VisibleRows, core_names_changed, filter_sort_indices,
    memo_is_fresh, restore_strat_sort,
};

/// A group with just the fields these tests filter and sort on.
fn g(name: &str, kind: &str, alive: Option<i64>, bl: i64, profit: f64) -> GroupStat {
    GroupStat {
        key: format!("{name}@1"),
        name: name.to_string(),
        kind: kind.to_string(),
        alive,
        bl,
        profit,
        ..Default::default()
    }
}

/// A group whose Core column either names one core or summarizes several.
fn core_group(core: &str, cores_n: i64) -> GroupStat {
    let mut group = g(core, "K", Some(2), 0, 0.0);
    group.core = core.to_string();
    group.cores_n = cores_n;
    group
}

/// `analytics/tuner/list/mod.rs:core_names_changed` must compare the set of names measured by
/// the Core column. Replacing its set comparison with row order would remeasure every glyph after
/// an otherwise identical report refresh, restoring the writer-path hitch this cache avoids.
#[test]
fn core_name_changes_ignore_row_order_and_multi_core_summaries() {
    let old = vec![core_group("Alpha", 1), core_group("Beta", 1)];
    let reordered = vec![core_group("Beta", 1), core_group("Alpha", 1)];
    let with_summary_only = vec![
        core_group("Beta", 1),
        core_group("Alpha", 1),
        core_group("Many cores", 2),
    ];
    let with_new_single_core = vec![
        core_group("Alpha", 1),
        core_group("Beta", 1),
        core_group("Gamma", 1),
    ];

    assert!(
        !core_names_changed(Some(&old), Some(&reordered)),
        "the same measured names in another row order retain the width cache"
    );
    assert!(
        !core_names_changed(Some(&old), Some(&with_summary_only)),
        "a multi-core summary is not a separately measured Core-column name"
    );
    assert!(
        core_names_changed(Some(&old), Some(&with_new_single_core)),
        "a newly measured single-core name needs a fresh width"
    );
    assert!(
        core_names_changed(Some(&old), Some(&[core_group("Alpha", 1)])),
        "a disappearing measured name also changes the rendered column"
    );
}

/// `analytics/tuner/list/mod.rs:core_names_changed` must treat an unpublished replacement as no
/// change. Treating `new == None` as changed clears the settled width cache for every failed or
/// split read, bringing the visible writer-path hitch back without new names to measure.
#[test]
fn unpublished_core_names_do_not_invalidate_a_settled_width() {
    let old = vec![core_group("Alpha", 1)];

    assert!(
        core_names_changed(None, Some(&old)),
        "the first published Core-column set needs an initial measurement"
    );
    assert!(
        !core_names_changed(Some(&old), None),
        "a failed or split replacement publishes no names and keeps the old width"
    );
}

/// The default key over group set `base`: no search, no kind, no list filter, active-only off,
/// unsorted.
fn key_over(base: usize) -> VisibleKey {
    VisibleKey {
        base,
        search_lower: String::new(),
        kind: None,
        lists: StratListFilter::All,
        active_only: false,
        sort: None,
    }
}

/// Build the common memo key for tests that do not vary the group-slice address.
fn key() -> VisibleKey {
    key_over(1)
}

/// Unchanged inputs must hit the cache; ANY change must miss it.
///
/// The filter/sort pass covers every group the replica holds, while hover and wheel events
/// repeatedly enter the render path. Plausible edit this catches: relaxing freshness to "a cache
/// exists" would make the list stop following its own filter bar.
#[test]
fn the_memo_is_reused_only_while_nothing_changed() {
    let cached = Some(VisibleRows {
        key: key(),
        idx: vec![0, 1, 2],
    });
    assert!(
        memo_is_fresh(cached.as_ref(), &key()),
        "same inputs, same data — no recompute"
    );
    assert!(
        !memo_is_fresh(None, &key()),
        "an empty cache is never fresh"
    );
    assert!(
        !memo_is_fresh(cached.as_ref(), &key_over(2)),
        "a new read is a new allocation, so the group set changed under the cache"
    );

    // Every filter input, one at a time.
    let mut k = key();
    k.search_lower = "btc".into();
    assert!(!memo_is_fresh(cached.as_ref(), &k), "search");
    let mut k = key();
    k.kind = Some("MoonShot".into());
    assert!(!memo_is_fresh(cached.as_ref(), &k), "kind");
    let mut k = key();
    k.lists = StratListFilter::Black;
    assert!(!memo_is_fresh(cached.as_ref(), &k), "coin lists");
    let mut k = key();
    k.active_only = true;
    assert!(!memo_is_fresh(cached.as_ref(), &k), "active only");
    let mut k = key();
    k.sort = Some(("profit".into(), true));
    assert!(!memo_is_fresh(cached.as_ref(), &k), "sort");
}

/// Active-only must not blank a list whose aliveness is simply unknown.
///
/// Plausible edit this catches: the `any(alive.is_some())` guard is dropped as redundant, and
/// a period with no strategies replica shows an empty table over rows that exist — read as
/// "these strategies are gone".
#[test]
fn active_only_falls_back_when_aliveness_is_unknown() {
    let all = vec![g("a", "K", None, 0, 1.0), g("b", "K", None, 0, 2.0)];
    let mut k = key();
    k.active_only = true;
    assert_eq!(
        filter_sort_indices(&all, &k),
        vec![0, 1],
        "aliveness is unknown for every row, so the filter has nothing to go on"
    );

    let known = vec![g("a", "K", Some(0), 0, 1.0), g("b", "K", Some(2), 0, 2.0)];
    assert_eq!(
        filter_sort_indices(&known, &k),
        vec![1],
        "once aliveness IS known, a deleted strategy is filtered out"
    );
}

/// The coin-list filter must not present "we cannot see the lists" as "no strategy has one".
#[test]
fn the_coin_list_filter_falls_back_when_no_list_is_visible() {
    let blind = vec![g("a", "K", Some(2), 0, 1.0), g("b", "K", Some(2), 0, 2.0)];
    let mut k = key();
    k.lists = StratListFilter::Black;
    assert_eq!(
        filter_sort_indices(&blind, &k),
        vec![0, 1],
        "no row carries a list count, so the filter cannot claim none has one"
    );

    let seen = vec![g("a", "K", Some(2), 0, 1.0), g("b", "K", Some(2), 4, 2.0)];
    assert_eq!(
        filter_sort_indices(&seen, &k),
        vec![1],
        "with counts visible the filter applies normally"
    );
}

/// Search and kind narrow the set, and the result is returned as indices into the input.
#[test]
fn search_and_kind_select_by_index() {
    let all = vec![
        g("BTC long", "MoonShot", Some(2), 0, 1.0),
        g("ETH long", "MoonHook", Some(2), 0, 2.0),
        g("BTC short", "MoonHook", Some(2), 0, 3.0),
    ];
    let mut k = key();
    k.search_lower = "btc".into();
    assert_eq!(filter_sort_indices(&all, &k), vec![0, 2]);
    k.kind = Some("MoonHook".into());
    assert_eq!(filter_sort_indices(&all, &k), vec![2]);
}

/// Saved sort state must restore a real column and reject a removed or mistyped key.
///
/// The oracle is the visible row order, not the tuple returned by the normalizer: names and
/// profits deliberately disagree, so accepting an unknown key would activate the consumer's
/// name-sort default and reverse the expected rows.
///
/// Breakage this pins: `list::restore_strat_sort` returning every saved key without validation.
/// A removed column id would then silently sort by strategy name after restart while its arrow
/// disappeared, making the user's saved method and the displayed method disagree.
#[test]
fn saved_strategy_sort_restores_only_real_columns() {
    let all = vec![
        g("A low", "K", Some(2), 0, 1.0),
        g("Z high", "K", Some(2), 0, 9.0),
    ];

    let mut invalid = key();
    invalid.sort = restore_strat_sort(Some(("removed.column".to_string(), false)));
    assert_eq!(
        filter_sort_indices(&all, &invalid),
        vec![1, 0],
        "an unknown key must restore profit descending, not fall through to name ascending"
    );

    let mut valid = key();
    valid.sort = restore_strat_sort(Some((SORT_NAME.to_string(), false)));
    assert_eq!(
        filter_sort_indices(&all, &valid),
        vec![0, 1],
        "a valid saved key and its ascending direction must survive"
    );
}

/// Unavailable mixed-quote money remains below comparable rows in both sort directions.
///
/// Reverting `list::filter_sort_indices` to `NaN => Equal` lets the key tie-break scatter mixed
/// quote rows through real Avg order values, so the visible sort no longer matches the column.
#[test]
fn unavailable_money_sorts_after_comparable_values() {
    let mut unavailable = g("A unavailable", "K", Some(2), 0, 0.0);
    unavailable.avg_order = f64::NAN;
    let mut low = g("B low", "K", Some(2), 0, 0.0);
    low.avg_order = 10.0;
    let mut high = g("C high", "K", Some(2), 0, 0.0);
    high.avg_order = 20.0;
    let all = vec![unavailable, low, high];

    let mut descending = key();
    descending.sort = Some(("analytics.col.avg_order".to_string(), true));
    assert_eq!(filter_sort_indices(&all, &descending), vec![2, 1, 0]);

    let mut ascending = key();
    ascending.sort = Some(("analytics.col.avg_order".to_string(), false));
    assert_eq!(filter_sort_indices(&all, &ascending), vec![1, 2, 0]);
}
