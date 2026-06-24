//! Типы инстансов геометрии линий ордеров (логические координаты time_rel/price).
//! Рисует их НАШ own-pass DX11 (chartdx); wgpu-пайплайны слоёв удалены вместе с
//! egui-движком.

pub mod order_lines;

pub use order_lines::{LineInstance, MarkerInstance, SegInstance, ZoneInstance};
