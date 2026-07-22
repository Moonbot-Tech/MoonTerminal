//! Chart math and geometry, wgpu-free. Provides the shared UI shell with:
//! layout constants for the axes and order book, axis tick math (`axes`), the view (`view::ChartView`
//! — zoom/pan/Y), order-line instance types (`layers`), and `build_order_geometry`
//! (logical time_rel/price → primitives).
//!
//! The data itself is rendered by our own-pass DX11 (`chartdx` in moon-ui-gpui), not by a wgpu engine:
//! the old wgpu renderer (Chart/canvas/layers/style) was removed with the egui binary.

// The UI shell draws the axis labels (as a GPUI overlay). Only layout constants live here.
pub const PRICE_AXIS_W: f32 = 56.0;
pub const TIME_AXIS_H: f32 = 16.0;
/// Width of the order book zone on the right (matching the reference's BOOK_WIDTH_CSS = 220), in physical pixels.
pub const GLASS_ZONE_PX: f32 = 220.0;

pub mod axes;
pub mod container;
// `data` / market-source models live in moon-core. Re-export them under the previous path.
pub use moon_core::data;
pub mod figures;
pub use figures::build_figure_geometry;
pub mod layers;
pub mod order_geometry;
pub use order_geometry::build_order_geometry;
pub mod paint;
pub mod view;
