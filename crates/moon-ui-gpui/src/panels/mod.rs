//! Group-window dock panels and related tool views, ported from egui's `src/dock/*`. Docked views
//! implement `moon_ui::Panel`; `DockArea` supplies tabs, splits, detachment, and persisted layout
//! with MoonPalette dock and tab styling. Modules are organized by surface:
//! - [`chart`] renders the central chart and handles its input and axes;
//! - [`detects`] shows the detachable group detection ribbon;
//! - [`orders`] shows the group's open-order table, filters, sorting, and chart navigation;
//! - [`assets`] shows balances and positions and supports wallet transfers;
//! - [`alerts`] lists the group's local and core-managed chart alerts;
//! - [`log`] provides virtualized live and file-backed log browsing;
//! - [`report`] queries and filters closed trades from SQLite;
//! - [`core_status`] renders per-core connection and resource telemetry;
//! - [`order_edit`] opens the active-order editor;
//! - [`common`] contains shared panel controls and rendering helpers;
//! - [`stub`] is the fallback view for an unknown detached-panel name.

mod alerts;
mod assets;
mod chart;
mod common;
mod core_status;
mod detects;
mod log;
mod order_edit;
mod orders;
mod report;
mod stub;

pub(crate) use common::{
    RadioMark, RenderGate, data_table_host, detach_button, icon_checkbox, num, radio_items,
};
pub(crate) use order_edit::open_order_edit;

pub use alerts::AlertsPanel;
pub use assets::{AssetsView, open as open_assets_window};
pub use chart::ChartPanel;
pub use core_status::CoreStatusView;
pub use detects::DetectsPanel;
pub use log::LogPanel;
pub use orders::OrdersPanel;
pub use report::ReportPanel;
pub use stub::StubPanel;
