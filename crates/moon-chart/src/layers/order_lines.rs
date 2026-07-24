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

/// Instance of an order price zone: a filled band between two prices extending to the right edge.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct ZoneInstance {
    pub price0: f32,
    pub price1: f32,
    pub color: [f32; 4],
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
    /// 0 = solid, 1 = DashDotDot, 2 = Dot (Moonbot trace parity).
    pub pattern: f32,
    /// 1 = the shader takes t1 from the userdata uniform edge (`cv_pad`).
    pub extend: f32,
    pub color: [f32; 4],
}

/// Marker anchored to a PRICE: the shader maps `price` through the chart's Y transform, so the
/// marker rides the data as the view pans and zooms.
pub const MARKER_ANCHOR_PRICE: f32 = 0.0;
/// Marker anchored to the PLOT BOTTOM in screen space: `price` is reinterpreted as a distance in
/// physical pixels ABOVE the plot's bottom edge, and the shader clips the marker horizontally to the
/// plot (never letting it spill into the price-axis gutter or the order book as it scrolls off).
/// Time still positions it on X. Used by news marks, which belong to a moment, not to a price.
pub const MARKER_ANCHOR_BOTTOM: f32 = 1.0;

/// Instance of a marker (cross/knot/news gem).
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct MarkerInstance {
    pub t_rel: f32,
    /// Price, or — under [`MARKER_ANCHOR_BOTTOM`] — physical pixels above the plot's bottom edge.
    pub price: f32,
    /// Half size in physical px; the news gem reads it as its half HEIGHT.
    pub size: f32,
    /// Line thickness in physical px; the news gem reads it as its half WIDTH, which is what makes
    /// the gem vertical without a second size field or a shader-side aspect constant.
    pub thickness: f32,
    pub shape: f32, // 0 = cross, 1 = filled knot, 2 = news gem (faceted diamond)
    /// [`MARKER_ANCHOR_PRICE`] or [`MARKER_ANCHOR_BOTTOM`].
    pub anchor: f32,
    /// Wedge this instance paints, `0..sectors`. Only the news gem is sliced; everything else
    /// leaves this at `0` with `sectors` `1` and draws whole.
    pub sector: f32,
    /// Number of wedges the mark is split into (1 = undivided).
    pub sectors: f32,
    pub color: [f32; 4],
}

/// `shape` of an order line's start/end cross.
pub const MARKER_SHAPE_CROSS: f32 = 0.0;
/// `shape` of a figure's square drag knot.
pub const MARKER_SHAPE_KNOT: f32 = 1.0;

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
}
