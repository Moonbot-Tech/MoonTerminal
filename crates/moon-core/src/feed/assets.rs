//! Активы на стороне feed: декаплинг moonproto (markets/balances/transfer_assets)
//! в доменные снимки `AssetsSnapshot` / `TransferAssetsSnapshot`. Moonproto остаётся
//! внутри feed-слоя; стор/UI получают только доменные структуры.

use moonproto::state::{BalancesState, ExchangeKind, MarketsState, TransferAssetsState};
use moonproto::BaseCurrency;

use super::{
    AssetRow, AssetsSnapshot, GlobalBalanceRow, TransferAssetRow, TransferAssetsSnapshot,
    WalletKind,
};

/// USD-стейблы — их курс к USDT считаем равным 1.
fn is_stable(q: &str) -> bool {
    matches!(
        q,
        "USDT" | "USDC" | "BUSD" | "USD" | "FDUSD" | "TUSD" | "DAI" | "USDP"
    )
}

/// Курс котировочной валюты `quote` в USDT (для USDT≈1, для BTC≈курс BTC/USDT).
/// Берём штатный `base_currency_price` ядра; fallback — 1 для стейблов, иначе 0.
fn quote_to_usdt(markets: &MarketsState, quote: &str) -> f64 {
    markets
        .base_currency_price(quote)
        .map(|b| b.last_price)
        .filter(|r| *r > 0.0)
        .unwrap_or_else(|| {
            if is_stable(&quote.to_ascii_uppercase()) {
                1.0
            } else {
                0.0
            }
        })
}

/// Курс базовой валюты аккаунта (`base`) в USDT. Для USDT/стейблов = 1; иначе ищем
/// рынок `<base>USDT` или `base_currency_price`. 0 = курс неизвестен.
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

/// Стоимость `qty` монеты `currency` в USDT через рынок `<currency>USDT`
/// (стейбл — как есть). 0 = курс неизвестен.
fn coin_to_usdt(markets: &MarketsState, currency: &str, qty: f64) -> f64 {
    let cur = currency.to_ascii_uppercase();
    if is_stable(&cur) {
        return qty;
    }
    markets
        .price(&format!("{cur}USDT"))
        .map(|p| p.p_last)
        .filter(|x| *x > 0.0)
        .map(|px| qty * px)
        .unwrap_or(0.0)
}

/// Кошелёк домена → moonproto `ExchangeKind`.
pub(super) fn to_exchange_kind(w: WalletKind) -> ExchangeKind {
    match w {
        WalletKind::Spot => ExchangeKind::Spot,
        WalletKind::Futures => ExchangeKind::Futures,
        WalletKind::Quarterly => ExchangeKind::Quarterly,
    }
}

/// Снимок активов ядра: по всем рынкам с ненулевым балансом/позицией читаем
/// balance_position + price + listed_type + base/quote, плюс account-итоги
/// (`GlobalBalance`). Пустые рынки пропускаем (иначе тысячи нулевых строк), но
/// пыль НЕ фильтруем — это делает UI (порог по USDT-стоимости).
pub(super) fn build_assets(
    markets: &MarketsState,
    balances: &BalancesState,
    base_currency: &str,
) -> AssetsSnapshot {
    let mut rows = Vec::new();
    let mut leverage = std::collections::HashMap::new();
    for h in markets.iter() {
        let bp = h.balance_position();
        let lev = h.with(|m| m.leverage_x);
        let empty = bp.asset_balance == 0.0
            && bp.asset_balance_full == 0.0
            && bp.pos_size == 0.0
            && bp.long_pos_size == 0.0
            && bp.short_pos_size == 0.0;
        // Карта плеча per-core: рынки с позицией/балансом ЛИБО с реальным плечом (>1). Дефолт-1
        // без account-данных НЕ кладём — там плечо неизвестно (ядро сбрасывает в 1), покажем «—».
        if lev > 0 && (!empty || lev > 1) {
            leverage.insert(h.name().to_string(), lev);
        }
        if empty {
            continue;
        }
        let market = h.name().to_string();
        let price = h.price();
        // coin = канонический токен (fallback market_currency); quote = base_currency;
        // listed выводим как `Market::listed_type()` (SPOT если futures_type=EMPTY,
        // иначе BOTH) — сам `ListedType` не реэкспортится из moonproto.
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
        let value_usdt = bp.asset_balance.abs() * price.p_last * rate;
        rows.push(AssetRow {
            market,
            coin,
            quote,
            listed,
            qty: bp.asset_balance,
            qty_full: bp.asset_balance_full,
            price: price.p_last,
            value_usdt,
            mark_price: price.mark_price,
            pos_size: bp.pos_size,
            pos_price: bp.pos_price,
            liq_price: bp.liq_price,
            leverage: lev,
            profit_b: bp.total_profit_b,
            profit_l: bp.total_profit_l,
            profit_s: bp.total_profit_s,
        });
    }
    let g = balances.global();
    // `btc_balance_*` исторически в БАЗОВОЙ валюте аккаунта (для USDT-бота это уже USDT,
    // курс=1; для BTC-бота — BTC, курс=BTCUSDT). Курс берём по базовой валюте сервера.
    let rate = base_rate(markets, base_currency);
    let global = GlobalBalanceRow {
        btc_total: g.btc_balance_total,
        btc_locked: g.btc_balance_locked,
        btc_full: g.btc_balance_full,
        special_coin: g.special_coin_balance,
        total_pnl: g.total_pnl,
        free_usdt: g.btc_balance_total * rate,
        total_usdt: g.btc_balance_full * rate,
        pnl_usdt: g.total_pnl * rate,
    };
    AssetsSnapshot {
        rows,
        global,
        leverage,
    }
}

/// Снимок transfer-активов ядра по кошелькам (Spot/Futures/Quarterly) для дерева переноса.
/// USDT-стоимость каждой строки считаем по рынку `<currency>USDT` (для веток в USDT).
pub(super) fn build_transfer_assets(
    markets: &MarketsState,
    st: &TransferAssetsState,
) -> TransferAssetsSnapshot {
    let conv = |kind: ExchangeKind| -> Vec<TransferAssetRow> {
        st.get(kind)
            .iter()
            .map(|a| TransferAssetRow {
                currency: a.currency.clone(),
                amount: a.amount,
                total: a.total,
                value_usdt: coin_to_usdt(markets, &a.currency, a.total),
            })
            .collect()
    };
    TransferAssetsSnapshot {
        spot: conv(ExchangeKind::Spot),
        futures: conv(ExchangeKind::Futures),
        quarterly: conv(ExchangeKind::Quarterly),
    }
}
