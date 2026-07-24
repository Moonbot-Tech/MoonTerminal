//! Instance types for line and marker geometry (logical time_rel/price coordinates).
//! They are rendered by the chartdx own-pass backends; the old moon-chart wgpu layer
//! pipelines were removed with the egui engine.

pub mod order_lines;

pub use order_lines::{
    LineInstance, MarkerInstance, SegInstance, ZoneInstance, MARKER_ANCHOR_BOTTOM,
    MARKER_ANCHOR_PRICE, MARKER_SHAPE_CROSS, MARKER_SHAPE_KNOT,
};
