//! Which order line a pointer grabs when several are within reach.
//!
//! Its own module because the exit line is PINNED to the plot's edge once its price leaves the
//! visible band: several exits then share one row of pixels, "closest in pixels" stops separating
//! them, and the choice has to be made from what the lines mean. That rule is worth stating and
//! testing on its own rather than buried in the hit-test loop that feeds it.

use moon_core::session::order_lines::LineKind;

/// One order line the pointer is close enough to grab, and everything the ranking needs.
pub(super) struct OrderCandidate {
    pub uid: u64,
    pub kind: LineKind,
    pub price: f32,
    pub short: bool,
    /// Vertical distance in pixels from the cursor to where the line is DRAWN — pinned or not.
    pub dist: f32,
    pub start_x: f32,
    pub fill_pct: f32,
    /// How far past the plot the line's own Y sat before it was pinned; zero while it is on screen.
    pub overshoot: f32,
    /// Position behind the line, the tie-break when two pinned lines share the edge.
    pub size: f32,
    /// Arrival sequence, the last resort so the choice never depends on iteration order.
    pub seq: u64,
    /// Whether the pin actually MOVED this line to the plot's edge. An exit still on screen is not
    /// pinned, however eligible it was.
    pub pinned: bool,
}

impl OrderCandidate {
    /// Whether this candidate should win the grab over `other`.
    ///
    /// Nearest to the cursor first, and that stays an EXACT comparison: rounding it to whole pixels
    /// was tried and it silently changed the ordinary grab, letting a stop half a pixel away lose to
    /// an exit a pixel and a half away — two lines that route to different commands on the core. The
    /// rounding was never needed for the pinned case either: lines pinned to one edge are clamped to
    /// the very same Y, so their distances are equal bit for bit and fall through on their own.
    ///
    /// Ties then go to the line nearest the price — for pinned lines the smallest overshoot is the
    /// one just off the edge rather than one far beyond it — then to the larger position, then to
    /// the later order, which is what puts the last one on top.
    ///
    /// `total_cmp` throughout, so a non-finite that slipped past the caller's filters orders
    /// predictably instead of making a candidate neither beat nor be beaten and freezing the choice
    /// on whichever one happened to be seen first.
    pub fn beats(&self, other: &Self) -> bool {
        other
            .dist
            .total_cmp(&self.dist)
            .then_with(|| other.overshoot.total_cmp(&self.overshoot))
            .then_with(|| self.size.total_cmp(&other.size))
            .then_with(|| self.seq.cmp(&other.seq))
            .is_gt()
    }
}

#[cfg(test)]
mod tests;
