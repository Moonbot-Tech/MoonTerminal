//! Instance types for line and marker geometry (logical time_rel/price coordinates).
//! They are rendered by the chartdx own-pass backends; the old moon-chart wgpu layer
//! pipelines were removed with the egui engine.

pub mod order_lines;

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

/// Expand a theme's 8-bit RGB triple into the normalized RGBA every instance colour field takes.
///
/// Shared by the builders that colour instances straight from a theme token. `news_marks` and
/// `figures` deliberately keep their own: one takes a PACKED `u32` and the other takes an RGBA
/// quad whose alpha it MULTIPLIES rather than replaces, so folding them in here would hide two
/// different contracts behind one name.
///
/// Args:
///     rgb: Red, green, and blue channels in the 8-bit theme representation.
///     alpha: Alpha channel to preserve as a normalized float.
///
/// Returns:
///     The four channels normalized for an instance colour field.
pub(crate) fn rgb_with_alpha(rgb: [u8; 3], alpha: f32) -> [f32; 4] {
    [
        rgb[0] as f32 / 255.0,
        rgb[1] as f32 / 255.0,
        rgb[2] as f32 / 255.0,
        alpha,
    ]
}

pub use order_lines::TIME_UNBOUNDED;
pub use order_lines::{
    LineInstance, MarkerInstance, SegInstance, ZoneInstance, MARKER_ANCHOR_BOTTOM,
    MARKER_ANCHOR_PRICE, MARKER_SHAPE_ARROW_DOWN, MARKER_SHAPE_ARROW_UP, MARKER_SHAPE_CROSS,
    MARKER_SHAPE_KNOT, SEG_CLAMP_NONE, SEG_CLAMP_PLOT, SEG_EXTEND_EDGE, SEG_EXTEND_NONE,
    SEG_EXTEND_RAY,
};
