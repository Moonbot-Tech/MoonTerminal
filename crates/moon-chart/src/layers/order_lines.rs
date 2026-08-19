//! Instance types for order lines and figure geometry in LOGICAL coordinates (time_rel/price);
//! chartdx own-pass shaders map time→x and price→y using chart uniforms. The retained-order
//! workload is tiny (dozens of orders). `crate::build_order_geometry` builds retained-order geometry,
//! while `crate::build_figure_geometry` builds user-defined figure geometry.

/// Instance of a continuous horizontal line (liquidation or figure).
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct LineInstance {
    pub price: f32,
    pub color: [f32; 4],
    pub style: f32, // 0 = solid, 1 = dashed
    pub thickness: f32,
}

/// Time value meaning "no bound on this side": the shader clamps it to the plot's own edge.
///
/// FINITE on purpose. The obvious sentinel is an infinity, but the Metal library is compiled with
/// fast math enabled, where IEEE handling of infinities is explicitly not guaranteed — an order
/// zone's full-width span would then be undefined on macOS. A value 17 orders of magnitude past
/// any real timestamp survives the multiply as a finite number and clamps the same way everywhere.
pub const TIME_UNBOUNDED: f32 = 1e30;

// Far past any real timestamp — a Unix millisecond stays under 1e13 for the next thousand years —
// so no figure's own span can collide with the sentinel, and finite so Metal's fast math keeps it.
const _: () = assert!(TIME_UNBOUNDED > 1e20 && TIME_UNBOUNDED < f32::MAX);

/// Instance of a filled band between two prices, bounded in time.
///
/// `t0_rel`/`t1_rel` are relative-to-epoch milliseconds like every other instance here;
/// ±[`TIME_UNBOUNDED`] means "to the plot's edge on that side", which is what an order zone wants.
/// The two fields ride in the GPU struct's two spare floats, so no buffer layout changed to gain
/// them.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct ZoneInstance {
    pub price0: f32,
    pub price1: f32,
    pub t0_rel: f32,
    pub t1_rel: f32,
    pub color: [f32; 4],
}

impl ZoneInstance {
    /// A band spanning the whole plot in time — an order zone, a price corridor.
    pub fn full_width(price0: f32, price1: f32, color: [f32; 4]) -> Self {
        Self {
            price0,
            price1,
            t0_rel: -TIME_UNBOUNDED,
            t1_rel: TIME_UNBOUNDED,
            color,
        }
    }
}

/// Instance of a line segment.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct SegInstance {
    pub t0_rel: f32,
    pub p0: f32,
    pub t1_rel: f32,
    pub p1: f32,
    pub thickness: f32,
    /// Delphi `TPenStyle` index: 0 = solid, 1 = dash, 2 = dot, 3 = dash-dot, 4 = dash-dot-dot.
    /// The shaders switch on it directly in `pattern_on`; the named constants live in
    /// [`crate::layers`] as `SEG_PATTERN_*`.
    pub pattern: f32,
    /// How the far end is found: [`SEG_EXTEND_NONE`], [`SEG_EXTEND_EDGE`] or [`SEG_EXTEND_RAY`].
    pub extend: f32,
    /// Whether the shader pins the segment to the plot when its price leaves the visible band:
    /// [`SEG_CLAMP_NONE`] or [`SEG_CLAMP_PLOT`].
    pub clamp: f32,
    pub color: [f32; 4],
}

/// The segment keeps whatever Y its prices map to, leaving the plot when they do.
pub const SEG_CLAMP_NONE: f32 = 0.0;
/// The segment is pinned to the nearer horizontal edge of the plot once its price leaves the
/// visible band: each endpoint's Y is clamped into the plot, centre exactly on the boundary.
///
/// No inset. Half a thickness would look tidier and was tried first, but the thickness is a
/// per-style setting the highlight multiplies by 1.7 on hover — the pinned line would step inward
/// under the pointer, and neither the hit test nor the label column has that number to agree with.
/// Exactness across four readers beats half a line of tidiness, and the three shaders and
/// [`crate::order_geometry::pin_line_y`] all say so by pointing here.
///
/// Only the Y is touched. The instance still carries the order's REAL price, and so does everything
/// downstream of it — the store, the labels and the price that reaches the core on a drag. This is a
/// drawing rule, not a value, which is why it rides the instance instead of being folded into `p0`
/// and `p1` on the CPU: folding it there would make the whole userdata layer depend on the Y view
/// and rebuild on every pan, while the shader already holds the transform and re-evaluates it free.
///
/// Mirrored by [`crate::order_geometry::pin_line_y`] for the hit test and the label column, which
/// have to agree with the shader on where a pinned line ended up. Three shader copies implement it —
/// `order_lines.hlsl`, `native_seg.wgsl`, `chart_native.metal`.
pub const SEG_CLAMP_PLOT: f32 = 1.0;

/// The segment ends where the tool put it.
pub const SEG_EXTEND_NONE: f32 = 0.0;
/// The shader takes `t1` from the userdata uniform edge (`cv_pad`) and KEEPS `p1`: a constant-price
/// line running to the right edge, which is what an order line is.
pub const SEG_EXTEND_EDGE: f32 = 1.0;
/// The shader extrapolates THROUGH `(t1, p1)` to the plot edge the direction points at — a ray.
///
/// Distinct from [`SEG_EXTEND_EDGE`] because that one flattens the line: it moves the far end in
/// time while leaving its price, which turns any sloped segment horizontal.
pub const SEG_EXTEND_RAY: f32 = 2.0;

/// Marker anchored to a PRICE: the shader maps `price` through the chart's Y transform, so the
/// marker rides the data as the view pans and zooms.
pub const MARKER_ANCHOR_PRICE: f32 = 0.0;
/// Marker anchored to the PLOT BOTTOM in screen space: `price` is reinterpreted as a distance in
/// physical pixels ABOVE the plot's bottom edge, and the shader clips the marker horizontally to the
/// plot (never letting it spill into the price-axis gutter or the order book as it scrolls off).
/// Time still positions it on X. Used by news marks, which belong to a moment, not to a price.
pub const MARKER_ANCHOR_BOTTOM: f32 = 1.0;

/// Instance of a marker glyph shared by orders, figures, news, warnings, and trade history.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct MarkerInstance {
    pub t_rel: f32,
    /// Price, or — under [`MARKER_ANCHOR_BOTTOM`] — physical pixels above the plot's bottom edge.
    pub price: f32,
    /// Half size in physical px; news gems and trade arrows read it as their half HEIGHT.
    pub size: f32,
    /// Line thickness in physical px; news gems and trade arrows read it as their half WIDTH, which
    /// lets non-square glyphs reuse the existing instance layout.
    pub thickness: f32,
    /// Which glyph the shader draws; see the registry on [`MARKER_SHAPE_CROSS`].
    pub shape: f32,
    /// [`MARKER_ANCHOR_PRICE`] or [`MARKER_ANCHOR_BOTTOM`].
    pub anchor: f32,
    /// A TAGGED field whose meaning is decided by [`Self::shape`] — read the two together.
    ///
    /// For the news gem: the wedge this instance paints, `0..sectors`. For a trade arrow
    /// ([`MARKER_SHAPE_ARROW_UP`] / [`MARKER_SHAPE_ARROW_DOWN`]): the number of trades the arrow's
    /// cluster aggregates, which the shader turns into a capped size increase. Everything else
    /// leaves this at `0` with `sectors` `1` and draws whole.
    ///
    /// The union is safe because no instance is ever both a gem and an arrow, and it keeps the GPU
    /// struct — and therefore all three backends' strides — untouched.
    pub sector: f32,
    /// Number of wedges the mark is split into (1 = undivided).
    pub sectors: f32,
    pub color: [f32; 4],
}

/// The full `shape` registry, in one place because the ids are split across two modules and the
/// shaders dispatch on bare numbers:
///
/// | id | shape | declared in |
/// |----|-------|-------------|
/// | 0 | order line's start/end cross | here |
/// | 1 | figure's square drag knot (a filled disc) | here |
/// | 2 | news gem (faceted diamond) | `crate::news_marks` |
/// | 3 | warning badge (triangle with an exclamation) | `crate::news_marks` |
/// | 4 | trade-history arrow pointing UP — a BUY action | here |
/// | 5 | trade-history arrow pointing DOWN — a SELL action | here |
///
/// Adding an id means editing all three shader copies (`order_lines.hlsl`, `native_marker.wgsl`,
/// `chart_native.metal`). Mind that the warning badge is dispatched there as an OPEN-ENDED
/// `shape > 2.5`, so a new id above 3 must be tested for BEFORE that branch or it renders as a
/// warning triangle. `theme_contract` asserts both the cross-copy presence and the branch order.
///
/// `shape` of an order line's start/end cross.
pub const MARKER_SHAPE_CROSS: f32 = 0.0;
/// `shape` of a figure's square drag knot.
pub const MARKER_SHAPE_KNOT: f32 = 1.0;
/// `shape` of a trade-history arrow pointing UP: a BUY action — a long's entry or a short's exit.
///
/// The shader hangs the arrow off its price so the apex points at the level from below and the
/// body clears the candle; the offset is derived from this id alone, costing no instance field.
pub const MARKER_SHAPE_ARROW_UP: f32 = 4.0;
/// `shape` of a trade-history arrow pointing DOWN: a SELL action — a long's exit or a short's entry.
pub const MARKER_SHAPE_ARROW_DOWN: f32 = 5.0;

impl MarkerInstance {
    /// A price-anchored, undivided marker — every marker except a news mark.
    ///
    /// Order crosses and figure knots differ only in `shape`, so they share one constructor rather
    /// than each spelling out the anchor and wedge fields that exist for news marks alone.
    pub fn at_price(
        t_rel: f32,
        price: f32,
        size: f32,
        thickness: f32,
        shape: f32,
        color: [f32; 4],
    ) -> Self {
        Self {
            t_rel,
            price,
            size,
            thickness,
            shape,
            anchor: MARKER_ANCHOR_PRICE,
            sector: 0.0,
            sectors: 1.0,
            color,
        }
    }

    /// A trade-history arrow carrying its cluster's member count.
    ///
    /// `half_h` and `half_w` are the RESTING half-extents; the shader grows both by a capped
    /// function of `count`, so a caller must not pre-scale them or the growth applies twice.
    ///
    /// Args:
    ///     t_rel: Arrow timestamp relative to the chart epoch.
    ///     price: Price touched by the arrow's apex.
    ///     half_h: Resting half-height in physical pixels.
    ///     half_w: Resting half-width in physical pixels.
    ///     shape: Trade-arrow shape id.
    ///     count: Number of trades aggregated into the arrow.
    ///     color: Normalized RGBA colour.
    ///
    /// Returns:
    ///     A price-anchored marker with the cluster count encoded for the shader.
    pub fn arrow(
        t_rel: f32,
        price: f32,
        half_h: f32,
        half_w: f32,
        shape: f32,
        count: u32,
        color: [f32; 4],
    ) -> Self {
        Self {
            sector: count.max(1) as f32,
            ..Self::at_price(t_rel, price, half_h, half_w, shape, color)
        }
    }
}
