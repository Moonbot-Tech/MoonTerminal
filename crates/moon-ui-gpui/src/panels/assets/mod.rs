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
mod table;
mod wallets;

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

/// Height of the Assets title bar, matching the Strategies tool window.
const ASSETS_HEADER_H: f32 = 32.0;

/// Core scope represented by an Assets view.
#[derive(Clone, PartialEq, Eq)]
enum AssetsScope {
    /// Group-window dock panel containing that group's cores.
    Group(String),
    /// Global window containing all connected cores.
    All,
}

/// Asset-table row associated with its core and computed USDT values.
#[derive(Clone)]
pub(super) struct AssetEntry {
    /// Row owner used by ticker navigation to Main and by trading actions.
    pub(super) core: CoreId,
    pub(super) core_name: String,
    pub(super) row: AssetRow,
    /// Raw `row.value_usdt`, used as the fixed [`sort_by_value`] key and for spot-row dust
    /// filtering. Futures position classification instead uses notional against `min_lot_usd`.
    ///
    /// NOT what the row displays: a USDT-margined futures position holds no coin balance
    /// (`feed::assets` builds `value_usdt` from `asset_balance*`), so this is ~0 for it while the
    /// position is worth its notional. Use [`Self::display_value`] for anything the user reads.
    pub(super) value: f64,
    /// The number the Value column actually shows: a position's notional
    /// (`|pos_size| * price`), otherwise [`Self::value`].
    ///
    /// Computed once during collection so the value cell and the footer's Σ use the same number;
    /// summing [`Self::value`] would understate futures rows whose coin balance is near zero.
    pub(super) display_value: f64,
    /// Whether `row.market` exists in the core's market catalog, gating the Market Sell button.
    /// A synthetic wallet row's `<coin><quote>` fallback may not exist, for example `USDTUSDC`.
    pub(super) market_exists: bool,
}

#[derive(Clone)]
pub(super) struct WalletColumnSnapshot {
    pub(super) kind: WalletKind,
    pub(super) total_count: usize,
    pub(super) rows: Vec<TransferAssetRow>,
}

/// Formats a USDT amount with spaces between thousands, `.` as the decimal mark, and a trailing
/// `$`. [`fmt::usd_grouped`] retains at most two decimal places and at least one:
/// `1 111.24$` or `1 111.0$`.
///
/// The decimal mark matches the header balance and the ticker price: the same account figure is
/// read across those surfaces, and one shared thousands separator with a differing decimal mark
/// reads as a single system contradicting itself.
pub(super) fn money(v: f64) -> String {
    let mut s = fmt::usd_grouped(v);
    s.push('$');
    s
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
    fn new(
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
                let Some((x, y, w, h)) = crate::windowing::window_geom(window) else {
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

    /// Render-gate signature for asset, transfer, sale-marker, and balance-freshness inputs.
    fn assets_sig(&self, b: &Backend) -> u64 {
        let store = b.session.store();
        self.scope_cores(b)
            .iter()
            // Include CoreId so canonical reordering invalidates the cache when state is unchanged.
            .map(|(id, _)| (*id, store.core(*id)))
            .fold(0u64, |a, (id, core)| {
                let a = a.wrapping_mul(31).wrapping_add(id);
                let Some(c) = core else {
                    return a;
                };
                a.wrapping_mul(31)
                    .wrapping_add(c.assets_rev)
                    .wrapping_mul(31)
                    .wrapping_add(c.transfer_rev)
                    .wrapping_mul(31)
                    .wrapping_add(c.orders_table_rev)
                    // Hash the rendered trust state rather than selected ingredients. Status
                    // transitions bump no data revision, but they can change `balance_state()`
                    // and must therefore invalidate the rendered balance immediately.
                    .wrapping_mul(31)
                    .wrapping_add(c.balance_state().code())
            })
    }

    /// Collects `(core, uppercase coin)` pairs with a nonterminal `SellSet` or
    /// `SellAlmostDone` order, marking the corresponding table rows as currently for sale.
    /// Hyperliquid orders and catalog markets may use an indexed name such as `@151`, while transfer
    /// wallet rows expose the canonical token name. Matching by the coin extracted from
    /// `market_display` bridges those representations.
    fn collect_sell_marked(&self, b: &Backend) -> std::collections::HashSet<(CoreId, String)> {
        let store = b.session.store();
        let mut out = std::collections::HashSet::new();
        for (id, _) in &self.cached_cores {
            let Some(cd) = store.core(*id) else { continue };
            for o in &cd.orders {
                if !o.job_is_done && matches!(o.status.as_str(), "SellSet" | "SellAlmostDone") {
                    // `market_display` resolves an indexed market such as `@N` to a display name
                    // such as `KHYPEUSDT`, from which `coin_of_market` extracts the coin normally.
                    let disp = if o.market_display.is_empty() {
                        &o.market
                    } else {
                        &o.market_display
                    };
                    out.insert((
                        *id,
                        moon_core::symbol::coin_of_market(disp).to_ascii_uppercase(),
                    ));
                }
            }
        }
        out
    }

    /// Collects asset rows from every filtered core and sorts them by descending held-balance USDT
    /// value. A positive `min_value_usd` retains spot holdings at or above the threshold and open
    /// positions at or above their minimum lot; a non-positive threshold disables filtering.
    fn collect(&self, b: &Backend) -> Vec<AssetEntry> {
        let store = b.session.store();
        // The top-bar dust threshold; a non-positive value shows every row.
        let thr = self.min_value_usd;
        let mut out = Vec::new();
        for (id, name) in self.scope_cores(b) {
            // An empty multi-core selection means every core in scope.
            if !balances::in_scope(&self.sel_cores, id) {
                continue;
            }
            let Some(cd) = store.core(id) else { continue };
            // `MOON_ASSETS_DIAG` logs the core's raw balance-position rows, distinguishing a row
            // hidden by filtering from one absent at the source.
            if std::env::var_os("MOON_ASSETS_DIAG").is_some() {
                log::error!(
                    "[assets_diag] core={name} futures_acc={} rows={}",
                    cd.assets.futures_account,
                    cd.assets.rows.len()
                );
                for r in &cd.assets.rows {
                    log::error!(
                        "[assets_diag]   market={} coin={} pos_size={} qty={} qty_full={} value={:.2} min_lot={:.2} price={}",
                        r.market,
                        r.coin,
                        r.pos_size,
                        r.qty,
                        r.qty_full,
                        r.value_usdt,
                        r.min_lot_usd,
                        r.price
                    );
                }
                // Also expose spot-wallet pricing: a zero value for an indexed asset indicates that
                // the exact-indexed and canonical-token pricing cascade both failed, causing dust
                // filtering.
                for w in &cd.transfer_assets.spot {
                    log::error!(
                        "[assets_diag]   wallet-spot currency={} total={} amount={} value={:.2}",
                        w.currency,
                        w.total,
                        w.amount,
                        w.value_usdt
                    );
                }
            }
            // Track coins already emitted from per-market rows to avoid duplicating them from the
            // spot transfer wallet below.
            let mut seen_coin: std::collections::HashSet<String> = std::collections::HashSet::new();
            for row in &cd.assets.rows {
                let row = row.clone();
                // Display the full open position, as Moonbot does. Do not subtract quantities in
                // closing sell or take-profit orders, which would hide a fully listed position.
                let value = row.value_usdt;
                // Moonbot visibility rules: futures cores, including Coin-M, show only open
                // positions whose notional reaches `min_lot_usd`, falling back to 1 USD when that
                // minimum is unknown. Their balances are quote collateral rather than purchased
                // coins. Spot cores instead show non-quote holdings whose raw value reaches the
                // user-selected `thr`; the minimum-lot fallback does not filter spot rows. A
                // non-positive threshold bypasses all filtering.
                let min_lot = if row.min_lot_usd > 0.0 {
                    row.min_lot_usd
                } else {
                    1.0
                };
                let is_position = row.pos_size != 0.0 && row.pos_size.abs() * row.price >= min_lot;
                let spot_coin_visible =
                    !cd.assets.futures_account && !row.is_quote_asset && value >= thr;
                let keep = thr <= 0.0 || is_position || spot_coin_visible;
                if !keep {
                    continue;
                }
                seen_coin.insert(row.coin.to_ascii_uppercase());
                // Same predicate the cell renderer uses (`assets_row`), NOT the dust-aware
                // `is_position` above: the displayed value and the summed value must be one
                // number, so they must also agree on what counts as a position.
                let display_value = if row.pos_size != 0.0 {
                    row.pos_size.abs() * row.price
                } else {
                    value
                };
                out.push(AssetEntry {
                    core: id,
                    core_name: name.clone(),
                    market_exists: cd.assets.markets.contains(&row.market),
                    row,
                    value,
                    display_value,
                });
            }
            // Some exchanges, including Bitget, expose purchased spot holdings only through
            // `transfer_assets`, with no corresponding per-market row. For spot accounts, turn
            // those holdings into display rows, excluding the quote asset, dust, and coins already
            // emitted above. Trading actions are enabled only when `resolve_market` finds a real
            // catalog market and sets `market_exists`.
            if !cd.assets.futures_account {
                // `base_currency` from BaseCheck is the account quote, for example USDC for a core
                // trading BTCUSDC. Its wallet balance is collateral, not a purchased coin, so hide
                // it just as per-market rows use `is_quote_asset`. Older cores with an empty value
                // fall back to the configured market's quote.
                let quote = {
                    let base = cd.assets.base_currency.trim();
                    if base.is_empty() {
                        self.core_quote(b, id)
                    } else {
                        base.to_string()
                    }
                };
                let quote_up = quote.to_ascii_uppercase();
                for w in &cd.transfer_assets.spot {
                    let coin_up = w.currency.to_ascii_uppercase();
                    if seen_coin.contains(&coin_up) {
                        continue;
                    }
                    let is_quote = coin_up == quote_up;
                    // Resolve the exchange-specific market name from the core catalog. If none
                    // exists, retain a concatenated display fallback but set `market_exists=false`
                    // so Market Sell remains hidden.
                    let resolved = resolve_market(&cd.assets.markets, &w.currency, &quote);
                    // Use the complete held wallet balance, as Moonbot does: `total` includes the
                    // free amount and quantities locked in orders, while `amount` is only free.
                    // Do not subtract listed sell quantities, or a fully listed spot holding would
                    // disappear under the dust filter. Wallet `value_usdt` already uses `total`.
                    let held_qty = w.total;
                    let held_value = w.value_usdt;
                    let keep = thr <= 0.0 || (!is_quote && held_value >= thr);
                    if !keep {
                        continue;
                    }
                    seen_coin.insert(coin_up);
                    let market_exists = resolved.is_some();
                    let market = resolved.unwrap_or_else(|| format!("{}{}", w.currency, quote));
                    let row = wallet_asset_row(w, &quote, is_quote, market, held_value, held_qty);
                    out.push(AssetEntry {
                        core: id,
                        core_name: name.clone(),
                        market_exists,
                        row,
                        value: held_value,
                        // Wallet rows carry no position, so the cell shows the held value too.
                        display_value: held_value,
                    });
                }
            }
        }
        sort_by_value(&mut out);
        out
    }

    /// Returns the quote currency resolved from the core's configured market, or `USDT` when the
    /// core is absent. Wallet-row construction uses it for `<coin><quote>` and quote-asset checks.
    fn core_quote(&self, b: &Backend, core: CoreId) -> String {
        b.config
            .servers
            .iter()
            .find(|sv| sv.id == core)
            .map(|sv| moon_core::symbol::resolve_quote(&sv.market))
            .unwrap_or_else(|| "USDT".to_string())
    }

    /// Per-core free/total USD balances and the store-owned trust state for each figure.
    /// Missing store entries are represented as `Awaiting` so every scoped core remains visible.
    fn per_core(&self, b: &Backend) -> Vec<CoreAgg> {
        let store = b.session.store();
        self.scope_cores(b)
            .into_iter()
            .map(|(id, name)| {
                let Some(cd) = store.core(id) else {
                    return CoreAgg {
                        id,
                        name,
                        free: 0.0,
                        total: 0.0,
                        state: BalanceState::Awaiting,
                    };
                };
                CoreAgg {
                    id,
                    name,
                    // The USDT balance is already computed core-side against the base currency.
                    free: cd.assets.global.free_usdt,
                    total: cd.assets.global.total_usdt,
                    // Classified by the core that owns the data, so the shell header and this
                    // panel cannot disagree about the same number.
                    state: cd.balance_state(),
                }
            })
            .collect()
    }

    /// Toggles the multi-core filter. `None` represents All: an explicit full selection collapses
    /// to the equivalent empty-means-all state; otherwise it selects every scoped core. `Some(id)`
    /// toggles one core. The filter is not persisted and reopens as All.
    pub(super) fn toggle_core(&mut self, id: Option<CoreId>, cx: &mut Context<Self>) {
        let all: HashSet<CoreId> = self
            .scope_cores(self.backend.read(cx))
            .into_iter()
            .map(|(id, _)| id)
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

    /// Cache identity: every input `collect`/`per_core` read, so a change to any of them forces
    /// a rebuild. Kept in one place because it is built at two sites (the backend observer and
    /// `rebuild_cache`) that must not drift apart.
    fn cache_key(&self, sig: u64) -> (u64, u64) {
        (sig, self.min_value_usd.to_bits())
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

    /// Rebuild all render caches from one backend snapshot.
    fn rebuild_cache(&mut self, b: &Backend) {
        let sig = self.assets_sig(b);
        // Cache membership data; the dropdown ranks again at render time.
        let cores: Vec<(CoreId, String)> = self.scope_cores(b).into_iter().collect();
        let selected_valid = self
            .selected_core
            .is_some_and(|core| cores.iter().any(|(id, _)| *id == core));
        if !selected_valid {
            self.selected_core = cores.first().map(|(id, _)| *id);
            self.cached_wallet_key = None;
        }
        self.cached_cores = cores;
        // Drop filter entries whose core is gone. Without this, deleting the one selected core
        // leaves a set that matches nothing, and "empty means all" never resumes — every
        // remaining core is filtered out and the panel reads as an empty account.
        if !self.sel_cores.is_empty() {
            self.sel_cores
                .retain(|id| self.cached_cores.iter().any(|(cid, _)| cid == id));
        }
        self.request_missing_transfers(b);
        self.sell_marked = Rc::new(self.collect_sell_marked(b));
        self.cached_entries = Rc::new(self.collect(b));
        self.cached_aggs = Rc::new(self.per_core(b));
        self.cached_all_futures = self.all_scope_cores_futures(b);
        self.rebuild_wallet_cache(b);
        // Skip non-finite row values so one bad price cannot turn the whole Σ into `NaN`, but
        // COUNT what was skipped — a silently shortened sum is indistinguishable from an honest
        // one, and the footer needs to say so.
        // Sum exactly what the rows DISPLAY, so Σ is the sum of the column above it.
        let (mut sum, mut excluded) = (0.0f64, 0usize);
        for e in self.cached_entries.iter() {
            if e.display_value.is_finite() {
                sum += e.display_value;
            } else {
                excluded += 1;
            }
        }
        self.cached_total_value = sum;
        self.cached_value_excluded = excluded;
        self.cache_sig = Some(self.cache_key(sig));
    }

    /// Whether every filtered core is KNOWN to be a futures core (BaseCheck mask).
    ///
    /// Requires a snapshot from each one: before the first snapshot `futures_account` is just
    /// its `false` default, and treating unknown as "not futures" would assert "no assets" for
    /// an account whose contents are merely not loaded yet. Any missing/unloaded core, or an
    /// empty set, yields `false` — the caller then keeps the generic message.
    fn all_scope_cores_futures(&self, b: &Backend) -> bool {
        let store = b.session.store();
        let mut seen = false;
        for (id, _) in &self.cached_cores {
            if !balances::in_scope(&self.sel_cores, *id) {
                continue;
            }
            let Some(cd) = store.core(*id) else {
                return false;
            };
            if cd.assets_rev == 0 || !cd.assets.futures_account {
                // Unknown counts as "not futures": asserting "no positions" for an account
                // whose contents merely have not loaded yet would be a guess stated as fact.
                return false;
            }
            seen = true;
        }
        seen
    }

    /// Requests transfer assets again for scoped cores that have not delivered a snapshot
    /// (`transfer_rev == 0`). The initial request in `new` may precede connection, so cache rebuilds
    /// retry until the first response. A positive revision stops retries even for an empty wallet.
    /// The upper table needs this because some exchanges expose purchased coins only via transfer
    /// assets.
    fn request_missing_transfers(&self, b: &Backend) {
        let store = b.session.store();
        for (id, _) in &self.cached_cores {
            let rev = store.core(*id).map(|cd| cd.transfer_rev).unwrap_or(0);
            if rev == 0 {
                let _ = b.session.refresh_transfer_assets(*id);
            }
        }
    }

    fn wallet_cache_key(&self, b: &Backend) -> (Option<CoreId>, u64, u64) {
        let transfer_rev = self
            .selected_core
            .and_then(|core| b.session.store().core(core).map(|cd| cd.transfer_rev))
            .unwrap_or(0);
        (
            self.selected_core,
            transfer_rev,
            self.min_value_usd.to_bits(),
        )
    }

    fn rebuild_wallet_cache(&mut self, b: &Backend) {
        let key = self.wallet_cache_key(b);
        if self.cached_wallet_key == Some(key) {
            return;
        }
        let Some(core) = key.0 else {
            self.cached_wallets = Rc::new(Vec::new());
            self.cached_wallet_key = Some(key);
            return;
        };
        let Some(cd) = b.session.store().core(core) else {
            self.cached_wallets = Rc::new(Vec::new());
            self.cached_wallet_key = Some(key);
            return;
        };
        let mut snapshots = Vec::new();
        for kind in WalletKind::ALL {
            let all_items = cd.transfer_assets.wallet(kind).to_vec();
            let total_count = all_items.len();
            let thr = self.min_value_usd;
            let mut rows: Vec<TransferAssetRow> = all_items
                .into_iter()
                .filter(|a| thr <= 0.0 || a.value_usdt > thr)
                .collect();
            rows.sort_by(|a, b| {
                b.value_usdt
                    .partial_cmp(&a.value_usdt)
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
            snapshots.push(WalletColumnSnapshot {
                kind,
                total_count,
                rows,
            });
        }
        self.cached_wallets = Rc::new(snapshots);
        self.cached_wallet_key = Some(key);
    }
}

/// Resolves a real `<coin>/<quote>` market name from the core catalog. Exchange formats differ:
/// Binance and Bitget concatenate (`BTCUSDC`), while Gate uses an underscore (`SOVRN_USDT`).
/// Returns the catalog name used by Market Sell and ticker navigation, or `None` when absent.
///
/// This deliberately does not map a canonical coin to an indexed Hyperliquid spot market such as
/// `KHYPE` to `@151`: Moonbot cannot market-sell those wallet holdings, so hiding the button is
/// correct. The for-sale badge is unaffected because [`AssetsView::collect_sell_marked`] matches
/// by coin.
fn resolve_market(
    markets: &std::collections::HashSet<String>,
    coin: &str,
    quote: &str,
) -> Option<String> {
    // Accept the coin itself as a market name; Hyperliquid spot indexes use `@699`, not `@699USDC`.
    if markets.contains(coin) {
        return Some(coin.to_string());
    }
    let concat = format!("{coin}{quote}");
    if markets.contains(&concat) {
        return Some(concat);
    }
    let under = format!("{coin}_{quote}");
    if markets.contains(&under) {
        return Some(under);
    }
    None
}

/// Builds a synthetic `AssetRow` for a spot-wallet coin absent from per-market balances. `market`
/// is either a catalog name or a concatenated display fallback; the caller separately hides Sell
/// when the fallback is not real. Price is derived from the wallet's total value and quantity.
/// The row is a pure spot balance with no position or PnL.
fn wallet_asset_row(
    w: &TransferAssetRow,
    quote: &str,
    is_quote: bool,
    market: String,
    free_value: f64,
    qty_free: f64,
) -> AssetRow {
    let price = if w.total.abs() > 0.0 {
        w.value_usdt / w.total
    } else {
        0.0
    };
    AssetRow {
        market,
        coin: w.currency.clone(),
        quote: quote.to_string(),
        listed: 1, // Spot.
        // Display the quantity supplied by the collector, currently the complete held balance so
        // quantities locked in sell orders remain visible.
        qty: qty_free,
        qty_full: w.total,
        price,
        // Use the corresponding collector-supplied value, currently the complete held value.
        value_usdt: free_value,
        min_lot_usd: 0.0,
        is_quote_asset: is_quote,
        mark_price: 0.0,
        pos_size: 0.0,
        pos_price: 0.0,
        liq_price: 0.0,
        leverage: 0,
        pnl_usdt: 0.0,
    }
}

/// Sorts rows by descending [`AssetEntry::value`], placing the largest held balances first.
pub(super) fn sort_by_value(out: &mut [AssetEntry]) {
    out.sort_by(|a, b| {
        b.value
            .partial_cmp(&a.value)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
}

impl EventEmitter<PanelEvent> for AssetsView {}
impl Focusable for AssetsView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Panel for AssetsView {
    fn panel_name(&self) -> &'static str {
        "Assets"
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
    fn dump(&self, _cx: &App) -> PanelState {
        let group = match &self.scope {
            AssetsScope::Group(g) => g.clone(),
            AssetsScope::All => String::new(),
        };
        crate::persistence::dock_persist::panel_state_with_group("Assets", &group)
    }
    fn on_added_to(
        &mut self,
        dock_area: WeakEntity<DockArea>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) {
        self.dock = Some(dock_area);
    }
    /// Builds the toolbar action that opens the singleton global Assets window for all cores. Unlike
    /// Orders detachment, this is not scoped to the current group.
    fn toolbar_buttons(
        &mut self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Vec<AnyElement>> {
        let backend = self.backend.clone();
        Some(vec![
            crate::persistence::table_persist::reset_button(
                "assets-reset-widths",
                &self.table_state,
            ),
            MoonButton::new("assets-open-global")
                .ghost()
                .size(MoonButtonSize::Action)
                .label("⧉")
                .tooltip(t!("assets.open_global_hint").to_string())
                .on_click(move |_, window, app| {
                    let owner_display = window.display(app).map(|d| d.id());
                    open(
                        backend.clone(),
                        Some(window.window_handle()),
                        owner_display,
                        app,
                    );
                })
                .render()
                .into_any_element(),
        ])
    }
}

impl Render for AssetsView {
    /// Render the always-present table and footer plus the optional window-only Wallets section.
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        crate::diag::bump(&crate::diag::ASSETS_RENDER);
        // Keep the shared Assets-view activity marker fresh. While any view renders at least once
        // per second through RenderGate, feed snapshots may publish after a one-second minimum;
        // without a visible view, the minimum interval rises to five seconds after domain events.
        moon_core::feed::note_assets_view_render();
        let cores = self.scope_cores(self.backend.read(cx));
        let entries = self.cached_entries.clone();
        let p = MoonPalette::active(cx);
        let windowed = self.windowed;

        let count = entries.len();
        // Natural table height is its header plus rows, or zero when empty. This lets the table grow
        // with content instead of stretching across a standalone window above the wallet section.
        let table_natural_h = if count == 0 {
            0.0
        } else {
            design::table_head_h(cx) + count as f32 * design::table_row_h(cx)
        };

        // The table and the footer are always present. Separate windows additionally render the
        // collapsible Wallets section, whose core list breaks the same balances down per core.
        let aggs = self.cached_aggs.clone();
        // The top bar owns filtering; the footer owns every summary figure the panel produces.
        let core_bar = self.core_bar(&cores, cx);
        let footer = self.footer(cx);
        // Standalone global and detached windows show the core list and transfer wallets. A dock tab
        // leaves them out and gives the asset table the full area.
        let wallets = self.cached_wallets.clone();
        let tree_section = self
            .show_wallets
            .then(|| self.bottom(&aggs, &wallets, cx).into_any_element());
        // Built only when it will actually be shown — a non-empty table is the common case, and
        // the message is pure dead work there. Use the position-specific copy only for a fully
        // loaded futures-only scope while the dust threshold is active; every other state keeps
        // the generic Assets copy.
        let empty_msg = if count > 0 {
            String::new()
        } else if self.cached_all_futures && self.min_value_usd > 0.0 {
            t!("assets.empty_no_positions").to_string()
        } else {
            t!("assets.empty").to_string()
        };
        let table = table::assets_table(
            "assets-table",
            entries,
            self.sell_marked.clone(),
            &self.table_state,
            empty_msg,
            cx,
        );
        // Supply the current width to the title-bar hit overlay for dragging, resizing, and controls.
        let chrome_width = match window.window_bounds() {
            WindowBounds::Windowed(bb)
            | WindowBounds::Maximized(bb)
            | WindowBounds::Fullscreen(bb) => f32::from(bb.size.width),
        };

        let mut root = v_flex()
            .id("assets-panel")
            .size_full()
            .relative()
            .min_h(px(0.0))
            .overflow_hidden()
            .track_focus(&self.focus)
            .font_family(design::mono())
            .text_size(design::t_body(cx))
            .bg(rgb(p.table_body))
            .when(windowed, |this| this.child(assets_header(p, cx)))
            .child(core_bar)
            .child(div().w_full().h(px(1.0)).flex_none().bg(rgb(p.border)));
        // The asset table is always present. With wallets it uses its natural content height so the
        // lower section can expand; in a dock tab it fills the space above the footer.
        let table_wrap = v_flex()
            .w_full()
            .min_h(px(0.0))
            .overflow_hidden()
            .child(table);
        root = root.child(if self.show_wallets {
            table_wrap.h(px(table_natural_h))
        } else {
            table_wrap.flex_1()
        });
        root = root.child(div().w_full().h(px(1.0)).flex_none().bg(rgb(p.border)));
        // In standalone views, let the wallet section consume the flexible space below the table.
        if let Some(tree) = tree_section {
            root = root
                .child(tree)
                .child(div().w_full().h(px(1.0)).flex_none().bg(rgb(p.border)));
        }
        // Footer: visible-row count and Σ on the left, scope account equity on the right.
        root = root.child(footer);
        if windowed {
            root = root.child(
                MoonWindowFrame::tool("assets-window-frame-hit", chrome_width)
                    .header_height(ASSETS_HEADER_H)
                    .leading_inset(design::titlebar_leading_inset())
                    .show_controls(design::show_custom_window_controls())
                    .hit_overlay(),
            );
        }
        root
    }
}

/// Builds the Assets window title bar with its drag cluster and optional system controls.
fn assets_header(p: MoonPalette, cx: &App) -> impl IntoElement {
    h_flex()
        .id("assets-window-header")
        .relative()
        .flex_none()
        .w_full()
        .h(design::fit_h_px(cx, ASSETS_HEADER_H, 14.0, 9.0))
        .justify_between()
        .pl(design::ui_px(cx, design::titlebar_leading_inset()))
        .pr(design::ui_px(cx, design::HEADER_PAD_X))
        .bg(rgb(p.shell_high))
        .border_b(px(1.0))
        .border_color(rgb(p.border))
        .child(
            MoonWindowFrame::tool("assets-titlebar-title", 0.0)
                .title_cluster(
                    crate::persistence::panel_meta::panel_title("Assets").to_string(),
                    cx,
                )
                .h_full()
                .flex_1()
                .min_w_0(),
        )
        .when(design::show_custom_window_controls(), |this| {
            this.child(
                MoonWindowFrame::tool("assets-window-frame-visual", 0.0)
                    .header_height(ASSETS_HEADER_H)
                    .show_controls(true)
                    .visual_controls(cx),
            )
        })
}

/// Opens the singleton secondary Assets tool window covering all cores. `Backend.assets_window`
/// provides deduplication and focus of an existing window.
pub fn open(
    backend: Entity<Backend>,
    owner: Option<AnyWindowHandle>,
    owner_display: Option<DisplayId>,
    cx: &mut App,
) {
    // Focus the existing singleton instead of opening a duplicate.
    if let Some(handle) = backend.read(cx).assets_window {
        if handle
            .update(cx, |_, window, _| window.activate_window())
            .is_ok()
        {
            return;
        }
    }
    let saved = backend.read(cx).layout.assets_window;
    let bounds = saved.map_or(
        Bounds {
            origin: point(px(140.0), px(110.0)),
            size: size(px(1180.0), px(720.0)),
        },
        |g| Bounds {
            origin: point(px(g.x as f32), px(g.y as f32)),
            size: size(px(g.w as f32), px(g.h as f32)),
        },
    );
    // Choose a display from the saved origin where supported, otherwise from the owner window.
    let display_id = crate::windowing::saved_or_owner_display_id(
        saved.map(|g| point(px(g.x as f32), px(g.y as f32))),
        owner,
        owner_display,
        cx,
    );
    let mut opts = crate::windowing::tool_window_options(
        t!("assets.window_title").to_string(),
        WindowBounds::Windowed(bounds),
        Some(size(px(900.0), px(560.0))),
        owner,
    );
    opts.display_id = display_id;
    let b = backend.clone();
    if let Ok(handle) = cx.open_window(opts, move |window, cx| {
        crate::windowing::configure_shell_clear_color(window, cx);
        let view = cx.new(|cx| AssetsView::new(b, AssetsScope::All, true, true, window, cx));
        cx.new(|cx| Root::new(view, window, cx).background_policy(MoonBackgroundPolicy::Opaque))
    }) {
        backend.update(cx, |bk, _| bk.assets_window = Some(handle));
        crate::windowing::activate_new_window(handle.into(), cx);
    }
}
