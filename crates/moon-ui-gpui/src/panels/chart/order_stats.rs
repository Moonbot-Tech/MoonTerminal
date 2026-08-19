//! Pure assembly of the chart overlay's open-order facts: which figures it states, the order in
//! which they may be dropped, and the width budget that decides how many survive.
//!
//! This is a SIBLING source to [`super::report_trades`], not an extension of it. That module reads
//! CLOSED trades out of the durable report replica; these figures come from the live session store
//! and describe orders that are still open. Sharing a status enum between the two would tie a
//! durable read's lifecycle to a live snapshot's, which is exactly the coupling the split avoids.
//!
//! The facts sit in a badge row absolutely positioned over the chart, so the row cannot measure
//! itself — but the render file DOES know the slot it sits in, and hands that down as a pixel
//! budget together with the measurement function for its own font. That is the difference from
//! `panels/report/totals.rs`, the in-repo reference for this pure/render split: the footer there
//! clips its tail at a flex edge, whereas here whole figures are dropped before they are ever
//! painted. A money figure clipped mid-number reads as a plausible WRONG number, so "never render a
//! partial figure" has to hold by construction rather than by a layout behaving as expected.
//!
//! The measurement arrives as a closure rather than being taken from the theme here, so this module
//! keeps no font knowledge of its own: the render file measures with the very size and weight it
//! draws at, and the tests measure deterministically. That is what lets the priority order and the
//! arithmetic be asserted with no window, no palette and no test app context.

use moon_core::feed::OrderRow;
use moon_core::util::fmt::{self, DeltaSign};
use rust_i18n::t;

use crate::order_math::{MONEY_DECIMALS, order_pnl, position_qty};

/// Colour role of one overlay figure, resolved against the palette by the render file.
///
/// A role rather than a resolved colour, for the same reason `panels/report/totals.rs` keeps one:
/// it is what lets the priority order and the sign of a figure be asserted with no active theme.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum StatTone {
    /// Ordinary supporting text, and money that rounds to exactly zero.
    Soft,
    /// Money above zero.
    Positive,
    /// Money below zero.
    Negative,
}

/// One rendered overlay figure, already localized.
pub(super) struct StatFact {
    /// Exactly the text the badge renders.
    pub(super) text: String,
    /// Colour role.
    pub(super) tone: StatTone,
    /// Whether this figure opens a new group of related figures.
    ///
    /// The render file draws a hairline rule before every such figure but the first one on screen,
    /// so the row reads as groups rather than as one undifferentiated run of badges. It lives here
    /// rather than in the render file because grouping is a property of what the figures MEAN — the
    /// profit and its percentage are one quantity stated twice — and that is this module's subject.
    pub(super) leads_group: bool,
}

/// The overlay's figures split by what a narrow chart may drop.
pub(super) struct OrderStats {
    /// Never dropped, however narrow the chart.
    pub(super) essential: Vec<StatFact>,
    /// Dropped from the end; earlier entries survive longer.
    pub(super) tail: Vec<StatFact>,
}

impl OrderStats {
    /// Iterate every surviving figure in render order.
    pub(super) fn iter(&self) -> impl Iterator<Item = &StatFact> {
        self.essential.iter().chain(&self.tail)
    }
}

/// Running aggregate over the orders that hold a position on this chart's market.
///
/// The exposure and the profit are counted over DIFFERENT sets on purpose. Exposure is defined by
/// the user as quantity times mark, and a row whose entry price has not arrived still has both of
/// those — withholding it would understate what is actually at risk. Profit and the percentage need
/// an entry price and simply have no value without one. Each figure therefore degrades on its own
/// inputs rather than dragging the others down with it.
#[derive(Default)]
struct Position {
    /// Open orders on this market, whether or not they hold anything.
    open: usize,
    /// Orders contributing to [`Self::mark_notional`].
    exposed: usize,
    /// Orders contributing to [`Self::pnl`] and [`Self::entry_notional`].
    valued: usize,
    /// Current notional: sum of `position_qty * mark`, in the quote currency.
    mark_notional: f64,
    /// Entry notional: sum of `position_qty * entry`, the denominator of the percentage.
    entry_notional: f64,
    /// Unrealized profit and loss, directional.
    pnl: f64,
}

/// Whether one row belongs to this chart's live open-order figures.
///
/// `job_is_done` is the authoritative closure flag — the core has finished with the order and is
/// only awaiting deferred removal — so a terminal row is still present in the batch for a moment.
/// Every consumer in this codebase that means OPEN orders filters it out (`strategies/tree/moon.rs`
/// spells the same predicate `open_orders_total`), and a count captioned "Orders: N" must agree
/// with them. The Orders TABLE deliberately does not filter: a row briefly outliving its order is
/// honest there, whereas a tally that counts it is simply wrong.
///
/// Args:
///     row: One order from the core's combined batch.
///     market: Data-key market of the chart's active pane.
///
/// Returns:
///     `true` for an order that is open on exactly this market.
fn is_open_here(row: &OrderRow, market: &str) -> bool {
    !row.job_is_done && row.market == market
}

/// Build one plain figure.
///
/// Args:
///     text: Localized text rendered on the badge.
///     tone: Theme-independent colour role.
///
/// Returns:
///     A figure ready for the essential head or the droppable tail.
fn fact(text: String, tone: StatTone, leads_group: bool) -> StatFact {
    StatFact {
        text,
        tone,
        leads_group,
    }
}

/// Choose a money colour role from the sign of the ROUNDED display amount.
///
/// Args:
///     sign: Classification returned by the shared amount formatter.
///
/// Returns:
///     Positive, negative, or soft-zero tone matching the rendered text.
fn money_tone(sign: DeltaSign) -> StatTone {
    sign.pick(StatTone::Positive, StatTone::Negative, StatTone::Soft)
}

/// Sum the live position across one market's open orders.
///
/// Quantity comes from [`position_qty`] and is always positive, so a short adds to exposure rather
/// than cancelling a long — the sum states how much is at risk, not a net direction. [`order_pnl`]
/// carries the direction itself, so the profit of a mixed book still nets.
///
/// Args:
///     rows: Open orders already filtered to this chart's market.
///
/// Returns:
///     The aggregate, whose three counts say how many orders each figure describes.
fn sum_position<'a>(rows: impl Iterator<Item = &'a OrderRow>) -> Position {
    let mut acc = Position::default();
    for row in rows {
        // Counted before any position gate: an order that is working but holds nothing is still an
        // open order, and the badge says how many there ARE, not how many are carrying risk.
        acc.open += 1;
        let Some(qty) = position_qty(row) else {
            continue;
        };
        let mark_notional = qty * row.price as f64;
        if row.price > 0.0 && mark_notional.is_finite() {
            acc.exposed += 1;
            acc.mark_notional += mark_notional;
        }
        // Profit needs an entry price that exposure does not; `order_pnl` owns that gate, so asking
        // it is also what keeps this module from re-stating the entry rule a second time.
        let entry_notional = qty * row.buy_price;
        if let Some(pnl) = order_pnl(row)
            && entry_notional.is_finite()
            && pnl.is_finite()
        {
            acc.valued += 1;
            acc.entry_notional += entry_notional;
            acc.pnl += pnl;
        }
    }
    acc
}

/// Assemble every figure the chart overlay states about this market's open orders.
///
/// The tail order is BOTH the reading order and the priority order, and the two are deliberately the
/// same list: a figure that survives a narrower chart must not also move sideways on it, or the
/// position a user has learnt to read a number from would depend on the window width. It runs from
/// what the position IS to how it is doing — the exposed notional first, then the unrealized profit,
/// then that profit as a percentage. The percentage closes the tail and is therefore the first
/// figure to go: its denominator is the ENTRY notional, a different quantity from the visible sum,
/// so a percentage read against that sum is a misreading the narrow case should not invite.
///
/// Args:
///     rows: Every order row of the chart's core; terminal rows and other markets are filtered here.
///     market: Data-key market of the chart's active pane.
///     budget: Width available to the WHOLE overlay, in the units `measure` returns. The head is
///         charged against it here, because only this function knows what the head says.
///     measure: Width of one rendered string, including the gap that follows it.
///
/// Returns:
///     The head and the surviving tail, or `None` when this market has no open order at all — the
///     overlay then states nothing rather than a row of zeroes, which is indistinguishable from a
///     flat position that genuinely exists.
pub(super) fn order_stats(
    rows: &[OrderRow],
    market: &str,
    budget: f32,
    measure: &dyn Fn(&str) -> f32,
) -> Option<OrderStats> {
    // One pass produces the count and every aggregate: this runs inside `render()`, which GPUI may
    // call per frame, so walking the core's whole order vector twice for the same predicate is a
    // cost paid on every repaint of every chart panel.
    let position = sum_position(rows.iter().filter(|row| is_open_here(row, market)));
    if position.open == 0 {
        return None;
    }
    // The count is the one figure that needs no price, no fill and no entry, and it names the scope
    // the money figures are about, so it is the head and is never dropped.
    let essential = vec![fact(
        t!("chart.order_stats.count", n = position.open).to_string(),
        StatTone::Soft,
        true,
    )];
    // The head is built HERE, so the caller could not have charged for it: it knows the room the
    // row has, not the strings this function chose to put in it. Charging it here is what keeps
    // `budget` meaning "room for the whole overlay" rather than "room for the tail", which is the
    // reading that would let head plus tail overrun the margin the caller reserved.
    let tail_budget = budget
        - essential
            .iter()
            .map(|item| measure(&item.text))
            .sum::<f32>();

    let mut candidates = Vec::new();
    // Orders that are working but hold nothing state their count alone. A `0.00` sum beside a live
    // order count is true but reads as "flat", and a `0.00%` asserts a position that does not
    // exist; absence is the honest rendering of a figure that has no value yet.
    if position.exposed > 0 {
        candidates.push(fact(
            t!(
                "chart.order_stats.sum",
                v = fmt::usd_grouped(position.mark_notional)
            )
            .to_string(),
            StatTone::Soft,
            true,
        ));
    }
    if position.valued > 0 {
        let (amount, sign) = fmt::signed_amount(position.pnl, MONEY_DECIMALS);
        candidates.push(fact(
            t!("chart.order_stats.pnl", v = amount).to_string(),
            money_tone(sign),
            true,
        ));
    }
    if position.valued > 0 && position.entry_notional > 0.0 {
        let pct = position.pnl / position.entry_notional * 100.0;
        if let Some((text, sign)) = fmt::signed_pct(pct, MONEY_DECIMALS) {
            // Same group as the profit it restates: one quantity, two units, no rule between them.
            candidates.push(fact(
                t!("chart.order_stats.pnl_pct", v = text).to_string(),
                money_tone(sign),
                false,
            ));
        }
    }

    OrderStats {
        essential,
        tail: fit(candidates, tail_budget, measure),
    }
    .into()
}

/// Take the leading figures that fit the budget, whole ones only.
///
/// Stops at the FIRST figure that does not fit rather than skipping it and trying the next. Skipping
/// would reorder the priority on screen, so a user who has learnt that the percentage is the last
/// figure would one day find it standing where the sum belongs.
///
/// Args:
///     candidates: Droppable figures in descending priority.
///     budget: Width available past the essential head.
///     measure: Width of one rendered string, including the gap that follows it.
///
/// Returns:
///     The surviving prefix, possibly empty.
fn fit(candidates: Vec<StatFact>, budget: f32, measure: &dyn Fn(&str) -> f32) -> Vec<StatFact> {
    let mut used = 0.0f32;
    let mut kept = Vec::with_capacity(candidates.len());
    for candidate in candidates {
        let cost = measure(&candidate.text);
        if used + cost > budget {
            break;
        }
        used += cost;
        kept.push(candidate);
    }
    kept
}

#[cfg(test)]
mod tests;
