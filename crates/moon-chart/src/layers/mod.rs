//! Instance types for line and marker geometry (logical time_rel/price coordinates).
//! They are rendered by the chartdx own-pass backends; the old moon-chart wgpu layer
//! pipelines were removed with the egui engine.

pub mod order_lines;

/// Dash pattern codes the segment shader takes in `SegInstance::pattern`. Moonbot's five pen
/// styles map onto these three: the shader has one dashed variant, not three.
/// Line pattern codes, shared by the full-width line and the segment.
///
/// The code IS Delphi's `TPenStyle` index, the same number the core's chart-object blob carries at
/// `@13` — so a figure drawn in Moonbot is drawn here in the style Moonbot named, without a table
/// in between that could disagree. The shaders switch on it directly; see `pattern_on` in
/// `order_lines.hlsl`, `chart_native.metal` and the two wgsl copies.
pub const SEG_PATTERN_SOLID: f32 = 0.0;
pub const SEG_PATTERN_DASH: f32 = 1.0;
pub const SEG_PATTERN_DOT: f32 = 2.0;
pub const SEG_PATTERN_DASH_DOT: f32 = 3.0;
pub const SEG_PATTERN_DASH_DOT_DOT: f32 = 4.0;

pub use order_lines::TIME_UNBOUNDED;
pub use order_lines::{
    LineInstance, MarkerInstance, SegInstance, ZoneInstance, MARKER_ANCHOR_BOTTOM,
    MARKER_ANCHOR_PRICE, MARKER_SHAPE_CROSS, MARKER_SHAPE_KNOT, SEG_EXTEND_EDGE, SEG_EXTEND_NONE,
    SEG_EXTEND_RAY,
};
