//! Core Status panel: connection state and typed protocol-v4 resource telemetry
//! (`Event::KernelHealth`) for every core in scope.
//!
//! `CoreData::sys` holds the latest sample, and `sys_rev` marks when metric values or the decoded
//! endpoint changed. The panel itself rebuilds on its own 1 s gate over backend notifications
//! rather than by polling that counter; the counters gate the NOTIFY, upstream in the session.
//!
//! Like the Assets panel, it is scoped to a window group and can live in a dock
//! tab or a detached window. [`crate::persistence::table_persist`] stores separate column
//! widths, dragged column order, and a separate remembered mode choice for `:dock` and `:win`.
//! The By IP tree has widths only: its columns are a fixed sequence, not a `MoonDataTable` order.
//! This module owns data and lifecycle; [`server_view`], [`table`], [`problems`], [`warnings`]
//! and [`updates_list`] own the five presentations.

mod by_ip_header;
mod by_ip_widths;
mod cache;
mod chart;
mod config_popup;
mod core_bar;
mod footer;
mod interactions;
mod ip_cell;
mod mode;
mod model;
mod ordering;
mod panel;
mod presentation;
mod problems;
mod render;
mod scope;
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
use std::rc::Rc;

use gpui::prelude::FluentBuilder;
use gpui::*;
use moon_ui::{
    DockArea, MoonDataTableState, MoonInputState, MoonPalette, MoonTreeState, h_flex, v_flex,
};

use crate::Backend;
use crate::controls::row_selection::RowSelection;
use crate::design;
use crate::workspace::scope_marker::{self, ScopeMarker};
use model::{CoreStatusRow, ServerKey, ServerStatusGroup};
use moon_core::session::CoreId;
use rust_i18n::t;

use mode::{ChartWindow, CoreStatusMode, mode_ctx_id, ordered_log_state};

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
    /// Revision of the row/group snapshot, advanced only by cache rebuilds.
    rows_generation: u64,
    /// Sorted rows for the current revision and sort; headings are always rebuilt.
    flat_cache: std::cell::RefCell<cache::FlatViewCache>,
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
    /// Active Problems sort as `(column key, ascending)`, or `None` for newest confirmation
    /// first. `None` is not written on open. A header click persists through the same
    /// `table_sorts` map Flat uses, under this table's own context id.
    problems_sort: Option<(String, bool)>,
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
    /// Last order observed for each table on this panel, keyed by its context id.
    ///
    /// A resize or a click notifies the same observer as a drag. Docked core-status tables in
    /// different groups share one `:dock` id, so writing on those other notifications would
    /// replace a drag another open panel just saved.
    order_seen: HashMap<String, Vec<SharedString>>,
    /// Column state for the Warnings list table (separate widths from the flat telemetry table).
    warn_table_state: Entity<MoonDataTableState>,
    /// Column state for the Updates list table (separate widths from every other table/tree here,
    /// the same way `warn_table_state` never folds into the Flat table's own state).
    updates_table_state: Entity<MoonDataTableState>,
    /// Column state for the Problems list table, independent for the same reason the two above
    /// are: it is its own grid, and folding it in would reset a hidden one.
    problems_table_state: Entity<MoonDataTableState>,
    /// Whether the alert-axis toggle popover (the gear beside the mode control) is open.
    warn_cfg_open: bool,
    /// Findings in scope this operator has not looked at. Drives the dock-tab badge and the count
    /// on the Problems tab.
    ///
    /// Cached rather than recomputed in `title_suffix`: the dock asks that on a per-frame path.
    unseen_problems: usize,
    /// Signature of the rows the read-mark last consumed, so a repeat frame does no work.
    ///
    /// NOT a "nothing unseen" test, which is what it started as and which was wrong: marking also
    /// PRUNES kinds a core has stopped reporting, and a zero count skipped that prune — so a
    /// finding that went away and came back stayed silent forever, the one property the identity
    /// set exists to provide. A signature is the honest early-out: it skips only frames where what
    /// is on screen has not moved.
    problems_mark_sig: u64,
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
    /// Scope marker for the footer and empty-state text, rebuilt alongside `cached_rows` /
    /// `cached_groups`.
    ///
    /// Unlike Assets, this panel has no group-less variant to fall back on — every instance is
    /// scoped to a window group, so this is never `Option`.
    cached_scope_marker: ScopeMarker,
    /// Which CORE ROWS the user has selected, in either presentation.
    ///
    /// NOT [`Self::sel_cores`], which is the retained Classic core FILTER deciding what this
    /// panel SHOWS. This is the transient on-screen selection a bulk action addresses, and it is
    /// keyed by [`CoreId`] rather than by a line index because the flat table draws exchange
    /// headings as lines of their own and both presentations re-sort under the user.
    /// Pruned against the visible rows on every cache rebuild (`cache::rebuild_cache`).
    core_selection: RowSelection<CoreId>,
    /// Core the three Problems actions are narrowed to, or `None` for the panel's whole scope.
    ///
    /// Written by the row click, which resolves its index against the list it was drawn from while
    /// that list is still the one on screen. Held as a CORE for the same reason: the finding list
    /// is rebuilt from live core data on every repaint, so a stored index outlives the row it
    /// named — one finding appearing or clearing above it re-points it at a different core, and
    /// the reset it narrows cannot be undone.
    problems_picked: Option<CoreId>,
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
        let saved_order = crate::persistence::table_persist::restored_order(
            backend.read(cx),
            &widths_id,
            table::order_keys(),
        );
        let flat_order_seen = saved_order.clone();
        let table_state = cx.new(|_| {
            let mut s = MoonDataTableState::new();
            s.column_widths = saved_widths;
            s.column_order = saved_order;
            if let Some((key, ascending)) = &flat_sort {
                s.set_sort(key.clone(), *ascending);
            }
            s
        });
        cx.observe(&table_state, |this, state, cx| {
            crate::persistence::table_persist::persist(&this.backend, &this.widths_id, &state, cx);
            let id = this.widths_id.clone();
            crate::persistence::table_persist::persist_order(
                &this.backend,
                &id,
                &state,
                this.order_seen.entry(id.clone()).or_default(),
                cx,
            );
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
        let warn_table_state = ordered_log_state(
            cx,
            &backend,
            "core-status-warnings",
            detached,
            warnings::ORDER_KEYS,
            None,
        );
        let problems_sort_id =
            crate::persistence::table_persist::ctx_id("core-status-problems", detached);
        let problems_sort = problems::restore_problems_sort(
            crate::persistence::table_persist::saved_sort(backend.read(cx), &problems_sort_id),
        );
        // The arrow shows the saved choice, or newest-confirmation-first when nothing was saved.
        // The default is not written here: opening the panel must not arm a layout flush.
        let problems_arrow = problems::shown_sort(problems_sort.as_ref());
        let problems_table_state = ordered_log_state(
            cx,
            &backend,
            "core-status-problems",
            detached,
            problems::ORDER_KEYS,
            Some((&problems_arrow.0, problems_arrow.1)),
        );
        let updates_table_state = ordered_log_state(
            cx,
            &backend,
            "core-status-updates",
            detached,
            updates_list::ORDER_KEYS,
            None,
        );
        let mut order_seen = HashMap::new();
        order_seen.insert(widths_id.clone(), flat_order_seen);
        order_seen.insert(
            crate::persistence::table_persist::ctx_id("core-status-warnings", detached),
            warn_table_state.read(cx).column_order.clone(),
        );
        order_seen.insert(
            crate::persistence::table_persist::ctx_id("core-status-problems", detached),
            problems_table_state.read(cx).column_order.clone(),
        );
        order_seen.insert(
            crate::persistence::table_persist::ctx_id("core-status-updates", detached),
            updates_table_state.read(cx).column_order.clone(),
        );
        let tree_state = cx.new(|cx| MoonTreeState::new(cx));
        let focus = cx.focus_handle();

        let mut this = Self {
            backend,
            group,
            sel_cores: HashSet::new(),
            core_selection: RowSelection::default(),
            problems_picked: None,
            last_repaint_ms: 0,
            last_update_rev: 0,
            last_history_rev: 0,
            has_warn: false,
            detached,
            chart_window: ChartWindow::default(),
            chart_server: None,
            chart_core: None,
            cached_rows: Rc::new(Vec::new()),
            rows_generation: 0,
            flat_cache: std::cell::RefCell::new(cache::FlatViewCache::default()),
            cached_groups: Rc::new(Vec::new()),
            ip_masked: false,
            editing: None,
            edit_input: None,
            flat_sort,
            problems_sort,
            exchange_logos_ready: false,
            group_sort,
            mode,
            tree_state,
            table_state,
            order_seen,
            warn_table_state,
            problems_table_state,
            updates_table_state,
            warn_cfg_open: false,
            unseen_problems: 0,
            problems_mark_sig: 0,
            by_ip_width: 0.0,
            widths_id,
            by_ip_col_widths,
            by_ip_widths_id,
            by_ip_drag: None,
            // Placeholder: no preset, nothing configured — reads as "nothing hidden" until the
            // `rebuild_cache` call below fills it in from the real membership boundary.
            cached_scope_marker: ScopeMarker::new(None, 0, 0),
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
}
