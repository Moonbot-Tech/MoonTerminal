//! Feed-side assets: decouples moonproto (markets/balances/transfer_assets)
//! into the `AssetsSnapshot` / `TransferAssetsSnapshot` domain snapshots. Moonproto remains
//! inside the feed layer; the store/UI receive only domain structures.

use moonproto::state::{BalancesState, ExchangeKind, MarketsState, TransferAssetsState};
use moonproto::{BaseCurrency, OrderType};

use super::{
    AssetRow, AssetsSnapshot, GlobalBalanceRow, TransferAssetRow, TransferAssetsSnapshot,
    WalletKind,
};

/// USD stablecoins whose exchange rate to USDT is treated as 1.
fn is_stable(q: &str) -> bool {
    matches!(
        q,
        "USDT" | "USDC" | "BUSD" | "USD" | "FDUSD" | "TUSD" | "DAI" | "USDP"
    )
}

/// Exchange rate of the `quote` currency in USDT (USDT≈1, BTC≈the BTC/USDT rate).
/// Uses the core's standard `base_currency_price`; falls back to 1 for stablecoins, otherwise 0.
///
/// An EMPTY `quote` denotes a USD-denominated contract (Binance COIN-M: `BTCUSD_PERP`,
/// `ETHUSD_260925` are quoted in USD with margin in the coin itself). The USD≈USDT rate is 1;
/// otherwise `p_last` (already the coin's USD price) would be multiplied by 0 and erase the value.
fn quote_to_usdt(markets: &MarketsState, quote: &str) -> f64 {
    let q = quote.to_ascii_uppercase();
    if quote.trim().is_empty() {
        return 1.0;
    }
    markets
        .base_currency_price(quote)
        .map(|b| b.last_price)
        .filter(|r| *r > 0.0)
        .unwrap_or_else(|| if is_stable(&q) { 1.0 } else { 0.0 })
}

/// Exchange rate of the account's base currency (`base`) in USDT. USDT/stablecoins use 1;
/// otherwise searches for the `<base>USDT` market or `base_currency_price`. 0 means unknown.
fn base_rate(markets: &MarketsState, base: &str) -> f64 {
    let b = base.to_ascii_uppercase();
    if is_stable(&b) {
        return 1.0;
    }
    markets
        .price(&format!("{b}USDT"))
        .map(|p| p.p_last)
        .filter(|x| *x > 0.0)
        .or_else(|| {
            markets
                .base_currency_price(base)
                .map(|bc| bc.last_price)
                .filter(|x| *x > 0.0)
        })
        .unwrap_or(0.0)
}

/// USDT value of `qty` units of `currency`. Stablecoins pass through unchanged; otherwise the
/// rate comes from the exact currency-named market, `<CUR>USDT`, `<CUR>USDC` (≈USD), any
/// USD-denominated contract with the `<CUR>USD` prefix, then the canonical-coin `coin_px`
/// fallback. COIN-M/quarterly cores have no `<CUR>USDT` market, but contract prices such as
/// `BTCUSD_PERP`/`BTCUSD_260925` are already in USD. 0 means unknown.
fn coin_to_usdt(
    markets: &MarketsState,
    currency: &str,
    qty: f64,
    coin_px: &std::collections::HashMap<String, f64>,
) -> f64 {
    let cur = currency.to_ascii_uppercase();
    if is_stable(&cur) {
        return qty;
    }
    let px = markets
        // The market can be the coin name ITSELF: Hyperliquid spot indexes such as `@699` use
        // that exact name, and no `@699USDT`/`@699USDC` concatenation exists. Without this check,
        // the wallet would be valued at 0 and hidden by the dust filter. Try it first using the
        // original name without uppercasing.
        .price(currency)
        .map(|p| p.p_last)
        .filter(|x| *x > 0.0)
        .or_else(|| {
            markets
                .price(&format!("{cur}USDT"))
                .map(|p| p.p_last)
                .filter(|x| *x > 0.0)
        })
        .or_else(|| {
            markets
                .price(&format!("{cur}USDC"))
                .map(|p| p.p_last)
                .filter(|x| *x > 0.0)
        })
        .or_else(|| {
            let prefix = format!("{cur}USD");
            markets
                .iter()
                .filter(|h| h.name().starts_with(&prefix))
                .map(|h| h.price().p_last)
                .find(|x| *x > 0.0)
        })
        // Hyperliquid spot: the token market is named by index (`@206`), while the wallet uses
        // the name (`UENA`). The `coin_px` index maps a market's base coin to its price, covering
        // this case (`@206` market price = UENA price, with USDC≈USD as the quote).
        .or_else(|| coin_px.get(&cur).copied().filter(|x| *x > 0.0));
    px.map(|px| qty * px).unwrap_or(0.0)
}

/// Converts a domain wallet to moonproto `ExchangeKind`.
pub(super) fn to_exchange_kind(w: WalletKind) -> ExchangeKind {
    match w {
        WalletKind::Spot => ExchangeKind::Spot,
        WalletKind::Futures => ExchangeKind::Futures,
        WalletKind::Quarterly => ExchangeKind::Quarterly,
    }
}

/// Build one core's asset snapshot from nonempty market balances and positions.
///
/// Empty markets are omitted, while dust filtering remains a UI concern. The global row also
/// records whether its published free/total USD valuation is complete and finite.
pub(super) fn build_assets(
    markets: &MarketsState,
    balances: &BalancesState,
    base_currency: &str,
    futures_account: bool,
) -> AssetsSnapshot {
    let mut rows = Vec::new();
    let mut leverage = std::collections::HashMap::new();
    // Core market-name catalog for gating the UI's `Market sell` button. Sell resolution accepts
    // the exact row market plus the `<coin><quote>` and `<coin>_<quote>` forms.
    let market_names: std::collections::HashSet<String> =
        markets.iter().map(|h| h.name().to_string()).collect();
    // COIN-M / quarterly: the wallet is denominated in the coin itself (BTC/ETH/…), not USDT,
    // and the exchange duplicates the SAME coin balance across all its contracts (PERP plus all
    // expirations). Calculate equity as the sum over UNIQUE coins (deduplicated), or BTC would
    // be counted three times. `_full` is the complete wallet; the plain field is the free amount.
    let mut seen_coin_wallet: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut coin_wallet_full_usdt = 0.0f64;
    let mut coin_wallet_free_usdt = 0.0f64;
    // Coin-wallet equity is usable only when every held coin contributes a finite quantity at
    // a finite positive USD price; otherwise a partial sum could masquerade as the full equity.
    let mut coin_wallet_all_priced = true;
    for h in markets.iter() {
        let bp = h.balance_position();
        let lev = h.with(|m| m.leverage_x);
        let empty = bp.asset_balance == 0.0
            && bp.asset_balance_full == 0.0
            && bp.pos_size == 0.0
            && bp.long_pos_size == 0.0
            && bp.short_pos_size == 0.0;
        // Per-core leverage map: markets with a position/balance OR actual leverage (>1). Do NOT
        // store the default 1 without account data: leverage is unknown there (the core resets it
        // to 1), so the UI should display `—`.
        if lev > 0 && (!empty || lev > 1) {
            leverage.insert(h.name().to_string(), lev);
        }
        if empty {
            continue;
        }
        let market = h.name().to_string();
        let price = h.price();
        // coin = canonical token (falling back to market_currency); quote = base_currency;
        // expose listed as `Market::listed_type()` would (SPOT when futures_type=EMPTY,
        // otherwise BOTH), because moonproto does not re-export `ListedType` itself.
        let (coin, quote, listed) = h.with(|m| {
            let canon = m.market_currency_canonic.trim();
            let coin = if canon.is_empty() {
                m.market_currency.clone()
            } else {
                canon.to_string()
            };
            let listed = if m.futures_type == BaseCurrency::EMPTY {
                1u8
            } else {
                3u8
            };
            (coin, m.base_currency.clone(), listed)
        });
        let rate = quote_to_usdt(markets, &quote);
        // ROW value uses the FULL held balance (free plus locked in open sell orders), as Moonbot
        // does: an open position holds the entire quantity in TP orders (free≈0), but it remains
        // our asset and must not be hidden (a row valued at 0 would fall below the dust filter).
        // The exchange may omit `_full`, so use max(full, free).
        let held_qty = if bp.asset_balance_full.abs() > bp.asset_balance.abs() {
            bp.asset_balance_full
        } else {
            bp.asset_balance
        };
        let value_usdt = held_qty.abs() * price.p_last * rate;
        // Deduplicate coin wallets (COIN-M): sum EACH coin's value once regardless of its number
        // of contracts. This is ACCOUNT equity, so calculate it from the FULL balance. Include
        // only an actual coin balance (asset_balance*), NOT position-only rows: a position without
        // a balance has no coin in the wallet and is a USDT-margined derivative.
        let qty_full_for_equity = if bp.asset_balance_full.abs() > bp.asset_balance.abs() {
            bp.asset_balance_full
        } else {
            bp.asset_balance
        };
        let has_coin_balance = bp.asset_balance != 0.0 || bp.asset_balance_full != 0.0;
        if has_coin_balance && seen_coin_wallet.insert(coin.clone()) {
            let coin_px_usdt = price.p_last * rate;
            let qty_full = qty_full_for_equity.abs();
            let qty_free = bp.asset_balance.abs();
            let (add_full, add_free) = (qty_full * coin_px_usdt, qty_free * coin_px_usdt);
            // Quantities also come from the wire, so validate the products as well as the price:
            // `NaN` would poison the sum and overflow would publish an infinite balance. A bad
            // nonzero holding is excluded and makes the coin-wallet valuation incomplete.
            if coin_px_usdt.is_finite()
                && coin_px_usdt > 0.0
                && add_full.is_finite()
                && add_free.is_finite()
            {
                coin_wallet_full_usdt += add_full;
                coin_wallet_free_usdt += add_free;
            } else if qty_full != 0.0 || qty_free != 0.0 {
                coin_wallet_all_priced = false;
            }
        }
        // The core's min_lot_size is already in quote currency:
        // max(step, min_qty)·price and min_notional.
        let min_lot_usd = price.min_lot_size * rate;
        let is_quote_asset = coin.eq_ignore_ascii_case(base_currency.trim());
        // LIVE position PnL: (price − entry) × size × direction, converted from quote currency
        // to USDT. The mark price is more accurate for futures PnL; fall back to mid. Hedge legs
        // take precedence over the net position. Do NOT use server total_profit_* for positions:
        // those values accumulate over a period and remain frozen between balance pushes.
        let mark = if price.mark_price > 0.0 {
            price.mark_price
        } else {
            price.p_last
        };
        let mut live_pnl = 0.0;
        let mut have_position_pnl = false;
        // A leg that exists but carries no entry price contributes nothing, so the sum covers only
        // part of the position — a half-truth that must not be published as the live figure (see
        // `pnl_live` below, and `AssetRow::pnl_live` for what a consumer does with it).
        let mut legs_complete = true;
        if mark > 0.0 {
            if bp.long_pos_size != 0.0 {
                if bp.long_pos_price > 0.0 {
                    live_pnl += (mark - bp.long_pos_price) * bp.long_pos_size.abs();
                    have_position_pnl = true;
                } else {
                    legs_complete = false;
                }
            }
            if bp.short_pos_size != 0.0 {
                if bp.short_pos_price > 0.0 {
                    live_pnl += (bp.short_pos_price - mark) * bp.short_pos_size.abs();
                    have_position_pnl = true;
                } else {
                    legs_complete = false;
                }
            }
            if !have_position_pnl && bp.pos_size != 0.0 && bp.pos_price > 0.0 {
                let short = bp.pos_size < 0.0 || bp.pos_dir == OrderType::Sell;
                let dir = if short { -1.0 } else { 1.0 };
                live_pnl = (mark - bp.pos_price) * bp.pos_size.abs() * dir;
                have_position_pnl = true;
                // The net position covers the whole market, legs included, so an unpriced leg above
                // no longer leaves anything out.
                legs_complete = true;
            }
        }
        let pnl_usdt = if have_position_pnl {
            live_pnl * rate
        } else {
            // No position/entry price (a spot balance without position data): fall back to the
            // server's accumulated market profit in quote currency.
            (bp.total_profit_b + bp.total_profit_l + bp.total_profit_s) * rate
        };
        // Only the live branch, covering EVERY leg, converted at a KNOWN rate, is an unrealized
        // figure a display may present as such. An unknown rate (`0`) turns any PnL into a finite
        // `0.0` that reads as a flat position, and a hedge whose second leg has no entry price
        // yields half the truth — neither qualifies.
        let pnl_live = have_position_pnl && legs_complete && rate > 0.0 && pnl_usdt.is_finite();
        // Row position size/price: use net `pos_size`; if the core keeps legs SEPARATELY
        // (hedge mode, or the server stores the short in `short_pos_size` while net = 0), use
        // the net of the legs (short is negative). Otherwise a real futures short with
        // `pos_size=0` fails the UI's `is_position` check and disappears from Assets (a
        // derivative has no coin balance).
        let (pos_size, pos_price) = if bp.pos_size != 0.0 {
            (bp.pos_size, bp.pos_price)
        } else if bp.short_pos_size.abs() > bp.long_pos_size.abs() {
            (-bp.short_pos_size.abs(), bp.short_pos_price)
        } else if bp.long_pos_size != 0.0 {
            (bp.long_pos_size.abs(), bp.long_pos_price)
        } else {
            (bp.pos_size, bp.pos_price)
        };
        rows.push(AssetRow {
            market,
            coin,
            quote,
            listed,
            qty: bp.asset_balance,
            qty_full: bp.asset_balance_full,
            price: price.p_last,
            value_usdt,
            min_lot_usd,
            is_quote_asset,
            mark_price: price.mark_price,
            pos_size,
            pos_price,
            liq_price: bp.liq_price,
            leverage: lev,
            pnl_usdt,
            pnl_live,
        });
    }
    let g = balances.global();
    // Historically, `btc_balance_*` is expressed in the account's BASE currency (already USDT
    // for a USDT bot at rate 1; BTC for a BTC bot at the BTCUSDT rate). Derive the rate from the
    // server's base currency.
    let rate = base_rate(markets, base_currency);
    // Read total/free from global. Fall back to `btc_total` when `btc_full` is absent: for some
    // spot accounts (for example, Binance spot USDC), the exchange does not publish `full`, and
    // the entire balance is in `available` (`btc_total`); otherwise the core card would show 0.
    // A CORRUPT `btc_full` is not an absent one. The fallback above exists for accounts that
    // never publish the field — a clean `0.0` — and `NaN.abs() > 1e-9` is false, so without this
    // guard a non-finite value would take the same path and silently swap equity for available
    // funds: locked balance and unrealized PnL vanish, and the result renders at full strength.
    let full_corrupt = !g.btc_balance_full.is_finite();
    let global_full = if !full_corrupt && g.btc_balance_full.abs() > 1e-9 {
        g.btc_balance_full
    } else {
        g.btc_balance_total
    };
    let global_total_usdt = global_full * rate;
    let global_free_usdt = g.btc_balance_total * rate;
    // For COIN-M, global's USDT equivalent is unusable (it is denominated in the coin, so the
    // rate is wrong), but the deduplicated coin wallets are already summed. If global is nearly
    // zero while coin wallets exist, use them as the quarterly/COIN-M account equity.
    let coin_margined = global_total_usdt.abs() < 1.0 && coin_wallet_full_usdt > 1.0;
    let (total_usdt, free_usdt) = if coin_margined {
        (coin_wallet_full_usdt, coin_wallet_free_usdt)
    } else {
        (global_total_usdt, global_free_usdt)
    };
    // Published sums must also be finite: rates and accumulation can overflow independently of
    // whether the source itself was priced.
    let usd_rate_known = source_priced(coin_margined, coin_wallet_all_priced, full_corrupt, rate)
        && total_usdt.is_finite()
        && free_usdt.is_finite();
    let global = GlobalBalanceRow {
        btc_total: g.btc_balance_total,
        btc_locked: g.btc_balance_locked,
        btc_full: g.btc_balance_full,
        special_coin: g.special_coin_balance,
        total_pnl: g.total_pnl,
        free_usdt,
        total_usdt,
        pnl_usdt: g.total_pnl * rate,
        usd_rate_known,
    };
    AssetsSnapshot {
        rows,
        global,
        futures_account,
        base_currency: base_currency.trim().to_string(),
        markets: market_names,
        leverage,
    }
}

/// Snapshot of the core's transfer assets by wallet (Spot/Futures/Quarterly) for the transfer tree.
/// Calculates each row's USDT value through `coin_to_usdt`'s available market-price fallbacks.
pub(super) fn build_transfer_assets(
    markets: &MarketsState,
    st: &TransferAssetsState,
) -> TransferAssetsSnapshot {
    // Index `market base coin (UPPER) → price`. This covers Hyperliquid spot, where the market is
    // named by INDEX (`@206`) while the wallet exposes the token by NAME (`UENA`), so concatenating
    // `UENA+quote` finds no market, produces a value of 0, and lets the dust filter hide the holding.
    // Build it ONCE (scanning per coin would be O(markets×coins) and CPU-expensive); the first market
    // for a coin wins.
    let mut coin_px: std::collections::HashMap<String, f64> = std::collections::HashMap::new();
    for h in markets.iter() {
        let px = h.price().p_last;
        if !(px > 0.0) {
            continue;
        }
        let coin = h.with(|m| {
            let c = m.market_currency_canonic.trim();
            if c.is_empty() {
                m.market_currency.clone()
            } else {
                c.to_string()
            }
        });
        if !coin.is_empty() {
            coin_px.entry(coin.to_ascii_uppercase()).or_insert(px);
        }
    }
    let conv = |kind: ExchangeKind| -> Vec<TransferAssetRow> {
        st.get(kind)
            .iter()
            .map(|a| TransferAssetRow {
                currency: a.currency.clone(),
                amount: a.amount,
                total: a.total,
                value_usdt: coin_to_usdt(markets, &a.currency, a.total, &coin_px),
            })
            .collect()
    };
    TransferAssetsSnapshot {
        spot: conv(ExchangeKind::Spot),
        futures: conv(ExchangeKind::Futures),
        quarterly: conv(ExchangeKind::Quarterly),
    }
}

/// Whether the source that ACTUALLY produced the published equity vouches for its own pricing.
///
/// Which source is asked depends on which one was used: a coin-margined account takes its equity
/// from the per-coin wallets, so every held coin must have been priceable; every other account
/// takes it from `global` scaled by the base-currency rate, so that rate must be finite and
/// positive AND the raw `btc_full` field must not have arrived corrupt.
///
/// Split out from `build_assets` because this is the decision that turns a number into a trusted
/// one, and it is worth pinning on its own: asking the wrong source, or letting a corrupt field
/// through, publishes a confident figure that is quietly wrong.
fn source_priced(
    coin_margined: bool,
    coin_wallet_all_priced: bool,
    full_corrupt: bool,
    rate: f64,
) -> bool {
    if coin_margined {
        coin_wallet_all_priced
    } else {
        !full_corrupt && rate.is_finite() && rate > 0.0
    }
}

#[cfg(test)]
/// Checks for the predicate that decides whether published USD equity may be trusted.
mod tests;
