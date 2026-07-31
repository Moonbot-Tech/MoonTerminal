//! Core Status panel: connection state and typed protocol-v4 resource telemetry
//! (`Event::KernelHealth`) for every core in scope.
//!
//! `CoreData::sys` holds the latest sample, while `sys_rev` invalidates the panel
//! when metric values or the decoded endpoint change.
//!
//! Like the Assets panel, it is scoped to a window group and can live in a dock
//! tab or a detached window. [`crate::persistence::table_persist`] stores separate column
//! widths for `:dock` and `:win`. This module owns data and lifecycle; [`server_view`]
//! and [`table`] own the two presentations.

mod by_ip_header;
mod by_ip_widths;
mod cache;
mod chart;
mod config_popup;
mod interactions;
mod model;
mod ordering;
mod presentation;
mod server_view;
mod table;
#[cfg(test)]
mod tests;
mod warnings;

use std::collections::{HashMap, HashSet};
use std::net::IpAddr;
use std::rc::Rc;

use gpui::*;
use moon_ui::{
    DockArea, MoonDataTableState, MoonInputState, MoonPalette, MoonSegmentItem,
    MoonSegmentedControl, MoonTreeState, Panel, PanelEvent, PanelState, h_flex, v_flex,
};

use crate::Backend;
use crate::core_order::{CoreOrder, OrderedCores};
use crate::design;
use model::{CoreStatusRow, ServerKey, ServerStatusGroup};
use moon_core::session::CoreId;
use rust_i18n::t;

/// Chart X-axis span selectable in the detached window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ChartWindow {
    /// Last five minutes.
    Min5,
    /// Last hour.
    Hour1,
}

impl ChartWindow {
    /// Number of seconds (and points at 1 Hz) the window spans.
    fn secs(self) -> usize {
        match self {
            Self::Min5 => 300,
            Self::Hour1 => 3600,
        }
    }

    /// Localization key for the span label (`5 мин` / `1 ч`).
    fn label_key(self) -> &'static str {
        match self {
            Self::Min5 => "core_status.chart_5m",
            Self::Hour1 => "core_status.chart_1h",
        }
    }
}

impl Default for ChartWindow {
    fn default() -> Self {
        Self::Min5
    }
}

/// Available Core Status presentations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CoreStatusMode {
    /// Expandable server rows grouped by endpoint address.
    ByIp,
    /// Existing one-row-per-core telemetry table.
    Flat,
    /// Recorded warning episodes from the database, newest first.
    Warnings,
}

/// How many recent warning episodes the Warnings list shows.
const WARN_LIST_LIMIT: usize = 500;

impl Default for CoreStatusMode {
    /// Return the server-by-IP presentation used on every new panel instance.
    fn default() -> Self {
        Self::ByIp
    }
}

/// Group-scoped Core Status panel for a dock tab or detached window.
pub struct CoreStatusView {
    pub(super) backend: Entity<Backend>,
    /// Window group whose cores define this panel's scope, matching the Assets panel.
    group: String,
    /// Multi-select core filter; an empty set means every core in the group.
    pub(super) sel_cores: HashSet<CoreId>,
    /// Unix ms of the last repaint; telemetry repaints at most once per second.
    last_repaint_ms: i64,
    /// Whether any server currently has a warning; drives the dock-tab badge.
    has_warn: bool,
    /// Whether this instance is a detached window (vs a dock tab). Only a window renders the chart.
    detached: bool,
    /// Selected chart X-axis span in the detached window.
    chart_window: ChartWindow,
    /// Server whose chart the detached window shows; clicking a server row selects it. `None` falls
    /// back to the first server.
    chart_server: Option<ServerKey>,
    /// A specific core to chart instead of the server aggregate; set by clicking a core in the
    /// expanded list, cleared when a server row is clicked. Takes precedence over `chart_server`.
    chart_core: Option<CoreId>,
    cached_rows: Rc<Vec<CoreStatusRow>>,
    cached_groups: Rc<Vec<ServerStatusGroup>>,
    /// Servers whose IP is momentarily revealed by the eye control. Transient: cleared on blur,
    /// never persisted, so IPs return to masked when focus leaves the panel.
    revealed_ips: HashSet<ServerKey>,
    /// Server whose name is being renamed inline, if any.
    editing: Option<ServerKey>,
    /// Input state backing the inline rename field while [`Self::editing`] is set.
    edit_input: Option<Entity<MoonInputState>>,
    /// Active flat-table sort as `(column key, ascending)`, or `None` for the default
    /// attention-first order.
    flat_sort: Option<(String, bool)>,
    /// Active By IP column sort as `(field, ascending)`. Default `(Name, ascending)` reproduces the
    /// former fixed order; warnings always pin to the top regardless of the field or direction.
    group_sort: (ordering::GroupSortField, bool),
    mode: CoreStatusMode,
    tree_state: Entity<MoonTreeState>,
    table_state: Entity<MoonDataTableState>,
    /// Column state for the Warnings list table (separate widths from the flat telemetry table).
    warn_table_state: Entity<MoonDataTableState>,
    /// Whether the alert-axis toggle popover (the gear beside the mode control) is open.
    warn_cfg_open: bool,
    /// Last measured width of the By IP list, in pixels; `0` until the first frame measures it.
    ///
    /// The By IP view draws its own fixed columns (it is a tree, not a data table), so it needs the
    /// rendered width to know when they no longer fit. [`server_view`] writes it from a measuring
    /// canvas and only when it actually changes, so a repaint does not feed itself.
    by_ip_width: f32,
    /// Context-qualified column-width persistence ID (`core-status-table:dock` or `:win`).
    widths_id: String,
    dock: Option<WeakEntity<DockArea>>,
    focus: FocusHandle,
}

impl CoreStatusView {
    /// Construct a group-scoped Core Status panel and its table/tree state.
    ///
    /// Args:
    ///     backend: Shared terminal backend.
    ///     group: Window group that defines the core scope.
    ///     detached: Whether column widths use the detached-window persistence key.
    ///     _window: Host window reserved for panel construction symmetry.
    ///     cx: View context used for observers and child entities.
    ///
    /// Returns:
    ///     A panel whose default presentation is server-by-IP.
    fn new(
        backend: Entity<Backend>,
        group: String,
        detached: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        // This fires on every backend notify (event-driven, ≤4 Hz — not a timer/poll), but the
        // rebuild is gated to once per second. Detection AND chart-history recording run in the
        // backend engine (backend-always), so the panel only rebuilds its display from that state.
        cx.observe(&backend, |this, _backend, cx| {
            let now = moon_chart::paint::now_unix_ms() as i64;
            if now - this.last_repaint_ms < 1000 {
                return;
            }
            this.last_repaint_ms = now;
            this.rebuild_cache(cx);
            cx.notify();
        })
        .detach();

        let widths_id = crate::persistence::table_persist::ctx_id("core-status-table", detached);
        let saved_widths = crate::persistence::table_persist::saved(backend.read(cx), &widths_id);
        let table_state = cx.new(|_| {
            let mut s = MoonDataTableState::new();
            s.column_widths = saved_widths;
            s
        });
        cx.observe(&table_state, |this, state, cx| {
            crate::persistence::table_persist::persist(&this.backend, &this.widths_id, &state, cx);
        })
        .detach();
        let warn_table_state = cx.new(|_| MoonDataTableState::new());
        let tree_state = cx.new(|cx| MoonTreeState::new(cx));
        let focus = cx.focus_handle();

        // A revealed IP is momentary: when focus leaves the panel it re-masks. The eye control
        // focuses this handle on reveal, so this blur fires when the user clicks away.
        cx.on_blur(&focus, window, |this, _window, cx| {
            if !this.revealed_ips.is_empty() {
                this.revealed_ips.clear();
                cx.notify();
            }
        })
        .detach();

        let mut this = Self {
            backend,
            group,
            sel_cores: HashSet::new(),
            last_repaint_ms: 0,
            has_warn: false,
            detached,
            chart_window: ChartWindow::default(),
            chart_server: None,
            chart_core: None,
            cached_rows: Rc::new(Vec::new()),
            cached_groups: Rc::new(Vec::new()),
            revealed_ips: HashSet::new(),
            editing: None,
            edit_input: None,
            flat_sort: None,
            group_sort: (ordering::GroupSortField::Name, true),
            mode: CoreStatusMode::default(),
            tree_state,
            table_state,
            warn_table_state,
            warn_cfg_open: false,
            by_ip_width: 0.0,
            widths_id,
            dock: None,
            focus,
        };
        this.rebuild_cache(cx);
        this
    }

    /// Reconstruct a dock tab from `docks.json` using the `:dock` width context.
    pub fn restored_group(
        backend: Entity<Backend>,
        group: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        Self::new(backend, group, false, window, cx)
    }

    /// Build detached-window content, framed by `DetachedWindow`, using the `:win` width context.
    pub fn detached_group(
        backend: Entity<Backend>,
        group: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        Self::new(backend, group, true, window, cx)
    }

    /// Return the table state used by the detached window's auto-width reset button.
    pub fn table_state(&self) -> Entity<MoonDataTableState> {
        self.table_state.clone()
    }

    /// Return this panel group's cores in canonical order.
    pub(super) fn scope_cores(&self, b: &Backend) -> OrderedCores {
        CoreOrder::new(&b.config).from_sessions(b.session.sessions(), |s| s.group == self.group)
    }
}

impl EventEmitter<PanelEvent> for CoreStatusView {}
impl Focusable for CoreStatusView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Panel for CoreStatusView {
    fn panel_name(&self) -> &'static str {
        "CoreStatus"
    }
    /// Visible tab caption. `panel_name` is the stable persistence key and stays untouched.
    fn tab_name(&self, _cx: &App) -> Option<SharedString> {
        crate::persistence::panel_meta::tab_label(self.panel_name())
    }
    fn closable(&self, _cx: &App) -> bool {
        true
    }
    fn show_dock_header(&self, _cx: &App) -> bool {
        true
    }
    fn title(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        crate::persistence::panel_meta::panel_title(self.panel_name())
    }
    /// Draw a warning triangle on the dock tab (right of the label) while any server warns, like
    /// the News tab's unread badge. It clears itself when every warning resolves.
    fn title_suffix(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let p = MoonPalette::active(cx);
        self.has_warn.then(|| {
            svg()
                .path("icons/triangle-alert.svg")
                .size(px(13.0))
                .flex_none()
                .text_color(rgb(p.amber))
                .into_any_element()
        })
    }
    fn dump(&self, _cx: &App) -> PanelState {
        crate::persistence::dock_persist::panel_state_with_group("CoreStatus", &self.group)
    }
    fn on_added_to(
        &mut self,
        dock_area: WeakEntity<DockArea>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) {
        self.dock = Some(dock_area);
    }
    fn toolbar_buttons(
        &mut self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Vec<AnyElement>> {
        Some(vec![crate::persistence::table_persist::reset_button(
            "core-status-reset-widths",
            &self.table_state,
        )])
    }
}

impl Render for CoreStatusView {
    /// Render the active presentation with shared filters and counters.
    ///
    /// Args:
    ///     _window: Host window reserved for renderer symmetry.
    ///     cx: View context used for backend reads, palette, and callbacks.
    ///
    /// Returns:
    ///     Full dock, detached-window, or group-host panel contents.
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let cores = self.scope_cores(self.backend.read(cx));
        let rows = self.cached_rows.clone();
        let groups = self.cached_groups.clone();
        let p = MoonPalette::active(cx);
        let total_cores = rows.len();

        let core_bar = self.core_bar(&cores, cx);
        let footer = self.footer(&groups, total_cores, cx);
        let content = match self.mode {
            CoreStatusMode::ByIp => server_view::grouped_server_view(
                groups.clone(),
                Rc::new(self.revealed_ips.clone()),
                self.editing,
                self.edit_input.clone(),
                self.chart_server,
                self.chart_core,
                self.group_sort,
                self.by_ip_width,
                // Row insets are `rems`, so the By-IP width budget needs the window's rem size —
                // MoonUI's Root sets it from the theme font size, which the Font slider moves.
                f32::from(window.rem_size()),
                &self.tree_state,
                cx,
            ),
            CoreStatusMode::Flat => {
                let server_names: HashMap<ServerKey, String> = groups
                    .iter()
                    .map(|group| (group.key, group.display_name.clone()))
                    .collect();
                table::core_status_table(
                    "core-status-table",
                    Rc::new(self.sorted_flat_rows(&rows)),
                    Rc::new(server_names),
                    &self.table_state,
                    cx,
                )
                .into_any_element()
            }
            CoreStatusMode::Warnings => {
                let b = self.backend.read(cx);
                let episodes = b.warn_episodes_recent(WARN_LIST_LIMIT);
                // Resolve each server IP to its display name (connected group, then a saved custom
                // name), never the raw IP — matching how the panel masks addresses.
                let server_names: HashMap<IpAddr, String> = episodes
                    .iter()
                    .filter_map(|episode| episode.server_ip)
                    .map(|ip| {
                        let name = groups
                            .iter()
                            .find(|group| group.address == Some(ip))
                            .map(|group| group.display_name.clone())
                            .or_else(|| b.layout.core_server_names.get(&ip.to_string()).cloned())
                            .unwrap_or_else(|| "—".to_string());
                        (ip, name)
                    })
                    .collect();
                let core_names: HashMap<CoreId, String> = b
                    .config
                    .servers
                    .iter()
                    .map(|server| (server.id, server.name.clone()))
                    .collect();
                warnings::warnings_table(
                    "core-status-warnings",
                    Rc::new(episodes),
                    Rc::new(server_names),
                    Rc::new(core_names),
                    &self.warn_table_state,
                    cx,
                )
                .into_any_element()
            }
        };

        // A detached window gets a live CPU/memory chart for ONE subject: a clicked core, else the
        // clicked (or first) server's machine aggregate — never per-core overlays. The dock tab builds
        // nothing here, so it pays no chart cost.
        let chart_el: Option<AnyElement> = self
            .detached
            .then(|| {
                let b = self.backend.read(cx);
                let now_sec = moon_chart::paint::now_unix_ms() as i64 / 1000;
                // A selected core, if it is still in scope, charts ITS OWN samples — CPU/RAM plus its
                // own client↔core and core→exchange pings, not the server-wide worst. Falls through to
                // the server aggregate otherwise.
                if let Some(core) = self.chart_core.and_then(|core_id| {
                    groups
                        .iter()
                        .flat_map(|group| group.cores.iter())
                        .find(|core| core.id == core_id)
                }) {
                    // Split the core's 4-metric samples into the (cpu, mem) machine lines and its own
                    // ping/exch series, all from the same per-core ring.
                    let (points, ping_points, exch_points): (
                        std::collections::VecDeque<(u8, u8)>,
                        std::collections::VecDeque<u16>,
                        std::collections::VecDeque<u16>,
                    ) = b
                        .core_line_hist
                        .ring(core.id)
                        .map(|r| {
                            (
                                r.iter().map(|m| (m.cpu, m.mem)).collect(),
                                r.iter().map(|m| m.ping).collect(),
                                r.iter().map(|m| m.exch).collect(),
                            )
                        })
                        .unwrap_or_default();
                    return Some(chart::server_chart(
                        &points,
                        &ping_points,
                        &exch_points,
                        core.name.clone(),
                        self.chart_window,
                        now_sec,
                        cx.entity().downgrade(),
                        p,
                        cx,
                    ));
                }
                // Follows the clicked server (chart_server), else the first server that HAS an address
                // — an address-less (unknown-endpoint) server has no history ring to chart, so it must
                // fall through rather than blank the chart.
                let target = self
                    .chart_server
                    .and_then(|key| groups.iter().find(|group| group.key == key))
                    .filter(|group| group.address.is_some())
                    .or_else(|| groups.iter().find(|group| group.address.is_some()))?;
                let ip = target.address?;
                let points = b.core_chart_hist.ring(ip).cloned().unwrap_or_default();
                let ping_points = b.server_ping_hist.ring(ip).cloned().unwrap_or_default();
                let exch_points = b.server_exch_hist.ring(ip).cloned().unwrap_or_default();
                Some(chart::server_chart(
                    &points,
                    &ping_points,
                    &exch_points,
                    target.display_name.clone(),
                    self.chart_window,
                    now_sec,
                    cx.entity().downgrade(),
                    p,
                    cx,
                ))
            })
            .flatten();

        v_flex()
            .id("core-status-panel")
            .size_full()
            .relative()
            .min_h(px(0.0))
            .overflow_hidden()
            .track_focus(&self.focus)
            .font_family(design::mono())
            .text_size(design::t_body(cx))
            .bg(rgb(p.table_body))
            .child(core_bar)
            .child(div().w_full().h(px(1.0)).flex_none().bg(rgb(p.border)))
            .child(
                v_flex()
                    .w_full()
                    .flex_1()
                    .min_h(px(0.0))
                    .overflow_hidden()
                    .child(content),
            )
            .children(chart_el.map(|el| {
                v_flex()
                    .w_full()
                    .flex_none()
                    .child(div().w_full().h(px(1.0)).bg(rgb(p.border)))
                    .child(el)
            }))
            .child(div().w_full().h(px(1.0)).flex_none().bg(rgb(p.border)))
            .child(footer)
    }
}

impl CoreStatusView {
    /// Render the core multi-selector in the top bar.
    ///
    /// Clicking a known exchange header batch-toggles its currently available group cores.
    ///
    /// Args:
    ///     cores: Scoped cores in canonical display order.
    ///     cx: View context used to read exchanges and wire selection callbacks.
    ///
    /// Returns:
    ///     The top-bar row containing the fixed-trigger dropdown.
    fn core_bar(&self, cores: &OrderedCores, cx: &Context<Self>) -> impl IntoElement {
        let view = cx.entity();
        let exchange_view = view.clone();
        let exchange_names = self
            .backend
            .read(cx)
            .session
            .market_source()
            .core_exchange_names();
        let combo = crate::controls::core_combo(
            "core-status-core",
            cores,
            &exchange_names,
            &self.sel_cores,
            t!("core_status.all_cores").to_string(),
            |n| t!("core_status.cores_n", n = n).to_string(),
            170.0,
            move |id, app| {
                view.update(app, |t, c| t.toggle_core(id, c));
            },
            move |exchange_cores, app| {
                exchange_view.update(app, |t, c| {
                    t.toggle_exchange_cores(exchange_cores, c);
                });
            },
        );
        let weak_view = cx.entity().downgrade();
        let modes = MoonSegmentedControl::new("core-status-mode")
            .items([
                MoonSegmentItem::new("", t!("core_status.mode.by_ip").to_string())
                    .fit_width(cx, 54.0, 88.0)
                    .selected(self.mode == CoreStatusMode::ByIp),
                MoonSegmentItem::new("", t!("core_status.mode.flat").to_string())
                    .fit_width(cx, 54.0, 88.0)
                    .selected(self.mode == CoreStatusMode::Flat),
                MoonSegmentItem::new("", t!("core_status.mode.warnings").to_string())
                    .fit_width(cx, 54.0, 88.0)
                    .selected(self.mode == CoreStatusMode::Warnings),
            ])
            .on_click(move |index, _, _, app| {
                let Some(view) = weak_view.upgrade() else {
                    return;
                };
                let mode = match index {
                    0 => CoreStatusMode::ByIp,
                    1 => CoreStatusMode::Flat,
                    _ => CoreStatusMode::Warnings,
                };
                view.update(app, |this, cx| this.set_mode(mode, cx));
            });
        // The core selector yields first in a narrow side dock, keeping both mode actions
        // reachable without introducing a horizontal scroll host.
        h_flex()
            .w_full()
            .min_w_0()
            .overflow_hidden()
            .flex_none()
            .gap_2()
            .items_center()
            .px_2()
            .py_1()
            .child(div().flex_1().min_w_0().overflow_hidden().child(combo))
            .child(modes)
            .child(self.warn_gear(cx))
    }

    /// Render server, core, and readiness counters.
    ///
    /// Args:
    ///     groups: Current server snapshots.
    ///     total_cores: Cores in the current selector scope.
    ///     cx: View context used for palette and localization.
    ///
    /// Returns:
    ///     Compact footer shared by grouped and flat presentations.
    fn footer(
        &self,
        groups: &[ServerStatusGroup],
        total_cores: usize,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        let p = MoonPalette::active(cx);
        let online = groups
            .iter()
            .filter(|group| group.ready_count == group.cores.len() && !group.cores.is_empty())
            .count();
        h_flex()
            .w_full()
            .flex_none()
            .gap_2()
            .items_center()
            .px_2()
            .py_1()
            .text_size(design::t_body(cx))
            .text_color(rgb(p.text_muted))
            .child(t!(
                "core_status.footer",
                servers = groups.len(),
                cores = total_cores,
                online = online
            ))
    }
}
