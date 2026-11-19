//! Which of a detector's figures a row keeps: a side filter against the current price, a ranking,
//! and a count — per side when the row takes both.
//!
//! A figure's side is decided by its primary line's price at the moment asked for
//! ([`super::FoundShape::price_at`]: the level of a horizontal or squeeze, the projection of a sloped
//! line). A line exactly at the reference price counts as above it. Cost O(F log F).

use moon_core::config::auto_figures::{StrengthBasis, Take, TakeOrder, TakeSide};

use super::Found;

/// Indices into `found` of the figures `take` keeps, ranked: the above-side list first, then the
/// below-side one.
///
/// Args:
///     found: The detector's output.
///     take: The row's selection.
///     ref_price: The current price the sides and distances are measured from.
///     at_ms: The moment sloped lines are projected to (normally the last bar or "now").
///
/// Returns:
///     At most `take.count` indices per side. Empty when `count` is 0 or `ref_price` is not a
///     positive finite price — without a reference there is no side and no distance to rank by. A
///     figure with no usable price at `at_ms` is never kept.
pub fn select(found: &[Found], take: &Take, ref_price: f64, at_ms: f64) -> Vec<usize> {
    if take.count == 0 || !(ref_price.is_finite() && ref_price > 0.0) {
        return Vec::new();
    }
    let count = usize::from(take.count);
    // (index, distance to the reference, strength) per side.
    let mut above: Vec<(usize, f64, f64)> = Vec::new();
    let mut below: Vec<(usize, f64, f64)> = Vec::new();
    for (i, f) in found.iter().enumerate() {
        let Some(price) = f.shape.price_at(at_ms) else {
            continue;
        };
        // What "strongest" means for this row: the detector's own rank, or the money traded on
        // the touching bars. Widened to f64 so a large volume keeps its digits; a non-finite
        // value ranks last rather than poisoning the sort.
        let strength = match take.strength {
            StrengthBasis::Touches => f64::from(f.strength),
            StrengthBasis::Volume => f.touch_volume,
        };
        let strength = if strength.is_finite() {
            strength
        } else {
            f64::NEG_INFINITY
        };
        let row = (i, (price - ref_price).abs(), strength);
        if price >= ref_price {
            above.push(row);
        } else {
            below.push(row);
        }
    }
    let rank = |v: &mut Vec<(usize, f64, f64)>| {
        v.sort_by(|a, b| match take.order {
            TakeOrder::Nearest => a.1.total_cmp(&b.1).then(a.0.cmp(&b.0)),
            TakeOrder::Strongest => {
                b.2.total_cmp(&a.2)
                    .then(a.1.total_cmp(&b.1))
                    .then(a.0.cmp(&b.0))
            }
        });
        v.truncate(count);
    };
    let mut out = Vec::new();
    if matches!(take.side, TakeSide::Above | TakeSide::Both) {
        rank(&mut above);
        out.extend(above.iter().map(|r| r.0));
    }
    if matches!(take.side, TakeSide::Below | TakeSide::Both) {
        rank(&mut below);
        out.extend(below.iter().map(|r| r.0));
    }
    out
}

#[cfg(test)]
mod tests;
