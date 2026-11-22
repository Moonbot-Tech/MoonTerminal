//! Shared rendering and aggregation for per-core balances ("free / total" in USDT).
//!
//! Two surfaces, one vocabulary. [`summary_group`] is the account side of the panel footer: a
//! scope-wide aggregate, on purpose — with 60-200 cores in play a per-core breakdown is
//! unreadable there. The breakdown lives only in the Wallets core list, where [`figure`] renders
//! each core's amount with its own trust state.
//!
//! Trust itself is not decided here: [`moon_core::session::BalanceState`] is classified by the
//! core that owns the data, so this panel, the shell header and any future consumer agree. This
//! module renders it. The sum itself is [`moon_core::session::balances`], shared with the Telegram
//! Mini App: it folds cores sharing an exchange account, then adds up what the trust state allows.

use super::*;
use moon_core::config::TotalMode;
use moon_core::session::BalanceState;
use moon_core::session::balances::{
    AccountAggregate, BalanceFigures, FoldedGroup, TotalMember, aggregate_accounts,
};
use moon_core::venue::{AccountMergeKey, Brand};
use moon_ui::{MoonText, MoonTooltip};
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
    /// Which exchange account and wallet this balance is, or `None` when unknown.
    pub(super) merge: Option<AccountMergeKey>,
    /// How this core counts toward the footer total (persisted per core).
    pub(super) mode: TotalMode,
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
    /// Cores the user set to stay out of the total.
    excluded_by_user: usize,
    /// Names of those user-excluded cores, for the tooltip.
    excluded_names: Vec<String>,
    /// Which core was counted for each shared account, and which were folded into it.
    fold_detail: Vec<FoldedGroup>,
    /// Every core in scope — the count a user checks against their own cores: "12 cores" is a
    /// fact about the window, while 10 independent contributions is a fact about accounts.
    ///
    /// Counted from the cores as they were passed in, never rebuilt from the fold's buckets. The
    /// fold leaves some cores in no bucket at all: a core with no usable reading on an account
    /// its sibling already reports is neither a contribution nor named as folded, and neither is
    /// any core but the first of a shared account where none has reported
    /// (`moon_core::session::balances::fold`). A sum of buckets therefore came up short exactly
    /// while a shared account was connecting.
    all_cores: usize,
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
/// total is partial instead of presenting it as complete. The arithmetic is
/// [`moon_core::session::balances::aggregate_accounts`]; this wrapper keeps the panel's
/// empty-selection-means-all filter, and the aggregate folds cores sharing one exchange account
/// into one contribution so that account's money is counted once.
fn scope_totals(aggs: &[CoreAgg], sel: &HashSet<CoreId>) -> ScopeTotals {
    let members: Vec<TotalMember> = aggs
        .iter()
        .filter(|a| in_scope(sel, a.id))
        .map(|a| TotalMember {
            name: a.name.clone(),
            figures: BalanceFigures {
                state: a.state,
                free: a.free,
                total: a.total,
            },
            merge: a.merge.clone(),
            mode: a.mode,
        })
        .collect();
    let AccountAggregate {
        sum: summed,
        folded,
        excluded_by_user,
    } = aggregate_accounts(&members);
    ScopeTotals {
        all_cores: members.len(),
        free: summed.free.unwrap_or(0.0),
        total: summed.total.unwrap_or(0.0),
        awaiting: summed.awaiting as usize,
        unpriced: summed.unpriced as usize,
        stale: summed.stale as usize,
        counted: summed.counted as usize,
        excluded_by_user: excluded_by_user.len(),
        excluded_names: excluded_by_user,
        fold_detail: folded,
    }
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
/// Returns fully localized text, `· ` separator included, as the row draws it. The tooltip
/// repeats the stale, unpriced and awaiting facts word for word, without the separator
/// ([`summary_tip`]) — that is what makes "the tooltip carries everything that can clip"
/// checkable in a test.
fn facts(t: &ScopeTotals) -> Vec<String> {
    // Every core in the window, never the number of contributions: the row used to print the
    // contribution count as "Cores" beside a separate "same account: N", which read as 10 cores
    // when the window held 12. Which cores share an account is the tooltip's job.
    [
        (t.all_cores, "assets.balances_cores_n", true),
        (
            t.excluded_by_user,
            "assets.balances_excluded_n",
            t.excluded_by_user > 0,
        ),
        (t.stale, "assets.balances_stale_n", t.stale > 0),
        (t.unpriced, "assets.balances_unpriced_n", t.unpriced > 0),
        (t.awaiting, "assets.balances_awaiting", t.awaiting > 0),
    ]
    .into_iter()
    .filter(|(_, _, shown)| *shown)
    .map(|(n, key, _)| format!("· {}", t!(key, n = n)))
    .collect()
}

/// What the footer tooltip says, before it is drawn: the amounts as label/figure rows, then what
/// they cover in short lines.
///
/// The row is terse (`free … / Total … · cores: 12`); the tooltip spells it out:
///
/// ```text
/// Free          1 200.00$
/// Total         3 400.00$
/// Cores: 12 · offline: 1
/// core-1 + core-2 — shared Bybit account, counted once
/// ```
///
/// Built apart from [`SummaryTipView`] so a test reads exactly what the hover says.
#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct SummaryTip {
    /// `(label, amount)` rows of the cores' balances: free, then total — what the row shows.
    rows: Vec<(String, String)>,
    /// Whether the total is complete and current ([`is_complete`]); anything less draws the
    /// amounts muted, the same trust signal the row gives.
    complete: bool,
    /// What the total covers: every core in scope, then every reason the total may be partial,
    /// in the row's own words — the facts a narrow dock clips off the row.
    facts: Vec<String>,
    /// One line each: the shared accounts, the Hyperliquid caveat, and the cores left out by
    /// their own setting. Empty for a scope with none of them.
    notes: Vec<String>,
}

/// Build the footer tooltip's content.
///
/// It is also the recovery path for a narrow dock, where the row clips its trailing facts: every
/// fact the row can lose is here too. The excluded-core count is carried by the list of those
/// cores, which says the same and more.
///
/// Args:
///     aggs: Per-core balance aggregates, the ones [`summary_group`] sums for the row.
///     sel: Retained core selection, or empty for every core.
///     marker: Workspace scope marker of the row, or `None`.
///
/// Returns:
///     The tooltip content.
pub(super) fn summary_tip_of(
    aggs: &[CoreAgg],
    sel: &HashSet<CoreId>,
    marker: Option<&ScopeMarker>,
) -> SummaryTip {
    let marker_facts = marker.map(ScopeMarker::facts).unwrap_or_default();
    summary_tip(&scope_totals(aggs, sel), &marker_facts)
}

/// [`summary_tip_of`] over already-folded totals and the marker's facts as the row renders them
/// (`· …`).
fn summary_tip(t: &ScopeTotals, marker_facts: &[String]) -> SummaryTip {
    let complete = is_complete(t);
    let rows = vec![
        (
            t!("assets.balances_tip_free").to_string(),
            scope_amount_text(t, t.free),
        ),
        (
            t!("assets.balances_total").to_string(),
            scope_amount_text(t, t.total),
        ),
    ];
    let mut facts = vec![t!("assets.cores_n", n = t.all_cores).to_string()];
    facts.extend(
        [
            (t.stale, "assets.balances_stale_n"),
            (t.unpriced, "assets.balances_unpriced_n"),
            (t.awaiting, "assets.balances_awaiting"),
        ]
        .into_iter()
        .filter(|(n, _)| *n > 0)
        .map(|(n, key)| t!(key, n = n).to_string()),
    );
    facts.extend(
        marker_facts
            .iter()
            .map(|fact| fact.trim_start_matches("· ").to_string()),
    );
    // Name every shared account: the totals count it once, and only here does the user see
    // WHICH cores were folded, to check an automatic fold against their own accounts.
    let mut notes: Vec<String> = t
        .fold_detail
        .iter()
        .map(|group| {
            let mut cores = vec![group.kept.as_str()];
            cores.extend(group.folded.iter().map(String::as_str));
            let cores = cores.join(" + ");
            match group.brand {
                Some(brand) => t!(
                    "assets.balances_tip_shared",
                    cores = cores,
                    brand = brand.display()
                )
                .to_string(),
                None => t!("assets.balances_tip_shared_unknown", cores = cores).to_string(),
            }
        })
        .collect();
    if t.fold_detail
        .iter()
        .any(|group| group.brand == Some(Brand::Hyperliquid))
    {
        notes.push(t!("assets.balances_folded_hl_caveat").to_string());
    }
    if !t.excluded_names.is_empty() {
        notes.push(
            t!(
                "assets.balances_excluded_list",
                cores = t.excluded_names.join(", ")
            )
            .to_string(),
        );
    }
    SummaryTip {
        rows,
        complete,
        facts,
        notes,
    }
}

/// Widest the footer tooltip grows, in design pixels: room for a shared-account line on one line.
const TIP_MAX_W: f32 = 360.0;

/// The footer tooltip: [`SummaryTip`] in the terminal's own tooltip box, kept current.
///
/// Words in the UI face, figures in mono — the rule every tooltip here follows
/// (`panels::common::text_tooltip`). The rows are two columns rather than one string per row:
/// every amount ends at one edge, so the figures read down as a column whatever their labels'
/// lengths, and no row wraps. The lines under them use the box's secondary text, the size and tone
/// `MoonTooltip` gives its own detail line.
///
/// GPUI builds a tooltip's view once, when the hover delay ends, while free balances move about
/// once a second: a snapshot would soon disagree with the row it half covers. So the view reads
/// the panel's footer ([`AssetsView::footer_tip`]) itself — as it opens, and again every time the
/// panel repaints. Nothing is built while no tooltip is open: the row never folds the balances a
/// second time for a tooltip nobody asked for, and a balance that moved during the hover delay
/// cannot open the tooltip on the figure from before it.
struct SummaryTipView {
    /// What the tooltip says now.
    tip: SummaryTip,
    /// Refreshes `tip` when the panel repaints; `None` once the panel is gone.
    _refresh: Option<Subscription>,
}

impl SummaryTipView {
    /// Build the tooltip view for the footer of `panel`.
    ///
    /// Args:
    ///     panel: The Assets panel whose footer is hovered.
    ///     cx: Context of the new view.
    ///
    /// Returns:
    ///     The view showing the panel's footer as it is now, subscribed to the panel's repaints
    ///     while the panel lives; an empty tooltip if the panel is already gone.
    fn new(panel: &WeakEntity<AssetsView>, cx: &mut Context<Self>) -> Self {
        let panel = panel.upgrade();
        let tip = panel
            .as_ref()
            .map(|panel| panel.read(cx).footer_tip())
            .unwrap_or_default();
        let refresh = panel.map(|panel| {
            cx.observe(&panel, |this: &mut Self, panel, cx| {
                let tip = panel.read(cx).footer_tip();
                if tip != this.tip {
                    this.tip = tip;
                    cx.notify();
                }
            })
        });
        Self {
            tip,
            _refresh: refresh,
        }
    }
}

impl Render for SummaryTipView {
    /// Draw the amounts as two columns, then the facts and notes in the box's secondary text.
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = MoonPalette::active(cx);
        let tip = &self.tip;
        // A partial total mutes its labels too: a label brighter than its amount would put the
        // weight on the words.
        let (label_color, figure_color) = if tip.complete {
            (p.text_soft, p.text)
        } else {
            (p.text_muted, p.text_muted)
        };
        // `MoonTooltip`'s own metrics: 10.5/14 for its text, 9.5/13 for its detail line.
        let cell = |text: &str, mono: bool, color: u32| {
            MoonText::new(text.to_string())
                .color(color)
                .font_size(10.5)
                .line_height(14.0)
                .weight(if mono { 500.0 } else { 400.0 })
                .mono(mono)
                .uppercase(false)
        };
        let small = |text: String| {
            MoonText::new(text)
                .color(p.text_muted)
                .font_size(9.5)
                .line_height(13.0)
                .weight(400.0)
                .mono(false)
                .uppercase(false)
        };
        // A column of one kind of cell: the labels, or the amounts ending at one edge.
        let column = |pick: fn(&(String, String)) -> &str, mono: bool, color: u32| {
            v_flex()
                .when(mono, |col| col.items_end())
                .children(tip.rows.iter().map(|row| cell(pick(row), mono, color)))
        };
        let figures = h_flex()
            .items_start()
            .gap(design::ui_px(cx, 16.0))
            .child(column(|(label, _)| label, false, label_color))
            .child(column(|(_, amount)| amount, true, figure_color));
        // One line of facts that wraps only BETWEEN facts: each is unbreakable, so "offline: 2"
        // never splits across lines.
        let facts = h_flex().flex_wrap().gap_x(design::ui_px(cx, 3.0)).children(
            tip.facts.iter().enumerate().map(|(i, fact)| {
                small(if i == 0 {
                    fact.clone()
                } else {
                    format!("· {fact}")
                })
            }),
        );
        MoonTooltip::empty("assets-balance-tip")
            .mono(false)
            .max_width(TIP_MAX_W)
            .child(figures)
            .child(
                v_flex()
                    .mt(design::ui_px(cx, 8.0))
                    .gap(design::ui_px(cx, 1.0))
                    .child(facts)
                    .children(tip.notes.iter().map(|note| small(note.clone()).wrap())),
            )
    }
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
/// through the amount colour ([`is_complete`]) and through the tooltip ([`summary_tip`]).
///
/// Args:
///     aggs: Per-core balance aggregates for the current scope.
///     sel: Retained core selection, or empty for every scoped core.
///     marker: Workspace scope marker for this group, or `None` for the global window.
///     panel: The panel the footer belongs to. The tooltip for these totals is not built here:
///         it reads the panel when it opens and on every repaint after ([`SummaryTipView`]).
///     cx: Application context used for rendering.
pub(super) fn summary_group(
    aggs: &[CoreAgg],
    sel: &HashSet<CoreId>,
    marker: Option<&ScopeMarker>,
    panel: WeakEntity<AssetsView>,
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
        .tooltip(move |_window, cx| cx.new(|cx| SummaryTipView::new(&panel, cx)).into())
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
