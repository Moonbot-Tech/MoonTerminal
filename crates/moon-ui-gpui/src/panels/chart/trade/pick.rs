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

/// How a hit-test decides which order lines are legal targets.
///
/// The pointer's drag/click path and the Tab/Del cancel path share the same geometry and ranking
/// but ask different target questions: which kinds, which fills, and whether the grab is limited
/// to the start-cross X band. Naming those questions as a mode keeps the keyboard from inheriting
/// the mouse's `bool` and makes "never cancel a sell line" a scan of `[Buy]` rather than a later
/// filter.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum OrderHitMode {
    /// The pointer's own grab: `cross_only` is separate-zone chart space, where the only target
    /// is an unfilled entry's start cross.
    Drag { cross_only: bool },
    /// The Tab/Del route: the whole ENTRY line, in any zone, at any fill.
    EntryCancel,
}

impl OrderHitMode {
    /// Which line kinds this mode scans, in ranking order.
    ///
    /// Drag Buy/Sell through `move_order` and SL/Trailing/TakeProfit through absolute
    /// `move_order_stop_price` updates. VStop and pending-condition lines have no price
    /// level set by dragging and are therefore excluded.
    pub(super) fn kinds(self) -> &'static [LineKind] {
        match self {
            Self::Drag { cross_only: false } => &[
                LineKind::Buy,
                LineKind::Sell,
                LineKind::Stop,
                LineKind::Trailing,
                LineKind::TakeProfit,
            ],
            Self::Drag { cross_only: true } => &[LineKind::Buy],
            Self::EntryCancel => &[LineKind::Buy],
        }
    }

    /// Whether a line of this kind, at this fill, is a legal target for this mode.
    ///
    /// Drag admits a Buy only while `fill_pct <= 0.0` — the same strict `>` / non-strict `<=`
    /// boundary as the old skip — because a filled entry's live limit is historical and the
    /// position is managed through its Sell exit and stops. EntryCancel admits a Buy at any fill:
    /// a partially filled entry is still live on the exchange and still cancellable.
    pub(super) fn admits(self, kind: LineKind, fill_pct: f32) -> bool {
        if !self.kinds().contains(&kind) {
            return false;
        }
        match self {
            Self::Drag { .. } if kind == LineKind::Buy => fill_pct <= 0.0,
            Self::Drag { .. } | Self::EntryCancel => true,
        }
    }

    /// Whether the hit is limited to the X band around the line's start cross.
    pub(super) fn cross_band_only(self) -> bool {
        matches!(self, Self::Drag { cross_only: true })
    }
}

#[cfg(test)]
mod tests;
