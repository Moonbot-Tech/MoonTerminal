//! Dock-панели окна группы (порт egui `src/dock/*`) как `moon_ui::Panel` —
//! получают вкладки, сплиты, отцепление в окно и персист раскладки от `DockArea`
//! через MoonPalette Dock/TabPanel. По файлу на панель:
//! - [`chart`] — чарт (offscreen wgpu-движок + ввод + оси), центр дока;
//! - [`detects`] — лента детектов группы (откпрепляемая);
//! - [`orders`] — таблица открытых ордеров группы (фильтры/сортировка/клик→чарт);
//! - [`order`] — кнопки BUY/SELL/Cancel/Panic;
//! - [`log`] — вкладка «Лог» (источник/файл/поиск/только ошибки, виртуализирован);
//! - [`report`] — вкладка «Отчёт» (закрытые сделки из SQLite, фильтры/сортировка);
//! - [`stub`] — заглушка Активы до подключения данных.

mod alerts;
mod assets;
mod chart;
mod common;
mod detects;
mod log;
mod orders;
mod report;
mod stub;

pub(crate) use common::{RadioMark, RenderGate, data_table_host, detach_button, num, radio_items};

pub use alerts::AlertsPanel;
pub use assets::{AssetsView, open as open_assets_window};
pub use chart::ChartPanel;
pub use detects::DetectsPanel;
pub use log::LogPanel;
pub use orders::OrdersPanel;
pub use report::ReportPanel;
pub use stub::StubPanel;
