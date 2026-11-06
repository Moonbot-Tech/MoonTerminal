//! Core Status panel: connection state and typed protocol-v4 resource telemetry
//! (`Event::KernelHealth`) for every core in scope.
//!
//! `CoreData::sys` holds the latest sample, and `sys_rev` marks when metric values or the decoded
//! endpoint changed. The warning engine publishes its completed 1 s samples through a dedicated
//! channel, so history and elapsed status time progress even without unrelated backend traffic.
//!
//! Like the Assets panel, it is scoped to a window group and can live in a dock
//! tab or a detached window. [`crate::persistence::table_persist`] stores separate column
//! widths, dragged column order, and a separate remembered mode choice for `:dock` and `:win`.
//! Servers is a card grid. Flat, Problems, Warnings and Updates are data tables.
//! This module owns data and lifecycle; [`servers_view`], [`table`], [`problems`], [`warnings`]
//! and [`updates_pane`] own the five presentations.

mod cache;
mod chart;
mod columns_fit;
mod config_popup;
mod core_bar;
mod frame_cache;
mod health;
mod health_strip;
mod interactions;
mod ip_cell;
#[cfg(test)]
mod layout_fixture;
mod mode;
mod model;
#[allow(dead_code)] // The core-commands UI integration batch calls open_updates.
mod open;
mod ordering;
mod panel;
mod presentation;
mod problems;
mod render;
mod same_line;
mod scope;
mod servers_view;
mod startup;
mod table;
#[cfg(test)]
mod tests;
mod time_offset;
mod uniform;
mod update_menu;
mod updates_list;
mod updates_pane;
mod version_mark;
mod warnings;

#[allow(unused_imports)] // Re-exported for the core-commands UI integration batch.
pub(crate) use open::open_updates;
pub(crate) use presentation::connection_status_text;
pub(crate) use startup::{problem_diagnostic_text, startup_diagnostic_text};

use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use gpui::prelude::FluentBuilder;
use gpui::*;
use moon_ui::{
    DockArea, MoonDataTableState, MoonInputEvent, MoonInputState, MoonPalette, h_flex, v_flex,
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
    /// `core_update_rev()` last consumed by the immediate update-transition observer.
    last_update_rev: u64,
    /// `core_update_history_rev()` last consumed alongside the phase revision: a completed
    /// attempt can append history without changing any phase.
    last_history_rev: u64,
    /// Defers sampled table snapshots while hidden, preserving one activation catch-up.
    sample_gate: crate::panels::RenderGate,
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
    /// Episode start times, Unix milliseconds, from the scoped warnings list.
    ///
    /// The health strip counts those inside 24 hours. An episode that is still open but started
    /// earlier stays in the list and does not count. The list is capped at `WARN_LIST_LIMIT`.
    /// Rebuilt only in [`Self::rebuild_cache`].
    cached_alarm_starts: Rc<Vec<i64>>,
    /// The Warnings list itself, with resolved names. Rebuilt from live state and the last completed
    /// background read, never from SQLite during `render`.
    alarms: warnings::AlarmsList,
    /// Persisted warning snapshots keyed by store revision and effective scope.
    warn_reads: cache::WarnReadCache,
    /// Current persisted read; replacement cancels the old foreground wait and sequence-gates its result.
    warn_read_task: Option<Task<()>>,
    /// Row scroll of the Warnings list, shared by the table and its always-visible scrollbar.
    warn_scroll: moon_ui::MoonVirtualListScrollHandle,
    /// Revision of the row/group snapshot, advanced only by cache rebuilds.
    rows_generation: u64,
    /// Sorted rows for the current revision and sort; headings are always rebuilt.
    flat_cache: std::cell::RefCell<cache::FlatViewCache>,
    /// Every other projection `render` draws, memoized per snapshot generation and view state:
    /// GPUI re-renders this whole view on each row hover and scroll tick of any list inside it.
    frame: std::cell::RefCell<frame_cache::FrameCache>,
    /// Whether server addresses are hidden behind the toolbar mask.
    ///
    /// Panel-wide rather than per-server, and it starts FALSE: this view exists to show addresses,
    /// so masking is a deliberate act before sharing a screen, not the resting state. The toolbar
    /// IP button owns it, so a fleet of servers costs one click instead of one each.
    /// Transient — never persisted, so a fresh panel always comes up showing addresses.
    ip_masked: bool,
    /// Health chip narrowing the Flat rows. Transient. A mode change clears it, including a
    /// re-select of the mode that is already showing.
    health_filter: Option<health::HealthFilter>,
    /// Toolbar search field. Its change event writes [`Self::search_query`].
    search_input: Entity<MoonInputState>,
    /// Trimmed, lowercased toolbar search. Empty matches every core.
    search_query: String,
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
    /// Presentation the mode strip is on. Restored from and written back to `layout.toml` under
    /// [`mode_ctx_id`], so this panel reopens in the mode the user last selected.
    mode: CoreStatusMode,
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
    /// Last measured width of the Flat table, in pixels. `0` until the first frame measures it.
    ///
    /// `0` means "not measured yet". The column drop treats that as level 0, so the first frame
    /// does not flash a stripped table. [`render`] writes it from a measuring canvas and only
    /// when it moves by more than half a pixel.
    flat_measured_w: f32,
    /// Last measured Servers grid width. `0` means not measured yet, and cards keep the design minimum.
    servers_measured_w: f32,
    /// Last measured toolbar width. `0` means not measured yet, and the mode strip keeps full labels.
    toolbar_measured_w: f32,
    /// Context-qualified column-width persistence ID (`core-status-table:dock` or `:win`).
    widths_id: String,
    /// Scope marker for the health strip and empty-state text, rebuilt alongside `cached_rows` /
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
    ///     window: Host window used to build the toolbar search field.
    ///     cx: View context used for observers and child entities.
    ///
    /// Returns:
    ///     A panel restored to its saved presentation, or Flat when no usable mode was stored.
    fn new(
        backend: Entity<Backend>,
        group: String,
        detached: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let core_status_revision = backend.read(cx).core_status_revision.clone();
        cx.observe(&core_status_revision, |this, _, cx| {
            this.refresh_sample(cx);
        })
        .detach();

        // Update transitions remain immediate. Other status, history, and age changes are read
        // from the dedicated sample above, independent of general backend notifications.
        cx.observe(&backend, |this, backend, cx| {
            this.take_updates_request(cx);
            let update_rev = backend.read(cx).session.core_update_rev();
            let history_rev = backend.read(cx).session.core_update_history_rev();
            let update_changed =
                update_rev != this.last_update_rev || history_rev != this.last_history_rev;
            if !update_changed {
                return;
            }
            this.last_update_rev = update_rev;
            this.last_history_rev = history_rev;
            this.refresh_sample(cx);
        })
        .detach();

        let display_time_revision = backend.read(cx).display_time_revision.clone();
        cx.observe(&display_time_revision, |this, _revision, cx| {
            this.refresh_sample(cx);
        })
        .detach();
        let workspace_revision = backend.read(cx).workspace_revision();
        cx.observe(&workspace_revision, |this, _revision, cx| {
            this.refresh_sample(cx);
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
        let flat_sort = ordering::restore_flat_sort(crate::persistence::table_persist::saved_sort(
            backend.read(cx),
            &widths_id,
        ));
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
        let search_input = cx.new(|cx| MoonInputState::new(window, cx));
        cx.subscribe(&search_input, |this, state, event: &MoonInputEvent, cx| {
            if matches!(event, MoonInputEvent::Change) {
                this.set_search(state.read(cx).value().to_string(), cx);
            }
        })
        .detach();
        let focus = cx.focus_handle();

        let mut this = Self {
            backend,
            group,
            sel_cores: HashSet::new(),
            core_selection: RowSelection::default(),
            problems_picked: None,
            last_update_rev: 0,
            last_history_rev: 0,
            sample_gate: crate::panels::RenderGate::default(),
            has_warn: false,
            detached,
            chart_window: ChartWindow::default(),
            chart_server: None,
            chart_core: None,
            cached_rows: Rc::new(Vec::new()),
            rows_generation: 0,
            flat_cache: std::cell::RefCell::new(cache::FlatViewCache::default()),
            frame: std::cell::RefCell::new(frame_cache::FrameCache::default()),
            cached_groups: Rc::new(Vec::new()),
            cached_alarm_starts: Rc::new(Vec::new()),
            alarms: warnings::AlarmsList::default(),
            warn_reads: cache::WarnReadCache::default(),
            warn_read_task: None,
            warn_scroll: moon_ui::MoonVirtualListScrollHandle::new(),
            ip_masked: false,
            health_filter: None,
            search_input,
            search_query: String::new(),
            editing: None,
            edit_input: None,
            flat_sort,
            problems_sort,
            exchange_logos_ready: false,
            mode,
            table_state,
            order_seen,
            warn_table_state,
            problems_table_state,
            updates_table_state,
            warn_cfg_open: false,
            unseen_problems: 0,
            problems_mark_sig: 0,
            flat_measured_w: 0.0,
            servers_measured_w: 0.0,
            toolbar_measured_w: 0.0,
            widths_id,
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
        this.take_updates_request(cx);
        this
    }

    /// Consume this group's Updates request once, persist the mode, and reveal the docked panel.
    fn take_updates_request(&mut self, cx: &mut Context<Self>) {
        let group = self.group.clone();
        let mut requested_mode = self.mode;
        let uid = self.backend.update(cx, |backend, _| {
            let mine = backend.core_status_updates_request.is_some_and(|uid| {
                backend
                    .config
                    .servers
                    .iter()
                    .any(|server| server.uid == uid && server.group == group)
            });
            open::apply_updates_request(
                &mut backend.core_status_updates_request,
                &mut requested_mode,
                mine,
            )
        });
        let Some(uid) = uid else {
            return;
        };
        self.set_mode(requested_mode, cx);
        let selected = {
            let backend = self.backend.read(cx);
            let updates = self.frame_updates(backend);
            open::updates_row_for_core(
                updates.live_rows.iter().map(|row| row.id),
                updates.history.iter().map(|record| record.core),
                uid,
            )
        };
        self.updates_table_state
            .update(cx, |state, cx| state.select_row(selected, cx));
        cx.notify();

        // Construction drains before `Panel::on_added_to` supplies the dock, so resolve it deferred.
        let view = cx.entity().downgrade();
        let backend = self.backend.clone();
        cx.defer(move |app| {
            let Some(view) = view.upgrade() else {
                return;
            };
            let dock = view.read(app).dock.clone().and_then(|dock| dock.upgrade());
            let Some(dock) = dock else {
                return;
            };
            let Some(handle) = backend.read(app).group_windows.get(&group).copied() else {
                return;
            };
            let _ = handle.update(app, |_, window, app| {
                window.activate_window();
                dock.update(app, |dock, cx| {
                    dock.activate_panel_by_name("CoreStatus", window, cx);
                });
            });
        });
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

    /// Let FireTest observe completed row snapshots without forcing a rebuild or repaint.
    pub(crate) fn firetest_rows_generation(&self) -> u64 {
        self.rows_generation
    }
}
