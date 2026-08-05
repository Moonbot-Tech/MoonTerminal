//! Instance types for line and marker geometry (logical time_rel/price coordinates).
//! They are rendered by the chartdx own-pass backends; the old moon-chart wgpu layer
//! pipelines were removed with the egui engine.

pub mod order_lines;

/// Dash pattern codes the segment shader takes in `SegInstance::pattern`. Moonbot's five pen
/// styles map onto these three: the shader has one dashed variant, not three.
pub const SEG_PATTERN_SOLID: f32 = 0.0;
pub const SEG_PATTERN_DASH_DOT_DOT: f32 = 1.0;
pub const SEG_PATTERN_DOT: f32 = 2.0;

pub use order_lines::{
    LineInstance, MarkerInstance, SegInstance, ZoneInstance, MARKER_ANCHOR_BOTTOM,
    MARKER_ANCHOR_PRICE, MARKER_SHAPE_CROSS, MARKER_SHAPE_KNOT,
};
