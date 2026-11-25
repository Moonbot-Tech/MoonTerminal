//! How long each position has been held, printed on its own order line (#1402).
//!
//! The corner caption «Время в позиции» states one clock for the whole coin, taken from the oldest
//! holder. A coin can hold many orders at once, so the entry line of EVERY order that holds a
//! position carries its own: `12м 03с [4]`. Both read the same fill through the same predicate and
//! spell it with the same formatter, so the oldest order's line and the caption always agree.
//!
//! Backend-free on purpose: the order-line text is laid out by the shared text pass that the
//! DX11, Metal and wgpu backends all draw from, and the decision of what to print lives here
//! once rather than beside any of them.

use super::format::fmt_position_age;
use super::pnl::position_since;
use crate::chartdx::OrderLabel;

#[cfg(test)]
mod tests;

/// The step the order-line ages move by, in milliseconds: they print seconds for a whole minute.
///
/// The same one-second wall-clock grid the caption clock (`ChartLabelsCfg::countdown_clock_ms`)
/// takes while the position caption is drawn, so a caption and a line stating the same fill step
/// on the same instant.
const ORDER_AGE_STEP_MS: i64 = 1_000;

/// What an entry-line caption needs to print how long its order has held a position.
///
/// The fill is kept as a START rather than an age, so the age climbs against the clock without the
/// order walk re-running every second — the same reason `BasisStats::position_since_ms` is one.
#[derive(Clone, Debug, PartialEq)]
pub(in crate::chartdx) struct OrderAge {
    /// The order's entry fill, Unix ms, as the wire dated it.
    pub since_ms: i64,
    /// The order's `[N]` tag printed after the age, or empty for an order with no chart number.
    pub tag: String,
}

/// The wall clock floored to the order-age step.
///
/// Floored, never rounded: a stopwatch must not claim a second that has not passed — see
/// `fmt_position_age` on why the fill itself is left unfloored.
pub(in crate::chartdx) fn order_age_clock(now_ms: i64) -> i64 {
    now_ms.div_euclid(ORDER_AGE_STEP_MS) * ORDER_AGE_STEP_MS
}

/// Entry fills of the orders on `market` that hold a position, as `(uid, fill)`.
///
/// The predicate is the caption's own (`position_since`): an order whose position quantity is
/// positive and whose entry leg states a fill. A resting entry that filled nothing, a cancelled one
/// that only carries a close time, and a sale of coins already held (no entry leg) yield nothing —
/// never a start invented from when this terminal first saw the order. Rows the core has already
/// marked done are skipped, as the caption skips them.
///
/// Args:
///     rows: The core's order rows, unfiltered.
///     market: Market key the pane is showing.
///
/// Returns:
///     One pair per holder; empty when nothing on the market holds a position.
pub(in crate::chartdx) fn position_fills(
    rows: &[moon_core::feed::OrderRow],
    market: &str,
) -> Vec<(u64, i64)> {
    rows.iter()
        .filter(|r| r.market == market && !r.job_is_done)
        .filter_map(|r| position_since(r).map(|fill| (r.uid, fill)))
        .collect()
}

/// The entry-line caption of a position-holding order: its age, then its tag.
///
/// A fill dated a whole second or more in the future prints no age, exactly as the caption prints
/// none, and the line keeps the tag alone — the text it carried before this readout existed.
///
/// Args:
///     age: The order's fill and tag.
///     now_ms: The order-age clock, floored to the second ([`order_age_clock`]).
///
/// Returns:
///     `12м 03с [4]`, `12м 03с` with no tag, or the bare tag for a fill in the future.
pub(in crate::chartdx) fn order_age_text(age: &OrderAge, now_ms: i64) -> String {
    match fmt_position_age(age.since_ms, now_ms) {
        Some(figure) if age.tag.is_empty() => figure,
        Some(figure) => format!("{figure} {}", age.tag),
        None => age.tag.clone(),
    }
}

/// Re-spell every aged caption against `now_ms`, reporting whether a caption that was DRAWN in the
/// last text pass changed.
///
/// Only the strings that move are touched: a caption whose figure is the same as before keeps its
/// string, so its retained text run does not reshape. The answer is limited to drawn captions
/// because that is the only change worth a present: an aged caption scrolled out of the plot is
/// re-spelled all the same — cheap, a few strings a second — so the frame that brings it back
/// finds its figure current rather than the one it left with.
///
/// Args:
///     labels: The pane's order-line captions.
///     now_ms: The order-age clock, floored to the second.
///
/// Returns:
///     Whether a caption the last text pass drew now reads differently.
pub(in crate::chartdx) fn step_order_ages(labels: &mut [OrderLabel], now_ms: i64) -> bool {
    let mut drawn_changed = false;
    for label in labels.iter_mut() {
        let Some(age) = &label.age else {
            continue;
        };
        let text = order_age_text(age, now_ms);
        if text != label.text {
            label.text = text;
            drawn_changed |= label.age_drawn;
        }
    }
    drawn_changed
}
