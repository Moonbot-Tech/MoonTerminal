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
    crate::panels::footer_caption(text, p, cx)
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

/// Rendered width of the figure [`figure`] draws for `a`, in font-scaled pixels.
///
/// Lives here rather than at the caller so the MEASURED string stays the string this module
/// actually renders: `agg_text` and `state_marker` are private, and a hand-copied format in the
/// roster's width arithmetic would drift silently the day either of them changes — the same
/// argument `analytics::tuner::list::table::core_col_w` makes for measuring `core_label(g)`
/// rather than the raw name.
///
/// Mirrors [`figure`]'s own composition exactly: the amount at body size, plus — only when a
/// trust marker is drawn — the row's gap and that marker at caption size.
///
/// Args:
///     a: Core aggregate the figure is drawn for, or `None` for the dash.
///     cx: Application context used to measure text.
///
/// Returns:
///     The figure's width in rendered pixels, already font-scaled.
pub(super) fn figure_width(a: Option<&CoreAgg>, cx: &App) -> f32 {
    let mut width = design::mono_body_text_width(cx, &agg_text(a), FontWeight::NORMAL.0);
    if let Some(marker) = state_marker(a) {
        width += f32::from(design::ui_px(cx, 4.0))
            + design::mono_caption_text_width(cx, &marker, FontWeight::NORMAL.0);
    }
    width
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
///
/// Args:
///     aggs: Per-core balance aggregates for the current scope.
///     sel: Retained core selection, or empty for every scoped core.
///     marker: Workspace scope marker for this group, or `None` for the global window.
///     cx: Application context used for rendering.
pub(super) fn summary_group(
    aggs: &[CoreAgg],
    sel: &HashSet<CoreId>,
    marker: Option<&ScopeMarker>,
    cx: &App,
) -> impl IntoElement {
    let p = MoonPalette::active(cx);
    let totals = scope_totals(aggs, sel);
    let mut tail = facts(&totals);
    // The scope marker's own facts, appended to this row's tail — never a second copy, and never
    // rendered when the active preset hides nothing (decision 1).
    if let Some(marker) = marker {
        tail.extend(marker.facts());
    }
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
mod tests;
