//! Asset-row assembly: the entry types, per-core aggregation, and market/coin resolution.

use super::*;

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
        // A wallet row is a pure spot balance: it has no position, so no unrealized PnL exists.
        pnl_live: false,
    }
}

/// Sorts rows by descending [`AssetEntry::value`], placing the largest held balances first.
///
/// Equal values (the whole dust block sits at zero) break by core and coin, so the order does not
/// depend on what the slice happened to hold before — a stable sort would otherwise preserve a
/// previous header sort inside that block and make "no sort" look like a leftover one.
pub(super) fn sort_by_value(out: &mut [AssetEntry]) {
    out.sort_by(|a, b| {
        b.value
            .partial_cmp(&a.value)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.core_name.cmp(&b.core_name))
            // The SAME ticker comparison the header sorts use, so the default order and a cleared
            // sort cannot disagree on a mixed-case wallet ticker.
            .then_with(|| super::columns::cmp_coin(a, b))
    });
}

impl AssetsView {
    /// Collects `(core, uppercase coin)` pairs with a nonterminal `SellSet` or
    /// `SellAlmostDone` order, marking the corresponding table rows as currently for sale.
    /// Hyperliquid orders and catalog markets may use an indexed name such as `@151`, while transfer
    /// wallet rows expose the canonical token name. Matching by the coin extracted from
    /// `market_display` bridges those representations.
    pub(super) fn collect_sell_marked(
        &self,
        b: &Backend,
    ) -> std::collections::HashSet<(CoreId, String)> {
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
    pub(super) fn collect(&self, b: &Backend) -> Vec<AssetEntry> {
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
    pub(super) fn core_quote(&self, b: &Backend, core: CoreId) -> String {
        b.config
            .servers
            .iter()
            .find(|sv| sv.id == core)
            .map(|sv| moon_core::symbol::resolve_quote(&sv.market))
            .unwrap_or_else(|| "USDT".to_string())
    }

    /// Per-core free/total USD balances and the store-owned trust state for each figure.
    /// Missing store entries are represented as `Awaiting` so every scoped core remains visible.
    pub(super) fn per_core(&self, b: &Backend) -> Vec<CoreAgg> {
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

    /// Whether every filtered core is KNOWN to be a futures core (BaseCheck mask).
    ///
    /// Requires a snapshot from each one: before the first snapshot `futures_account` is just
    /// its `false` default, and treating unknown as "not futures" would assert "no assets" for
    /// an account whose contents are merely not loaded yet. Any missing/unloaded core, or an
    /// empty set, yields `false` — the caller then keeps the generic message.
    pub(super) fn all_scope_cores_futures(&self, b: &Backend) -> bool {
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
}
