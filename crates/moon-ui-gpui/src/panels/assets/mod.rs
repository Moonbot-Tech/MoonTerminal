//! Assets panel/window. Top: core selector and dust threshold; then the positions/balances table
//! across every in-scope core (values and totals in USDT); then a footer carrying both summaries
//! — visible-row count and Σ on the left, the scope's account equity on the right.
//! Bottom (separate global or detached window only): the core list on the left (free/total) and
//! three wallet containers (Spot/Futures/Quarterly) on the right — dragging a coin between them
//! opens a quantity dialog (defaulting to the whole free amount) and performs the transfer.
//!
//! The same `AssetsView` lives in two shapes:
//! - as a dock panel inside a group window (`AssetsScope::Group`) — that group's cores;
//! - as a global singleton window (`AssetsScope::All`, opened via the "⧉" button) — ALL
//!   connected cores. Window dedup lives in `Backend.assets_window` (like "Strategies").
//!
//! A futures core shows ONLY open positions in the table (the Moonbot rule, see
//! [`AssetsView::collect`]), so an account with no positions would look empty: the account
//! balance comes from the trust-aware balance surfaces ([`balances`]), not from a table row. A
//! synthetic per-market row would duplicate the margin onto every market, which is what that
//! rule exists to prevent.
//!
//! Split by responsibility: state/data/lifecycle/window here; the table, the core bar/list and
//! the footer in [`table`]; balance aggregation and its trust-aware rendering in [`balances`];
//! the 3 wallet containers and the drag&drop transfer dialog in [`wallets`].

mod balances;
mod cache;
mod collect;
mod render;
mod table;
mod wallets;
mod window;

use collect::{AssetEntry, WalletColumnSnapshot, money};
pub use window::open;
use window::{ASSETS_HEADER_H, assets_header};

use std::collections::HashSet;
use std::rc::Rc;

use gpui::prelude::FluentBuilder;
use gpui::*;
use moon_ui::{
    DockArea, MoonBackgroundPolicy, MoonButton, MoonButtonSize, MoonDataCell, MoonDataRow,
    MoonDataTable, MoonDataTableColumn, MoonDataTableState, MoonInput, MoonInputState, MoonPalette,
    MoonSlider, MoonSliderEvent, MoonSliderState, MoonTone, MoonWindowFrame, Panel, PanelEvent,
    PanelState, Root, h_flex, v_flex,
};

use crate::Backend;
use crate::core_order::{CoreOrder, OrderedCores};
use crate::design;
use crate::panels::{RenderGate, num};
use moon_core::feed::{AssetRow, TransferAssetRow, WalletKind};
use moon_core::session::CoreId;
use moon_core::util::fmt;
use rust_i18n::t;

use balances::CoreAgg;
use moon_core::session::BalanceState;
use wallets::PendingTransfer;

/// Core scope represented by an Assets view.
#[derive(Clone, PartialEq, Eq)]
pub(super) enum AssetsScope {
    /// Group-window dock panel containing that group's cores.
    Group(String),
    /// Global window containing all connected cores.
    All,
}

/// Assets dock panel or standalone window content.
pub struct AssetsView {
    pub(super) backend: Entity<Backend>,
    scope: AssetsScope,
    /// Whether this view draws its own OS-window frame and persists its geometry. This is true for
    /// the global window; `DetachedWindow` frames detached views, and dock tabs need no frame.
    windowed: bool,
    /// Whether to show the lower transfer area: the core list and Spot, Futures, and Quarterly
    /// wallets. Every standalone window enables it; a dock tab does not.
    show_wallets: bool,
    /// Core selected for the lower wallet containers.
    pub(super) selected_core: Option<CoreId>,
    /// Hide asset rows worth less than this USDT threshold while retaining open positions whose
    /// notional reaches the market's minimum lot. A non-positive threshold shows every row.
    pub(super) min_value_usd: f64,
    /// Top-bar threshold slider state, ranging from 0 through 100 USD in steps of 1, defaulting to 1.
    min_value_slider: Entity<MoonSliderState>,
    /// Multi-selected core filter, like Orders and Report. Empty means every core in scope.
    pub(super) sel_cores: HashSet<CoreId>,
    /// Whether the core list and Spot, Futures, and Quarterly wallet section is collapsed.
    pub(super) wallets_collapsed: bool,
    /// Open transfer-quantity dialog and its input. `PendingTransfer` is private to `wallets`, so
    /// this field remains private while child modules can access it.
    pending_transfer: Option<PendingTransfer>,
    transfer_input: Option<Entity<MoonInputState>>,
    /// Redraw gate driven by the asset-related signature or a new one-second bucket, with a 250 ms
    /// minimum notification interval.
    gate: RenderGate,
    /// Inputs represented by the current caches: data revisions and the dust threshold.
    cache_sig: Option<(u64, u64)>,
    cached_cores: Vec<(CoreId, String)>,
    cached_entries: Rc<Vec<AssetEntry>>,
    /// `(core, uppercase coin)` pairs with an active `SellSet` or `SellAlmostDone` order. Their rows
    /// are marked as currently for sale. Rebuilt by `rebuild_cache`; the signature includes each
    /// core's `orders_table_rev`.
    pub(super) sell_marked: Rc<std::collections::HashSet<(CoreId, String)>>,
    /// Per-core balance figures and their trust classifications for the current scope.
    cached_aggs: Rc<Vec<CoreAgg>>,
    /// Every in-scope core (after the filter) is a futures core. An empty table then means "no
    /// open positions" rather than "no assets": futures balances are quote-denominated and never
    /// reach the table. Computed in `rebuild_cache` to keep the store out of `render`.
    cached_all_futures: bool,
    cached_wallet_key: Option<(Option<CoreId>, u64, u64)>,
    cached_wallets: Rc<Vec<WalletColumnSnapshot>>,
    /// Finite USDT value summed across the currently visible table rows.
    cached_total_value: f64,
    /// Visible rows whose value was not finite and so contributed nothing to `cached_total_value`.
    /// Counted rather than discarded: the row count includes them, so without this Σ would claim
    /// to cover rows it silently dropped — the same "partial sum shown as complete" the balance
    /// side of the footer is built to prevent.
    cached_value_excluded: usize,
    /// Asset-table column widths and sorting state. Its widths persist through
    /// [`crate::persistence::table_persist`].
    table_state: Entity<MoonDataTableState>,
    /// Contextual width-storage ID: `assets-table:dock` for a dock tab and `assets-table:win` for
    /// standalone or detached views with wallets. Each mode retains independent widths.
    widths_id: String,
    dock: Option<WeakEntity<DockArea>>,
    focus: FocusHandle,
}

impl AssetsView {
    /// Build an Assets view for a core scope and the requested window surfaces.
    pub(super) fn new(
        backend: Entity<Backend>,
        scope: AssetsScope,
        windowed: bool,
        show_wallets: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        // Rebuild after an asset-related signature change or the gate's once-per-second refresh.
        cx.observe(&backend, |this, backend, cx| {
            let now = moon_chart::paint::now_unix_ms();
            let b = backend.read(cx);
            let sig = this.assets_sig(b);
            let key = this.cache_key(sig);
            let changed = this.cache_sig != Some(key);
            let due = this.gate.should_notify(sig, now);
            if changed || due {
                this.rebuild_cache(b);
                cx.notify();
            }
        })
        .detach();

        // Only the global standalone window owns persisted geometry; a dock panel uses its group window.
        if windowed {
            cx.observe_window_bounds(window, |this, window, cx| {
                let Some((x, y, w, h)) = crate::window::windowing::window_geom(window) else {
                    return;
                };
                this.backend.update(cx, |b, _| {
                    if b.layout.assets_window.map(|g| (g.x, g.y, g.w, g.h)) != Some((x, y, w, h)) {
                        b.layout.assets_window =
                            Some(moon_core::config::layout::GeomRect { x, y, w, h });
                        b.layout_dirty = true;
                    }
                });
            })
            .detach();
        }

        // Standalone and detached views with wallet containers use the `:win` width context; dock
        // tabs use `:dock`, retaining separate widths for each mode.
        let widths_id = crate::persistence::table_persist::ctx_id("assets-table", show_wallets);
        let saved_widths = crate::persistence::table_persist::saved(backend.read(cx), &widths_id);
        let table_state = cx.new(|_| {
            let mut s = MoonDataTableState::new();
            s.column_widths = saved_widths;
            s
        });
        // Column resizing mutates the state; persist the resulting widths through the shared saver.
        cx.observe(&table_state, |this, state, cx| {
            crate::persistence::table_persist::persist(&this.backend, &this.widths_id, &state, cx);
        })
        .detach();

        // Restore the shared "hide below N USD" threshold from `layout.toml`; default to 1 USD.
        let min_value_usd = backend
            .read(cx)
            .layout
            .assets_min_value
            .unwrap_or(1.0)
            .clamp(0.0, 100.0);
        // Top-bar threshold slider: 0 through 100, step 1, initialized from the persisted value.
        let min_value_slider = cx.new(|_| {
            MoonSliderState::new()
                .min(0.0)
                .max(100.0)
                .step(1.0)
                .default_value(min_value_usd as f32)
        });
        // A slider change immediately rebuilds the cached snapshot independently of the redraw
        // gate, persists the threshold, and requests a repaint.
        cx.subscribe(&min_value_slider, |this, _e, ev: &MoonSliderEvent, cx| {
            if let MoonSliderEvent::Change(v) = ev {
                let v = v.end() as f64;
                if this.min_value_usd != v {
                    this.min_value_usd = v;
                    let backend = this.backend.clone();
                    this.rebuild_cache(backend.read(cx));
                    this.persist_min_value(cx);
                    cx.notify();
                }
            }
        })
        .detach();

        let mut this = Self {
            backend,
            scope,
            windowed,
            show_wallets,
            selected_core: None,
            min_value_usd,
            min_value_slider,
            sel_cores: HashSet::new(),
            wallets_collapsed: false,
            pending_transfer: None,
            transfer_input: None,
            gate: RenderGate::default(),
            cache_sig: None,
            cached_cores: Vec::new(),
            cached_entries: Rc::new(Vec::new()),
            sell_marked: Rc::new(std::collections::HashSet::new()),
            cached_aggs: Rc::new(Vec::new()),
            cached_all_futures: false,
            cached_wallet_key: None,
            cached_wallets: Rc::new(Vec::new()),
            cached_total_value: 0.0,
            cached_value_excluded: 0,
            table_state,
            widths_id,
            dock: None,
            focus: cx.focus_handle(),
        };
        // Request transfer assets from every scoped core. Spot wallets feed both the selected
        // core's lower containers and the upper table because some exchanges, including Bitget,
        // expose purchased coins only through `transfer_assets`, not per-market balances.
        let cores: Vec<CoreId> = this
            .scope_cores(this.backend.read(cx))
            .into_iter()
            .map(|(id, _)| id)
            .collect();
        this.selected_core = cores.first().copied();
        for core in &cores {
            if let Err(error) = this.backend.read(cx).session.refresh_transfer_assets(*core) {
                log::warn!("assets initial refresh failed for core {core}: {error}");
            }
        }
        let backend_for_initial_cache = this.backend.clone();
        this.rebuild_cache(backend_for_initial_cache.read(cx));
        this
    }

    /// Restores a group-scoped dock tab from `docks.json`, without wallet containers.
    pub fn restored_group(
        backend: Entity<Backend>,
        group: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        Self::new(backend, AssetsScope::Group(group), false, false, window, cx)
    }

    /// Builds group-scoped detached-window content with lower transfer containers.
    /// `DetachedWindow` supplies the frame and geometry persistence.
    pub fn detached_group(
        backend: Entity<Backend>,
        group: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        Self::new(backend, AssetsScope::Group(group), false, true, window, cx)
    }

    /// Return connected scope cores in canonical order: one group or all groups.
    pub(super) fn scope_cores(&self, b: &Backend) -> OrderedCores {
        CoreOrder::new(&b.config).from_sessions(b.session.sessions(), |s| match &self.scope {
            AssetsScope::Group(g) => &s.group == g,
            AssetsScope::All => true,
        })
    }

    /// Toggles the multi-core filter. `None` represents All: a selection containing every scoped
    /// core collapses to the equivalent empty-means-all state; otherwise it selects every scoped
    /// core. Stale ids do not stand in for current cores. `Some(id)` toggles one core. The filter is
    /// not persisted and reopens as All.
    ///
    /// Args:
    ///     id: Core to toggle, or `None` for the All row.
    ///     cx: View context used to rebuild cached rows and request a repaint.
    ///
    /// Returns:
    ///     Nothing; the in-memory filter and cached rows are updated in place.
    pub(super) fn toggle_core(&mut self, id: Option<CoreId>, cx: &mut Context<Self>) {
        let all: HashSet<CoreId> = self
            .scope_cores(self.backend.read(cx))
            .into_iter()
            .map(|(id, _)| id)
            .collect();
        match id {
            None => crate::controls::toggle_all_core_selection(&mut self.sel_cores, all),
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

    /// Toggle every still-available core from one clicked exchange section.
    ///
    /// Empty means All before the click, so the first exchange selection becomes explicit. A
    /// fully selected exchange is removed without changing selections from other exchanges.
    /// Rendered ids that left the current group or global scope are ignored.
    ///
    /// Args:
    ///     exchange_cores: Core ids captured from one rendered exchange section.
    ///     cx: View context used to rebuild cached rows and request a repaint.
    ///
    /// Returns:
    ///     Nothing; a changed selection rebuilds the cache once, while a stale-only batch is a
    ///     no-op.
    pub(super) fn toggle_exchange_cores(
        &mut self,
        exchange_cores: Vec<CoreId>,
        cx: &mut Context<Self>,
    ) {
        let available = self
            .scope_cores(self.backend.read(cx))
            .into_iter()
            .map(|(id, _)| id)
            .collect();
        if crate::controls::toggle_exchange_cores(&mut self.sel_cores, &available, exchange_cores) {
            let backend = self.backend.clone();
            self.rebuild_cache(backend.read(cx));
            cx.notify();
        }
    }

    /// Persists the dust threshold to `layout.toml`. One value is shared by every Assets tab and
    /// window, so it has no scope key. Slider and wheel handlers call this method.
    pub(super) fn persist_min_value(&self, cx: &mut Context<Self>) {
        let v = self.min_value_usd;
        self.backend.update(cx, |b, _| {
            if b.layout.assets_min_value != Some(v) {
                b.layout.assets_min_value = Some(v);
                b.layout_dirty = true;
            }
        });
    }

    /// Handles a Core-cell click as set-to-single or clear rather than a multi-toggle. Clicking the
    /// sole selected core again resets the filter to All.
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
}
