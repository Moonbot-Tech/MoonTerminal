//! Shared rendering and aggregation for per-core balances ("free / total" in USDT).
//!
//! Two surfaces, one vocabulary. [`summary_group`] is the account side of the panel footer: a
//! scope-wide aggregate, on purpose — with 60-200 cores in play a per-core breakdown is
//! unreadable there. The breakdown lives only in the Wallets core list, where [`figure`] renders
//! each core's amount with its own trust state.
//!
//! Trust itself is not decided here: [`moon_core::session::BalanceState`] is classified by the
//! core that owns the data, so this panel, the shell header and any future consumer agree. This
//! module aggregates and renders it.

use super::*;
use moon_core::session::BalanceState;
use rust_i18n::t;

/// Per-core subtotal: free/total balance in USDT plus how much it can be trusted.
///
/// Carries the core's display name so both surfaces can iterate this one slice instead of
/// re-joining it against the core list by id.
#[derive(Clone)]
pub(super) struct CoreAgg {
    /// Core identifier used by filtering and selection.
    pub(super) id: CoreId,
    /// Display name shown beside the balance.
    pub(super) name: String,
    /// Free balance in USDT (`global.free_usdt`).
    pub(super) free: f64,
    /// Total balance in USDT (`global.total_usdt`, includes unrealized PnL).
    pub(super) total: f64,
    /// Store-owned trust classification for `free` and `total`.
    pub(super) state: BalanceState,
}

/// Scope total plus the trust metadata needed to caption it honestly.
struct ScopeTotals {
    /// Sum of usable free balances in scope.
    free: f64,
    /// Sum of usable total balances in scope.
    total: f64,
    /// Cores with no snapshot yet — excluded from the sum. Transient: it clears on its own.
    awaiting: usize,
    /// Cores with a snapshot but no valid valuation — excluded from the sum. A later snapshot
    /// must supply complete pricing, so this is reported separately from `awaiting`.
    unpriced: usize,
    /// Cores included with a figure that the store classifies as stale.
    stale: usize,
    /// Cores that actually contributed. Zero means the sum is a placeholder, not a balance:
    /// rendering `0$` there would state "the account is empty" when the truth is "nothing has
    /// reported yet".
    counted: usize,
}

impl ScopeTotals {
    /// How many cores the scope covers.
    ///
    /// Derived rather than counted separately: every in-scope core lands in exactly one of the
    /// three buckets, so the caption cannot drift out of step with the sum it labels.
    fn cores(&self) -> usize {
        self.counted + self.awaiting + self.unpriced
    }
}

/// Panel-wide core filter: an empty selection means all cores.
///
/// The single definition of "in scope" — the footer summary, its total, the table's `collect`
/// and the empty-state check all call this, or they would drift into disagreeing about what the
/// panel is currently showing.
pub(super) fn in_scope(sel: &HashSet<CoreId>, id: CoreId) -> bool {
    sel.is_empty() || sel.contains(&id)
}

/// Money formatter for balance surfaces: a dash instead of `NaN`/`inf`.
///
/// A dash is more honest than `inf$` in a terminal where people act on the number. Shared with
/// the footer's Σ so one row cannot print `inf` beside a "—".
///
/// Guards only NON-FINITE values. A scope figure must additionally go through
/// [`scope_amount_text`], which knows that a `0.0` from an empty sum means "nothing reported".
pub(super) fn money_or_dash(v: f64) -> String {
    if v.is_finite() {
        money(v)
    } else {
        DASH.to_string()
    }
}

/// Rendered when there is no usable figure at all. Shared so the footer's Σ says "no data" with
/// the same glyph as the balance group beside it.
pub(super) const DASH: &str = "—";

/// One core's balance text: "free / total", or a dash when there is no usable figure.
fn agg_text(a: Option<&CoreAgg>) -> String {
    match a {
        Some(a) if a.state.has_value() => {
            format!("{} / {}", money_or_dash(a.free), money_or_dash(a.total))
        }
        _ => DASH.to_string(),
    }
}

/// Localized marker for a non-live figure, or `None` when it is current. `Awaiting` needs none:
/// its dash already says the balance has not arrived.
fn state_marker(a: Option<&CoreAgg>) -> Option<String> {
    let key = match a?.state {
        BalanceState::Stale => "assets.balance_stale",
        BalanceState::Unpriced => "assets.balance_unpriced",
        BalanceState::Live | BalanceState::Awaiting => return None,
    };
    Some(t!(key).to_string())
}

/// Small muted caption — the trust markers and the total's labels.
fn caption(text: String, p: MoonPalette, cx: &App) -> Div {
    div()
        .text_size(design::t_caption(cx))
        .text_color(rgb(p.text_muted))
        .child(text)
}

/// A money figure at body size in the caller's colour — the companion to [`caption`].
///
/// Every balance figure on the panel goes through here (per-core, scope total, and the footer's
/// Σ), so "muted means do not trust this at face value" is one rendering rather than three
/// hand-copied ones that can drift apart in tone.
pub(super) fn amount(text: String, color: u32, cx: &App) -> Div {
    div()
        .text_size(design::t_body(cx))
        .text_color(rgb(color))
        .child(text)
}

/// One core's balance figure with its trust marker, for the Wallets core list.
///
/// Not reused by [`summary_group`]: this renders ONE core's [`BalanceState`], while an aggregate's
/// trust is a property of the whole scope and is not a single state. `None` renders the same dash
/// as a core with no snapshot.
pub(super) fn figure(a: Option<&CoreAgg>, p: MoonPalette, cx: &App) -> impl IntoElement {
    let color = if a.is_some_and(|a| a.state.is_current()) {
        p.text
    } else {
        p.text_muted
    };
    h_flex()
        .items_center()
        .gap(design::ui_px(cx, 4.0))
        .child(amount(agg_text(a), color, cx))
        .when_some(state_marker(a), |el, marker| {
            el.child(caption(marker, p, cx))
        })
}

/// Scope total with trust metadata.
///
/// Cores without a usable figure are NOT summed: their balance is unknown, and a silent zero
/// would understate the total. They are counted as awaiting or unpriced so the caller can say the
/// total is partial instead of presenting it as complete.
fn scope_totals(aggs: &[CoreAgg], sel: &HashSet<CoreId>) -> ScopeTotals {
    let mut out = ScopeTotals {
        free: 0.0,
        total: 0.0,
        awaiting: 0,
        unpriced: 0,
        stale: 0,
        counted: 0,
    };
    for a in aggs.iter().filter(|a| in_scope(sel, a.id)) {
        // A figure that cannot be added is not a contribution, whatever its state says. The
        // producer validates these values, but keeping the check structural prevents a malformed
        // aggregate from being counted while its arithmetic is silently skipped.
        let usable = a.state.has_value() && a.free.is_finite() && a.total.is_finite();
        if !usable {
            if a.state == BalanceState::Awaiting {
                out.awaiting += 1;
            } else {
                out.unpriced += 1;
            }
            continue;
        }
        out.counted += 1;
        if a.state == BalanceState::Stale {
            out.stale += 1;
        }
        out.free += a.free;
        out.total += a.total;
    }
    out
}

/// Whether every in-scope core contributed a figure classified as live.
///
/// This is the clip-proof half of the trust signal. The footer renders a complete total at full
/// strength and anything less in the muted tone; colour cannot be pushed off the edge of a narrow
/// dock, so it still warns after the captions that explain WHY have been clipped away.
fn is_complete(t: &ScopeTotals) -> bool {
    t.counted > 0 && t.awaiting == 0 && t.unpriced == 0 && t.stale == 0
}

/// One scope amount as text — a dash whenever nothing was counted.
///
/// [`money_or_dash`] alone is not enough here: an empty sum is a perfectly finite `0.0`, and
/// printing `0$` would state "the account is empty" when the truth is "nothing has reported yet".
/// Shared by the rendered row and the tooltip so the two cannot disagree about that distinction.
fn scope_amount_text(t: &ScopeTotals, v: f64) -> String {
    if t.counted > 0 {
        money_or_dash(v)
    } else {
        DASH.to_string()
    }
}

/// The trailing facts in the order they may disappear: what the total covers, then every reason
/// it may be incomplete.
///
/// Summing whatever happens to be available and rendering it like a complete figure is exactly
/// how a wrong number gets trusted, so each exclusion is named. The core count renders even at
/// zero — an empty scope then says "no cores" instead of leaving a bare dash unexplained.
/// `Unpriced` is reported apart from `Awaiting` because it needs valid pricing rather than merely
/// the first snapshot.
///
/// Returns fully localized text, `· ` separator included, so the row and the tooltip render
/// byte-identical strings — that is what makes "the tooltip carries everything that can clip"
/// checkable in a test.
fn facts(t: &ScopeTotals) -> Vec<String> {
    [
        (t.cores(), "assets.cores_n", true),
        (t.stale, "assets.balances_stale_n", false),
        (t.unpriced, "assets.balances_unpriced_n", false),
        (t.awaiting, "assets.balances_awaiting", false),
    ]
    .into_iter()
    .filter(|(n, _, always)| *n > 0 || *always)
    .map(|(n, key, _)| format!("· {}", t!(key, n = n)))
    .collect()
}

/// Build tooltip text with free first, total after the slash, then every trailing fact.
///
/// The recovery path for a narrow dock. The row clips its least important content from the right,
/// so on a cramped panel the reason a total is partial can sit off-screen while the total itself
/// is still on it; the tooltip is where that reason stays reachable. The closing line names the
/// scope because the same row also carries the table's Σ, and the two are different quantities.
///
/// Takes the already-rendered `tail` rather than recomputing it, so "the tooltip repeats the row
/// verbatim" holds by construction instead of by both sides calling the same function the same way.
fn summary_tooltip(t: &ScopeTotals, tail: &[String]) -> String {
    // Same order as the rendered footer and the shell header: free, then total.
    let mut out = format!(
        "{} {} / {} {}",
        t!("assets.balances_free"),
        scope_amount_text(t, t.free),
        t!("assets.balances_total"),
        scope_amount_text(t, t.total),
    );
    for fact in tail {
        out.push(' ');
        out.push_str(fact);
    }
    out.push('\n');
    out.push_str(&t!("assets.balances_scope_hint"));
    out
}

/// Render the account side of the footer as free then total across every in-scope core.
///
/// Deliberately an aggregate only: this terminal is routinely driven with 60+ cores and up to
/// ~200, so a chip per core is unreadable at real scale. Per-core figures live in the "Wallets"
/// core list of a separate Assets window ([`figure`]), and narrowing the core filter re-scopes
/// this total.
///
/// Returns the group ALONE: the divider separating it from the table's Σ is a property of the
/// footer row, not of this group (see `AssetsView::footer`).
///
/// Narrow-dock behaviour is the point of the two-child split below: the essentials resist
/// shrinking while the tail is one clipping box. Trust survives that clipping twice over —
/// through the amount colour ([`is_complete`]) and through the tooltip ([`summary_tooltip`]).
pub(super) fn summary_group(aggs: &[CoreAgg], sel: &HashSet<CoreId>, cx: &App) -> impl IntoElement {
    let p = MoonPalette::active(cx);
    let totals = scope_totals(aggs, sel);
    let tail = facts(&totals);
    let tip = summary_tooltip(&totals, &tail);
    let color = if is_complete(&totals) {
        p.text
    } else {
        p.text_muted
    };
    let figure = |v: f64| amount(scope_amount_text(&totals, v), color, cx).flex_none();
    h_flex()
        .id("assets-balance-summary")
        .items_center()
        .min_w_0()
        .gap(design::ui_px(cx, 5.0))
        .tooltip(move |_window, cx| {
            cx.new(|_| moon_ui::MoonTooltipView::new(tip.clone()))
                .into()
        })
        // Free first, total after the slash — the same reading order as the shell header's
        // "Balance: free / total", so the two surfaces state one account the same way round.
        // `flex_none` keeps the essential amount from shrinking before the trailing detail.
        .child(
            h_flex()
                .flex_none()
                .items_center()
                .gap(design::ui_px(cx, 5.0))
                .child(caption(t!("assets.balances_free").to_string(), p, cx).flex_none())
                .child(figure(totals.free)),
        )
        // Yielding tail — ONE clipping box, not two shrinkable siblings. Flex shrink is
        // distributed proportionally to base size, so siblings would erode the total and the
        // facts simultaneously; with every child `flex_none` inside a single `min_w_0` +
        // `overflow_hidden` box, LTR clipping IS the priority order — the rightmost facts go
        // first, the total last. No measurement, no width breakpoints.
        .child(
            h_flex()
                .min_w_0()
                .overflow_hidden()
                .items_center()
                .gap(design::ui_px(cx, 5.0))
                .child(caption(format!("/ {}", t!("assets.balances_total")), p, cx).flex_none())
                .child(figure(totals.total))
                .children(tail.into_iter().map(|f| caption(f, p, cx).flex_none())),
        )
}

#[cfg(test)]
/// Unit checks for scope filtering, trust-aware totals, and defensive formatting.
mod tests {
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
}
