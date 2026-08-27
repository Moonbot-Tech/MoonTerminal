//! Assets panel/window. Top: core selector and dust threshold; then the positions/balances table
//! across every in-scope core (values and totals in USDT); then a footer carrying both summaries
//! — visible-row count and Σ on the left, the scope's account equity on the right.
//! Bottom (global window, detached Classic window, and Auto dock tab): the core list on the left
//! (free/total) and three wallet containers (Spot/Futures/Quarterly) on the right — dragging a
//! coin between them opens a quantity dialog (defaulting to the whole free amount) and performs
//! the transfer. Classic dock tabs omit this section; Auto cannot detach, so the docked tab is
//! the window form.
//!
//! The same `AssetsView` lives in two shapes:
//! - as a dock panel inside a group window (`AssetsScope::Group`) — that group's cores. Classic
//!   dock tabs are table-only; Auto dock tabs show wallets because Auto cannot detach;
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
//! the footer in [`table`]; the selectable fields, their persistence and the header sort in
//! [`columns`]; balance aggregation and its trust-aware rendering in [`balances`];
//! the 3 wallet containers and the drag&drop transfer dialog in [`wallets`].

mod balances;
mod cache;
mod collect;
mod columns;
mod render;
mod settings;
mod roster_width;
mod table;
#[cfg(test)]
mod tests;
mod wallets;
mod window;

use collect::{AssetEntry, WalletColumnSnapshot, money};
use columns::AssetCol;
pub use window::open;
use window::{ASSETS_HEADER_H, assets_header};

use std::collections::HashSet;
use std::rc::Rc;

use gpui::prelude::FluentBuilder;
use gpui::*;
use moon_ui::{
    DockArea, MoonBackgroundPolicy, MoonButton, MoonButtonSize, MoonButtonVariant, MoonDataCell,
    MoonDataRow, MoonDataTable, MoonDataTableColumn, MoonDataTableState, MoonDropdown, MoonInput,
    MoonInputState, MoonMenuItem, MoonMenuSize, MoonPalette, MoonSlider, MoonSliderEvent,
    MoonSliderState, MoonTone, MoonWindowFrame, Panel, PanelEvent, PanelState, Root, h_flex,
    v_flex,
};

use crate::Backend;
use crate::core_order::{CoreOrder, OrderedCores};
use crate::design;
use crate::panels::{RenderGate, num};
use crate::workspace::{EffectiveCoreScope, RetainedCoreScope};
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
    /// Whether this host always shows the lower transfer area (global and detached windows).
    /// Auto dock tabs compute the same visibility from workspace mode so Classic tabs stay compact.
    show_wallets: bool,
    /// Auto Overview local wallet host. Independent of Classic [`Self::selected_core`] so a
    /// mode switch cannot leak a Classic click into Overview transfer.
    overview_wallet_pick: Option<CoreId>,
    /// Core selected for the lower wallet containers.
    pub(super) selected_core: Option<CoreId>,
    /// Hide asset rows worth less than this USDT threshold while retaining open positions whose
    /// notional reaches the market's minimum lot. A non-positive threshold shows every row.
    pub(super) min_value_usd: f64,
    /// Top-bar threshold slider state, ranging from 0 through 100 USD in steps of 1, defaulting to 1.
    min_value_slider: Entity<MoonSliderState>,
    /// Retained core filter for Classic group views and the global aggregate view; empty means all
    /// scoped cores. Group Auto pins its effective workspace scope without mutating this set.
    pub(super) sel_cores: HashSet<CoreId>,
    /// Whether the core list and Spot, Futures, and Quarterly wallet section is collapsed.
    pub(super) wallets_collapsed: bool,
    /// Whether that settings popup is showing. Process-lifetime state, never persisted.
    pub(super) wallet_settings_open: bool,
    /// UI-thread edge proving exchange logos were prewarmed off the render path.
    pub(super) exchange_logos_ready: bool,
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
    /// Fields hidden by the column selector, persisted per context through
    /// [`crate::persistence::table_persist`]. Empty means every field is shown; the action buttons
    /// are a field like any other and can be hidden too.
    hidden_cols: Vec<AssetCol>,
    /// Active header sort as `(column, ascending)`. `None` keeps the default order — largest value
    /// first. Applied while rebuilding the cache, never during a repaint.
    sort: Option<(AssetCol, bool)>,
    /// Asset-table column widths and sorting state. Both persist through
    /// [`crate::persistence::table_persist`].
    table_state: Entity<MoonDataTableState>,
    /// Contextual width-storage ID: `assets-table:dock` for a dock tab and `assets-table:win` for
    /// standalone or detached views with wallets. Each mode retains independent widths.
    widths_id: String,
    /// Roster column's own width bag — NOT a `MoonDataTable`, just a single-entry width store
    /// reusing `MoonDataTableState` the way `core_status`'s By-IP width bag does
    /// (`panels/core_status/mod.rs`'s `by_ip_col_widths`). Persists through
    /// [`crate::persistence::table_persist`] under [`Self::roster_widths_id`].
    roster_widths: Entity<MoonDataTableState>,
    /// Contextual width-storage ID for the roster bag: `assets-roster:dock` for a dock tab,
    /// `assets-roster:win` for standalone or detached views with wallets. The discriminator is
    /// the CONSTRUCTOR's `show_wallets` argument, not the render-time [`Self::wallets_visible`]:
    /// `restored_group` always passes `false`, so an Auto dock tab — which `wallets_visible`
    /// makes show the section anyway — keeps its own `:dock` roster width rather than sharing the
    /// detached/global `:win` one. Same behaviour [`Self::widths_id`] already has.
    roster_widths_id: String,
    /// Live roster-column divider drag, or `None` between drags.
    roster_drag: Option<table::RosterDragAnchor>,
    dock: Option<WeakEntity<DockArea>>,
    focus: FocusHandle,
}

impl crate::controls::CoreComboHost for AssetsView {
    /// Group Auto owns the effective scope and leaves the retained selection untouched.
    fn core_selection_pinned(&self, cx: &App) -> bool {
        self.effective_scope(self.backend.read(cx))
            .is_some_and(|scope| scope.is_workspace_owned())
    }

    /// Return the retained Classic or global core filter for shared picker edits.
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

        // Only the global standalone window owns persisted geometry; a dock panel uses its group window.
        if windowed {
            cx.observe_window_bounds(window, |this, window, cx| {
                let geom = crate::window::windowing::window_geom_rect(window, cx);
                this.backend.update(cx, |b, _| {
                    let geom = geom.keeping_display_of(b.layout.assets_window);
                    if b.layout.assets_window != Some(geom) {
                        b.layout.assets_window = Some(geom);
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

        // Roster column's own width bag, mirroring `table_state` above but for the single-entry
        // roster geometry (`roster_width::WIDTH_KEY`) instead of a `MoonDataTable`'s columns.
        let roster_widths_id =
            crate::persistence::table_persist::ctx_id("assets-roster", show_wallets);
        let saved_roster =
            crate::persistence::table_persist::saved(backend.read(cx), &roster_widths_id);
        let roster_widths = cx.new(|_| {
            let mut s = MoonDataTableState::new();
            s.column_widths = saved_roster;
            s
        });
        cx.observe(&roster_widths, |this, state, cx| {
            crate::persistence::table_persist::persist(
                &this.backend,
                &this.roster_widths_id,
                &state,
                cx,
            );
            // Mandatory: nothing else observes this bag -- `bottom()` reads it during THIS view's
            // render, so without the notify a drag or a reset paints on some unrelated repaint.
            cx.notify();
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

        let mut this = Self {
            backend,
            scope,
            windowed,
            show_wallets,
            overview_wallet_pick: None,
            selected_core: None,
            min_value_usd,
            min_value_slider,
            sel_cores: HashSet::new(),
            wallets_collapsed: false,
            wallet_settings_open: false,
            exchange_logos_ready: false,
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
            hidden_cols: Vec::new(),
            sort: None,
            table_state,
            widths_id,
            roster_widths,
            roster_widths_id,
            roster_drag: None,
            dock: None,
            focus: cx.focus_handle(),
        };
        // Request transfer assets from every scoped core. Spot wallets feed both the selected
        // core's lower containers and the upper table because some exchanges, including Bitget,
        // expose purchased coins only through `transfer_assets`, not per-market balances.
        let all_cores: Vec<CoreId> = this
            .scope_cores(this.backend.read(cx))
            .into_iter()
            .map(|(id, _)| id)
            .collect();
        this.selected_core = all_cores.first().copied();
        let query_cores: Vec<CoreId> = this
            .query_cores(this.backend.read(cx))
            .into_iter()
            .map(|(id, _)| id)
            .collect();
        // Restore the persisted field set before the first cache build so the initial frame already
        // renders the user's columns.
        this.apply_ctx_columns(cx);
        this.apply_ctx_sort(cx);
        for core in &query_cores {
            if let Err(error) = this.backend.read(cx).session.refresh_transfer_assets(*core) {
                log::warn!(
                    "assets initial refresh failed for core {}: {error}",
                    moon_core::feed::core_label(*core)
                );
            }
        }
        // A group panel created while a filter is on air joins it, so a detached or restored tab is
        // not the one surface still showing every core.
        this.adopt_broadcast_core_filter(cx);
        let backend_for_initial_cache = this.backend.clone();
        this.rebuild_cache(backend_for_initial_cache.read(cx));
        this
    }

    /// Restores a group-scoped dock tab from `docks.json`.
    ///
    /// Classic keeps this table-only. Auto shows wallets at render time because it cannot detach.
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

    /// Resolve workspace ownership for a group view while leaving the global Assets window alone.
    ///
    /// Args:
    ///     b: Backend snapshot containing workspace authority and live group membership.
    ///
    /// Returns:
    ///     Effective group scope, or `None` for the deliberately aggregate global view.
    pub(super) fn effective_scope(&self, b: &Backend) -> Option<EffectiveCoreScope> {
        let AssetsScope::Group(group) = &self.scope else {
            return None;
        };
        let retained: Vec<CoreId> = self.sel_cores.iter().copied().collect();
        let retained = if retained.is_empty() {
            RetainedCoreScope::All
        } else {
            RetainedCoreScope::Explicit(&retained)
        };
        Some(b.effective_workspace_scope(group, retained))
    }

    /// Return the exact core/name pairs used by Assets queries and render caches.
    ///
    /// Args:
    ///     b: Backend snapshot containing configured cores and workspace authority.
    ///
    /// Returns:
    ///     Canonically ordered effective core/name pairs, or every core for global Assets.
    pub(super) fn query_cores(&self, b: &Backend) -> Vec<(CoreId, String)> {
        let all: Vec<(CoreId, String)> = self.scope_cores(b).into_iter().collect();
        let Some(scope) = self.effective_scope(b) else {
            return all;
        };
        all.into_iter()
            .filter(|(core, _)| scope.contains(*core))
            .collect()
    }

    /// Return the wallet-detail core visible under the effective workspace scope.
    ///
    /// Auto Overview still needs a concrete transfer host so the window-form wallets stay usable.
    /// That host is the Overview list pick when it remains in scope, otherwise the first in-scope
    /// core. Classic retained `selected_core` is never consulted here: `resolve_workspace_wallet_core`
    /// keeps Overview as absence so a mode switch cannot leak a Classic click.
    ///
    /// Args:
    ///     b: Backend snapshot containing workspace authority.
    ///
    /// Returns:
    ///     Auto's selected core, Auto Overview's local host, or the retained Classic core.
    pub(super) fn effective_wallet_core(&self, b: &Backend) -> Option<CoreId> {
        let scope = self.effective_scope(b);
        let workspace_owned = scope
            .as_ref()
            .is_some_and(EffectiveCoreScope::is_workspace_owned);
        let workspace_core = scope.as_ref().and_then(|scope| match scope.label() {
            crate::workspace::EffectiveScopeLabel::Core(core) => Some(core),
            crate::workspace::EffectiveScopeLabel::All
            | crate::workspace::EffectiveScopeLabel::Selection(_)
            | crate::workspace::EffectiveScopeLabel::Overview => None,
        });
        if let Some(core) =
            resolve_workspace_wallet_core(workspace_owned, workspace_core, self.selected_core)
        {
            return Some(core);
        }
        if workspace_owned {
            if let Some(scope) = scope.as_ref() {
                let ids = scope.ids();
                return self
                    .overview_wallet_pick
                    .filter(|core| ids.contains(core))
                    .or_else(|| ids.first().copied());
            }
        }
        None
    }

    /// Return whether this view should render the core list and transfer wallets.
    ///
    /// Global and detached hosts always show them. An Auto group dock tab shows them too, because
    /// Auto refuses detached windows; a Classic dock tab stays table-only.
    ///
    /// Args:
    ///     cx: Application context used to read the group's workspace mode.
    ///
    /// Returns:
    ///     `true` when the lower transfer section belongs on this host.
    pub(super) fn wallets_visible(&self, cx: &App) -> bool {
        if self.show_wallets {
            return true;
        }
        match &self.scope {
            AssetsScope::Group(group) => {
                self.backend.read(cx).workspace_mode(group)
                    == moon_core::config::WorkspaceMode::AutoTrading
            }
            AssetsScope::All => true,
        }
    }

    /// Toggles the multi-core filter. `None` represents All and clears the explicit selection back
    /// to the empty-means-all state. `Some(id)` toggles one core. The filter is not persisted and
    /// reopens as All. Group Auto owns the effective scope, so this method leaves the retained
    /// Classic selection unchanged.
    ///
    /// Args:
    ///     id: Core to toggle, or `None` for the All row.
    ///     cx: View context used to rebuild cached rows and request a repaint.
    ///
    /// Returns:
    ///     Nothing; Classic or global mode updates the filter, while group Auto is a no-op.
    pub(super) fn toggle_core(&mut self, id: Option<CoreId>, cx: &mut Context<Self>) {
        if self
            .effective_scope(self.backend.read(cx))
            .is_some_and(|scope| scope.is_workspace_owned())
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

    /// Toggle one exchange section in the retained Classic or global Assets filter.
    ///
    /// Empty means All before the click, so the first exchange selection becomes explicit. A
    /// fully selected exchange is removed without changing selections from other exchanges.
    /// Rendered ids that left the current group or global scope are ignored. Group Auto leaves the
    /// retained Classic selection unchanged.
    ///
    /// Args:
    ///     exchange_cores: Core ids captured from one rendered exchange section.
    ///     cx: View context used to rebuild cached rows and request a repaint.
    ///
    /// Returns:
    ///     Nothing; a retained-scope change rebuilds once, while stale-only and group Auto calls
    ///     are no-ops.
    pub(super) fn toggle_exchange_cores(
        &mut self,
        exchange_cores: Vec<CoreId>,
        cx: &mut Context<Self>,
    ) {
        if self
            .effective_scope(self.backend.read(cx))
            .is_some_and(|scope| scope.is_workspace_owned())
        {
            return;
        }
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

    /// Handle a Core-cell click under global, Classic, or Auto authority.
    ///
    /// Global and Classic group views set the retained filter to the clicked core, or clear it when
    /// that core is already the sole selection. Group Auto ignores the shortcut because only the
    /// Shell rail owns selection.
    ///
    /// Args:
    ///     id: Core identity from the clicked asset row.
    ///     cx: View context used to update the owning authority and visible cache.
    ///
    /// Returns:
    ///     Nothing.
    pub(super) fn filter_to_core(&mut self, id: CoreId, cx: &mut Context<Self>) {
        if self
            .effective_scope(self.backend.read(cx))
            .is_some_and(|scope| scope.is_workspace_owned())
        {
            return;
        }
        self.sel_cores = crate::controls::next_core_filter(&self.sel_cores, &[id], false);
        let backend = self.backend.clone();
        self.rebuild_cache(backend.read(cx));
        cx.notify();
    }

    /// Replace the retained filter with the one the Profit Monitor broadcast.
    ///
    /// Only a GROUP view adopts. The global Assets window is an application-wide aggregate by
    /// design — it has no workspace scope at all, which is the fact this reads — and the broadcast
    /// is a main-window gesture; narrowing that window from another one would take away the only
    /// surface that answers "everything I hold, everywhere".
    ///
    /// The intersection matters more here than anywhere else: `rebuild_cache` prunes the retained
    /// set against this scope on the very next line, and a set pruned empty means ALL cores — so a
    /// raw foreign id would WIDEN Assets while narrowing its neighbours.
    ///
    /// Args:
    ///     cx: View context used to rebuild cached rows and request a repaint.
    ///
    /// Returns:
    ///     Nothing; the global window, a broadcast about other groups, and an unchanged selection
    ///     all rebuild nothing.
    fn adopt_broadcast_core_filter(&mut self, cx: &mut Context<Self>) {
        let broadcast = self.backend.read(cx).core_filter().clone();
        // Nothing published and nothing retained: leave before paying for the scope's core list.
        if broadcast.is_empty() && self.sel_cores.is_empty() {
            return;
        }
        // An absent scope IS the global window — the same authority that makes it an
        // application-wide aggregate everywhere else, rather than a second test for the same fact.
        // `is_workspace_owned` reads the group's mode, not the retained set, so one resolve before
        // the write answers both questions.
        let Some(scope) = self.effective_scope(self.backend.read(cx)) else {
            return;
        };
        let available: Vec<CoreId> = self
            .scope_cores(self.backend.read(cx))
            .into_iter()
            .map(|(id, _)| id)
            .collect();
        if !crate::controls::apply_core_broadcast(&mut self.sel_cores, &broadcast, available) {
            return;
        }
        if scope.is_workspace_owned() {
            return;
        }
        let backend = self.backend.clone();
        self.rebuild_cache(backend.read(cx));
        cx.notify();
    }
}

/// Reconcile retained Classic Assets state only against the full configured/live scope.
///
/// Args:
///     valid: Full configured/live scope, never the narrower effective Auto query scope.
///     effective: Current query scope, supplied explicitly to prevent the two domains being
///         accidentally conflated during future cache refactors.
///     selected_filter: Retained Classic multi-core filter to prune for removed cores.
///     selected_wallet: Retained Classic wallet/detail core to repair when removed.
///
/// Returns:
///     `true` when the retained wallet/detail core changed and its cache must be rebuilt.
fn reconcile_retained_assets_state(
    valid: &[CoreId],
    effective: &[CoreId],
    selected_filter: &mut HashSet<CoreId>,
    selected_wallet: &mut Option<CoreId>,
) -> bool {
    debug_assert!(effective.iter().all(|core| valid.contains(core)));
    // Auto's one-core `effective` slice is not evidence that any retained Classic selection was
    // removed from the configured/live universe.
    selected_filter.retain(|core| valid.contains(core));
    if selected_wallet.is_some_and(|core| valid.contains(&core)) {
        return false;
    }
    let next = valid.first().copied();
    let changed = *selected_wallet != next;
    *selected_wallet = next;
    changed
}

/// Resolve Auto's temporary wallet target without overwriting the retained Classic detail core.
///
/// Args:
///     workspace_owned: Whether Auto currently owns the group panel's core selection.
///     workspace_core: Selected Auto core, or `None` for Auto Overview.
///     retained_core: User-selected Classic wallet/detail core.
///
/// Returns:
///     Auto's exact selected core or Overview absence; otherwise the retained Classic core.
fn resolve_workspace_wallet_core(
    workspace_owned: bool,
    workspace_core: Option<CoreId>,
    retained_core: Option<CoreId>,
) -> Option<CoreId> {
    if workspace_owned {
        workspace_core
    } else {
        retained_core
    }
}
