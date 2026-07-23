//! Orders panel: a full-width table of open orders across a group's cores.
//! The columns follow the original egui view and use `MoonPalette` styling:
//! Core · Side · Token · Size · Buy · Cur.P · TP.P · Fill · PnL · PnL% · PNL TP · SL · TS · Vstop · Strat.
//!
//! Side: pending `BUY` and `Short-S` entries use the negative tone; executed `SELL` and
//! `Short-B` exit legs use the info tone. Emulated orders add `(E)`.
//! SL/TS/Vstop display effective ON/OFF flags; ON is positive, while OFF is muted except
//! for SL, where it uses the danger tone.
//!
//! Responsibilities are split across this module for state, view, and lifecycle;
//! [`controls`] for field selectors and sort menus; and [`table`] for table columns and cells.

mod controls;
mod table;

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
use moon_core::feed::OrderRow;
use moon_core::session::CoreId;
// Used by `table.rs` through `use super::*`, though not directly by this module.
use moon_core::symbol;

/// One order-table row associated with its source core, ported from `OrderEntry`.
///
/// No quote asset is stored here; the token cell strips it from the displayed market name with
/// `symbol` helpers.
#[derive(Clone)]
pub(super) struct OrderEntry {
    pub(super) core: CoreId,
    pub(super) core_name: String,
    pub(super) row: OrderRow,
}

#[derive(Clone, PartialEq, Eq)]
struct OrdersCacheKey {
    data_sig: u64,
    view: OrdersViewState,
    /// Sorted core-filter selection; changing it changes the row set.
    sel_cores: Vec<CoreId>,
    current: Option<(CoreId, String)>,
    /// Markets open in the group's Main stack; changes affect row highlighting and ordering.
    main_open: Vec<(CoreId, String)>,
}

/// Primary sort key selected by the menu's mutually exclusive options.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum PrimarySort {
    SellFirst,
    BuyFirst,
    Creation,
    /// Put profitable positions first by sorting locally calculated PnL in descending order.
    ProfitFirst,
}

impl PrimarySort {
    /// Stable persistence code for `docks.json`, ported from egui's `to_u8`/`from_u8`.
    fn to_u8(self) -> u8 {
        match self {
            PrimarySort::Creation => 0,
            PrimarySort::SellFirst => 1,
            PrimarySort::BuyFirst => 2,
            PrimarySort::ProfitFirst => 3,
        }
    }
    fn from_u8(v: u8) -> Self {
        match v {
            1 => PrimarySort::SellFirst,
            2 => PrimarySort::BuyFirst,
            3 => PrimarySort::ProfitFirst,
            _ => PrimarySort::Creation,
        }
    }
}

/// Persisted per-view mode for lifting rows associated with Main to the top of the list.
///
/// The sort menu exposes two mutually exclusive checked options, either of which can be disabled.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum MainOnTop {
    /// Keep the regular sort order.
    Off,
    /// Lift every order for a market open on Main, across all cores.
    AllTicker,
    /// Lift only the highlighted row for each `(core, market)` pair open on Main.
    Highlighted,
}

impl MainOnTop {
    fn to_u8(self) -> u8 {
        match self {
            MainOnTop::Off => 0,
            MainOnTop::AllTicker => 1,
            MainOnTop::Highlighted => 2,
        }
    }
    fn from_u8(v: u8) -> Self {
        match v {
            1 => MainOnTop::AllTicker,
            2 => MainOnTop::Highlighted,
            _ => MainOnTop::Off,
        }
    }
}

/// Order-kind filter: all, real, or emulated.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum OrderKind {
    All,
    Real,
    Emu,
}

impl OrderKind {
    /// Stable persistence code for `docks.json`, ported from egui's `to_u8`/`from_u8`.
    fn to_u8(self) -> u8 {
        match self {
            OrderKind::All => 0,
            OrderKind::Real => 1,
            OrderKind::Emu => 2,
        }
    }
    fn from_u8(v: u8) -> Self {
        match v {
            1 => OrderKind::Real,
            2 => OrderKind::Emu,
            _ => OrderKind::All,
        }
    }
}

/// Order-table columns in canonical order.
///
/// A column's position in [`OrdCol::ALL`] is its bit number in
/// [`OrdersViewState::columns`]. [`OrdCol::key`] returns a stable `docks.json` persistence key
/// independent of enum order.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum OrdCol {
    Core,
    Side,
    Token,
    Size,
    Buy,
    CurP,
    /// Take-profit price: the exit leg's target price from `sell_price`.
    TpPrice,
    Fill,
    Pnl,
    PnlPct,
    /// Estimated PnL at the take-profit price: `(tp - entry) * qty * direction`.
    PnlTp,
    Sl,
    Ts,
    Vstop,
    Strat,
}

impl OrdCol {
    // Keep SL/TS/Vstop on the right beside Strat, leaving the important price and PnL fields
    // together. TP price sits beside Buy/Cur.P, and PNL TP sits beside PnL/PnL%.
    pub(super) const ALL: [OrdCol; 15] = [
        OrdCol::Core,
        OrdCol::Side,
        OrdCol::Token,
        OrdCol::Size,
        OrdCol::Buy,
        OrdCol::CurP,
        OrdCol::TpPrice,
        OrdCol::Fill,
        OrdCol::Pnl,
        OrdCol::PnlPct,
        OrdCol::PnlTp,
        OrdCol::Sl,
        OrdCol::Ts,
        OrdCol::Vstop,
        OrdCol::Strat,
    ];

    /// Stable key used by `docks.json` persistence and menu elements.
    pub(super) fn key(self) -> &'static str {
        match self {
            OrdCol::Core => "core",
            OrdCol::Side => "side",
            OrdCol::Token => "token",
            OrdCol::Size => "size",
            OrdCol::Sl => "sl",
            OrdCol::Ts => "ts",
            OrdCol::Vstop => "vstop",
            OrdCol::Buy => "buy",
            OrdCol::CurP => "cur.p",
            OrdCol::TpPrice => "tp.p",
            OrdCol::Fill => "fill",
            OrdCol::Pnl => "pnl",
            OrdCol::PnlPct => "pnl.pct",
            OrdCol::PnlTp => "pnl.tp",
            OrdCol::Strat => "strat",
        }
    }

    /// Column bit in the visibility mask, derived from its position in [`Self::ALL`].
    pub(super) fn bit(self) -> u16 {
        let idx = OrdCol::ALL
            .iter()
            .position(|c| *c == self)
            .unwrap_or_default();
        1u16 << idx
    }

    fn from_key(key: &str) -> Option<OrdCol> {
        OrdCol::ALL.iter().copied().find(|c| c.key() == key)
    }
}

/// Default view mask with every column visible.
pub(super) const ALL_COLUMNS_MASK: u16 = (1u16 << OrdCol::ALL.len()) - 1;

/// Per-panel table view state: order kind, current-market filter, sorting, and visible columns.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) struct OrdersViewState {
    pub(super) kind: OrderKind,
    pub(super) only_current_market: bool,
    pub(super) primary: PrimarySort,
    pub(super) newest_first: bool,
    /// Whether to lift no Main rows, every matching market row, or only highlighted rows.
    pub(super) main_on_top: MainOnTop,
    /// Visible-column bit mask, where each bit comes from [`OrdCol::bit`]. Persisted as keys.
    pub(super) columns: u16,
}

impl OrdersViewState {
    /// Return whether a column is visible in the current view.
    pub(super) fn shows(&self, col: OrdCol) -> bool {
        self.columns & col.bit() != 0
    }

    /// Return visible columns in canonical order.
    pub(super) fn visible_columns(&self) -> Vec<OrdCol> {
        OrdCol::ALL
            .iter()
            .copied()
            .filter(|c| self.shows(*c))
            .collect()
    }
}

impl Default for OrdersViewState {
    fn default() -> Self {
        Self {
            // By default, show only real orders and put executed entries first so active positions
            // and their exit legs are immediately visible. The sort and kind menus persist changes
            // to `docks.json`.
            kind: OrderKind::Real,
            only_current_market: false,
            primary: PrimarySort::SellFirst,
            newest_first: true,
            main_on_top: MainOnTop::Highlighted,
            columns: ALL_COLUMNS_MASK,
        }
    }
}

/// Return whether the entry has executed, using the authoritative worker status rather than a
/// leg's `fill_pct`.
///
/// The phase machine is shared by both directions: `None` and `BuySet` mean the entry is pending;
/// `BuyDone` and every represented `Sell*` phase mean the position has been entered. Using the
/// entry leg's `fill_pct` previously read an empty `sell_order` for shorts and left them displayed
/// as `Short-S` indefinitely.
pub(super) fn executed(r: &OrderRow) -> bool {
    matches!(
        r.status.as_str(),
        "BuyDone" | "SellSet" | "SellDone" | "SellFail" | "SellCancel" | "SellAlmostDone"
    )
}
/// Return whether either a long or short entry has executed, for `SellFirst` sorting.
pub(super) fn is_sell(r: &OrderRow) -> bool {
    executed(r)
}
/// Return whether this is a pending long entry, displayed as `BUY`.
pub(super) fn is_buy(r: &OrderRow) -> bool {
    !r.is_short && !executed(r)
}

/// Panel displaying open orders for one core group.
pub struct OrdersPanel {
    pub(super) backend: Entity<Backend>,
    pub(super) group: String,
    pub(super) view: OrdersViewState,
    /// Multi-select core filter; an empty set means every core in the group.
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

impl OrdersPanel {
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
        let backend_for_initial_cache = this.backend.clone();
        this.rebuild_cache(backend_for_initial_cache.read(cx));
        this
    }

    /// Collect open orders from every session in the group, attaching each source core and name.
    fn collect(&self, b: &Backend) -> Vec<OrderEntry> {
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

    fn cache_key(&self, b: &Backend) -> OrdersCacheKey {
        OrdersCacheKey {
            data_sig: orders_sig(b, &self.group),
            view: self.view,
            sel_cores: {
                let mut v: Vec<CoreId> = self.sel_cores.iter().copied().collect();
                v.sort_unstable();
                v
            },
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

    /// Return the group's cores in canonical order for the source selector.
    ///
    /// Built on render so an open panel immediately reflects sort-mode changes.
    fn group_cores(&self, b: &Backend) -> OrderedCores {
        CoreOrder::new(&b.config).from_sessions(b.session.sessions(), |s| s.group == self.group)
    }

    /// Toggle the selected-core filter.
    ///
    /// `None` represents the All item: if every group core is explicitly selected, clear the set
    /// back to the empty-means-all form; otherwise select every group core. `Some(id)` toggles one
    /// core. The selection is not persisted and resets to all cores.
    pub(super) fn toggle_core(&mut self, id: Option<CoreId>, cx: &mut Context<Self>) {
        let all: HashSet<CoreId> = self
            .backend
            .read(cx)
            .session
            .sessions()
            .iter()
            .filter(|s| s.group == self.group)
            .map(|s| s.id)
            .collect();
        match id {
            None => {
                if !all.is_empty() && self.sel_cores.len() == all.len() {
                    self.sel_cores.clear();
                } else {
                    self.sel_cores = all;
                }
            }
            Some(id) => {
                if !self.sel_cores.remove(&id) {
                    self.sel_cores.insert(id);
                }
            }
        }
        let backend = self.backend.clone();
        self.rebuild_cache(backend.read(cx));
        cx.notify();
    }

    /// Set the filter to exactly one core after a Core-cell click, or clear it when that core is
    /// already the sole selection. Unlike [`Self::toggle_core`], this is not a multi-select toggle.
    pub(super) fn filter_to_core(&mut self, id: CoreId, cx: &mut Context<Self>) {
        if self.sel_cores.len() == 1 && self.sel_cores.contains(&id) {
            self.sel_cores.clear();
        } else {
            self.sel_cores = HashSet::from([id]);
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
        // Apply the multi-select core filter, where empty means all, and the current-market filter
        // before applying the order-kind filter.
        entries.retain(|e| {
            let by_source = self.sel_cores.is_empty() || self.sel_cores.contains(&e.core);
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

    fn rebuild_cache(&mut self, b: &Backend) {
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

    /// Reconstruct the panel from `docks.json` by applying the `PanelInfo` view state after `new`.
    ///
    /// Sorting, order kind, current-market filtering, and column settings are restored. The core
    /// selection is intentionally not persisted and therefore starts as all cores, matching the
    /// original egui product behavior.
    pub fn restored(
        backend: Entity<Backend>,
        group: String,
        info: &PanelInfo,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut this = Self::new(backend, group, window, cx);
        this.view = view_from_info(info);
        // A per-context visible-column set in shared storage overrides the legacy `docks.json` set.
        // Without one, the legacy set remains as a migration seed until the first column toggle
        // writes it to shared storage.
        this.apply_ctx_columns(cx);
        // Persist drag-defined column order separately because it is not part of the copyable view.
        let order = column_order_from_info(info);
        if !order.is_empty() {
            this.col_order_cache = order.clone();
            this.table_state.update(cx, |s, _| s.column_order = order);
        }
        this
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

    /// Apply a saved per-context visible-column set for `widths_id`, when valid.
    ///
    /// Docked `:dock` and detached `:win` modes use distinct shared-storage entries. An empty or
    /// invalid entry with no recognized keys leaves the current view intact rather than producing
    /// an empty table.
    fn apply_ctx_columns(&mut self, cx: &App) {
        if let Some(keys) =
            crate::persistence::table_persist::visible(self.backend.read(cx), &self.widths_id)
        {
            let mask = keys
                .iter()
                .filter_map(|k| OrdCol::from_key(k))
                .fold(0u16, |m, c| m | c.bit());
            if mask != 0 {
                self.view.columns = mask;
            }
        }
    }

    /// Save the current visible-column set in per-context storage under `widths_id`.
    ///
    /// Called by [`Self::mutate`] when the visibility mask changes.
    fn save_ctx_columns(&self, cx: &mut App) {
        let keys: Vec<String> = self
            .view
            .visible_columns()
            .iter()
            .map(|c| c.key().to_string())
            .collect();
        crate::persistence::table_persist::set_visible(&self.backend, &self.widths_id, keys, cx);
    }

    /// Mutate the copyable view state and, only when it changes, rebuild, repaint, and persist it.
    ///
    /// Dock dumping occurs at the `App` level after releasing the panel borrow because
    /// `dock.dump()` reads this panel and would otherwise re-enter it.
    pub(super) fn mutate(view: &Entity<Self>, app: &mut App, f: impl FnOnce(&mut OrdersViewState)) {
        let changed = view.update(app, |this, cx| {
            let mut next = this.view;
            f(&mut next);
            if next != this.view {
                let cols_changed = next.columns != this.view.columns;
                this.view = next;
                let backend = this.backend.clone();
                this.rebuild_cache(backend.read(cx));
                // Persist a changed visible-column set through the shared per-context descriptor.
                if cols_changed {
                    this.save_ctx_columns(cx);
                }
                cx.notify();
                true
            } else {
                false
            }
        });
        if changed {
            Self::persist(view, app);
        }
    }

    /// Dump the current dock layout to backend state for `docks.json` persistence.
    ///
    /// Order-view changes emit no `DockEvent`, so this panel persists them directly to prevent the
    /// sort and filter state from resetting when reopened.
    fn persist(view: &Entity<Self>, app: &mut App) {
        let (dock, group, backend) = {
            let p = view.read(app);
            (p.dock.clone(), p.group.clone(), p.backend.clone())
        };
        let Some(dock) = dock.and_then(|d| d.upgrade()) else {
            return;
        };
        let state = dock.read(app).dump(app);
        backend.update(app, |b, _| {
            b.dock_states.insert(group, state);
            b.dock_dirty = true;
        });
    }
}

/// Return the displayed-side grouping key, accounting for execution; zero sorts first.
fn primary_key(p: PrimarySort, r: &OrderRow) -> u8 {
    match p {
        // `ProfitFirst` is handled separately in `sort_entries`, so its group is neutral here.
        PrimarySort::Creation | PrimarySort::ProfitFirst => 0,
        PrimarySort::SellFirst => u8::from(!is_sell(r)),
        PrimarySort::BuyFirst => u8::from(!is_buy(r)),
    }
}

/// Apply the base primary sort plus the newest/oldest UID tie-breaker.
///
/// [`OrdersPanel::rebuild_cache`] subsequently applies the stable Main lift over this order.
fn sort_entries(entries: &mut [OrderEntry], view: &OrdersViewState) {
    // Profit-first sorting uses descending locally calculated PnL. Rows without a position sort
    // last, with UID as the newest/oldest tie-breaker just like the other modes.
    if view.primary == PrimarySort::ProfitFirst {
        entries.sort_by(|a, b| {
            let pa = table::order_pnl(&a.row);
            let pb = table::order_pnl(&b.row);
            // Treat `None` as negative infinity so it sorts at the bottom.
            let key = |v: Option<f64>| v.unwrap_or(f64::NEG_INFINITY);
            key(pb)
                .partial_cmp(&key(pa))
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| {
                    let c = a.row.uid.cmp(&b.row.uid);
                    if view.newest_first { c.reverse() } else { c }
                })
        });
        return;
    }
    entries.sort_by(|a, b| {
        let ka = primary_key(view.primary, &a.row);
        let kb = primary_key(view.primary, &b.row);
        ka.cmp(&kb).then_with(|| {
            let c = a.row.uid.cmp(&b.row.uid);
            if view.newest_first { c.reverse() } else { c }
        })
    });
}

/// Restore persisted view fields from `PanelInfo`, keeping defaults for absent fields.
///
/// The core selection is stored outside [`OrdersViewState`] and intentionally omitted from
/// persistence, so restored panels start with all cores.
fn view_from_info(info: &PanelInfo) -> OrdersViewState {
    let mut v = OrdersViewState::default();
    if let PanelInfo::Panel(j) = info {
        if let Some(p) = j.get("primary").and_then(|x| x.as_u64()) {
            v.primary = PrimarySort::from_u8(p as u8);
        }
        if let Some(k) = j.get("kind").and_then(|x| x.as_u64()) {
            v.kind = OrderKind::from_u8(k as u8);
        }
        if let Some(n) = j.get("newest_first").and_then(|x| x.as_bool()) {
            v.newest_first = n;
        }
        if let Some(o) = j.get("only_current").and_then(|x| x.as_bool()) {
            v.only_current_market = o;
        }
        if let Some(m) = j.get("main_on_top").and_then(|x| x.as_u64()) {
            v.main_on_top = MainOnTop::from_u8(m as u8);
        }
        // Convert visible-column keys to a mask. A missing or empty list, or one containing no valid
        // keys, retains the all-visible default instead of producing an empty table.
        if let Some(arr) = j.get("columns").and_then(|x| x.as_array()) {
            let mask = arr
                .iter()
                .filter_map(|x| x.as_str())
                .filter_map(OrdCol::from_key)
                .fold(0u16, |m, c| m | c.bit());
            if mask != 0 {
                v.columns = mask;
            }
        }
    }
    v
}

/// Read drag-defined column order from `PanelInfo` as a list of recognized [`OrdCol`] keys.
///
/// Ignoring unknown keys tolerates stale or malformed persistence data. An empty result leaves the
/// table's default [`OrdCol::ALL`] order in effect.
fn column_order_from_info(info: &PanelInfo) -> Vec<SharedString> {
    let PanelInfo::Panel(j) = info else {
        return Vec::new();
    };
    j.get("column_order")
        .and_then(|x| x.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|x| x.as_str())
                .filter(|s| OrdCol::from_key(s).is_some())
                .map(SharedString::from)
                .collect()
        })
        .unwrap_or_default()
}

impl EventEmitter<PanelEvent> for OrdersPanel {}
impl Focusable for OrdersPanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}
impl Panel for OrdersPanel {
    fn panel_name(&self) -> &'static str {
        "Orders"
    }
    /// Visible tab caption. `panel_name` is the stable persistence key and stays untouched.
    fn tab_name(&self, _cx: &App) -> Option<SharedString> {
        crate::persistence::panel_meta::tab_label(self.panel_name())
    }
    // Closing returns the panel to the bottom row rather than deleting it; see
    // `Shell::PanelCloseRequested` handling.
    fn closable(&self, _cx: &App) -> bool {
        true
    }
    // A single panel split out on its own keeps its header so it still has a drag handle and close
    // button.
    fn show_dock_header(&self, _cx: &App) -> bool {
        true
    }
    fn title(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        crate::persistence::panel_meta::panel_title(self.panel_name())
    }
    fn dump(&self, cx: &App) -> PanelState {
        // Persist the group for reconstruction and the view's sorting, kind, filter, and column
        // settings. Since schema v11, Core IDs equal stable `ServerConfig.uid` values, but the
        // product intentionally resets the selected-core filter to All instead of persisting it,
        // matching the egui behavior.
        PanelState {
            panel_name: "Orders".to_string(),
            children: Vec::new(),
            info: PanelInfo::panel(serde_json::json!({
                "group": self.group,
                "primary": self.view.primary.to_u8(),
                "kind": self.view.kind.to_u8(),
                "newest_first": self.view.newest_first,
                "only_current": self.view.only_current_market,
                "main_on_top": self.view.main_on_top.to_u8(),
                // Store visible columns as stable keys rather than a mask so enum reordering is
                // harmless. A missing field restores every column as visible.
                "columns": self
                    .view
                    .visible_columns()
                    .iter()
                    .map(|c| c.key())
                    .collect::<Vec<_>>(),
                // Persist header drag order as stable keys. It lives in `table_state`, so read it
                // directly; an empty list leaves the default `ALL` order.
                "column_order": self
                    .table_state
                    .read(cx)
                    .column_order
                    .iter()
                    .map(|k| k.to_string())
                    .collect::<Vec<_>>(),
            })),
        }
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
        Some(vec![
            crate::persistence::table_persist::reset_button(
                "orders-reset-widths",
                &self.table_state,
            ),
            crate::panels::detach_button(
                "Orders",
                self.group.clone(),
                self.backend.clone(),
                self.dock.clone(),
            ),
        ])
    }
}

impl Render for OrdersPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        crate::diag::bump(&crate::diag::ORDERS_RENDER);
        let view = self.view;
        let cores = self.group_cores(self.backend.read(cx));
        let entries = self.cached_entries.clone();
        let p = MoonPalette::active(cx);

        // Control bar.
        let mut controls = h_flex()
            .w_full()
            .flex_none()
            .gap_2()
            .items_center()
            .px_2()
            .py_1()
            .child(self.source_combo(&cores, cx))
            .child(self.kind_combo(cx))
            .child(self.columns_menu(cx))
            .child(self.sort_menu(cx));
        if view.only_current_market {
            controls = controls.child(
                div()
                    .text_size(design::t_body(cx))
                    .text_color(rgb(p.text_muted))
                    .child(format!("· {}", t!("orders.only_current"))),
            );
        }

        // Virtualized table using the reference HTML geometry.
        // Highlight one row per Main-open `(core, market)` pair; see `table::orders_table`.
        // Prune optimistic stop toggles aged three seconds or more, then snapshot the survivors for
        // SL/TS/Vstop cells.
        self.stop_overlay
            .retain(|_, (_, t)| t.elapsed() < Duration::from_secs(3));
        let stop_overlay: Rc<std::collections::HashMap<(CoreId, u64, u8), bool>> = Rc::new(
            self.stop_overlay
                .iter()
                .map(|(k, (v, _))| (*k, *v))
                .collect(),
        );
        let table = table::orders_table(
            entries,
            view.columns,
            &self.table_state,
            self.highlight.clone(),
            stop_overlay,
            cx,
        );

        // Footer with total, real, and emulated open-order counts.
        let total = self.count_real + self.count_emu;
        let footer = h_flex()
            .w_full()
            .flex_none()
            .gap_2()
            .items_center()
            .px_2()
            .py_1()
            .child(
                div()
                    .text_size(design::t_body(cx))
                    .text_color(rgb(p.text_soft))
                    .child(
                        t!(
                            "orders.footer_total",
                            total = total,
                            real = self.count_real,
                            emu = self.count_emu
                        )
                        .to_string(),
                    ),
            );

        v_flex()
            .id("orders-panel")
            .size_full()
            .min_h(px(0.0))
            .overflow_hidden()
            .track_focus(&self.focus)
            .font_family(design::mono())
            .text_size(design::t_body(cx))
            .bg(rgb(p.table_body))
            .child(controls)
            .child(div().w_full().h(px(1.0)).flex_none().bg(rgb(p.border)))
            .child(table)
            .child(div().w_full().h(px(1.0)).flex_none().bg(rgb(p.border)))
            .child(footer)
    }
}

/// Return the group's order-table signature from each core's table revision.
///
/// This deliberately uses the table revision rather than chart-line revisions so numeric fields and
/// statuses refresh independently of chart userdata.
fn orders_sig(b: &Backend, group: &str) -> u64 {
    let store = b.session.store();
    b.session
        .sessions()
        .iter()
        .filter(|s| s.group == group)
        .filter_map(|s| store.core(s.id))
        .fold(0u64, |a, c| {
            a.wrapping_mul(31).wrapping_add(c.orders_table_rev)
        })
}
