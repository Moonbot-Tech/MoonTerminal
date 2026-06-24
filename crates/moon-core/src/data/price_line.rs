//! GPU-side price-line point format for chart rendering.

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct PriceLinePoint {
    pub time_rel_ms: f32,
    pub price: f32,
}
