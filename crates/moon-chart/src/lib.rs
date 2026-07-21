//! Чарт-математика и геометрия, wgpu-free. Даёт общей UI-оболочке:
//! layout-константы осей/стакана, тик-математику осей (`axes`), вид (`view::ChartView`
//! — зум/пан/Y), типы инстансов линий ордеров (`layers`), и `build_order_geometry`
//! (логические time_rel/price → примитивы).
//!
//! Сами данные рисует НАШ own-pass DX11 (`chartdx` в moon-ui-gpui), а не wgpu-движок:
//! старый wgpu-рендер (Chart/canvas/слои/style) удалён вместе с egui-бинарём.

// Подписи осей рисует UI-оболочка (GPUI-оверлей). Здесь — только layout-константы.
pub const PRICE_AXIS_W: f32 = 56.0;
pub const TIME_AXIS_H: f32 = 16.0;
/// Ширина зоны стакана справа (как BOOK_WIDTH_CSS стенда = 220), физ. пиксели.
pub const GLASS_ZONE_PX: f32 = 220.0;

pub mod axes;
pub mod container;
// `data` / market-source models live in moon-core. Ре-экспорт под прежним путём.
pub use moon_core::data;
pub mod figures;
pub use figures::build_figure_geometry;
pub mod layers;
pub mod order_geometry;
pub use order_geometry::build_order_geometry;
pub mod paint;
pub mod view;
