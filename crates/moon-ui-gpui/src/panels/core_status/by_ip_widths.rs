//! By-IP column geometry: the design widths, the user's own widths, and the ONE factor that
//! shrinks them together.
//!
//! The By-IP view is a tree, not a `MoonDataTable`, so it draws its own columns at fixed widths —
//! a changing number must never reflow the row, and every caption has to land over its values. That
//! fixed layout is also why a narrow dock used to clip the right-hand columns instead of shrinking
//! them: nothing recomputed the widths.
//!
//! # How a user width and the shrink factor coexist
//!
//! A dragged column width is authoritative WHILE THE ROW FITS, and below that it shrinks with
//! everything else by the same shared factor. The two obvious alternatives are both worse: honouring
//! a user width unconditionally lets the columns overflow the panel again, which is the exact bug
//! the shrink was added to fix and which a tree cannot absorb (CONTRIBUTING forbids a panel
//! horizontal scrollbar); rescaling only the user's columns would break the ratio the header and the
//! rows depend on to stay aligned. Shrinking everything by one factor preserves the RELATIVE sizing
//! the user chose, which is what they were actually expressing — the absolute pixels only ever held
//! while there was room for them. [`MAX_COL_W`] is what keeps that promise honest: without a
//! ceiling, one wide drag would drive the factor to [`MIN_SCALE`] and silently halve the other eight
//! columns.
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

use std::collections::HashMap;

/// Widths of the By-IP columns and their shrink-coupled row chrome, in raw pixels.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(super) struct ByIpWidths {
    /// Server/core name column; the name truncates and the pencil pins to its right edge.
    pub(super) name: f32,
    /// Masked or revealed address column.
    pub(super) ip: f32,
    pub(super) cpu: f32,
    pub(super) mem: f32,
    /// Client↔core latency column.
    ///
    /// Was shared with [`Self::exch`] until the columns became resizable: one width meant the
    /// exchange caption had no edge of its own to grab, and dragging either moved both.
    pub(super) ping: f32,
    /// Core→exchange latency column. Same design width as [`Self::ping`], so the default layout is
    /// unchanged; they only diverge once a user drags one.
    pub(super) exch: f32,
    /// API-key lifetime cell. Wider than a latency because it holds a word ("истёк") as often as a
    /// number, and its heading is longer than the others ("АПИ (дн)"). Like every column here it
    /// still shrinks by the shared factor, so on a very narrow dock the caption clips like the rest.
    pub(super) api: f32,
    /// Ready/total core-count cell.
    pub(super) cores: f32,
    /// Reported MoonBot build. Like `cores` and `startup` it carries NO warning-icon lead: a build
    /// number has no `WarnAxis` behind it, and this workspace defines no minimum version to warn
    /// against, so reserving a lead here would be space that can never light.
    pub(super) version: f32,
    /// Startup-progress cell. Like `cores` it carries NO warning-icon lead: startup has no
    /// `WarnAxis` behind it, so reserving a lead here would be space that can never light.
    pub(super) startup: f32,
    /// Measured clock-offset cell. Like `startup` and `version` it carries NO warning-icon lead:
    /// nothing warns on a core's clock offset, so reserving a lead here would be space that can
    /// never light.
    pub(super) tz_off: f32,
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

/// Number of `ROW_GAP_W` gaps the row's BODY spends: the fourteen between its fifteen children
/// (name · ip · spacer · cpu · mem · ping · exch · key · version · startup · tz_off · cores ·
/// warning slot · dot · scrollbar slot). The outer chevron↔body gap is NOT in here —
/// [`Self::row_chrome`] adds it as its own trailing `ROW_GAP_W` term, so the budget covers fifteen
/// gaps in total.
///
/// The trailing SCROLLBAR SLOT is a real flex child of the row (`server_view`'s
/// `div().w(px(TREE_SCROLLBAR_W))`), so it brings a gap of its own. `reserved` covers that slot's
/// WIDTH but no gap, and this constant read `11.0` while the row rendered more gaps than that once
/// the warning slot `row_chrome` adds separately is counted — an eight-pixel undercount that made
/// the shrink engage one gap too late and let the row overflow inside that band. The core-version
/// column then added one more child, and therefore one more gap; the tz-off column added a
/// fifteenth. Count the children in `server_row` before touching this.
const ROW_GAPS: f32 = 14.0;

/// Number of `CELL_GAP_W` gaps: one inside each of the five metric cells.
const CELL_GAPS: f32 = 5.0;

/// Floor for the shrink factor. Below roughly half the columns stop carrying their values (the
/// numbers are already truncated), so the row clips the remainder rather than shrinking into
/// unreadable slivers.
const MIN_SCALE: f32 = 0.5;

/// The columns a user can drag, in visual left-to-right order.
///
/// `icon` and `indent` are absent on purpose: they are row chrome, not columns, and neither has a
/// header cell to hang a divider off. Every column that a caption labels IS here, including both
/// latency columns separately — they carry the same design width but are not the same column, and
/// a caption a user can see but cannot grab reads as a bug.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum ByIpCol {
    Name,
    Ip,
    Cpu,
    Mem,
    Ping,
    Exch,
    Api,
    Version,
    Startup,
    TzOffset,
    Cores,
}

impl ByIpCol {
    /// Every resizable column, in the order the header lays them out.
    pub(super) const ALL: [Self; 11] = [
        Self::Name,
        Self::Ip,
        Self::Cpu,
        Self::Mem,
        Self::Ping,
        Self::Exch,
        Self::Api,
        Self::Version,
        Self::Startup,
        Self::TzOffset,
        Self::Cores,
    ];

    /// The key this column's width persists under.
    ///
    /// NEVER rename one of these: they are the map keys inside `layout.table_column_widths`, so a
    /// rename silently orphans the width every existing user has already dragged.
    pub(super) fn key(self) -> &'static str {
        match self {
            Self::Name => "name",
            Self::Ip => "ip",
            Self::Cpu => "cpu",
            Self::Mem => "mem",
            Self::Ping => "ping",
            Self::Exch => "exch",
            Self::Api => "api",
            Self::Version => "version",
            Self::Startup => "startup",
            Self::TzOffset => "tz_off",
            Self::Cores => "cores",
        }
    }

    /// The element id of this column's divider handle.
    ///
    /// A `&'static str` rather than a `format!` on [`Self::key`]: the header builds one handle per
    /// caption on every repaint, and these ten ids are a fixed set known at compile time. Distinct
    /// from `key` on purpose — that one is persisted and must never move, this one is throwaway
    /// element identity.
    pub(super) fn handle_id(self) -> &'static str {
        match self {
            Self::Name => "by-ip-resize-name",
            Self::Ip => "by-ip-resize-ip",
            Self::Cpu => "by-ip-resize-cpu",
            Self::Mem => "by-ip-resize-mem",
            Self::Ping => "by-ip-resize-ping",
            Self::Exch => "by-ip-resize-exch",
            Self::Api => "by-ip-resize-api",
            Self::Version => "by-ip-resize-version",
            Self::Startup => "by-ip-resize-startup",
            Self::TzOffset => "by-ip-resize-tz-off",
            Self::Cores => "by-ip-resize-cores",
        }
    }

    /// Read this column's width out of a resolved geometry.
    ///
    /// Args:
    ///     w: Resolved or painted By-IP geometry containing this column's width.
    ///
    /// Returns:
    ///     The width assigned to this column in `w`.
    pub(super) fn width_of(self, w: ByIpWidths) -> f32 {
        match self {
            Self::Name => w.name,
            Self::Ip => w.ip,
            Self::Cpu => w.cpu,
            Self::Mem => w.mem,
            Self::Ping => w.ping,
            Self::Exch => w.exch,
            Self::Api => w.api,
            Self::Version => w.version,
            Self::Startup => w.startup,
            Self::TzOffset => w.tz_off,
            Self::Cores => w.cores,
        }
    }

    /// Write this column's width into a geometry being resolved.
    ///
    /// Args:
    ///     w: Mutable geometry receiving the width.
    ///     value: Width to assign to this column.
    ///
    /// Returns:
    ///     Nothing.
    fn set(self, w: &mut ByIpWidths, value: f32) {
        match self {
            Self::Name => w.name = value,
            Self::Ip => w.ip = value,
            Self::Cpu => w.cpu = value,
            Self::Mem => w.mem = value,
            Self::Ping => w.ping = value,
            Self::Exch => w.exch = value,
            Self::Api => w.api = value,
            Self::Version => w.version = value,
            Self::Startup => w.startup = value,
            Self::TzOffset => w.tz_off = value,
            Self::Cores => w.cores = value,
        }
    }
}

/// Narrowest a user may drag a column.
///
/// MIRRORS MoonUI: `MIN_COLUMN_WIDTH` in `moon/data_table.rs`, which is private there, so nothing
/// checks this — if it moves, this must follow by hand. Matching it keeps a By-IP column from
/// reaching a width the flat table would have refused for the same content.
///
/// It bounds the DRAG, not the painted width. A column dragged to 40 still shrinks with everything
/// else on a narrow panel and can paint at [`MIN_SCALE`] × 40 = 20 px — which is not a new
/// weakness: `cores` is 40 at BASE and already painted at 20 there before any of this was
/// draggable. Giving each column a per-column PAINTED floor is MoonUI's water-fill answer
/// (`downscale_columns_to_available`), and it is deliberately not copied here: it would break the
/// single-shared-factor property that keeps the separate header element aligned over the rows.
pub(super) const MIN_COL_W: f32 = 40.0;

/// Widest a user may drag a column.
///
/// Deliberately NOT a MoonUI value: the data table has a horizontal scroller to absorb an over-wide
/// column and this tree has none. The cap is what makes the shrink rule in the module doc a fair
/// deal rather than a trap — see there for why one uncapped drag would resize all eight other
/// columns. 400 is far past the widest design width (150) and still leaves every other column its
/// base width on a panel around 1100 px.
pub(super) const MAX_COL_W: f32 = 400.0;

impl ByIpWidths {
    /// Design widths, used whenever the row has room for them.
    pub(super) const BASE: Self = Self {
        name: 150.0,
        ip: 118.0,
        cpu: 100.0,
        mem: 116.0,
        ping: 64.0,
        exch: 64.0,
        api: 84.0,
        cores: 40.0,
        version: 84.0,
        startup: 84.0,
        tz_off: 84.0,
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
            + self.ping
            + self.exch
            + self.api
            + self.cores
            + self.version
            + self.startup
            + self.tz_off
            + self.icon * 5.0
    }

    /// Everything a row spends besides its columns, at the current `rem` size.
    ///
    /// Both insets, the chevron gutter, every gap, and the connectivity-warning triangle a degraded
    /// server row inserts before its status dot. That triangle is reserved unconditionally: it
    /// appears exactly on the rows whose numbers matter most, and a budget that ignores it would let
    /// those rows — and only those — overflow.
    ///
    /// It reads `BASE.icon` rather than a resolved width, and that is correct rather than an
    /// oversight: `icon` is row chrome and is NOT in [`ByIpCol`], so no drag can move it and the
    /// budget stays constant across every user override.
    fn row_chrome(rem: f32) -> f32 {
        let icon = Self::BASE.icon;
        ROW_INSET_REMS * rem * 2.0
            + CHEVRON_W
            + ROW_GAPS * ROW_GAP_W
            + CELL_GAPS * CELL_GAP_W
            + icon
            + ROW_GAP_W
    }

    /// The design widths with the user's dragged widths applied, clamped, BEFORE any shrink.
    ///
    /// This is the LOGICAL geometry: what the columns would be on a panel wide enough for them. The
    /// header's drag handles anchor in it rather than in the painted widths, because anchoring in
    /// painted widths would let the shrink factor apply twice and snap the column narrower on the
    /// first pixel of a drag.
    ///
    /// A non-finite stored width is skipped rather than clamped — `f32::clamp` PANICS on NaN, and
    /// this map is deserialized from `layout.toml`, a plain file a user can hand-edit, so it is
    /// untrusted input rather than something only the drag handler ever writes. Unknown keys are
    /// ignored for the same reason.
    ///
    /// Args:
    ///     user: Stored per-column widths, keyed by [`ByIpCol::key`].
    ///
    /// Returns:
    ///     The design widths with every recognized override substituted.
    pub(super) fn resolved(user: &HashMap<String, f32>) -> Self {
        let mut w = Self::BASE;
        for col in ByIpCol::ALL {
            if let Some(&value) = user.get(col.key())
                && value.is_finite()
            {
                col.set(&mut w, value.clamp(MIN_COL_W, MAX_COL_W));
            }
        }
        w
    }

    /// Widths for a row of `available` pixels, starting from whatever the user dragged.
    ///
    /// Args:
    ///     available: Measured row width in pixels; a non-finite or non-positive value means the
    ///         first frame has not measured the view yet.
    ///     reserved: Right-edge width reserved for the status dot and overlay scrollbar.
    ///     rem: Window rem size, which sets the row insets.
    ///     user: Persisted width bag, keyed by [`ByIpCol::key`].
    ///
    /// Returns:
    ///     Resolved widths unscaled while the row still fits: the layout keeps a `flex_1` spacer
    ///     that absorbs any surplus, so growing the columns is not this function's job. A row
    ///     narrower than its columns gets one shared factor, floored at [`MIN_SCALE`], applied to
    ///     the RESOLVED widths — the module doc argues why the user's columns shrink with the rest
    ///     instead of being exempted.
    ///
    /// `available <= 0.0` means the view has not been measured yet (the first frame): return the
    /// resolved widths rather than collapsing everything to the floor for one frame.
    pub(super) fn for_width(
        available: f32,
        reserved: f32,
        rem: f32,
        user: &HashMap<String, f32>,
    ) -> Self {
        let base = Self::resolved(user);
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
            exch: self.exch * k,
            api: self.api * k,
            cores: self.cores * k,
            version: self.version * k,
            startup: self.startup * k,
            tz_off: self.tz_off * k,
            icon: self.icon * k,
            indent: self.indent * k,
        }
    }

    /// The narrowest row that still fits the resolved widths — the width at which shrinking begins.
    ///
    /// Exposed for the tests, which pin the crossover: a wrong chrome budget moves it, and every
    /// "does it shrink at all" assertion would still pass. It takes the same `user` bag as
    /// [`for_width`](Self::for_width) because a dragged column MOVES the crossover, and a crossover
    /// test that could not see the overrides would silently stop testing them.
    ///
    /// Args:
    ///     reserved: Right-edge width reserved for the status dot and overlay scrollbar.
    ///     rem: Window rem size, which sets the row insets.
    ///     user: Persisted width bag, keyed by [`ByIpCol::key`].
    ///
    /// Returns:
    ///     The available row width at which the resolved columns begin to shrink.
    #[cfg(test)]
    pub(super) fn full_width(reserved: f32, rem: f32, user: &HashMap<String, f32>) -> f32 {
        Self::resolved(user).columns_w() + Self::row_chrome(rem) + reserved
    }
}

#[cfg(test)]
/// Shrink-factor behaviour of the By-IP columns.
mod tests;
