//! Price-ladder view of the order book: the book's levels gathered into rows of `tick × step`.
//!
//! The ladder draws through the SAME instance buffer and shader as Moonbot's glass. Its rows are
//! emitted as level-line instances (`kind` 2 and 3) centred on the row's price; the chart sets the
//! line height to the row height, which every row shares, so each row becomes a translucent bar in
//! the theme's level colours without a new shader on any backend.
//!
//! Bucketing is done in whole TICKS, never by dividing an `f32` price: the price `2.0664` arrives as
//! `2.06640005`, and `ceil(2.06640005 / 0.00001)` lands one row too high. Rounding to the tick
//! first puts every level on the exchange's own grid, and the integer step division after it is
//! exact.

use super::{LevelInstance, OrderBookModel};

/// The price grid a ladder is built on.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LadderGrid {
    /// The market's price tick: the smallest price move the exchange accepts.
    pub tick: f64,
    /// How many ticks one row spans.
    pub step: u32,
}

impl LadderGrid {
    /// Height of one row in price units.
    pub fn row_height(&self) -> f64 {
        self.tick * f64::from(self.step)
    }

    /// Whether the grid can bucket anything: a positive finite tick and a non-zero step.
    pub fn is_usable(&self) -> bool {
        self.tick.is_finite() && self.tick > 0.0 && self.step > 0
    }
}

/// One ladder row: every level of one side whose price falls into the row's bucket.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LadderRow {
    /// The row's own grid price — the price the row prints and the centre it is drawn at.
    pub price: f64,
    /// Summed level notional (`qty × price`) in the quote currency.
    pub notional: f32,
    pub is_ask: bool,
}

impl OrderBookModel {
    /// Gathers the book into ladder rows whose bucket overlaps `[lo, hi]`.
    ///
    /// A bid rounds DOWN to its row and an ask rounds UP, the way a DOM ladder groups them: a row
    /// then never holds both sides, and the best bid and best ask stay on distinct rows however
    /// coarse the step. Rows come out bids first (descending), then asks (ascending), the book's
    /// own order.
    ///
    /// Args:
    ///     grid: Tick and step of the rows; an unusable grid yields no rows.
    ///     lo: Lower edge of the price window, inclusive of a row that straddles it.
    ///     hi: Upper edge of the price window, inclusive of a row that straddles it.
    ///     out: Cleared, then filled with the rows.
    pub fn build_ladder(&self, grid: LadderGrid, lo: f32, hi: f32, out: &mut Vec<LadderRow>) {
        out.clear();
        if !grid.is_usable() {
            return;
        }
        let step = i64::from(grid.step);
        let row_h = grid.row_height();
        let half = row_h * 0.5;
        let (lo, hi) = (f64::from(lo), f64::from(hi));
        // The row the previous level went to, with its side: the book is sorted per side, so the
        // levels of one row arrive together and merge into the row just pushed.
        let mut last: Option<(i64, bool)> = None;
        for r in &self.raw {
            if !r.price.is_finite() || !r.notional.is_finite() {
                continue;
            }
            let ticks = (f64::from(r.price) / grid.tick).round();
            // Past 2^53 the integer no longer names one tick; such a level is not a price.
            if !ticks.is_finite() || ticks.abs() > 9.0e15 {
                continue;
            }
            let ticks = ticks as i64;
            let row = if r.is_ask {
                ticks.div_euclid(step) + i64::from(ticks.rem_euclid(step) != 0)
            } else {
                ticks.div_euclid(step)
            };
            let price = row as f64 * row_h;
            if price + half < lo || price - half > hi {
                continue;
            }
            match out.last_mut() {
                Some(prev) if last == Some((row, r.is_ask)) => prev.notional += r.notional,
                _ => {
                    out.push(LadderRow {
                        price,
                        notional: r.notional,
                        is_ask: r.is_ask,
                    });
                    last = Some((row, r.is_ask));
                }
            }
        }
    }

    /// The book's own price spacing, read as a tick: the smallest gap between two adjacent levels
    /// of one side, snapped onto a tick an exchange could have (see [`snap_tick`]).
    ///
    /// A FALLBACK for a market whose tick the source does not know — one that has not arrived yet,
    /// or a feed with no market catalog. The snap is what keeps `f32` prices usable: the gap
    /// between `2.0664` and `2.06641` reads `0.0000099…`, and only a snap gives `0.00001` back.
    /// It errs FINE, never coarse: [`OrderBookModel::build_ladder`] rounds every level onto the
    /// tick, so a grid coarser than the real one moves levels, while a finer one only means the
    /// row step stacks more ticks into a row.
    ///
    /// Returns:
    ///     The inferred tick, or `None` for a book without two levels on one side.
    pub fn infer_tick(&self) -> Option<f64> {
        let gap = self
            .raw
            .windows(2)
            .filter(|w| w[0].is_ask == w[1].is_ask)
            .map(|w| (f64::from(w[1].price) - f64::from(w[0].price)).abs())
            .filter(|g| g.is_finite() && *g > 0.0)
            .fold(f64::INFINITY, f64::min);
        gap.is_finite().then(|| snap_tick(gap)).flatten()
    }
}

/// Snap a measured price gap onto a grid every level of the book sits on: `2.5 × 10ⁿ` or
/// `5 × 10ⁿ` when the gap IS that within 2 %, otherwise the largest power of ten not above it.
///
/// A power of ten divides every `1, 2, 5 × 10ᵐ` tick at or above it, so a gap of several ticks —
/// three ticks of 0.01 read 0.03 — gives a grid the real prices still sit on (0.01), where a
/// rounded mantissa (0.02, 0.025) would not and the rounding in `build_ladder` would move levels.
/// The 2 % slack absorbs the `f32` noise that reads a 0.00001 gap as 0.0000099; a noisier gap
/// (a fine tick at a high price) lands a decade finer, which is still a grid the prices sit on.
///
/// Deliberately NOT the tuner's `db::tuner::ticks::infer_tick`, which snaps to the nearest
/// `1, 2, 5 × 10ⁿ`: that one names a price step for a model, where the nearest guess is the best
/// guess; this one is a bucketing grid, where only a divisor of the real tick is safe.
fn snap_tick(gap: f64) -> Option<f64> {
    let decade = 10f64.powi(gap.log10().floor() as i32);
    if !(decade.is_finite() && decade > 0.0) {
        return None;
    }
    let m = gap / decade;
    for exact in [2.5, 5.0] {
        if (m / exact - 1.0).abs() <= 0.02 {
            return Some(exact * decade);
        }
    }
    Some(if m * 1.02 >= 10.0 {
        10.0 * decade
    } else {
        decade
    })
}

/// Turns ladder rows into level-line instances centred on each row's price, scaled to the largest
/// row centred inside `[lo, hi]`.
///
/// One call for both, so the bars and the highlight — which reads the returned figure — cannot
/// scale against two different "biggest walls". A row only in the emitted margin does not set the
/// scale; its bar clamps to full length. The chart sets the level-line height to the row height for
/// a ladder, which is what turns these lines into row bars.
///
/// Args:
///     rows: Rows from [`OrderBookModel::build_ladder`].
///     lo: Lower edge of the visible price window.
///     hi: Upper edge of the visible price window.
///     out: Cleared, then filled with one instance per row.
///
/// Returns:
///     The largest visible row notional the bars were scaled to, zero when no row is visible.
pub fn ladder_instances(rows: &[LadderRow], lo: f32, hi: f32, out: &mut Vec<LevelInstance>) -> f32 {
    let (lo, hi) = (f64::from(lo), f64::from(hi));
    let max = rows
        .iter()
        .filter(|r| r.price >= lo && r.price <= hi)
        .map(|r| r.notional)
        .fold(0.0_f32, f32::max);
    let inv = if max > 0.0 { 1.0 / max } else { 0.0 };
    out.clear();
    out.extend(rows.iter().map(|r| LevelInstance {
        price: r.price as f32,
        span: 0.0,
        len_norm: (r.notional * inv).clamp(0.0, 1.0),
        kind: if r.is_ask { 3.0 } else { 2.0 },
    }));
    max
}

#[cfg(test)]
mod tests;
