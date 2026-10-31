//! GPU-side price-line point format for chart rendering.

/// The price a line point carries to say "no data here": the segments on either side of it are
/// not drawn, so a stretch nothing was recorded for stays empty instead of being bridged by a
/// straight line. Negative because a drawn price is always positive — every reader filters to
/// `price > 0` — and not NaN because shader compilers may fold a NaN test away under fast math.
pub const PRICE_LINE_GAP: f32 = -1.0;

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct PriceLinePoint {
    pub time_rel_ms: f32,
    pub price: f32,
}

impl PriceLinePoint {
    /// Whether this point is a [`PRICE_LINE_GAP`] rather than a price.
    pub fn is_gap(&self) -> bool {
        self.price < 0.0
    }
}
