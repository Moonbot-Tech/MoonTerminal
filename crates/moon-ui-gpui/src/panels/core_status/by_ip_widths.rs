//! By-IP column geometry: the base column widths and the ONE factor that shrinks them together.
//!
//! The By-IP view is a tree, not a `MoonDataTable`, so it draws its own columns at fixed widths —
//! a changing number must never reflow the row, and every caption has to land over its values. That
//! fixed layout is also why a narrow dock used to clip the right-hand columns instead of shrinking
//! them: nothing recomputed the widths.
//!
//! MoonUI's data table solves the same problem differently (`downscale_columns_to_available` in
//! `moon/data_table.rs`): it water-fills columns down to an absolute 40 px floor and sends the
//! remainder to a horizontal scrollbar. Neither half fits here — the tree has no horizontal scroll
//! (and CONTRIBUTING forbids adding one to a panel), and the header is a separate element from the
//! rows, so per-column collapsing would have to be replayed identically in both. ONE shared factor
//! keeps them aligned by construction: both sides read the same [`ByIpWidths`].
//!
//! Everything the row spends besides its columns is modelled in [`ByIpWidths::row_chrome`]. Getting
//! that budget wrong is not cosmetic: too small and the shrink engages late and still overflows,
//! too large and it shrinks a row that would have fitted.

/// Widths of the By-IP columns, in raw pixels, plus the two insets that scale with them.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(super) struct ByIpWidths {
    /// Server/core name column; the name truncates and the pencil pins to its right edge.
    pub(super) name: f32,
    /// Masked or revealed address column.
    pub(super) ip: f32,
    pub(super) cpu: f32,
    pub(super) mem: f32,
    /// Both latency columns (client↔core and core→exchange) use this width.
    pub(super) ping: f32,
    /// API-key lifetime cell; wider than a latency because it also holds a word.
    pub(super) api: f32,
    /// Ready/total core-count cell.
    pub(super) cores: f32,
    /// Warning-icon lead inside a metric cell; a header reserves the same lead.
    pub(super) icon: f32,
    /// Extra left indent of a core name under its server.
    pub(super) indent: f32,
}

/// Gap between the row's own children, in pixels.
///
/// Explicit pixels, NOT GPUI's `gap_2`: that is `rems(0.5)`, and `MoonRoot` sets the rem size to the
/// theme's base font size, which the "Шрифт" slider moves. A budget written against `8.0` while the
/// row actually spends 7 or 9 would drift with a slider that has nothing to do with these columns.
/// The row uses this same constant, so the model and the layout cannot disagree.
pub(super) const ROW_GAP_W: f32 = 8.0;

/// Gap between a metric cell's warning-icon lead and its value box. Explicit for the same reason.
pub(super) const CELL_GAP_W: f32 = 4.0;

/// Width of the chevron gutter that opens a server, and of the empty slot a core row keeps for it.
pub(super) const CHEVRON_W: f32 = 12.0;

/// Width the tree's vertical scrollbar covers at the row's right edge.
///
/// MIRRORS MoonUI: `scroll/scrollbar.rs` builds it as `4*2 + 8` and paints it as an OVERLAY, so it
/// takes no layout width — a row sized to the full measurement would put its last column under it.
/// That constant is private there, so nothing checks this; if it moves, this must follow by hand.
pub(super) const TREE_SCROLLBAR_W: f32 = 16.0;

/// Row inset, as a fraction of the rem size.
///
/// `MoonListItem` insets its content by `px_3` = `rems(0.75)`, and the header mirrors it with the
/// same unit — so the two stay aligned at every font setting instead of at exactly one of them.
pub(super) const ROW_INSET_REMS: f32 = 0.75;

/// Number of `ROW_GAP_W` gaps in one row: the outer chevron↔body gap plus the nine between the
/// body's ten children (name · ip · spacer · cpu · mem · ping · exch · key · cores · dot).
const ROW_GAPS: f32 = 10.0;

/// Number of `CELL_GAP_W` gaps: one inside each of the five metric cells.
const CELL_GAPS: f32 = 5.0;

/// Floor for the shrink factor. Below roughly half the columns stop carrying their values (the
/// numbers are already truncated), so the row clips the remainder rather than shrinking into
/// unreadable slivers.
const MIN_SCALE: f32 = 0.5;

impl ByIpWidths {
    /// Design widths, used whenever the row has room for them.
    pub(super) const BASE: Self = Self {
        name: 150.0,
        ip: 118.0,
        cpu: 100.0,
        mem: 116.0,
        ping: 64.0,
        api: 72.0,
        cores: 40.0,
        icon: 12.0,
        indent: 16.0,
    };

    /// Total width the columns claim on one row, the metric icon leads included.
    ///
    /// The indent is deliberately absent: it sits INSIDE the name column, so it costs no extra row
    /// width.
    fn columns_w(self) -> f32 {
        // Five metric cells (cpu, mem, ping, exch, key), each preceded by its icon lead.
        self.name
            + self.ip
            + self.cpu
            + self.mem
            + self.ping * 2.0
            + self.api
            + self.cores
            + self.icon * 5.0
    }

    /// Everything a row spends besides its columns, at the current `rem` size.
    ///
    /// Both insets, the chevron gutter, every gap, and the connectivity-warning triangle a degraded
    /// server row inserts before its status dot. That triangle is reserved unconditionally: it
    /// appears exactly on the rows whose numbers matter most, and a budget that ignores it would let
    /// those rows — and only those — overflow.
    fn row_chrome(rem: f32) -> f32 {
        let icon = Self::BASE.icon;
        ROW_INSET_REMS * rem * 2.0
            + CHEVRON_W
            + ROW_GAPS * ROW_GAP_W
            + CELL_GAPS * CELL_GAP_W
            + icon
            + ROW_GAP_W
    }

    /// Widths for a row of `available` pixels.
    ///
    /// `reserved` is what the row must leave free at its right edge — the status dot and the tree's
    /// overlay scrollbar — and `rem` is the window's rem size, which sets the insets.
    ///
    /// Returns [`Self::BASE`] while the row still fits: the layout keeps a `flex_1` spacer that
    /// absorbs any surplus, so growing the columns is not this function's job. A row narrower than
    /// its columns gets one shared factor, floored at [`MIN_SCALE`].
    ///
    /// `available <= 0.0` means the view has not been measured yet (the first frame): return the
    /// base widths rather than collapsing everything to the floor for one frame.
    pub(super) fn for_width(available: f32, reserved: f32, rem: f32) -> Self {
        let base = Self::BASE;
        if !available.is_finite() || available <= 0.0 {
            return base;
        }
        let columns = base.columns_w();
        let room = available - Self::row_chrome(rem.max(0.0)) - reserved.max(0.0);
        if room >= columns {
            return base;
        }
        base.scaled((room / columns).max(MIN_SCALE))
    }

    /// Scale every width by one factor.
    fn scaled(self, k: f32) -> Self {
        Self {
            name: self.name * k,
            ip: self.ip * k,
            cpu: self.cpu * k,
            mem: self.mem * k,
            ping: self.ping * k,
            api: self.api * k,
            cores: self.cores * k,
            icon: self.icon * k,
            indent: self.indent * k,
        }
    }

    /// The narrowest row that still fits the base widths — the width at which shrinking begins.
    ///
    /// Exposed for the tests, which pin the crossover: a wrong chrome budget moves it, and every
    /// "does it shrink at all" assertion would still pass.
    #[cfg(test)]
    pub(super) fn full_width(reserved: f32, rem: f32) -> f32 {
        Self::BASE.columns_w() + Self::row_chrome(rem) + reserved
    }
}

#[cfg(test)]
/// Shrink-factor behaviour of the By-IP columns.
mod tests;
