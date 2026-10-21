//! Backend-free decisions for the order-book ladder: which tick a pane's grid stands on, how many
//! ticks a row spans at the current zoom, and how many decimals its price prints with.
//!
//! The rows themselves come from [`moon_core::data::OrderBookModel::build_ladder`]; the chart layer
//! owns the zone geometry, the upload and the text. Everything here is pure, so the choices can be
//! asserted in tests the way [`crate::hvol`] is.

/// Highlight thresholds the reader can pick, in percent of the largest visible row. `0` is off.
pub const LADDER_BIG_PCTS: [u8; 4] = [0, 25, 50, 75];

/// The mantissas the step walks through within each decade: 1, 2, 4, 10, 20, 40, 100 … ticks, the
/// steps a DOM ladder offers (`x1 x2 x4 x10`) continued.
const STEP_MANTISSAS: [u32; 3] = [1, 2, 4];

/// The coarsest row the step tries, in ticks. Past it the zoom is so far out that a ladder of rows
/// tall enough to read would hold a handful of rows; the chart keeps the glass instead.
const MAX_STEP: u32 = 1_000_000;

/// How often a pane whose market tick is still unknown asks the source for it again, in
/// milliseconds: the read takes the source lock and a snapshot, so not every sync.
pub const TICK_RETRY_MS: i64 = 1_000;

/// The finest grid a price prints at, in decimals: past it an `f32` price holds no further digits,
/// so a finer tick still prints its rows at this many.
pub const MAX_TICK_DECIMALS: usize = 12;

/// Snap a stored highlight threshold onto [`LADDER_BIG_PCTS`].
pub fn snap_big_pct(pct: u8) -> u8 {
    LADDER_BIG_PCTS
        .iter()
        .copied()
        .min_by_key(|p| p.abs_diff(pct))
        .unwrap_or(0)
}

/// What a pane knows about its market's tick: the exchange's own, once the source has it, and
/// until then the one read from the book's level spacing.
///
/// The exchange's tick wins the moment it arrives, and is not asked for again — a tick does not
/// move. The book's only ever gets FINER: a book's spacing narrows as it fills, and a grid that
/// widened again on a thin moment would rebuild every row with it.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct LadderTick {
    exact: Option<f64>,
    inferred: Option<f64>,
    asked_ms: Option<i64>,
}

impl LadderTick {
    /// Whether the source should be asked now: only while the exact tick is unknown, and at most
    /// once per [`TICK_RETRY_MS`].
    pub fn due(&self, now_ms: i64) -> bool {
        self.exact.is_none()
            && self
                .asked_ms
                .is_none_or(|at| now_ms.saturating_sub(at) >= TICK_RETRY_MS)
    }

    /// Record one ask.
    ///
    /// Args:
    ///     now_ms: When the ask was made, unix milliseconds.
    ///     exact: The source's tick, `None` while it does not know it.
    ///     inferred: Reads the book's spacing; called only while `exact` is unknown, so a pane
    ///         whose tick arrived never scans its book for one.
    pub fn record(
        &mut self,
        now_ms: i64,
        exact: Option<f64>,
        inferred: impl FnOnce() -> Option<f64>,
    ) {
        let usable = |t: &f64| t.is_finite() && *t > 0.0;
        self.asked_ms = Some(now_ms);
        self.exact = exact.filter(usable);
        if self.exact.is_none() {
            let seen = inferred().filter(usable);
            self.inferred = match (self.inferred, seen) {
                (Some(old), Some(new)) => Some(old.min(new)),
                (old, new) => old.or(new),
            };
        }
    }

    /// The tick the grid stands on, `None` while neither source has one.
    pub fn tick(&self) -> Option<f64> {
        self.exact.or(self.inferred)
    }
}

/// The step, in ticks, a ladder row spans at the current zoom: the smallest of 1, 2, 4, 10, 20,
/// 40 … whose row is at least `min_row_px` tall, so every row has room for its text.
///
/// Always automatic, by decision: a fixed step would only ever be wrong for some zoom — rows too
/// thin to read, or too few to show the book.
///
/// Args:
///     tick: The market's price tick.
///     px_per_price: Physical pixels per price unit in the book zone.
///     min_row_px: Physical height a row needs for one line of text.
///
/// Returns:
///     The step, or `None` when the grid is unusable or no readable step exists at this zoom —
///     the chart then draws the glass.
pub fn row_step(tick: f64, px_per_price: f32, min_row_px: f32) -> Option<u32> {
    if !(tick.is_finite() && tick > 0.0) || !(px_per_price.is_finite() && px_per_price > 0.0) {
        return None;
    }
    let tick_px = tick * f64::from(px_per_price);
    let need = f64::from(min_row_px.max(1.0));
    let mut decade = 1u32;
    loop {
        for m in STEP_MANTISSAS {
            let step = m * decade;
            if step > MAX_STEP {
                return None;
            }
            if tick_px * f64::from(step) >= need {
                return Some(step);
            }
        }
        decade *= 10;
    }
}

/// Decimals a ladder price prints with: as many as the tick needs, so every row of every step
/// prints on the exchange's own grid (`2.07200`, not `2.072`) and adjacent rows line up.
///
/// Not [`crate::axes::price_decimals`], which sizes the decimals from a PRICE's magnitude: this
/// reads them off the grid.
pub fn tick_decimals(tick: f64) -> usize {
    if !(tick.is_finite() && tick > 0.0) {
        return 2;
    }
    (0..=MAX_TICK_DECIMALS)
        .find(|&d| {
            let scaled = tick * 10f64.powi(d as i32);
            (scaled - scaled.round()).abs() <= scaled.abs() * 1e-6
        })
        .unwrap_or(MAX_TICK_DECIMALS)
}

#[cfg(test)]
mod tests;
