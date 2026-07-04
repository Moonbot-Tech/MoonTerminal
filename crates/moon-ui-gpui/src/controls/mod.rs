//! Торговый тулбар: прикладная сборка терминала поверх MoonPalette.
//!
//! Логика остаётся терминальной: size/sell пока логируют todo, scale/live пишут в
//! `Backend`. Визуальные контролы берём из палитры, выведенной из HTML-эталона.
//!
//! Разнесено по подмодулям (mod.rs = границы слайдеров + re-export'ы):
//! - [`fmt`] — форматирование значений (size/sell/поля) и шаги колеса мыши;
//! - [`metric`] — торговые метрики TP/SL/Lev: кнопки-триггеры и контент попапов;
//! - [`strips`] — полосы пресетов размера/продажи с overlay-взаимодействием;
//! - [`scale`] — дропдауны масштаба цены (вкладки/AddToChart-stack);
//! - [`toolbar`] — сборка полосы тулбара.

mod fmt;
mod metric;
mod scale;
mod strips;
mod toolbar;

pub use fmt::{fmt_adaptive, fmt_field2, fmt_field2_signed};
pub use metric::{TradeMetric, metric_popup_content};
pub(crate) use scale::{scale_dropdown_for_add_stack, scale_dropdown_for_tabs};
pub use toolbar::toolbar;

use crate::design;

/// Границы слайдеров торговых метрик `(min, max, step)` (по смыслу ядра). Использует и
/// `Shell` при создании состояний слайдеров.
pub const TP_NORMAL: (f32, f32, f32) = (2.0, 100.0, 1.0); // x_tmode off: 2..100% (мин = 2)
/// Граница, ниже которой работает файн-слайдер (суб-процент через scalp). Верхний TP на 2
/// = на минимуме → нижний слайдер активен (0..2). Выше — нижний disabled. Используется для
/// надписи/стыка («2%»).
pub const TP_FINE_MAX: f32 = 2.0;
/// Кап ЗНАЧЕНИЯ файн-слайдера (scalp): ровно 2% должно уходить в ОСНОВНОЙ TP (верхний слайдер,
/// `x_sell`), иначе бот уходит в «scalping mode» и продаёт по минимуму (~0.4%). Поэтому скальп
/// ограничиваем 1.99%; шкала/надпись остаются «2%».
pub const TP_FINE_CAP: f32 = 1.99;
pub const TP_EXT: (f32, f32, f32) = (100.0, 900.0, 10.0); // x_tmode on («s9»): 100..900%
pub const SL_BOUNDS: (f32, f32, f32) = (-20.0, 1.0, 0.01); // знаковый: -20..+1%
pub const LEV_BOUNDS: (f32, f32, f32) = (1.0, 125.0, 1.0);

/// Высота полосы тулбара: 2-я строка header из HTML-эталона.
pub const TOOLBAR_H: f32 = design::TOOLBAR_H;
