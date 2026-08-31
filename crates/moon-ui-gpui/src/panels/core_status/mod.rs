//! Core Status panel: connection state and typed protocol-v4 resource telemetry
//! (`Event::KernelHealth`) for every core in scope.
//!
//! `CoreData::sys` holds the latest sample, while `sys_rev` invalidates the panel
//! when metric values or the decoded endpoint change.
//!
//! Like the Assets panel, it is scoped to a window group and can live in a dock
//! tab or a detached window. [`crate::persistence::table_persist`] stores separate column
//! widths and a separate remembered mode choice for `:dock` and `:win`. This module owns data and
//! lifecycle; [`server_view`] and [`table`] own the two presentations.

mod by_ip_header;
mod by_ip_widths;
mod cache;
mod chart;
mod config_popup;
mod interactions;
mod ip_cell;
mod model;
mod ordering;
mod presentation;
mod server_view;
mod startup;
mod table;
#[cfg(test)]
mod tests;
mod time_offset;
mod update_menu;
mod updates_list;
mod warnings;

pub(crate) use presentation::connection_status_text;
pub(crate) use startup::{problem_diagnostic_text, startup_diagnostic_text};

use std::collections::{HashMap, HashSet};
use std::net::IpAddr;
use std::rc::Rc;

use gpui::prelude::FluentBuilder;
use gpui::*;
use moon_ui::{
    DockArea, MoonButton, MoonButtonSize, MoonButtonVariant, MoonDataTableState, MoonInputState,
    MoonPalette, MoonSegmentItem, MoonSegmentedControl, MoonTreeState, Panel, PanelEvent,
    PanelState, h_flex, v_flex,
};

use crate::Backend;
use crate::core_order::{CoreOrder, OrderedCores};
use crate::design;
use crate::workspace::{EffectiveCoreScope, RetainedCoreScope};
use model::{CoreStatusRow, ServerKey, ServerStatusGroup};
use moon_core::session::CoreId;
use moon_core::session::core_update::CoreUpdatePhase;
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
    /// The update-history log, merged with attempts still in flight.
    Updates,
}

/// How many recent warning episodes the Warnings list shows.
const WARN_LIST_LIMIT: usize = 500;

/// Position of the dead separator cell in the mode strip: after the two LIVE views (By-IP, Flat)
/// and before the two HISTORY views (Warnings, Updates).
///
/// Named because it is load-bearing in two places that must agree — the item list and the
/// click-index match. A literal in both is how a separator quietly becomes a mode.
const MODE_DIVIDER_INDEX: usize = 2;

/// Unscaled width of that separator cell. Wide enough to read as a gap with a rule in it, narrow
/// enough not to read as a missing button.
const MODE_DIVIDER_WIDTH: f32 = 13.0;

/// How many recent update-history rows the Updates list shows, after scope filtering. Matches
/// `HISTORY_CAP` in `crates/moon-core/src/session/core_update.rs`: the backing history itself
/// never holds more than this, so the cap only ever bites when scoping filters less than the
/// whole retained log.
const UPDATE_LIST_LIMIT: usize = 2_000;

impl Default for CoreStatusMode {
    /// Return the server-by-IP presentation a panel opens on when nothing was ever remembered.
    fn default() -> Self {
        Self::ByIp
    }
}

impl CoreStatusMode {
    /// Stable machine code written to `layout.toml`.
    ///
    /// Never localized and never derived from the tab caption: the captions come from
    /// `core_status.mode.*` and change with the locale, while this is the persistence contract and
    /// must not. Kebab-case matches `WorkspaceMode::code` in `moon-core`.
    ///
    /// Returns:
    ///     The stable, non-localized persistence code for this presentation.
    const fn code(self) -> &'static str {
        match self {
            Self::ByIp => "by-ip",
            Self::Flat => "flat",
            Self::Warnings => "warnings",
            Self::Updates => "updates",
        }
    }

    /// Resolve a persisted code without letting a hand edit change what the panel does.
    ///
    /// Leading and trailing whitespace is ignored. Anything unknown — an empty value, a typo, or a
    /// code a newer build wrote — yields the first-run default rather than an error, so a single bad
    /// entry costs one remembered mode and never the window layout around it.
    ///
    /// Args:
    ///     code: Persisted machine code, potentially hand-edited.
    ///
    /// Returns:
    ///     The matching presentation, or By IP for an empty or unrecognized code.
    fn from_code(code: &str) -> Self {
        match code.trim() {
            "flat" => Self::Flat,
            "warnings" => Self::Warnings,
            "updates" => Self::Updates,
            _ => Self::default(),
        }
    }
}

/// Context-qualified storage id for the Core Status presentation choice.
///
/// A panel-level choice rather than a property of a table, so it takes its own base and never
/// shares `core-status-table`'s. The `:dock`/`:win` split is what lets a docked tab and a detached
/// window remember different modes; a detached window therefore opens on whatever `:win` last held,
/// and is deliberately NOT seeded from the docked panel it was torn off.
///
/// Args:
///     detached: Whether this panel instance is hosted in a detached window.
///
/// Returns:
///     The context-qualified persistence key for the panel's presentation mode.
fn mode_ctx_id(detached: bool) -> String {
    crate::persistence::table_persist::ctx_id("core-status-mode", detached)
}

/// Group-scoped Core Status panel for a dock tab or detached window.
pub struct CoreStatusView {
    pub(super) backend: Entity<Backend>,
    /// Window group whose cores define this panel's scope, matching the Assets panel.
    group: String,
    /// Retained Classic multi-select core filter; an empty set means every group core. Auto mode
    /// pins its effective workspace scope without using or mutating this selection.
    pub(super) sel_cores: HashSet<CoreId>,
    /// Unix ms of the last repaint; telemetry repaints at most once per second.
    last_repaint_ms: i64,
    /// `core_update_rev()` as of the last rebuild, bypassing the 1 s repaint gate on change so a
    /// phase transition never sits stale for up to a second — see the backend observer below.
    last_update_rev: u64,
    /// `core_update_history_rev()` as of the last rebuild, folded into the same bypass as
    /// `last_update_rev`: a record appended by a completing attempt moves the history but need
    /// not move any phase, so gating on the phase revision alone would leave the Updates list
    /// stale for up to a second.
    last_history_rev: u64,
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
    /// Whether the By-IP address column is hidden behind its mask.
    ///
    /// Panel-wide rather than per-server, and it starts FALSE: this view exists to show addresses,
    /// so masking is a deliberate act before sharing a screen, not the resting state. One control
    /// in the column header owns it, so a fleet of servers costs one click instead of one each.
    /// Transient — never persisted, so a fresh panel always comes up showing addresses.
    ip_masked: bool,
    /// Server whose name is being renamed inline, if any.
    editing: Option<ServerKey>,
    /// Input state backing the inline rename field while [`Self::editing`] is set.
    edit_input: Option<Entity<MoonInputState>>,
    /// Active flat-table sort as `(column key, ascending)`, or `None` for the default
    /// attention-first order.
    flat_sort: Option<(String, bool)>,
    /// Whether the exchange logos have finished decoding off-thread.
    ///
    /// The Flat view's exchange headings gate on this: drawing before the prewarm lands would make
    /// the first frame block on an SVG decode.
    exchange_logos_ready: bool,
    /// Active By IP column sort as `(field, ascending)`. Default `(Name, ascending)` reproduces the
    /// former fixed order; warnings always pin to the top regardless of the field or direction.
    group_sort: (ordering::GroupSortField, bool),
    /// Presentation the mode strip is on. Restored from and written back to `layout.toml` under
    /// [`mode_ctx_id`], so this panel reopens in the mode the user last selected.
    mode: CoreStatusMode,
    tree_state: Entity<MoonTreeState>,
    table_state: Entity<MoonDataTableState>,
    /// Column state for the Warnings list table (separate widths from the flat telemetry table).
    warn_table_state: Entity<MoonDataTableState>,
    /// Column state for the Updates list table (separate widths from every other table/tree here,
    /// the same way `warn_table_state` never folds into the Flat table's own state).
    updates_table_state: Entity<MoonDataTableState>,
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
    /// User-dragged column widths for the By IP tree, keyed by [`by_ip_widths::ByIpCol::key`].
    ///
    /// A `MoonDataTableState` used purely as a persistence-shaped BAG, never as a table: By IP is a
    /// tree, not a `MoonDataTable`. Reusing the type is what lets [`crate::persistence::table_persist`]
    /// store, restore and reset these widths with no storage code of its own — including the
    /// toolbar's existing reset button, which takes exactly this entity.
    by_ip_col_widths: Entity<MoonDataTableState>,
    /// Context-qualified persistence ID for [`Self::by_ip_col_widths`].
    by_ip_widths_id: String,
    /// Pointer x and LOGICAL column width captured when a header divider drag started.
    ///
    /// The drag cannot be computed from the live cell origin: the `flex_1` spacer in the header
    /// right-anchors every column after IP, so growing one moves its own left edge and the delta
    /// compounds every frame. Anchoring once, at the grab, makes the arithmetic independent of
    /// relayout. `None` whenever no drag is in flight.
    by_ip_drag: Option<by_ip_header::ByIpDragAnchor>,
    dock: Option<WeakEntity<DockArea>>,
    focus: FocusHandle,
}

impl crate::controls::CoreComboHost for CoreStatusView {
    /// Auto owns the effective scope and leaves the retained Classic selection untouched.
    fn core_selection_pinned(&self, cx: &App) -> bool {
        self.effective_scope(self.backend.read(cx))
            .is_workspace_owned()
    }

    /// Return the retained Classic core filter for shared picker edits.
    fn core_selection_mut(&mut self) -> &mut HashSet<CoreId> {
        &mut self.sel_cores
    }

    /// Rebuild the cached rows against the new filter and repaint.
    fn after_core_selection_change(&mut self, cx: &mut Context<Self>) {
        self.rebuild_cache(cx);
        cx.notify();
    }
}

impl CoreStatusView {
    /// Construct a group-scoped Core Status panel and its table/tree state.
    ///
    /// Args:
    ///     backend: Shared terminal backend.
    ///     group: Window group that defines the core scope.
    ///     detached: Whether widths and presentation mode use the detached-window persistence keys.
    ///     _window: Host window reserved for panel construction symmetry.
    ///     cx: View context used for observers and child entities.
    ///
    /// Returns:
    ///     A panel restored to its saved presentation, or By IP when no usable mode was stored.
    fn new(
        backend: Entity<Backend>,
        group: String,
        detached: bool,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        // This fires on every backend notify (event-driven, ≤4 Hz — not a timer/poll), but the
        // rebuild is gated to once per second. Detection AND chart-history recording run in the
        // backend engine (backend-always), so the panel only rebuilds its display from that state.
        cx.observe(&backend, |this, backend, cx| {
            let now = moon_chart::paint::now_unix_ms() as i64;
            // OR alongside the 1 s time gate, never a replacement for it: a signature-driven
            // bypass here is what keeps a phase transition from sitting stale for up to a second,
            // while the time gate still governs every OTHER backend notify -- up to 4 Hz over a
            // 200-core fleet, which is why this must not become the panel's only gate.
            let update_rev = backend.read(cx).session.core_update_rev();
            let history_rev = backend.read(cx).session.core_update_history_rev();
            let update_changed =
                update_rev != this.last_update_rev || history_rev != this.last_history_rev;
            if !update_changed && now - this.last_repaint_ms < 1000 {
                return;
            }
            this.last_repaint_ms = now;
            this.last_update_rev = update_rev;
            this.last_history_rev = history_rev;
            this.rebuild_cache(cx);
            cx.notify();
        })
        .detach();

        let display_time_revision = backend.read(cx).display_time_revision.clone();
        cx.observe(&display_time_revision, |this, _revision, cx| {
            this.rebuild_cache(cx);
            cx.notify();
        })
        .detach();
        let workspace_revision = backend.read(cx).workspace_revision();
        cx.observe(&workspace_revision, |this, _revision, cx| {
            this.rebuild_cache(cx);
            cx.notify();
        })
        .detach();
        let core_filter_revision = backend.read(cx).core_filter_revision();
        cx.observe(&core_filter_revision, |this, _revision, cx| {
            this.adopt_broadcast_core_filter(cx)
        })
        .detach();

        // Off-thread, like every other exchange-logo call site: `prewarm` decodes the shipped SVGs
        // and would block the first frame if it ran on the render thread.
        cx.spawn(async move |view, cx| {
            cx.background_spawn(async { crate::media::exchange_logos::prewarm() })
                .await;
            cx.update(|cx| {
                let _ = view.update(cx, |this, cx| {
                    this.exchange_logos_ready = true;
                    cx.notify();
                });
            });
        })
        .detach();

        let widths_id = crate::persistence::table_persist::ctx_id("core-status-table", detached);
        let by_ip_sort_id =
            crate::persistence::table_persist::ctx_id("core-status-by-ip", detached);
        let flat_sort = ordering::restore_flat_sort(crate::persistence::table_persist::saved_sort(
            backend.read(cx),
            &widths_id,
        ));
        let group_sort = ordering::restore_group_sort(
            crate::persistence::table_persist::saved_sort(backend.read(cx), &by_ip_sort_id),
        );
        // Restore only: writing the resolved code back here would insert an entry for every context
        // on first launch and arm a layout flush merely by opening the panel.
        let mode = crate::persistence::table_persist::core_status_mode(
            backend.read(cx),
            &mode_ctx_id(detached),
        )
        .map_or_else(CoreStatusMode::default, CoreStatusMode::from_code);
        let saved_widths = crate::persistence::table_persist::saved(backend.read(cx), &widths_id);
        let table_state = cx.new(|_| {
            let mut s = MoonDataTableState::new();
            s.column_widths = saved_widths;
            if let Some((key, ascending)) = &flat_sort {
                s.set_sort(key.clone(), *ascending);
            }
            s
        });
        cx.observe(&table_state, |this, state, cx| {
            crate::persistence::table_persist::persist(&this.backend, &this.widths_id, &state, cx);
        })
        .detach();
        // The By IP width bag. Its own `ctx_id` base, so a docked tab and a detached window keep
        // separate By-IP widths exactly as they already keep separate flat-table widths, and neither
        // can collide with `core-status-table`.
        let by_ip_widths_id =
            crate::persistence::table_persist::ctx_id("core-status-by-ip-widths", detached);
        let saved_by_ip =
            crate::persistence::table_persist::saved(backend.read(cx), &by_ip_widths_id);
        let by_ip_col_widths = cx.new(|_| {
            let mut s = MoonDataTableState::new();
            s.column_widths = saved_by_ip;
            s
        });
        cx.observe(&by_ip_col_widths, |this, state, cx| {
            crate::persistence::table_persist::persist(
                &this.backend,
                &this.by_ip_widths_id,
                &state,
                cx,
            );
            // Unlike `table_state`, NOTHING else observes this bag: a `MoonDataTable` observes its
            // own state, but the By IP header and rows read these widths during THIS view's render.
            // Without the notify the toolbar reset appears to do nothing until some unrelated
            // repaint happens to arrive.
            cx.notify();
        })
        .detach();
        let warn_table_state = cx.new(|_| MoonDataTableState::new());
        let updates_table_state = cx.new(|_| MoonDataTableState::new());
        let tree_state = cx.new(|cx| MoonTreeState::new(cx));
        let focus = cx.focus_handle();

        let mut this = Self {
            backend,
            group,
            sel_cores: HashSet::new(),
            last_repaint_ms: 0,
            last_update_rev: 0,
            last_history_rev: 0,
            has_warn: false,
            detached,
            chart_window: ChartWindow::default(),
            chart_server: None,
            chart_core: None,
            cached_rows: Rc::new(Vec::new()),
            cached_groups: Rc::new(Vec::new()),
            ip_masked: false,
            editing: None,
            edit_input: None,
            flat_sort,
            exchange_logos_ready: false,
            group_sort,
            mode,
            tree_state,
            table_state,
            warn_table_state,
            updates_table_state,
            warn_cfg_open: false,
            by_ip_width: 0.0,
            widths_id,
            by_ip_col_widths,
            by_ip_widths_id,
            by_ip_drag: None,
            dock: None,
            focus,
        };
        // A panel created while a filter is on air joins it, so a detached or restored tab is not
        // the one surface still showing every core.
        this.adopt_broadcast_core_filter(cx);
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

    /// Resolve the effective core scope without overwriting the retained Classic filter.
    ///
    /// Args:
    ///     b: Backend snapshot containing workspace authority and live group membership.
    ///
    /// Returns:
    ///     Effective core scope used by status data and controls.
    pub(super) fn effective_scope(&self, b: &Backend) -> EffectiveCoreScope {
        let retained: Vec<CoreId> = self.sel_cores.iter().copied().collect();
        let retained = if retained.is_empty() {
            RetainedCoreScope::All
        } else {
            RetainedCoreScope::Explicit(&retained)
        };
        b.effective_workspace_scope(&self.group, retained)
    }

    /// Return canonically ordered core/name pairs in the current effective scope.
    ///
    /// Args:
    ///     b: Backend snapshot containing canonical configured core order.
    ///
    /// Returns:
    ///     Effective core/name pairs for queries and caches.
    pub(super) fn query_cores(&self, b: &Backend) -> Vec<(CoreId, String)> {
        let scope = self.effective_scope(b);
        self.scope_cores(b)
            .into_iter()
            .filter(|(core, _)| scope.contains(*core))
            .collect()
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
        // ONE button, pointed at whatever grid the panel is currently showing: a user sees one set
        // of columns and expects one reset. By IP has its own width bag because it is a tree, not
        // the flat table.
        //
        // The DETACHED window does not come through here. `panels/registry.rs` resolves
        // `table_state()` ONCE at window construction into `DetachedContent.widths_reset`, so its
        // header button keeps resetting the flat table whatever the mode; there, By-IP reset is
        // reachable by double-clicking a divider (Shift+double-click for all of them). Fixing that
        // means giving `widths_reset` a closure instead of an entity, which is a change to two
        // files outside this panel.
        let state = match self.mode {
            CoreStatusMode::ByIp => &self.by_ip_col_widths,
            CoreStatusMode::Flat => &self.table_state,
            // Warnings is its OWN table with its own widths (`warnings_table` is handed
            // `warn_table_state`), so it must not be folded in with Flat: doing that resets the
            // hidden grid and leaves the visible one untouched.
            CoreStatusMode::Warnings => &self.warn_table_state,
            // Same reasoning as Warnings: Updates is its own table with its own widths.
            CoreStatusMode::Updates => &self.updates_table_state,
        };
        Some(vec![crate::persistence::table_persist::reset_button(
            "core-status-reset-widths",
            state,
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
        crate::diag::bump(&crate::diag::CORE_STATUS_RENDER);
        let _render_us = crate::diag::scope(&crate::diag::CORE_STATUS_RENDER_US);
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
                self.ip_masked,
                self.editing,
                self.edit_input.clone(),
                self.chart_server,
                self.chart_core,
                self.group_sort,
                self.by_ip_width,
                // Row insets are `rems`, so the By-IP width budget needs the window's rem size —
                // MoonUI's Root sets it from the theme font size, which the Font slider moves.
                f32::from(window.rem_size()),
                // The user's dragged widths, BORROWED: the callee resolves them into `Copy`
                // geometries synchronously and nothing in the render tree holds the map, so a
                // per-frame clone of it would buy nothing on a path that repaints on every hover.
                &self.by_ip_col_widths.read(cx).column_widths,
                &self.tree_state,
                // Handed down rather than read off the view inside the callee: we are already
                // inside this view's own update here, and `cx.entity().read(cx)` there is a
                // process-killing panic. We hold the handle, so we pass it.
                &self.backend,
                // `&Window` is enough: `Window::listener_for` takes `&self`, so the header's
                // drag-move listener needs no mutable borrow.
                window,
                cx,
            ),
            CoreStatusMode::Flat => {
                let server_names: HashMap<ServerKey, String> = groups
                    .iter()
                    .map(|group| (group.key, group.display_name.clone()))
                    .collect();
                let (flat_rows, flat_lines) = self.flat_view(cx);
                table::core_status_table(
                    "core-status-table",
                    flat_rows,
                    flat_lines,
                    Rc::new(server_names),
                    self.exchange_logos_ready,
                    self.flat_sort.is_some(),
                    &self.table_state,
                    // Same reason as the By-IP arm above: the callee must not read this view.
                    &self.backend,
                    cx,
                )
                .into_any_element()
            }
            CoreStatusMode::Warnings => {
                let b = self.backend.read(cx);
                let scope = self.effective_scope(b);
                let core_ids: HashSet<CoreId> = scope.ids().iter().copied().collect();
                let server_addresses: HashSet<IpAddr> =
                    groups.iter().filter_map(|group| group.address).collect();
                let episodes =
                    b.warn_episodes_recent_for_scope(&core_ids, &server_addresses, WARN_LIST_LIMIT);
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
                    crate::chrome::clock::resolved_header_clock_zone(b.header_clock_zone()),
                    cx,
                )
                .into_any_element()
            }
            CoreStatusMode::Updates => {
                let b = self.backend.read(cx);
                let scope = self.effective_scope(b);
                let core_ids: HashSet<CoreId> = scope.ids().iter().copied().collect();
                let now_ms = moon_core::util::now_unix_ms_i64();
                // Live rows come straight off `rows`: `CoreStatusRow.update` is already this
                // core's current phase, scoped by `query_cores` the same way every other mode
                // here is scoped. `Done`/`None` are excluded -- a finished attempt is already a
                // history record, and a core the queue has never touched has nothing to show.
                let live_rows: Vec<CoreStatusRow> = rows
                    .iter()
                    .filter(|row| {
                        matches!(
                            row.update,
                            Some(CoreUpdatePhase::Queued { .. })
                                | Some(CoreUpdatePhase::Sent { .. })
                                | Some(CoreUpdatePhase::Waiting { .. })
                                | Some(CoreUpdatePhase::Verifying { .. })
                        )
                    })
                    .cloned()
                    .collect();
                // Admit a record when EITHER: its core is still configured and in `core_ids` --
                // this keeps the sibling leak closed, so a still-configured core the user did not
                // select never shows through on a shared IP -- OR the core has been removed from
                // configuration entirely (it can no longer appear in `core_ids` at all, see
                // `EffectiveCoreScope::ids`) and its lane ran on an address this panel's scope
                // still covers. Without the second half, `reconcile_vanished_updates`'s
                // `Failed(Gone)` row -- and every other history row for that core -- becomes
                // unreachable the instant the core leaves the config, even though
                // `CoreUpdateRecord::core_name` was snapshotted at enqueue precisely so the row
                // could keep naming a core that no longer exists.
                let configured_core_ids: HashSet<CoreId> =
                    b.config.servers.iter().map(|server| server.id).collect();
                // The SCOPE's server addresses, not the live rows' (`update_ips`, below, is
                // downstream of this filter and only ever covers cores already admitted). Reuses
                // the same `groups` aggregate the Warnings branch above already derives its own
                // server-address set from.
                let scope_server_addrs: HashSet<IpAddr> =
                    groups.iter().filter_map(|group| group.address).collect();
                let history: Vec<moon_core::session::core_update::CoreUpdateRecord> = b
                    .session
                    .core_update_history()
                    .iter()
                    .rev()
                    .filter(|record| {
                        core_ids.contains(&record.core)
                            || (!configured_core_ids.contains(&record.core)
                                && scope_server_addrs.contains(&record.lane_addr))
                    })
                    .take(UPDATE_LIST_LIMIT)
                    .cloned()
                    .collect();
                let update_ips: HashSet<IpAddr> = live_rows
                    .iter()
                    .filter_map(|row| row.endpoint.map(|ep| ep.address))
                    .chain(history.iter().map(|record| record.lane_addr))
                    .collect();
                let server_names: HashMap<IpAddr, String> = update_ips
                    .into_iter()
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
                updates_list::updates_table(
                    "core-status-updates",
                    Rc::new(live_rows),
                    Rc::new(history),
                    Rc::new(server_names),
                    &self.updates_table_state,
                    crate::chrome::clock::resolved_header_clock_zone(b.header_clock_zone()),
                    now_ms,
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
    /// Render the effective core scope in the top bar.
    ///
    /// Classic mode exposes the retained multi-selector and exchange batch toggles. Auto mode pins
    /// the workspace label and disables the selector without changing retained Classic state.
    ///
    /// Args:
    ///     cores: Group cores in canonical display order.
    ///     cx: View context used to read exchanges and wire selection callbacks.
    ///
    /// Returns:
    ///     The top-bar row containing an interactive Classic selector or pinned Auto indicator.
    fn core_bar(&self, cores: &OrderedCores, cx: &Context<Self>) -> impl IntoElement {
        let scope = self.effective_scope(self.backend.read(cx));
        let workspace_owned = scope.is_workspace_owned();
        let effective_selection: HashSet<CoreId> = scope.ids().iter().copied().collect();
        let pinned_label = match scope.label() {
            crate::workspace::EffectiveScopeLabel::Overview => {
                Some(t!("workspace.overview").to_string())
            }
            crate::workspace::EffectiveScopeLabel::Core(core) => cores
                .iter()
                .find(|(id, _)| *id == core)
                .map(|(_, name)| name.clone()),
            crate::workspace::EffectiveScopeLabel::All
            | crate::workspace::EffectiveScopeLabel::Selection(_) => None,
        };
        let view = cx.entity();
        let exchange_view = view.clone();
        let venues = self.backend.read(cx).session.core_venues();
        let extras = crate::controls::core_combo_extras(!workspace_owned, &view, &self.backend, cx);
        let combo = crate::controls::core_combo(
            "core-status-core",
            cores,
            &venues,
            if workspace_owned {
                &effective_selection
            } else {
                &self.sel_cores
            },
            crate::controls::CoreAllRowMode::ImplicitOrComplete,
            t!("core_status.all_cores").to_string(),
            |n| t!("core_status.cores_n", n = n).to_string(),
            170.0,
            extras,
            move |id, app| {
                view.update(app, |t, c| t.toggle_core(id, c));
            },
            move |exchange_cores, app| {
                exchange_view.update(app, |t, c| {
                    t.toggle_exchange_cores(exchange_cores, c);
                });
            },
        )
        .disabled(workspace_owned);
        let combo = if let Some(label) = pinned_label {
            combo.label(label)
        } else {
            combo
        };
        let weak_view = cx.entity().downgrade();
        let mode_palette = MoonPalette::active(cx);
        // The four modes are two different KINDS of surface: By-IP and Flat are live views of the
        // fleet as it stands, while Warnings and Updates are history — records of what already
        // happened. `MODE_DIVIDER_INDEX` is a dead cell that draws the boundary between them.
        //
        // One control rather than two, deliberately: the selection is one value, so two controls
        // would each need to render "nothing selected" while the other holds it, and any drift
        // between them shows the user two highlighted tabs. A replaced cell "preserves its
        // resolved width and selected underline but exposes no segment click, scroll, hover,
        // cursor, or tooltip behavior" (MoonUI `segment.rs::replace_item`), which is exactly a
        // separator: it can never be selected and can never be clicked, so the index below it
        // simply never arrives.
        let modes = MoonSegmentedControl::new("core-status-mode")
            .items([
                MoonSegmentItem::new("", t!("core_status.mode.by_ip").to_string())
                    .fit_width(cx, 54.0, 88.0)
                    .selected(self.mode == CoreStatusMode::ByIp),
                MoonSegmentItem::new("", t!("core_status.mode.flat").to_string())
                    .fit_width(cx, 54.0, 88.0)
                    .selected(self.mode == CoreStatusMode::Flat),
                // Narrow by explicit width, not `fit_width`: a separator sized like a label cell
                // would open a 54-88px hole in the strip, which reads as a missing button rather
                // than a boundary.
                MoonSegmentItem::new("", String::new())
                    .width(MODE_DIVIDER_WIDTH)
                    .disabled(true),
                MoonSegmentItem::new("", t!("core_status.mode.warnings").to_string())
                    .fit_width(cx, 54.0, 88.0)
                    .selected(self.mode == CoreStatusMode::Warnings),
                MoonSegmentItem::new("", t!("core_status.mode.updates").to_string())
                    .fit_width(cx, 54.0, 88.0)
                    .selected(self.mode == CoreStatusMode::Updates),
            ])
            .replace_item(
                MODE_DIVIDER_INDEX,
                h_flex()
                    .w_full()
                    .h_full()
                    .justify_center()
                    .items_center()
                    .child(design::chrome_divider(cx, mode_palette)),
            )
            .on_click(move |index, _, _, app| {
                let Some(view) = weak_view.upgrade() else {
                    return;
                };
                let mode = match index {
                    0 => CoreStatusMode::ByIp,
                    1 => CoreStatusMode::Flat,
                    // 2 is the separator and never reports a click.
                    3 => CoreStatusMode::Warnings,
                    _ => CoreStatusMode::Updates,
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
        // Fleets run 3 to 200 cores, so the campaign totals must be readable WITHOUT expanding a
        // single server -- drawn only while a campaign is actually on, never a permanent "0
        // updating" fixture. `done` and `lanes_stalled` are deliberately excluded: this is the
        // fleet's ATTENTION state, and a finished or held-but-not-failed row needs none.
        let summary = self.backend.read(cx).session.core_update_summary();
        let update_parts: Vec<String> = [
            (summary.updating, "core_update.summary.updating"),
            (summary.queued, "core_update.summary.queued"),
            (summary.failed, "core_update.summary.failed"),
        ]
        .into_iter()
        .filter(|(n, _)| *n > 0)
        .map(|(n, key)| format!("{n} {}", t!(key)))
        .collect();
        // Fleet-relative, not release-relative -- see `session::core_update`'s own doc comment.
        // Empty here means every core already agrees with the fleet's newest build, never that no
        // release exists, so the button explains that in its tooltip instead of just going gray.
        let behind_empty = self.backend.read(cx).session.cores_behind().is_empty();
        let update_all_view = cx.entity();
        let update_behind_view = update_all_view.clone();
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
            .child(div().flex_1())
            // A campaign's summary plus both bulk buttons, as ONE group that may shrink and wrap
            // onto its own row rather than clip: up to three localized counters joined with the
            // fleet summary and two localized buttons on a single non-shrinking row overruns a
            // narrow dock. `min_w_0` is what makes the wrap reachable -- only a group allowed to
            // shrink below its content wraps rather than overflowing -- following the same idiom
            // `panels/report/controls.rs::selection_actions` already uses for its own count-plus-
            // commands group.
            .child(
                h_flex()
                    .min_w_0()
                    .flex_wrap()
                    .justify_end()
                    .items_center()
                    .gap_2()
                    .when(!update_parts.is_empty(), |row| {
                        row.child(
                            div()
                                .flex_none()
                                .text_color(rgb(p.amber))
                                .child(update_parts.join(" - ")),
                        )
                    })
                    .child(
                        MoonButton::new("core-status-update-all")
                            .label(t!("core_update.fleet.all").to_string())
                            .size(MoonButtonSize::Micro)
                            .variant(MoonButtonVariant::Panel)
                            .on_click(move |_, window, cx| {
                                update_all_view.update(cx, |this, cx| {
                                    this.confirm_fleet_update(false, window, cx);
                                });
                            })
                            .render(),
                    )
                    .child({
                        let behind_button = MoonButton::new("core-status-update-behind")
                            .label(t!("core_update.fleet.behind").to_string())
                            .size(MoonButtonSize::Micro)
                            .variant(MoonButtonVariant::Panel)
                            .disabled(behind_empty)
                            .on_click(move |_, window, cx| {
                                update_behind_view.update(cx, |this, cx| {
                                    this.confirm_fleet_update(true, window, cx);
                                });
                            });
                        if behind_empty {
                            behind_button.tooltip(t!("core_update.fleet.behind_none").to_string())
                        } else {
                            behind_button
                        }
                        .render()
                    }),
            )
    }
}
