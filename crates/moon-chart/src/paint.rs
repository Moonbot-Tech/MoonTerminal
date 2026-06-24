//! Общие мелочи времени для UI-оболочки. wgpu-рендер панелей (`render_panes`) и
//! сигнатура видимых панелей (`panes_visible_sig`) удалены вместе с egui-движком —
//! own-pass (chartdx) имеет свой рендер и `data_signature`.

/// Текущее unix-время в мс (та же шкала, что time_ms тиков). Реэкспорт единого
/// источника из `moon_core::util` — исторически жил здесь, путь сохранён ради
/// многочисленных `moon_chart::paint::now_unix_ms` в UI-оболочке.
pub use moon_core::util::now_unix_ms;
