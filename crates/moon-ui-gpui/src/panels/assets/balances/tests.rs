// Explicit imports, NOT `use super::*`: the parent re-exports `gpui::*`, which carries its
// own `test` and shadows the built-in attribute — `#[test]` then expands recursively.
use super::{
    CoreAgg, DASH, agg_text, facts, in_scope, is_complete, money_or_dash, scope_amount_text,
    scope_totals, summary_tooltip,
};
use moon_core::session::{BalanceState, CoreId};
use std::collections::HashSet;

/// Build a test aggregate with a deterministic display name.
fn agg(id: CoreId, free: f64, total: f64, state: BalanceState) -> CoreAgg {
    CoreAgg {
        id,
        name: format!("core-{id}"),
        free,
        total,
        state,
    }
}

/// An empty filter means all cores — the same rule as `collect`.
#[test]
fn empty_selection_sums_every_core() {
    let aggs = vec![
        agg(1, 10.0, 20.0, BalanceState::Live),
        agg(2, 1.0, 2.0, BalanceState::Live),
    ];
    let t = scope_totals(&aggs, &HashSet::new());
    assert_eq!((t.free, t.total), (11.0, 22.0));
    assert_eq!((t.awaiting, t.unpriced, t.stale), (0, 0, 0));
}

/// A non-empty filter sums only the selected cores.
#[test]
fn selection_restricts_sum() {
    let aggs = vec![
        agg(1, 10.0, 20.0, BalanceState::Live),
        agg(2, 1.0, 2.0, BalanceState::Live),
    ];
    let sel = HashSet::from([2u64]);
    let t = scope_totals(&aggs, &sel);
    assert_eq!((t.free, t.total), (1.0, 2.0));
    assert!(in_scope(&HashSet::new(), 99));
    assert!(!in_scope(&sel, 1));
}

/// A core without a snapshot must not understate the total with a silent zero.
#[test]
fn awaiting_core_is_excluded_and_counted() {
    let aggs = vec![
        agg(1, 10.0, 20.0, BalanceState::Live),
        agg(2, 0.0, 0.0, BalanceState::Awaiting),
    ];
    let t = scope_totals(&aggs, &HashSet::new());
    assert_eq!((t.free, t.total), (10.0, 20.0));
    assert_eq!((t.awaiting, t.unpriced), (1, 0));
    assert_eq!(agg_text(Some(&aggs[1])), DASH);
}

/// An invalid valuation is not a zero balance: it is excluded and shown as a dash.
#[test]
fn unpriced_core_is_never_shown_as_zero() {
    let aggs = vec![
        agg(1, 10.0, 20.0, BalanceState::Live),
        agg(2, 0.0, 0.0, BalanceState::Unpriced),
    ];
    let t = scope_totals(&aggs, &HashSet::new());
    assert_eq!((t.free, t.total), (10.0, 20.0));
    assert_eq!((t.awaiting, t.unpriced), (0, 1));
    assert_eq!(agg_text(Some(&aggs[1])), DASH);
}

/// A stale snapshot still counts — it is the last known figure — but is reported as stale so
/// the total can be captioned instead of passing for current.
#[test]
fn stale_core_counts_but_is_reported() {
    let aggs = vec![
        agg(1, 5.0, 7.0, BalanceState::Stale),
        agg(2, 1.0, 1.0, BalanceState::Live),
    ];
    let t = scope_totals(&aggs, &HashSet::new());
    assert_eq!((t.free, t.total), (6.0, 8.0));
    assert_eq!(t.stale, 1);
    assert_eq!((t.awaiting, t.unpriced), (0, 0));
}

/// A core with no aggregate at all renders the same dash as one with no snapshot.
#[test]
fn missing_aggregate_renders_dash() {
    assert_eq!(agg_text(None), DASH);
}

/// Nothing has reported yet: the sum is a placeholder, and `counted == 0` is what makes the
/// footer render a dash instead of stating `0$` — "empty account" and "no data" must not look
/// the same.
#[test]
fn scope_with_no_usable_figures_counts_nothing() {
    let aggs = vec![
        agg(1, 0.0, 0.0, BalanceState::Awaiting),
        agg(2, 0.0, 0.0, BalanceState::Unpriced),
    ];
    let t = scope_totals(&aggs, &HashSet::new());
    assert_eq!(t.counted, 0);
    // Split on purpose: one core will report, the other will not until a rate exists.
    assert_eq!((t.awaiting, t.unpriced), (1, 1));
    assert_eq!((t.free, t.total), (0.0, 0.0));
}

/// A scope is only "complete" when every in-scope core contributed a current figure.
#[test]
fn counted_tracks_contributing_cores() {
    let aggs = vec![
        agg(1, 10.0, 20.0, BalanceState::Live),
        agg(2, 5.0, 5.0, BalanceState::Stale),
        agg(3, 0.0, 0.0, BalanceState::Awaiting),
    ];
    let t = scope_totals(&aggs, &HashSet::new());
    assert_eq!(t.counted, 2);
    assert_eq!((t.stale, t.awaiting, t.unpriced), (1, 1, 0));
}

/// Non-finite values neither poison the total nor render as `inf`/`NaN` — and, critically,
/// the core carrying them is NOT counted as a contributor. Counting it would let the total
/// pass for complete while one core's balance had silently vanished from it.
#[test]
fn non_finite_values_are_dropped() {
    let aggs = vec![
        agg(1, f64::NAN, f64::INFINITY, BalanceState::Live),
        agg(2, 3.0, 4.0, BalanceState::Live),
    ];
    let t = scope_totals(&aggs, &HashSet::new());
    assert_eq!((t.free, t.total), (3.0, 4.0));
    assert_eq!(t.counted, 1, "an unusable figure is not a contribution");
    assert_eq!(t.unpriced, 1);
    assert!(
        !is_complete(&t),
        "a dropped contribution is not a complete total"
    );
    // The scope still covers both cores, so the caption cannot understate what it describes.
    assert_eq!(t.cores(), 2);
    assert_eq!(money_or_dash(f64::NAN), DASH);
    assert_eq!(money_or_dash(f64::INFINITY), DASH);
}

/// The colour is the trust signal that cannot be clipped off a narrow dock, so it must go
/// muted for EVERY way a scope can fall short — not just the obvious disconnect.
#[test]
fn complete_only_when_every_core_is_current() {
    let live = vec![agg(1, 1.0, 2.0, BalanceState::Live)];
    assert!(is_complete(&scope_totals(&live, &HashSet::new())));

    for degraded in [
        BalanceState::Stale,
        BalanceState::Awaiting,
        BalanceState::Unpriced,
    ] {
        let aggs = vec![
            agg(1, 1.0, 2.0, BalanceState::Live),
            agg(2, 0.0, 0.0, degraded),
        ];
        assert!(
            !is_complete(&scope_totals(&aggs, &HashSet::new())),
            "{degraded:?} must not read as a complete total"
        );
    }
    // Nothing counted at all is not "complete and zero" either.
    let empty: Vec<CoreAgg> = Vec::new();
    assert!(!is_complete(&scope_totals(&empty, &HashSet::new())));
}

/// A clean scope still says what the total covers — otherwise a bare figure claims a scope
/// it never states.
#[test]
fn facts_always_name_the_core_count() {
    let aggs = vec![agg(1, 10.0, 20.0, BalanceState::Live)];
    let t = scope_totals(&aggs, &HashSet::new());
    assert_eq!(facts(&t).len(), 1);
}

/// Every exclusion gets its own caption, and the core count leads. Asserted on length and
/// position, never on translated text, so the test holds in any locale.
#[test]
fn facts_list_every_reason_in_priority_order() {
    let aggs = vec![
        agg(1, 10.0, 20.0, BalanceState::Live),
        agg(2, 5.0, 5.0, BalanceState::Stale),
        agg(3, 0.0, 0.0, BalanceState::Unpriced),
        agg(4, 0.0, 0.0, BalanceState::Awaiting),
    ];
    let t = scope_totals(&aggs, &HashSet::new());
    let f = facts(&t);
    assert_eq!(f.len(), 4, "cores + stale + unpriced + awaiting");
    // The leading entry is the core count, and it covers the degraded cores too — compared
    // against a healthy scope of the SAME size, so the check holds in any locale.
    let healthy: Vec<CoreAgg> = (1..=4)
        .map(|i| agg(i, 1.0, 1.0, BalanceState::Live))
        .collect();
    let healthy_facts = facts(&scope_totals(&healthy, &HashSet::new()));
    assert_eq!(healthy_facts.len(), 1);
    assert_eq!(f[0], healthy_facts[0]);
}

/// The load-bearing narrow-dock guarantee: the row clips its trailing facts from the right,
/// so anything that can vanish must remain readable in the tooltip. A total whose reason for
/// being partial must not lose its explanation when the trailing facts are clipped.
#[test]
fn tooltip_carries_every_fact_that_can_clip() {
    let aggs = vec![
        agg(1, 10.0, 20.0, BalanceState::Live),
        agg(2, 5.0, 5.0, BalanceState::Stale),
        agg(3, 0.0, 0.0, BalanceState::Unpriced),
        agg(4, 0.0, 0.0, BalanceState::Awaiting),
    ];
    let t = scope_totals(&aggs, &HashSet::new());
    let tail = facts(&t);
    let tip = summary_tooltip(&t, &tail);
    for fact in &tail {
        assert!(tip.contains(fact), "tooltip lost {fact:?}");
    }
}

/// The tooltip obeys the same "no data is not zero" rule as the row it explains.
#[test]
fn tooltip_shows_dash_when_nothing_reported() {
    let aggs = vec![agg(1, 0.0, 0.0, BalanceState::Awaiting)];
    let t = scope_totals(&aggs, &HashSet::new());
    assert_eq!(scope_amount_text(&t, t.total), DASH);
    let tip = summary_tooltip(&t, &facts(&t));
    assert!(tip.contains(DASH));
    assert!(
        !tip.contains("0.0$"),
        "an unreported scope must not read as an empty one"
    );
}
