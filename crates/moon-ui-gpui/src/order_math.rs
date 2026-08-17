//! Local unrealized-PnL estimates over one [`OrderRow`].
//!
//! Pure, GPUI-free and theme-free: every function here is a function of the row alone, so the
//! surfaces that state money about an open order all quote the SAME arithmetic. Today those are
//! the Orders table's PNL / PNL % / PNL TP cells, the Orders sort comparator, and the chart's
//! open-order overlay.
//!
//! The estimates live in the UI crate rather than beside [`OrderRow`] in `moon-core` on purpose:
//! the wire carries no server PnL, so this is what the TERMINAL computes to match what the Assets
//! panel shows. That is a presentation-consistency contract between UI surfaces, not a core
//! invariant, and no `moon-core` consumer wants it.

use moon_core::feed::OrderRow;

/// Decimal places every surface states an unrealized-PnL amount or percent to.
///
/// It lives beside the arithmetic for the same reason the arithmetic is shared: two surfaces
/// quoting one figure to different precisions disagree about the number even when the maths
/// underneath is identical.
pub const MONEY_DECIMALS: usize = 2;

/// Is `v` a price these estimates can compute money from — finite AND strictly positive?
///
/// Stated as a positive predicate rather than the naive `v <= 0.0` rejection because every
/// comparison against NaN is FALSE: `NaN <= 0.0` does not reject, so a NaN price would flow into
/// the arithmetic below and the estimate would come back `Some(NaN)`. That value then reaches the
/// Orders sort, where `partial_cmp` answers `None` for it and the comparator degenerates into a
/// non-total order — which ABORTS the process rather than merely mis-sorting.
///
/// `is_finite` rather than `is_nan`: every estimate here SUBTRACTS one price from another, and
/// `inf - inf` is NaN, so admitting an infinite price would reopen the very same hole one step
/// later. Rejecting it costs nothing — a non-finite price is not a figure any cell can state.
fn usable_price(v: f64) -> bool {
    v.is_finite() && v > 0.0
}

/// Return the position quantity used by the PnL calculations.
///
/// When `filled` indicates an executed entry or active exit leg, use `remaining_size` when positive
/// and otherwise the original `size`. This preserves positions such as a sale from an already-held
/// asset whose `fill_pct` is zero. Before that state, use the filled entry quantity
/// (`size * fill_pct`). Return `None` when the resulting quantity is not positive.
pub(crate) fn position_qty(r: &OrderRow) -> Option<f64> {
    let qty = if r.filled {
        if r.remaining_size > 0.0 {
            r.remaining_size
        } else {
            r.size
        }
    } else {
        r.size * (r.fill_pct as f64) / 100.0
    };
    (qty > 0.0).then_some(qty)
}

/// Estimate unrealized PnL locally as `(mark - entry) * position_qty * direction`.
///
/// [`OrderRow`] carries no server PnL, so this calculates it just as the Assets panel does.
/// Return `None` when there is no position or either entry or mark price is unavailable.
///
/// `OrderRow::buy_price` is the normalized entry for both directions, and Moonbot models
/// `buy_order` as the entry leg for both long and short lifecycle phases. The raw core snapshot
/// stores a BREAK-EVEN price including round-trip commission in `buy_price`, so the feed converter
/// resolves the entry itself before constructing [`OrderRow`]: the entry leg's `mean_price`, then
/// its `actual_price`, then the raw `buy_price` (`feed/live/convert.rs::build_order_row`). It
/// deliberately does not consult the market's average position price: that value is shared by
/// every order on a coin, whereas this estimate must preserve each order's own entry.
///
/// `sell_price` is the exit target; for a profitable short it lies below entry. Treating it as a
/// short entry previously calculated PnL from the exit price and diverged from Assets, for example
/// VELVET showed -3.96 from `sell_price` versus about zero from the resolved entry.
pub(crate) fn order_pnl(r: &OrderRow) -> Option<f64> {
    let qty = position_qty(r)?;
    let entry = r.buy_price;
    let mark = r.price as f64;
    // Gated through [`usable_price`], not `<= 0.0`: every comparison against NaN is false, so a
    // naive `entry <= 0.0 || mark <= 0.0` lets a NaN price through and this returns `Some(NaN)`.
    // That value then reaches the Orders sort, where `partial_cmp` answers `None` for it and the
    // comparator degenerates into a non-total order — which aborts the process rather than merely
    // mis-sorting.
    if !usable_price(entry) || !usable_price(mark) {
        return None;
    }
    let dir = if r.is_short { -1.0 } else { 1.0 };
    Some((mark - entry) * qty * dir)
}

/// Estimate PnL if the position closes at its take-profit price (`sell_price`).
///
/// This uses the same formula as [`order_pnl`] with the take target in place of the current mark,
/// which shows the expected profit from a split grid of sell orders. Return `None` without a
/// position, entry price, or take-profit price.
pub(crate) fn order_pnl_at_tp(r: &OrderRow) -> Option<f64> {
    let qty = position_qty(r)?;
    let entry = r.buy_price;
    let tp = r.sell_price;
    // Gated through [`usable_price`] for the NaN reason spelled out in [`order_pnl`]: a naive
    // `entry <= 0.0 || tp <= 0.0` rejects neither NaN nor an infinity that `inf - inf` turns
    // into one.
    if !usable_price(entry) || !usable_price(tp) {
        return None;
    }
    let dir = if r.is_short { -1.0 } else { 1.0 };
    Some((tp - entry) * qty * dir)
}

/// Calculate directional PnL percentage as `(mark - entry) / entry * direction * 100`.
///
/// Return `None` under the same conditions as [`order_pnl`]. `buy_price` is the entry for both
/// directions.
pub(crate) fn order_pnl_pct(r: &OrderRow) -> Option<f64> {
    position_qty(r)?; // Apply the same in-position gate as `order_pnl`.
    let entry = r.buy_price;
    let mark = r.price as f64;
    // Gated through [`usable_price`] for the NaN reason spelled out in [`order_pnl`]. The division
    // adds nothing to that argument: `entry` is already known non-zero here, so the guard it needs
    // is the same one, not a weaker `!= 0.0`.
    if !usable_price(entry) || !usable_price(mark) {
        return None;
    }
    let dir = if r.is_short { -1.0 } else { 1.0 };
    Some((mark - entry) / entry * dir * 100.0)
}

#[cfg(test)]
mod tests;
