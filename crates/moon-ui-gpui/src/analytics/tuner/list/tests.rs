//! The strategy list's filter/sort pass and the memo that keeps it off the render path.
//!
//! Explicit imports, never `use super::*`: the parent re-exports `gpui::*`, whose own `test`
//! shadows the built-in attribute and makes `#[test]` expand recursively (CONTRIBUTING.md).

use std::collections::HashMap;
use std::sync::Arc;

use moon_core::db::analytics::GroupStat;

use super::super::auto_picks::{AutoClass, AutoFilter, AutoPicks, AutoView};
use super::super::ready::{ReadyClass, ReadyFilter, ReadyPass, ReadyView};
use super::super::{STRAT_MODES, StratMode};
use super::{
    SORT_AUTO, SORT_NAME, SORT_READY, StratListFilter, VisibleKey, VisibleRows, axis_sort,
    core_names_changed, filter_sort_indices, memo_is_fresh, ready_priority_indices,
    restore_strat_sort,
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
        conditions: Vec::new(),
        percent: false,
        sort: None,
        auto: AutoView::default(),
        ready: ReadyView::default(),
    }
}

/// Build the common memo key for tests that do not vary the group-slice address.
fn key() -> VisibleKey {
    key_over(1)
}

/// Numeric filters must share the memoized pipeline and leave the counter denominator stable.
#[test]
fn numeric_conditions_filter_visible_rows_and_invalidate_the_memo() {
    let all = vec![
        g("STRAT_A", "K", Some(2), 0, 0.0),
        g("STRAT_B", "K", Some(2), 0, 0.0),
    ];
    let mut all = all;
    all[0].n = 10;
    all[1].n = 30;
    let mut key = key();
    key.conditions.push(super::conditions::Condition {
        id: 0,
        column: 0,
        operator: super::conditions::Operator::Greater,
        number: "20".into(),
    });
    let (idx, available, pool) = filter_sort_indices(&all, &key);
    assert_eq!(idx, vec![1]);
    assert!(available);
    assert_eq!(pool, 2);
    let cached = VisibleRows {
        key: key_with(&key),
        idx,
        enabled_filter_available: available,
        pool,
    };
    key.conditions[0].number = "-".into();
    assert!(!memo_is_fresh(Some(&cached), &key));
    assert_eq!(filter_sort_indices(&all, &key).0, vec![0, 1]);
}

/// A conditions reset must not replace search-empty or pending classification explanations.
#[test]
fn conditions_empty_message_requires_rows_restorable_by_its_reset() {
    let mut row = g("STRAT_A", "K", Some(2), 0, 0.0);
    row.n = 10;
    let all = [row];
    let numeric = super::conditions::Condition {
        id: 0,
        column: 0,
        operator: super::conditions::Operator::Greater,
        number: "20".into(),
    };
    let mut state = key();
    state.conditions.push(numeric.clone());
    assert!(filter_sort_indices(&all, &state).0.is_empty());
    assert!(super::conditions_clear_restores_rows(&all, state, false));

    for cause in ["search", "auto-pending", "ready-pending"] {
        let mut state = key();
        state.conditions.push(numeric.clone());
        match cause {
            "search" => state.search_lower = "absent".into(),
            "auto-pending" => state.auto.filter = AutoFilter::Found,
            _ => state.ready.filter = ReadyFilter::Can,
        }
        assert!(filter_sort_indices(&all, &state).0.is_empty());
        assert!(
            !super::conditions_clear_restores_rows(&all, state, false),
            "resetting conditions cannot resolve {cause}"
        );
    }
}

/// A partial Can result that fails numeric conditions cannot prove the pending rows fail too.
#[test]
fn partial_readiness_keeps_computing_before_a_conditions_empty_message() {
    let mut counted = g("STRAT_A", "K", Some(2), 0, 0.0);
    counted.n = 100;
    let mut pending = g("STRAT_B", "K", Some(2), 0, 0.0);
    pending.n = 300;
    let all = [counted, pending];
    let mut state = key();
    state.conditions.push(super::conditions::Condition {
        id: 0,
        column: 0,
        operator: super::conditions::Operator::Greater,
        number: "200".into(),
    });
    state.ready.filter = ReadyFilter::Can;
    state.ready.classes = Arc::new(HashMap::from([(
        all[0].key.clone(),
        ReadyClass::Can { fit: 100 },
    )]));
    assert!(filter_sort_indices(&all, &state).0.is_empty());
    let mut without_conditions = key_with(&state);
    without_conditions.conditions.clear();
    assert_eq!(filter_sort_indices(&all, &without_conditions).0, vec![0]);
    assert!(
        !super::conditions_clear_restores_rows(&all, state, true),
        "the uncounted row may still pass both readiness and numeric conditions"
    );
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
        enabled_filter_available: true,
        pool: 3,
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
        filter_sort_indices(&all, &k).0,
        vec![0, 1],
        "aliveness is unknown for every row, so the filter has nothing to go on"
    );

    let known = vec![g("a", "K", Some(0), 0, 1.0), g("b", "K", Some(2), 0, 2.0)];
    assert_eq!(
        filter_sort_indices(&known, &k).0,
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
        filter_sort_indices(&blind, &k).0,
        vec![0, 1],
        "no row carries a list count, so the filter cannot claim none has one"
    );

    let seen = vec![g("a", "K", Some(2), 0, 1.0), g("b", "K", Some(2), 4, 2.0)];
    assert_eq!(
        filter_sort_indices(&seen, &k).0,
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
    assert_eq!(filter_sort_indices(&all, &k).0, vec![0, 2]);
    k.kind = Some("MoonHook".into());
    assert_eq!(filter_sort_indices(&all, &k).0, vec![2]);
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
        filter_sort_indices(&all, &invalid).0,
        vec![1, 0],
        "an unknown key must restore profit descending, not fall through to name ascending"
    );

    let mut valid = key();
    valid.sort = restore_strat_sort(Some((SORT_NAME.to_string(), false)));
    assert_eq!(
        filter_sort_indices(&all, &valid).0,
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
    assert_eq!(filter_sort_indices(&all, &descending).0, vec![2, 1, 0]);

    let mut ascending = key();
    ascending.sort = Some(("analytics.col.avg_order".to_string(), false));
    assert_eq!(filter_sort_indices(&all, &ascending).0, vec![1, 2, 0]);
}

/// Reverting to alive >= 1 keeps unchecked strategies visible with enabled-only selected.
#[test]
fn enabled_only_hides_unchecked_deleted_and_unknown_strategies() {
    let all = vec![
        g("deleted", "K", Some(0), 0, 1.0),
        g("unchecked", "K", Some(1), 0, 2.0),
        g("enabled", "K", Some(2), 0, 3.0),
        g("unknown", "K", None, 0, 4.0),
    ];
    let mut k = key();
    assert_eq!(filter_sort_indices(&all, &k), (vec![0, 1, 2, 3], true, 4));
    k.active_only = true;
    assert_eq!(filter_sort_indices(&all, &k), (vec![2], true, 1));
    assert_eq!(filter_sort_indices(&all[..2], &k), (vec![], true, 0));
}

/// Checking availability before other filters leaves an enabled checkbox silently bypassed
/// when search, type or coin-list filtering leaves only unknown replica statuses.
#[test]
fn enabled_availability_matches_the_filtered_candidates() {
    let all = vec![
        g("known", "Known", Some(2), 0, 1.0),
        g("unknown", "Unknown", None, 1, 2.0),
    ];
    let mut k = key();
    k.active_only = true;
    assert_eq!(filter_sort_indices(&all, &k), (vec![0], true, 1));
    k.search_lower = "unknown".into();
    assert_eq!(filter_sort_indices(&all, &k), (vec![1], false, 1));
    k.search_lower.clear();
    k.kind = Some("Unknown".into());
    assert_eq!(filter_sort_indices(&all, &k), (vec![1], false, 1));
    k.kind = None;
    k.lists = StratListFilter::Black;
    assert_eq!(filter_sort_indices(&all, &k), (vec![1], false, 1));
    assert_eq!(filter_sort_indices(&[], &k), (vec![], false, 0));
}

/// Four rows with one batch class each: a noise row, a confirmed one, a bare one, an unread one.
fn auto_rows() -> (Vec<GroupStat>, AutoView) {
    let all = vec![
        g("noise", "K", Some(2), 0, 4.0),
        g("useful", "K", Some(2), 0, 3.0),
        g("nothing", "K", Some(2), 0, 2.0),
        g("pending", "K", Some(2), 0, 1.0),
    ];
    let classes: HashMap<String, AutoClass> = [
        ("noise@1", AutoClass::Noise { n: 2, white: false }),
        ("useful@1", AutoClass::Useful { n: 3, white: false }),
        ("nothing@1", AutoClass::Nothing),
    ]
    .into_iter()
    .map(|(k, c)| (k.to_string(), c))
    .collect();
    let view = AutoView {
        generation: 1,
        filter: AutoFilter::All,
        classes: Arc::new(classes),
        missing: AutoClass::Pending,
    };
    (all, view)
}

/// "With a finding" must keep noise with candidates beside the confirmed ones, and
/// "confirmed" only those: swapping the two predicates hides exactly the rows the user asked
/// for, with a list that still looks plausible.
#[test]
fn auto_filter_keeps_findings_and_confirmed_only() {
    let (all, view) = auto_rows();
    let mut k = key();
    k.auto = view;
    assert_eq!(filter_sort_indices(&all, &k).0, vec![0, 1, 2, 3]);
    k.auto.filter = AutoFilter::Found;
    assert_eq!(filter_sort_indices(&all, &k).0, vec![0, 1]);
    k.auto.filter = AutoFilter::Confirmed;
    assert_eq!(filter_sort_indices(&all, &k).0, vec![1]);
}

/// "Shown X of Y" must count Y over the same rows with and without the auto-pick filter: a Y
/// taken from the unfiltered list under the filter (and from the searched list without it)
/// makes the denominator jump when the filter is toggled.
#[test]
fn the_counter_denominator_ignores_only_the_auto_filter() {
    let (all, view) = auto_rows();
    let mut k = key();
    k.auto = view;
    k.search_lower = "n".into();
    let (idx, _, pool) = filter_sort_indices(&all, &k);
    assert_eq!((idx.len(), pool), (3, 3));
    for filter in [AutoFilter::Found, AutoFilter::Confirmed] {
        k.auto.filter = filter;
        assert_eq!(filter_sort_indices(&all, &k).2, 3, "{filter:?}");
    }
}

/// The first click on "Auto" must put confirmed findings on top, noise next and the rest
/// after; ascending mirrors it. A comparator keyed on the candidate count alone would let a
/// big noise row outrank a confirmed one.
#[test]
fn auto_sort_puts_confirmed_then_noise_first() {
    let (all, view) = auto_rows();
    let mut k = key();
    k.auto = view;
    k.sort = Some((SORT_AUTO.to_string(), true));
    assert_eq!(filter_sort_indices(&all, &k).0, vec![1, 0, 2, 3]);
    k.sort = Some((SORT_AUTO.to_string(), false));
    assert_eq!(filter_sort_indices(&all, &k).0, vec![3, 2, 0, 1]);
    assert_eq!(
        restore_strat_sort(Some((SORT_AUTO.to_string(), true))),
        Some((SORT_AUTO.to_string(), true))
    );
}

/// A landed batch must re-sort the list: a memo that ignored the batch generation would keep
/// the "…" order after the answers arrived.
#[test]
fn a_new_batch_generation_misses_the_memo() {
    let (_, view) = auto_rows();
    let cached = VisibleRows {
        key: key(),
        idx: vec![],
        enabled_filter_available: false,
        pool: 0,
    };
    let mut k = key();
    assert!(memo_is_fresh(Some(&cached), &k));
    k.auto = view;
    assert!(!memo_is_fresh(Some(&cached), &k));
}

/// The auto-pick filter and an Auto-column sort are "By coin" controls: on every other axis a
/// narrowing choice (and a saved sort by Auto) leaves the list whole and in its own order, and
/// back on "By coin" the same choice narrows again. A filter that leaked to the other tabs hid
/// strategies behind a control those tabs do not draw.
#[test]
fn the_auto_filter_and_sort_act_on_by_coin_only() {
    let (all, view) = auto_rows();
    let saved_sort = Some((SORT_AUTO.to_string(), true));
    for chosen in [AutoFilter::Found, AutoFilter::Confirmed] {
        let mut picks = AutoPicks::default();
        picks.chosen = Some(chosen);
        for mode in STRAT_MODES {
            let mut k = key();
            k.auto = view.clone();
            k.auto.filter = picks.filter(mode.shows_auto());
            k.sort = axis_sort(&saved_sort, mode);
            let idx = filter_sort_indices(&all, &k).0;
            if mode == StratMode::Coins {
                assert_eq!(k.auto.filter, chosen);
                assert!(idx.len() < all.len(), "{chosen:?} narrows By coin");
                assert_eq!(idx[0], 1, "Auto sort puts the confirmed row first");
            } else {
                assert_eq!(
                    idx,
                    vec![0, 1, 2, 3],
                    "{chosen:?} must not narrow this axis"
                );
            }
        }
    }
    assert_eq!(
        axis_sort(&Some((SORT_NAME.to_string(), false)), StratMode::Time),
        Some((SORT_NAME.to_string(), false))
    );
}

/// Four rows with one readiness class each: few, can (small), can (big), pending.
fn ready_rows() -> (Vec<GroupStat>, ReadyView) {
    let all = vec![
        g("few", "K", Some(2), 0, 4.0),
        g("can_small", "K", Some(2), 0, 3.0),
        g("can_big", "K", Some(2), 0, 2.0),
        g("pending", "K", Some(2), 0, 1.0),
    ];
    let classes: HashMap<String, ReadyClass> = [
        ("few@1", ReadyClass::Few { fit: 10 }),
        ("can_small@1", ReadyClass::Can { fit: 21 }),
        ("can_big@1", ReadyClass::Can { fit: 34 }),
    ]
    .into_iter()
    .map(|(k, c)| (k.to_string(), c))
    .collect();
    let view = ReadyView {
        generation: 1,
        filter: ReadyFilter::All,
        classes: Arc::new(classes),
        missing: ReadyClass::Pending,
    };
    (all, view)
}

/// "Can tune" keeps the tunable strategies only; the counter's "of M" does not move with it;
/// the first click on the column puts the tunable ones first, the bigger sample leading.
#[test]
fn the_ready_filter_keeps_tunable_and_sorts_them_first() {
    let (all, view) = ready_rows();
    let mut k = key();
    k.ready = view;
    assert_eq!(filter_sort_indices(&all, &k).0, vec![0, 1, 2, 3]);
    k.ready.filter = ReadyFilter::Can;
    let (idx, _, pool) = filter_sort_indices(&all, &k);
    assert_eq!((idx, pool), (vec![1, 2], 4));
    k.ready.filter = ReadyFilter::All;
    k.sort = Some((SORT_READY.to_string(), true));
    assert_eq!(filter_sort_indices(&all, &k).0, vec![2, 1, 0, 3]);
    assert_eq!(
        restore_strat_sort(Some((SORT_READY.to_string(), true))),
        Some((SORT_READY.to_string(), true))
    );
}

/// The tuning filter and a Tuning-column sort are "Entry/Exit" controls: on every other axis
/// they leave the list whole and in its own order.
#[test]
fn the_ready_filter_and_sort_act_on_entry_exit_only() {
    let (all, view) = ready_rows();
    let saved_sort = Some((SORT_READY.to_string(), true));
    let mut pass = ReadyPass::default();
    for mode in STRAT_MODES {
        pass.chosen = Some(ReadyFilter::Can);
        let mut k = key();
        k.ready = view.clone();
        k.ready.filter = pass.filter(mode.shows_ready());
        k.sort = axis_sort(&saved_sort, mode);
        let idx = filter_sort_indices(&all, &k).0;
        if mode == StratMode::Ticks {
            assert_eq!(
                idx,
                vec![2, 1],
                "Can tune narrows Entry/Exit, big sample first"
            );
        } else {
            assert_eq!(idx, vec![0, 1, 2, 3], "must not narrow off Entry/Exit");
        }
    }
}

/// Before the pass answers, every row is pending and the default "can tune" filter draws
/// nothing; the pass still counts the rows the user would see first, in the list's own sort —
/// and by trade count when the list is sorted by the Tuning column it is filling.
#[test]
fn the_readiness_pass_counts_the_rows_the_list_would_draw_first() {
    let mut all = vec![
        g("b", "Spread", Some(2), 0, 1.0),
        g("a", "Spread", Some(2), 0, 3.0),
        g("c", "Spread", Some(2), 0, 2.0),
    ];
    for (group, n) in all.iter_mut().zip([5, 20, 9]) {
        group.n = n;
    }
    let mut k = key();
    k.ready.filter = ReadyFilter::Can;
    k.sort = Some((SORT_NAME.to_string(), false));
    assert!(
        filter_sort_indices(&all, &k).0.is_empty(),
        "nothing is drawn while every row is pending"
    );
    assert_eq!(ready_priority_indices(&all, key_with(&k)), vec![1, 0, 2]);
    k.sort = Some((SORT_READY.to_string(), true));
    assert_eq!(ready_priority_indices(&all, k), vec![1, 2, 0]);
}

/// A copy of `k`, the memo key being move-only.
fn key_with(k: &VisibleKey) -> VisibleKey {
    VisibleKey {
        base: k.base,
        search_lower: k.search_lower.clone(),
        kind: k.kind.clone(),
        lists: k.lists,
        active_only: k.active_only,
        conditions: k.conditions.clone(),
        percent: k.percent,
        sort: k.sort.clone(),
        auto: k.auto.clone(),
        ready: k.ready.clone(),
    }
}
