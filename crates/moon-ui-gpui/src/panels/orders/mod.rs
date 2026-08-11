//! Orders panel: a full-width table of open orders across a group's cores.
//! The columns follow the original egui view and use `MoonPalette` styling:
//! Core · Side · Token · Size · Buy · Cur.P · TP.P · Fill · PnL · PnL% · PNL TP · SL · TS · Vstop ·
//! Strat (kind) · Name (strategy name).
//!
//! Side: pending `BUY` and `Short-S` entries use the negative tone; executed `SELL` and
//! `Short-B` exit legs use the info tone. Emulated orders add `(E)`.
//! SL/TS/Vstop display effective ON/OFF flags; ON is positive, while OFF is muted except
//! for SL, where it uses the danger tone.
//!
//! Responsibilities are split across this module for state, view, and lifecycle;
//! [`controls`] for field selectors and sort menus; and [`table`] for table columns and cells.

mod controls;
mod persist;
mod render;
mod sort;
mod table;
mod view;

pub(crate) use sort::executed;
use sort::sort_entries;
use view::{ALL_COLUMNS_MASK, MainOnTop, OrdCol, OrderKind, OrdersViewState, PrimarySort};

use std::collections::HashSet;
use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui::*;
use moon_ui::{
    DockArea, MoonButtonSize, MoonButtonVariant, MoonDataCell, MoonDataRow, MoonDataTable,
    MoonDataTableColumn, MoonDataTableState, MoonDropdown, MoonMenuItem, MoonMenuSize, MoonPalette,
    MoonTone, Panel, PanelEvent, PanelInfo, PanelState, h_flex, v_flex,
};

use rust_i18n::t;

use crate::Backend;
use crate::core_order::{CoreOrder, OrderedCores};
use crate::design;
use crate::panels::{RenderGate, num};
use crate::workspace::{EffectiveCoreScope, RetainedCoreScope};
use moon_core::feed::OrderRow;
use moon_core::session::CoreId;

/// One order-table row associated with its source core, ported from `OrderEntry`.
///
/// No quote asset is stored here; the token cell renders `OrderRow::coin`, which the feed resolved
/// with the core's own exchange rules.
#[derive(Clone)]
pub(super) struct OrderEntry {
    pub(super) core: CoreId,
    pub(super) core_name: String,
    pub(super) row: OrderRow,
}

/// Complete identity of the cached Orders rows and their presentation-sensitive ordering.
#[derive(Clone, PartialEq, Eq)]
struct OrdersCacheKey {
    data_sig: u64,
    view: OrdersViewState,
    /// Canonically ordered effective scope; any Classic or Auto scope change changes the row set.
    scope_cores: Vec<CoreId>,
    current: Option<(CoreId, String)>,
    /// Markets open in the group's Main stack; changes affect row highlighting and ordering.
    main_open: Vec<(CoreId, String)>,
}

/// Panel displaying open orders for one core group.
pub struct OrdersPanel {
    pub(super) backend: Entity<Backend>,
    pub(super) group: String,
    pub(super) view: OrdersViewState,
    /// Retained Classic multi-select core filter; an empty set means every group core. Auto mode
    /// pins its effective workspace scope without using or mutating this selection.
    pub(super) sel_cores: HashSet<CoreId>,
    /// Repaint gate for frequent order and market-driven price/PnL updates.
    ///
    /// `RenderGate` accepts a signature change or new one-second bucket subject to a 250 ms floor,
    /// reducing idle UI work while still refreshing time-sensitive values.
    gate: RenderGate,
    cache_key: Option<OrdersCacheKey>,
    cached_entries: Rc<Vec<OrderEntry>>,
    /// Retained table state containing column order and widths.
    ///
    /// Owning it here allows initialization from `docks.json` and persistence after header drag
    /// reordering instead of leaving it in anonymous window `use_keyed_state` storage.
    table_state: Entity<MoonDataTableState>,
    /// Last persisted column-order list, preventing selection or resize notifications from dumping
    /// the dock unless that list actually changed.
    col_order_cache: Vec<SharedString>,
    /// Context-qualified column-storage ID: `orders-table:dock` or `orders-table:win`.
    /// [`Self::mark_table_detached`] switches detached windows to `:win`, keeping widths per mode.
    widths_id: String,
    /// `(core, market)` pairs open in this group's Main stack, used for lift-to-top sorting.
    /// Rebuilt by [`Self::rebuild_cache`].
    main_open: Rc<HashSet<(CoreId, String)>>,
    /// `(core, uid)` of the first base-sorted order for each Main-open `(core, market)` pair.
    /// Exactly these rows are highlighted, one per pair. Rebuilt by [`Self::rebuild_cache`].
    highlight: Rc<HashSet<(CoreId, u64)>>,
    /// Optimistic SL/TS/Vstop rendering keyed by `(core, uid, stop_tag)`.
    ///
    /// A click immediately stores the target flag and timestamp instead of waiting for rows to be
    /// rebuilt from order events. Each render removes entries aged three seconds or more before
    /// building the cell snapshot, so stale overrides cannot be rendered again. Without an
    /// intervening render, expired map entries can remain allocated longer. Feed rows normally
    /// converge on the next tick because the feed keeps a matching override.
    pub(super) stop_overlay: std::collections::HashMap<(CoreId, u64, u8), (bool, Instant)>,
    /// Footer counts for real and emulated orders before the kind filter but after core and current
    /// market filters. Updated by [`Self::rebuild_cache`].
    count_real: usize,
    count_emu: usize,
    dock: Option<WeakEntity<DockArea>>,
    focus: FocusHandle,
}

impl crate::controls::CoreComboHost for OrdersPanel {
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
        let backend = self.backend.clone();
        self.rebuild_cache(backend.read(cx));
        cx.notify();
    }
}

impl OrdersPanel {
    /// Build a group-scoped Orders panel and subscribe it to data and workspace revisions.
    ///
    /// Args:
    ///     backend: Shared terminal state and workspace authority.
    ///     group: Window group whose cores supply order rows.
    ///     _window: Owning window; retained for the panel-constructor contract.
    ///     cx: Panel context used to create table state and subscriptions.
    ///
    /// Returns:
    ///     Initialized panel with an effective-scope row cache.
    pub fn new(
        backend: Entity<Backend>,
        group: String,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        // Repaint after a backend drain only when the represented cache key changed or the refresh
        // gate admits its periodic update.
        cx.observe(&backend, |this, backend, cx| {
            crate::diag::bump(&crate::diag::ORDERS_OBS_FIRE);
            let now = moon_chart::paint::now_unix_ms();
            let b = backend.read(cx);
            let key = this.cache_key(b);
            let changed = this.cache_key.as_ref() != Some(&key);
            let due = this.gate.should_notify(key.data_sig, now);
            if changed || due {
                this.rebuild_cache(b);
                crate::diag::bump(&crate::diag::ORDERS_OBS_NOTIFY);
                cx.notify();
            }
        })
        .detach();
        let workspace_revision = backend.read(cx).workspace_revision();
        cx.observe(&workspace_revision, |this, _revision, cx| {
            let backend = this.backend.clone();
            this.rebuild_cache(backend.read(cx));
            cx.notify();
        })
        .detach();
        let core_filter_revision = backend.read(cx).core_filter_revision();
        cx.observe(&core_filter_revision, |this, _revision, cx| {
            this.adopt_broadcast_core_filter(cx)
        })
        .detach();
        // Column reordering and resizing mutate `table_state` and emit `notify`. Observe those
        // changes and dump the dock to `docks.json` only when `column_order` changes. The dump reads
        // this panel, so defer it until after the current borrow, as in `mutate`.
        let widths_id = crate::persistence::table_persist::ctx_id("orders-table", false);
        let saved_widths = crate::persistence::table_persist::saved(backend.read(cx), &widths_id);
        let table_state = cx.new(|_| {
            let mut s = MoonDataTableState::new();
            s.column_widths = saved_widths;
            s
        });
        cx.observe(&table_state, |this, state, cx| {
            // A column resize mutates `table_state`; persist widths through the shared storage.
            crate::persistence::table_persist::persist(&this.backend, &this.widths_id, &state, cx);
            let cur = state.read(cx).column_order.clone();
            if cur != this.col_order_cache {
                this.col_order_cache = cur;
                let view = cx.entity();
                cx.defer(move |app| Self::persist(&view, app));
            }
        })
        .detach();
        let mut this = Self {
            backend,
            group,
            view: OrdersViewState::default(),
            sel_cores: HashSet::new(),
            gate: RenderGate::default(),
            cache_key: None,
            cached_entries: Rc::new(Vec::new()),
            table_state,
            widths_id,
            col_order_cache: Vec::new(),
            main_open: Rc::new(HashSet::new()),
            highlight: Rc::new(HashSet::new()),
            stop_overlay: std::collections::HashMap::new(),
            count_real: 0,
            count_emu: 0,
            dock: None,
            focus: cx.focus_handle(),
        };
        this.apply_ctx_columns(cx);
        // A panel created while a filter is on air joins it: a tab detached, re-docked or restored
        // from a saved layout must not be the one surface still showing every core.
        this.adopt_broadcast_core_filter(cx);
        let backend_for_initial_cache = this.backend.clone();
        this.rebuild_cache(backend_for_initial_cache.read(cx));
        this
    }

    /// Collect open orders from every session in the group, attaching each source core and name.
    pub(super) fn collect(&self, b: &Backend) -> Vec<OrderEntry> {
        let store = b.session.store();
        let mut rows = Vec::new();
        for s in b
            .session
            .sessions()
            .iter()
            .filter(|s| s.group == self.group)
        {
            if let Some(d) = store.core(s.id) {
                for o in &d.orders {
                    rows.push(OrderEntry {
                        core: s.id,
                        core_name: s.name.clone(),
                        row: o.clone(),
                    });
                }
            }
        }
        rows
    }

    /// Return the `(core, market)` targeted by the group's Main chart for current-market filtering.
    fn current_market(&self, b: &Backend) -> Option<(CoreId, String)> {
        b.main_chart_target(&self.group)
    }

    /// Build the complete row-cache identity from effective data and presentation inputs.
    ///
    /// Args:
    ///     b: Backend snapshot providing effective scope, order revisions, and Main chart state.
    ///
    /// Returns:
    ///     Cache key that changes for every input consumed by [`Self::build_entries`].
    fn cache_key(&self, b: &Backend) -> OrdersCacheKey {
        let scope = self.effective_scope(b);
        OrdersCacheKey {
            data_sig: sort::orders_sig(b, &scope),
            view: self.view,
            scope_cores: scope.ids().to_vec(),
            current: self
                .view
                .only_current_market
                .then(|| self.current_market(b))
                .flatten(),
            // Track every market open in the Main stack, whether it holds one fullscreen chart or
            // several charts. One row per `(core, market)` is highlighted and may be lifted.
            main_open: b.main_open_markets(&self.group).to_vec(),
        }
    }

    /// Resolve the rows and actions owned by the current Classic or Auto core scope.
    ///
    /// Args:
    ///     b: Backend snapshot containing workspace authority and live group membership.
    ///
    /// Returns:
    ///     Canonically ordered effective core scope without mutating `sel_cores`.
    pub(super) fn effective_scope(&self, b: &Backend) -> EffectiveCoreScope {
        let retained: Vec<CoreId> = self.sel_cores.iter().copied().collect();
        let retained = if retained.is_empty() {
            RetainedCoreScope::All
        } else {
            RetainedCoreScope::Explicit(&retained)
        };
        b.effective_workspace_scope(&self.group, retained)
    }

    /// Return the group's cores in canonical order for the source selector.
    ///
    /// Built on render so an open panel immediately reflects sort-mode changes.
    pub(super) fn group_cores(&self, b: &Backend) -> OrderedCores {
        CoreOrder::new(&b.config).from_sessions(b.session.sessions(), |s| s.group == self.group)
    }

    /// Toggle the retained Classic selected-core filter.
    ///
    /// `None` represents the All item and clears the explicit selection back to the empty-means-all
    /// form. `Some(id)` toggles one core. The selection is not persisted and resets to all. Auto
    /// owns and pins the effective scope, so this method does nothing in that mode.
    ///
    /// Args:
    ///     id: Core to toggle, or `None` for the All row.
    ///     cx: Panel context used to rebuild cached rows and request a repaint.
    ///
    /// Returns:
    ///     Nothing; Classic updates the retained filter and rows, while Auto changes neither.
    pub(super) fn toggle_core(&mut self, id: Option<CoreId>, cx: &mut Context<Self>) {
        if self
            .effective_scope(self.backend.read(cx))
            .is_workspace_owned()
        {
            return;
        }
        if !crate::controls::toggle_core_selection(&mut self.sel_cores, id) {
            return;
        }
        let backend = self.backend.clone();
        self.rebuild_cache(backend.read(cx));
        cx.notify();
    }

    /// Toggle every still-available core from one exchange section in the Classic filter.
    ///
    /// Empty means All before the click, so the first exchange selection becomes explicit. A
    /// fully selected exchange is removed without changing selections from other exchanges.
    /// Rendered ids that left this panel's group are ignored. Auto leaves the retained Classic
    /// selection and cached rows unchanged.
    ///
    /// Args:
    ///     exchange_cores: Core ids captured from one rendered exchange section.
    ///     cx: Panel context used to rebuild cached rows and request a repaint.
    ///
    /// Returns:
    ///     Nothing; a Classic change rebuilds once, while stale-only and Auto-owned calls are
    ///     no-ops.
    pub(super) fn toggle_exchange_cores(
        &mut self,
        exchange_cores: Vec<CoreId>,
        cx: &mut Context<Self>,
    ) {
        if self
            .effective_scope(self.backend.read(cx))
            .is_workspace_owned()
        {
            return;
        }
        let available = self
            .group_cores(self.backend.read(cx))
            .into_iter()
            .map(|(id, _)| id)
            .collect();
        if crate::controls::toggle_exchange_cores(&mut self.sel_cores, &available, exchange_cores) {
            let backend = self.backend.clone();
            self.rebuild_cache(backend.read(cx));
            cx.notify();
        }
    }

    /// Handle a Core-cell click under the current workspace authority.
    ///
    /// Classic sets the retained filter to the clicked core, or clears it when that core is already
    /// the sole selection. Auto ignores the shortcut because only the Shell rail owns selection.
    ///
    /// Args:
    ///     id: Core identity from the clicked order row.
    ///     cx: Panel context used to update the owning authority and visible cache.
    ///
    /// Returns:
    ///     Nothing.
    pub(super) fn filter_to_core(&mut self, id: CoreId, cx: &mut Context<Self>) {
        if self
            .effective_scope(self.backend.read(cx))
            .is_workspace_owned()
        {
            return;
        }
        self.sel_cores = crate::controls::next_core_filter(&self.sel_cores, &[id], false);
        let backend = self.backend.clone();
        self.rebuild_cache(backend.read(cx));
        cx.notify();
    }

    /// Replace the retained Classic filter with the one the Profit Monitor broadcast.
    ///
    /// `apply_core_broadcast` owns the three-way rule — release, ignore, or take the intersection —
    /// so every adopting panel answers a cross-group broadcast the same way. The retained filter is
    /// written even while Auto owns the scope: it is dormant there, and leaving it stale would make
    /// the panel contradict the monitor the moment the user switches back to Classic. Only the
    /// rebuild is skipped, because Auto's effective scope cannot have changed.
    ///
    /// Args:
    ///     cx: Panel context used to rebuild cached rows and request a repaint.
    ///
    /// Returns:
    ///     Nothing; a broadcast about other groups and an unchanged selection both rebuild nothing.
    fn adopt_broadcast_core_filter(&mut self, cx: &mut Context<Self>) {
        let broadcast = self.backend.read(cx).core_filter().clone();
        // Nothing published and nothing retained is the whole life of a terminal where the feature
        // is never used, and every panel construction passes through here: leave before paying for
        // the group's core list.
        if broadcast.is_empty() && self.sel_cores.is_empty() {
            return;
        }
        let available: Vec<CoreId> = self
            .group_cores(self.backend.read(cx))
            .into_iter()
            .map(|(id, _)| id)
            .collect();
        if !crate::controls::apply_core_broadcast(&mut self.sel_cores, &broadcast, available) {
            return;
        }
        if self
            .effective_scope(self.backend.read(cx))
            .is_workspace_owned()
        {
            return;
        }
        let backend = self.backend.clone();
        self.rebuild_cache(backend.read(cx));
        cx.notify();
    }

    /// Collect, filter, and base-sort rows.
    ///
    /// Return `(rows, real_count, emulated_count)`. Counts apply the core and current-market filters
    /// but precede the all/real/emulated kind filter, matching the footer totals.
    fn build_entries(
        &self,
        b: &Backend,
        view: &OrdersViewState,
        current: &Option<(CoreId, String)>,
    ) -> (Vec<OrderEntry>, usize, usize) {
        let mut entries = self.collect(b);
        // Apply the effective Classic or Auto core scope and the current-market filter before the
        // order-kind filter.
        let scope = self.effective_scope(b);
        entries.retain(|e| {
            let by_source = scope.contains(e.core);
            by_source
                && (!view.only_current_market
                    || match current {
                        Some((c, m)) => e.core == *c && &e.row.market == m,
                        None => true,
                    })
        });
        // Split this pre-kind-filter set into real and emulated footer counts.
        let count_real = entries.iter().filter(|e| !e.row.emulator).count();
        let count_emu = entries.len() - count_real;
        // Apply the all, real, or emulated order-kind filter.
        entries.retain(|e| match view.kind {
            OrderKind::All => true,
            OrderKind::Real => !e.row.emulator,
            OrderKind::Emu => e.row.emulator,
        });
        sort_entries(&mut entries, view);
        (entries, count_real, count_emu)
    }

    pub(super) fn rebuild_cache(&mut self, b: &Backend) {
        let key = self.cache_key(b);
        self.main_open = Rc::new(key.main_open.iter().cloned().collect());
        // Build the primary plus newest/oldest base order and the real/emulated footer counts.
        let (mut entries, count_real, count_emu) = self.build_entries(b, &key.view, &key.current);
        self.count_real = count_real;
        self.count_emu = count_emu;
        // Highlight the first row in base order for each Main-open `(core, market)` pair, not every
        // order for that market.
        let mut seen: HashSet<(CoreId, String)> = HashSet::new();
        let mut highlight: HashSet<(CoreId, u64)> = HashSet::new();
        for e in entries.iter() {
            let pair = (e.core, e.row.market.clone());
            if self.main_open.contains(&pair) && seen.insert(pair) {
                highlight.insert((e.core, e.row.uid));
            }
        }
        // Stably lift Main-associated rows over the base order, preserving order within each group.
        match key.view.main_on_top {
            MainOnTop::Off => {}
            MainOnTop::Highlighted => {
                entries.sort_by_key(|e| u8::from(!highlight.contains(&(e.core, e.row.uid))));
            }
            MainOnTop::AllTicker => {
                let markets: HashSet<&str> =
                    self.main_open.iter().map(|(_, m)| m.as_str()).collect();
                entries.sort_by_key(|e| u8::from(!markets.contains(e.row.market.as_str())));
            }
        }
        self.highlight = Rc::new(highlight);
        self.cached_entries = Rc::new(entries);
        self.cache_key = Some(key);
    }

    /// Return the retained table state used by the detached window's automatic-width reset button.
    pub(crate) fn table_state(&self) -> Entity<MoonDataTableState> {
        self.table_state.clone()
    }

    /// Switch a newly created detached panel to the `:win` column-storage context.
    ///
    /// Called immediately after construction by the detached-window path. It reloads widths and
    /// visible columns for that mode so docked and detached views keep independent settings.
    pub(crate) fn mark_table_detached(&mut self, cx: &mut Context<Self>) {
        self.widths_id = crate::persistence::table_persist::ctx_id("orders-table", true);
        let saved =
            crate::persistence::table_persist::saved(self.backend.read(cx), &self.widths_id);
        self.table_state.update(cx, |s, c| {
            s.column_widths = saved;
            c.notify();
        });
        self.apply_ctx_columns(cx);
        let backend = self.backend.clone();
        self.rebuild_cache(backend.read(cx));
        cx.notify();
    }
}
