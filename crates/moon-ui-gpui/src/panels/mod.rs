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

mod assets;
mod chart;
mod common;
mod detects;
mod log;
mod order;
mod orders;
mod report;
mod stub;

pub(crate) use common::{RenderGate, detach_button, num};

pub use assets::{AssetsView, open as open_assets_window};
pub use chart::ChartPanel;
pub use detects::DetectsPanel;
pub use log::LogPanel;
pub use order::OrderPanel;
pub use orders::{OrdersPanel, count_orders};
pub use report::ReportPanel;
pub use stub::StubPanel;
