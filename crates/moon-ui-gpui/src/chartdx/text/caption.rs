//! Geometry of the chart's corner caption — the coin and the core name drawn over the order book.
//!
//! This module is the ONE home for that arithmetic. `prepare.rs` asks it where the caption goes,
//! draws the text there and publishes the finished plate rectangles; `render_state.rs` draws those
//! rectangles verbatim and computes nothing. Keep it that way: a second copy of the geometry cannot
//! be kept in step with this one by hand, and the plate silently drifting out from under the text
//! is what that costs.

/// Where the corner caption may draw, in LOGICAL pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct CaptionGeom {
    /// Left edge of the zone the caption may occupy: the order book's own left edge when the book
    /// is on, otherwise a book-sized budget carved off the plot's right side.
    pub(super) zone_left: f32,
    /// Right anchor, already inset clear of the pane's close button.
    pub(super) right_x: f32,
    /// Top of the first caption line.
    pub(super) top_y: f32,
    /// Horizontal budget for one caption line: text wider than this is truncated to fit.
    pub(super) max_w: f32,
    /// Whether an order book is present, and therefore whether there is a zone to align the core
    /// name to on the LEFT. Without one, both lines stay right-anchored — there is nothing to the
    /// left but chart.
    pub(super) over_book: bool,
}

/// Where the order book's zone starts, in LOGICAL pixels.
///
/// The shared primitive: the caption sits inside this zone, and the ghost cursor's labels anchor
/// against it. The engine can narrow the book on a cramped pane or widen it to the whole pane in
/// book-only broom mode, so consumers read the REAL rectangle whenever one exists and reserve
/// [`moon_chart::GLASS_ZONE_PX`] only for the no-book fallback.
///
/// Args:
///     pane_left: Left edge of the whole pane.
///     right_bound: Right edge the zone may reach — already inset by the caller if it needs to be.
///     plot_left: Left edge of the plot area.
///     plot_right: Right edge of the plot area.
///     orderbook_enabled: Whether the order book is drawn for this pane.
///     orderbook_left: Left edge of the order book, meaningful only when it is enabled.
///
/// Returns:
///     The zone's left edge, always within `[pane_left, right_bound]`.
pub(super) fn book_zone_left(
    pane_left: f32,
    right_bound: f32,
    plot_left: f32,
    plot_right: f32,
    orderbook_enabled: bool,
    orderbook_left: f32,
) -> f32 {
    if orderbook_enabled && orderbook_left.is_finite() {
        // Clamp into the pane: a book rectangle from a previous layout can briefly sit outside it.
        return orderbook_left.clamp(pane_left.min(right_bound), right_bound);
    }
    // No book: reserve the same slice the book would have taken, never crossing the plot's left
    // edge, so the anchor cannot walk across the whole chart.
    let budget = moon_chart::GLASS_ZONE_PX.min((plot_right - plot_left).max(0.0) * 0.5);
    (right_bound - budget).max(plot_left)
}

/// Resolve where the caption may draw for one pane, or `None` when there is no room at all.
///
/// The order book's real rectangle is READ rather than re-derived from [`moon_chart::GLASS_ZONE_PX`]:
/// the engine narrows the book on a cramped pane and widens it to the whole pane in book-only broom
/// mode, and a second copy of that decision here would drift from it. The constant is used only for
/// the no-book case, where there is no rectangle to read.
///
/// Args:
///     pane_left: Left edge of the whole pane.
///     pane_right: Right edge of the whole pane.
///     plot_left: Left edge of the plot area.
///     plot_right: Right edge of the plot area.
///     plot_top: Top of the plot area.
///     orderbook_enabled: Whether the order book is drawn for this pane.
///     orderbook_left: Left edge of the order book, meaningful only when it is enabled.
///     pad_x: Inset from the right edge that clears the pane's close button.
///     pad_y: Inset from the plot's top edge.
///
/// Returns:
///     The caption's zone, or `None` for a degenerate pane where nothing can be drawn legibly.
#[allow(clippy::too_many_arguments)]
pub(super) fn caption_geom(
    pane_left: f32,
    pane_right: f32,
    plot_left: f32,
    plot_right: f32,
    plot_top: f32,
    orderbook_enabled: bool,
    orderbook_left: f32,
    pad_x: f32,
    pad_y: f32,
) -> Option<CaptionGeom> {
    // A non-finite or inverted rectangle means the pane has not been laid out yet this frame.
    if ![pane_left, pane_right, plot_left, plot_right, plot_top]
        .iter()
        .all(|v| v.is_finite())
        || pane_right <= pane_left
    {
        return None;
    }
    let right_edge = if orderbook_enabled {
        pane_right
    } else {
        plot_right
    };
    let right_x = right_edge - pad_x;
    // A pane narrower than the close-button inset leaves the anchor left of the pane itself. Bail
    // before the clamp below, whose bounds would then be inverted.
    if right_x <= pane_left {
        return None;
    }
    let zone_left = book_zone_left(
        pane_left,
        right_x,
        plot_left,
        plot_right,
        orderbook_enabled,
        orderbook_left,
    );
    let max_w = right_x - zone_left;
    if !max_w.is_finite() || max_w <= 1.0 {
        return None;
    }
    Some(CaptionGeom {
        zone_left,
        right_x,
        top_y: plot_top + pad_y,
        max_w,
        over_book: orderbook_enabled,
    })
}

/// Gap between the core name's right edge and the order book's left edge.
const CAPTION_SPLIT_GAP: f32 = 6.0;

/// Least width worth handing the core name once it hangs left of the book.
///
/// Below roughly four monospaced glyphs plus an ellipsis the truncated name says nothing, and the
/// caption is better off stacked inside the book's own zone than split into an unreadable sliver.
const CAPTION_MIN_CORE_W: f32 = 40.0;

/// Where each of the caption's two lines anchors, in LOGICAL pixels.
///
/// The lines are placed independently because they answer different questions: the coin belongs to
/// the order book it labels and reads best centred on it, while the core name qualifies the whole
/// pane and has chart to its left to hang over. `ax` is the horizontal alignment fraction the text
/// pass takes — 0.0 anchors the run's left edge at `x`, 0.5 its centre, 1.0 its right edge.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct CaptionLayout {
    /// Horizontal anchor for the coin run.
    pub(super) coin_x: f32,
    /// Horizontal alignment fraction applied at [`Self::coin_x`].
    pub(super) coin_ax: f32,
    /// Maximum width available to the coin run.
    pub(super) coin_max_w: f32,
    /// Horizontal anchor for the core-name run.
    pub(super) core_x: f32,
    /// Horizontal alignment fraction applied at [`Self::core_x`].
    pub(super) core_ax: f32,
    /// Maximum width available to the core-name run.
    pub(super) core_max_w: f32,
    /// Whether the two lines sit in separate columns. When false they share one column and one
    /// backing-plate geometry.
    pub(super) split: bool,
}

/// Place the caption's two lines within an already-resolved zone.
///
/// Args:
///     g: The caption's zone for this pane.
///     plot_left: Left edge of the plot area — the boundary the core name must not cross, since
///         beyond it lies the price-axis gutter rather than chart.
///
/// Returns:
///     Where each line anchors and whether they were split into two columns.
pub(super) fn caption_layout(g: &CaptionGeom, plot_left: f32) -> CaptionLayout {
    // No book: there is nothing but chart to the left, so there is no zone to centre on and no
    // edge to hang from. Both lines stay right-anchored.
    if !g.over_book {
        return CaptionLayout {
            coin_x: g.right_x,
            coin_ax: 1.0,
            coin_max_w: g.max_w,
            core_x: g.right_x,
            core_ax: 1.0,
            core_max_w: g.max_w,
            split: false,
        };
    }
    let left_avail = g.zone_left - CAPTION_SPLIT_GAP - plot_left;
    if left_avail >= CAPTION_MIN_CORE_W {
        return CaptionLayout {
            // Centred on the book's own span. The right end of that span is already inset clear of
            // the pane's close button, so a wide coin cannot grow out from under it.
            coin_x: (g.zone_left + g.right_x) * 0.5,
            coin_ax: 0.5,
            coin_max_w: g.max_w,
            core_x: g.zone_left - CAPTION_SPLIT_GAP,
            core_ax: 1.0,
            core_max_w: left_avail,
            split: true,
        };
    }
    // Too little chart left of the book to hang a legible name over — a narrow pane, or broom mode
    // where the book has taken the whole width. Stack both lines inside the zone instead.
    CaptionLayout {
        coin_x: g.zone_left,
        coin_ax: 0.0,
        coin_max_w: g.max_w,
        core_x: g.zone_left,
        core_ax: 0.0,
        core_max_w: g.max_w,
        split: false,
    }
}

/// Bounding box of the runs actually drawn in one caption column, in LOGICAL pixels.
///
/// The plate under the caption is measured from what was DRAWN rather than computed from an
/// assumed set of rows: the scale badge is optional, the broom delta is optional, and the coin and
/// core name each vanish when their source is empty. Any formula written against a fixed roster of
/// rows is wrong for every other combination — which is how a plate ends up half-covering the text
/// it exists to make legible.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(super) struct CaptionBox {
    left: f32,
    right: f32,
    top: f32,
    bottom: f32,
    any: bool,
}

impl CaptionBox {
    /// Grow the box to include one drawn run.
    ///
    /// Args:
    ///     left: The run's left edge, already resolved through its alignment.
    ///     width: The run's width; a run with no width and no height is ignored.
    ///     top: The run's top edge.
    ///     height: The run's height.
    pub(super) fn add(&mut self, left: f32, width: f32, top: f32, height: f32) {
        if width <= 0.0 && height <= 0.0 {
            return;
        }
        if !self.any {
            self.left = left;
            self.right = left + width;
            self.top = top;
            self.bottom = top + height;
            self.any = true;
            return;
        }
        self.left = self.left.min(left);
        self.right = self.right.max(left + width);
        self.top = self.top.min(top);
        self.bottom = self.bottom.max(top + height);
    }

    /// The padded plate rectangle in DEVICE pixels, or an empty one for a column with no runs.
    ///
    /// The padding is asymmetric on purpose: a left inset large enough to clear the glyphs'
    /// bearing, and a tighter right one so the plate does not visibly overhang the text.
    ///
    /// Args:
    ///     sf: Device scale factor applied on the way out.
    ///
    /// Returns:
    ///     `[x, y, width, height]` in device pixels.
    pub(super) fn plate(&self, sf: f32) -> [f32; 4] {
        if !self.any {
            return [0.0; 4];
        }
        let (pad_l, pad_r, pad_y) = (5.0_f32, 3.0_f32, 2.0_f32);
        [
            (self.left - pad_l) * sf,
            (self.top - pad_y) * sf,
            (self.right - self.left + pad_l + pad_r) * sf,
            (self.bottom - self.top + pad_y * 2.0) * sf,
        ]
    }
}

#[cfg(test)]
mod tests;
