// Explicit imports, NOT `use super::*`: the parent re-exports `gpui::*`, which carries its
// own `test` and shadows the built-in attribute — `#[test]` then expands recursively.
use super::{
    CoreAgg, DASH, IdleNote, SummaryTip, agg_text, facts, in_scope, is_complete, money_or_dash,
    scope_amount_text, scope_totals, summary_tip,
};
use moon_core::session::{BalanceState, CoreId};
use std::collections::HashSet;

/// Everything the tooltip says, one line per row, fact and note — for `contains` checks.
fn flat(tip: &SummaryTip) -> String {
    let mut lines: Vec<String> = all_rows(tip)
        .map(|(label, amount)| format!("{label} {amount}"))
        .collect();
    lines.extend(tip.facts.iter().cloned());
    lines.extend(tip.notes.iter().cloned());
    lines.join("\n")
}

/// Every `(label, amount)` row of the tooltip, top to bottom.
fn all_rows(tip: &SummaryTip) -> impl Iterator<Item = &(String, String)> {
    tip.rows.iter().chain(&tip.idle_rows)
}

/// The tooltip of `t` with no scope marker and no idle money.
fn tooltip(t: &super::ScopeTotals) -> String {
    flat(&summary_tip(t, &[], &IdleNote::default()))
}

/// Build a test aggregate with a deterministic display name.
fn agg(id: CoreId, free: f64, total: f64, state: BalanceState) -> CoreAgg {
    CoreAgg {
        id,
        name: format!("core-{id}"),
        free,
        total,
        state,
        merge: None,
        mode: Default::default(),
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
    assert_eq!(t.all_cores, 2);
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
    let _locale = crate::test_locale::force("en");
    let aggs = vec![agg(1, 10.0, 20.0, BalanceState::Live)];
    let t = scope_totals(&aggs, &HashSet::new());
    assert_eq!(facts(&t).len(), 1);
}

/// Every exclusion gets its own caption, and the core count leads. Asserted on length and
/// position, never on translated text, so the test holds in any locale.
#[test]
fn facts_list_every_reason_in_priority_order() {
    let _locale = crate::test_locale::force("en");
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
/// so anything that can vanish must remain readable in the tooltip.
#[test]
fn tooltip_carries_every_fact_that_can_clip() {
    let _locale = crate::test_locale::force("en");
    let aggs = vec![
        agg(1, 10.0, 20.0, BalanceState::Live),
        agg(2, 5.0, 5.0, BalanceState::Stale),
        agg(3, 0.0, 0.0, BalanceState::Unpriced),
        agg(4, 0.0, 0.0, BalanceState::Awaiting),
    ];
    let t = scope_totals(&aggs, &HashSet::new());
    let tail = facts(&t);
    let marker = ["· mode: MANUAL".to_string()];
    let tip = summary_tip(&t, &marker, &IdleNote::default());
    // The core count is worded differently in the tooltip; every reason for a partial sum, and
    // the scope marker's facts, are repeated as the row says them.
    assert_eq!(tip.facts[0], rust_i18n::t!("assets.cores_n", n = 4));
    for fact in tail[1..].iter().chain(&marker) {
        let fact = fact.trim_start_matches("· ");
        assert!(tip.facts.iter().any(|f| f == fact), "tooltip lost {fact:?}");
    }
    assert!(
        !tip.complete,
        "a partial total is drawn muted in the tooltip too"
    );
}

/// The tooltip obeys the same "no data is not zero" rule as the row it explains.
#[test]
fn tooltip_shows_dash_when_nothing_reported() {
    let _locale = crate::test_locale::force("en");
    let aggs = vec![agg(1, 0.0, 0.0, BalanceState::Awaiting)];
    let t = scope_totals(&aggs, &HashSet::new());
    assert_eq!(scope_amount_text(&t, t.total), DASH);
    let tip = summary_tip(&t, &[], &IdleNote::default());
    assert!(tip.rows.iter().all(|(_, amount)| amount == DASH));
    assert!(tip.idle_rows.is_empty());
    assert!(
        !flat(&tip).contains("0.0$"),
        "an unreported scope must not read as an empty one"
    );
}

/// Breakage: the row printing the contribution count as "cores" — a 12-core window with two
/// cores folded into shared accounts read "Cores: 10". It must count every core in the window.
#[test]
fn the_row_counts_every_core_in_the_window() {
    let _locale = crate::test_locale::force("en");
    use moon_core::config::TotalMode;
    use moon_core::venue::Brand;
    let aggs = vec![
        on_account(1, 100.0, Brand::Bybit, TotalMode::Auto),
        on_account(2, 100.0, Brand::Bybit, TotalMode::Auto),
        agg(3, 10.0, 10.0, BalanceState::Live),
    ];
    let t = scope_totals(&aggs, &HashSet::new());
    assert_eq!(
        facts(&t),
        [format!(
            "· {}",
            rust_i18n::t!("assets.balances_cores_n", n = 3)
        )]
    );
}

/// Breakage: the count rebuilt from the fold's buckets — `fold_accounts` puts a core with no
/// usable reading in none of them when its account already has one, and likewise every core but
/// the first of a shared account where none has reported. Three cores then read "cores: 2"
/// exactly while a shared account was connecting.
#[test]
fn the_row_counts_cores_the_fold_puts_in_no_bucket() {
    let _locale = crate::test_locale::force("en");
    use moon_core::config::TotalMode;
    use moon_core::venue::Brand;
    let three = format!("· {}", rust_i18n::t!("assets.balances_cores_n", n = 3));
    let awaiting = |id| CoreAgg {
        state: BalanceState::Awaiting,
        ..on_account(id, 0.0, Brand::Bybit, TotalMode::Auto)
    };
    // One core of the shared account reports, its sibling has no snapshot yet.
    let one_waiting = vec![
        on_account(1, 100.0, Brand::Bybit, TotalMode::Auto),
        awaiting(2),
        agg(3, 10.0, 10.0, BalanceState::Live),
    ];
    let t = scope_totals(&one_waiting, &HashSet::new());
    assert_eq!(facts(&t)[0], three);
    assert_eq!(
        summary_tip(&t, &[], &IdleNote::default()).facts[0],
        rust_i18n::t!("assets.cores_n", n = 3)
    );
    // Neither core of the shared account has reported.
    let both_waiting = vec![
        awaiting(1),
        awaiting(2),
        agg(3, 10.0, 10.0, BalanceState::Live),
    ];
    let t = scope_totals(&both_waiting, &HashSet::new());
    assert_eq!(facts(&t)[0], three);
}

/// An aggregate on a known account, so `scope_totals` can fold it.
fn on_account(
    id: CoreId,
    total: f64,
    brand: moon_core::venue::Brand,
    mode: moon_core::config::TotalMode,
) -> CoreAgg {
    CoreAgg {
        merge: moon_core::venue::merge_key(
            Some(moon_core::venue::Venue {
                brand,
                kind: moon_core::venue::MarketKind::Futures,
            }),
            moon_core::account::AccountIdentity::from_auth("acct-a", "", 0),
        ),
        mode,
        ..agg(id, total, total, BalanceState::Live)
    }
}

/// Breakage: `balances.rs::scope_totals` summing the raw rows instead of `aggregate_accounts` —
/// two cores on one account would show that account twice in the footer. The tooltip must name
/// which core was folded into which.
#[test]
fn two_cores_on_one_account_are_one_account_in_the_footer() {
    let _locale = crate::test_locale::force("en");
    use moon_core::config::TotalMode;
    use moon_core::venue::Brand;
    let aggs = vec![
        on_account(1, 100.0, Brand::Bybit, TotalMode::Auto),
        on_account(2, 100.0, Brand::Bybit, TotalMode::Auto),
    ];
    let t = scope_totals(&aggs, &HashSet::new());
    assert_eq!((t.free, t.total), (100.0, 100.0));
    let tip = tooltip(&t);
    let line = rust_i18n::t!(
        "assets.balances_tip_shared",
        cores = "core-1 + core-2",
        brand = "Bybit"
    );
    assert!(
        tip.contains(line.as_ref()),
        "tooltip {tip:?} lacks the shared-account line"
    );
    assert!(tip.contains(rust_i18n::t!("assets.cores_n", n = 2).as_ref()));
    assert!(
        !tip.contains(rust_i18n::t!("assets.balances_folded_hl_caveat").as_ref()),
        "a non-Hyperliquid fold carries no Hyperliquid caveat"
    );
}

/// Breakage: `summary_tip` dropping the excluded list or `facts` the excluded count — a core
/// the user left out of the total would vanish from it without a word.
#[test]
fn an_excluded_core_is_named_in_facts_and_tooltip() {
    let _locale = crate::test_locale::force("en");
    use moon_core::config::TotalMode;
    use moon_core::venue::Brand;
    let aggs = vec![
        agg(1, 10.0, 10.0, BalanceState::Live),
        on_account(2, 50.0, Brand::Bybit, TotalMode::Exclude),
    ];
    let t = scope_totals(&aggs, &HashSet::new());
    assert_eq!(t.total, 10.0);
    let f = facts(&t);
    let excluded = format!("· {}", rust_i18n::t!("assets.balances_excluded_n", n = 1));
    assert!(f.contains(&excluded), "facts {f:?} lack the excluded count");
    // The tooltip names the excluded cores instead of repeating their count beside the list.
    let tip = tooltip(&t);
    let line = rust_i18n::t!("assets.balances_excluded_list", cores = "core-2");
    assert!(
        tip.contains(line.as_ref()),
        "tooltip {tip:?} lacks the excluded list"
    );
}

/// Breakage: `summary_tip` losing the Hyperliquid caveat — a folded Hyperliquid account
/// would read as exact when its cores report the shared wallet differently.
#[test]
fn a_hyperliquid_fold_carries_its_caveat() {
    let _locale = crate::test_locale::force("en");
    use moon_core::config::TotalMode;
    use moon_core::venue::Brand;
    let aggs = vec![
        on_account(1, 100.0, Brand::Hyperliquid, TotalMode::Auto),
        on_account(2, 90.0, Brand::Hyperliquid, TotalMode::Auto),
    ];
    let t = scope_totals(&aggs, &HashSet::new());
    let tip = tooltip(&t);
    assert!(tip.contains(rust_i18n::t!("assets.balances_folded_hl_caveat").as_ref()));
}

/// `total` USDT of idle money as of `as_of`, with nothing waiting on a core.
fn idle(total: f64, as_of: &str) -> IdleNote {
    IdleNote {
        total,
        as_of: Some(as_of.to_string()),
        waiting_for: 0,
    }
}

/// Breakage: `summary_tip` dropping the idle-money row, or printing it at zero — money no core
/// reports would be missing from the one place that says the total is not all the money, and a
/// zero row would be noise on a fleet with nothing to move.
#[test]
fn idle_money_is_named_only_when_there_is_some() {
    let _locale = crate::test_locale::force("en");
    let aggs = vec![agg(1, 10.0, 20.0, BalanceState::Live)];
    let t = scope_totals(&aggs, &HashSet::new());
    let label = rust_i18n::t!("assets.balances_tip_idle").to_string();
    let row = |tip: &SummaryTip| all_rows(tip).find(|(l, _)| *l == label).cloned();
    assert_eq!(
        row(&summary_tip(&t, &[], &idle(150.0, "14:05"))),
        Some((label.clone(), money_or_dash(150.0)))
    );
    let none = summary_tip(&t, &[], &idle(0.0, "14:05"));
    assert_eq!(row(&none), None);
    assert_eq!(none.rows.len(), 2, "free and total only");
    assert!(
        none.notes.is_empty(),
        "no figure, no age to state: {:?}",
        none.notes
    );
}

/// Breakage: idle money added to the total, in its row or in a sum beside it. The total follows
/// the live balance feed and the idle figure is a wallet snapshot: right after a transfer the
/// total already holds the moved money and the snapshot still does, so any sum of the two counts
/// it twice. The row stays apart from the cores' balances and says how old it is.
#[test]
fn idle_money_is_never_summed_with_the_total_and_states_its_age() {
    let _locale = crate::test_locale::force("en");
    let live = vec![agg(1, 10.0, 20.0, BalanceState::Live)];
    let tip = summary_tip(
        &scope_totals(&live, &HashSet::new()),
        &[],
        &idle(5.0, "14:05"),
    );
    assert_eq!(
        tip.rows,
        [
            (
                rust_i18n::t!("assets.balances_tip_free").to_string(),
                money_or_dash(10.0)
            ),
            (
                rust_i18n::t!("assets.balances_total").to_string(),
                money_or_dash(20.0)
            ),
        ],
        "the cores' balances stay as they are"
    );
    assert_eq!(
        tip.idle_rows,
        [(
            rust_i18n::t!("assets.balances_tip_idle").to_string(),
            money_or_dash(5.0)
        )],
        "one row: the snapshot, and no sum of it with the live total"
    );
    assert!(
        !flat(&tip).contains(&money_or_dash(25.0)),
        "20 + 5 must appear nowhere"
    );
    assert_eq!(
        tip.notes,
        [rust_i18n::t!("assets.balances_tip_idle_as_of", t = "14:05").to_string()]
    );
}

/// Breakage: idle money that waits for a core to identify itself going unmentioned — the footer
/// would then look exactly as it does when there is none. The note stands on its own, with or
/// without a figure beside it.
#[test]
fn the_tooltip_says_when_idle_money_waits_for_unidentified_cores() {
    let _locale = crate::test_locale::force("en");
    let live = vec![agg(1, 10.0, 20.0, BalanceState::Live)];
    let waiting = IdleNote {
        total: 0.0,
        as_of: None,
        waiting_for: 2,
    };
    let tip = summary_tip(&scope_totals(&live, &HashSet::new()), &[], &waiting);
    assert!(tip.idle_rows.is_empty());
    assert_eq!(
        tip.notes,
        [rust_i18n::t!("assets.balances_tip_idle_withheld", n = 2).to_string()]
    );
}

/// A clean scope says no more than it must: the amounts and the core count, no notes — the
/// row already names its subject, so a closing line restating it would only lengthen the hover.
#[test]
fn a_clean_scope_has_no_notes() {
    let _locale = crate::test_locale::force("en");
    let aggs = vec![agg(1, 10.0, 20.0, BalanceState::Live)];
    let tip = summary_tip(
        &scope_totals(&aggs, &HashSet::new()),
        &[],
        &IdleNote::default(),
    );
    assert_eq!(
        tip.facts,
        [rust_i18n::t!("assets.cores_n", n = 1).to_string()]
    );
    assert!(tip.notes.is_empty(), "{:?}", tip.notes);
}
