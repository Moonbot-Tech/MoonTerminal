//! Group-window dock panels and related tool views, ported from egui's `src/dock/*`. Docked views
//! implement `moon_ui::Panel`; `DockArea` supplies tabs, splits, detachment, and persisted layout
//! with MoonPalette dock and tab styling. Modules are organized by surface:
//! - [`chart`] renders the central chart and handles its input and axes;
//! - [`detects`] shows the detachable group detection ribbon;
//! - [`orders`] shows the group's open-order table, filters, sorting, and chart navigation;
//! - [`assets`] shows balances and positions and supports wallet transfers;
//! - [`alerts`] lists the group's local and core-managed chart alerts;
//! - [`log`] provides virtualized live and file-backed log browsing;
//! - [`news`] shows the group's news feed, merged and deduplicated across cores by `meta.id`;
//! - [`report`] queries and filters closed trades from SQLite;
//! - [`core_status`] renders per-core connection and resource telemetry;
//! - [`order_edit`] opens the active-order editor;
//! - [`common`] contains shared panel controls and rendering helpers;
//! - [`registry`] is the single source of truth for building each detachable panel docked and
//!   detached, driving persistence registration, detachment, and the default home strip;
//! - [`tab_menu`] builds the right-click menu for the dock tabs that have one;
//! - [`stub`] is the fallback view for an unknown detached-panel name.

mod alerts;
mod assets;
mod chart;
mod common;
mod core_status;
mod detects;
mod log;
mod news;
mod order_edit;
mod orders;
pub(crate) mod registry;
mod report;
mod stub;
pub(crate) mod tab_menu;

pub(crate) use common::{
    POPUP_GROUP_INSET, RadioMark, RenderGate, data_table_host, detach_button, icon_checkbox, num,
    popup_close_button, popup_group, popup_group_inset_px, popup_title, radio_items,
};
pub(crate) use order_edit::open_order_edit;

pub use alerts::AlertsPanel;
pub use assets::{AssetsView, open as open_assets_window};
pub use chart::ChartPanel;
pub use core_status::CoreStatusView;
pub use detects::DetectsPanel;
pub use log::LogPanel;
pub use news::NewsView;
pub use orders::OrdersPanel;
pub use report::ReportPanel;
pub use stub::StubPanel;
