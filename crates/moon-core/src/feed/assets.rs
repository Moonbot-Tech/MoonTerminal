//! Активы на стороне feed: декаплинг moonproto (markets/balances/transfer_assets)
//! в доменные снимки `AssetsSnapshot` / `TransferAssetsSnapshot`. Moonproto остаётся
//! внутри feed-слоя; стор/UI получают только доменные структуры.

use moonproto::state::{BalancesState, ExchangeKind, MarketsState, TransferAssetsState};
use moonproto::{BaseCurrency, OrderType};

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
///
/// ПУСТОЙ `quote` = USD-деноминированный контракт (Binance COIN-M: `BTCUSD_PERP`,
/// `ETHUSD_260925` — котируются в USD, маржа в самой монете). Курс USD≈USDT=1, иначе
/// `p_last` (уже цена монеты в USD) домножился бы на 0 и стоимость схлопнулась в ноль.
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

/// Стоимость `qty` монеты `currency` в USDT (стейбл — как есть). Курс: рынок
/// `<CUR>USDT`, затем `<CUR>USDC` (≈USD), затем ЛЮБОЙ USD-деноминированный рынок
/// монеты по префиксу `<CUR>USD` — на COIN-M/квартальных ядрах рынков `<CUR>USDT`
/// НЕТ, но цена контрактов `BTCUSD_PERP`/`BTCUSD_260925` уже в USD (иначе весь
/// квартальный кошелёк оценивался бы в 0 и прятался фильтром пыли). 0 = неизвестно.
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
        // Рынок = САМО имя монеты: Hyperliquid спот-индексы («@699») зовутся так и есть,
        // конкатенации «@699USDT»/«@699USDC» не существует → иначе кошелёк оценивался бы в 0
        // и монета пряталась фильтром пыли. Пробуем первым (по исходному имени, без upper).
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
        // Hyperliquid-спот: рынок токена назван индексом («@206»), а кошелёк — именем («UENA»).
        // Индекс `coin_px` мапит базовую монету рынка → цену, покрывая этот случай (цена рынка
        // «@206» = цена UENA, quote USDC≈USD).
        .or_else(|| coin_px.get(&cur).copied().filter(|x| *x > 0.0));
    px.map(|px| qty * px).unwrap_or(0.0)
}

/// Кошелёк домена → moonproto `ExchangeKind`.
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
    // Каталог имён рынков ядра — для гейта кнопки «Market sell» в UI (продать монету можно
    // лишь если рынок `<coin><quote>` существует).
    let market_names: std::collections::HashSet<String> =
        markets.iter().map(|h| h.name().to_string()).collect();
    // COIN-M / квартальные: кошелёк деноминирован в самой монете (BTC/ETH/…), а не в
    // USDT, и ОДИН и тот же баланс монеты дублируется биржей на все её контракты
    // (PERP + все экспирации). Считаем эквити как Σ по УНИКАЛЬНЫМ монетам (дедуп),
    // иначе BTC учтётся трижды. `_full` = полный кошелёк, обычный = свободно.
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
        // Стоимость СТРОКИ — от ПОЛНОГО удерживаемого баланса (free + заблокировано в открытых
        // sell-ордерах), как Moonbot: открытая поза держит всё количество в TP-ордерах (free≈0),
        // но это по-прежнему наш актив — прятать его нельзя (иначе строка с value 0 уходит под
        // фильтр пыли). `_full` может быть не заполнен биржей → берём max(full, free).
        let held_qty = if bp.asset_balance_full.abs() > bp.asset_balance.abs() {
            bp.asset_balance_full
        } else {
            bp.asset_balance
        };
        let value_usdt = held_qty.abs() * price.p_last * rate;
        // Дедуп монетных кошельков (COIN-M): суммируем стоимость КАЖДОЙ монеты один раз,
        // независимо от числа её контрактов. Здесь эквити АККАУНТА — считаем от ПОЛНОГО
        // остатка (в отличие от строк таблицы). Учитываем только реальный баланс монеты
        // (asset_balance*), но НЕ чисто позиционные строки (pos без баланса — там монеты
        // на кошельке нет, это дериватив на USDT-марже).
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
        // min_lot_size ядра уже в котируемой валюте: max(step, min_qty)·цена и min_notional.
        let min_lot_usd = price.min_lot_size * rate;
        let is_quote_asset = coin.eq_ignore_ascii_case(base_currency.trim());
        // ЖИВОЙ PnL позиции: (цена − вход) × размер × направление, в котируемой → USDT.
        // Марк-цена (фьючи) точнее для PnL; нет марка — mid. Ноги хеджа приоритетнее
        // нетто-позиции. Серверные total_profit_* НЕ используем для позиций: они
        // накопленные за период и замерзают между balance-пушами.
        let mark = if price.mark_price > 0.0 {
            price.mark_price
        } else {
            price.p_last
        };
        let mut live_pnl = 0.0;
        let mut have_position_pnl = false;
        if mark > 0.0 {
            if bp.long_pos_size != 0.0 && bp.long_pos_price > 0.0 {
                live_pnl += (mark - bp.long_pos_price) * bp.long_pos_size.abs();
                have_position_pnl = true;
            }
            if bp.short_pos_size != 0.0 && bp.short_pos_price > 0.0 {
                live_pnl += (bp.short_pos_price - mark) * bp.short_pos_size.abs();
                have_position_pnl = true;
            }
            if !have_position_pnl && bp.pos_size != 0.0 && bp.pos_price > 0.0 {
                let short = bp.pos_size < 0.0 || bp.pos_dir == OrderType::Sell;
                let dir = if short { -1.0 } else { 1.0 };
                live_pnl = (mark - bp.pos_price) * bp.pos_size.abs() * dir;
                have_position_pnl = true;
            }
        }
        let pnl_usdt = if have_position_pnl {
            live_pnl * rate
        } else {
            // Нет позиции/цены входа (спот-баланс без pos-данных) — серверный накопленный
            // профит рынка (в котируемой валюте) как fallback.
            (bp.total_profit_b + bp.total_profit_l + bp.total_profit_s) * rate
        };
        // Размер/цена позиции строки: нетто `pos_size`, а если ядро держит ноги РАЗДЕЛЬНО
        // (hedge-режим либо сервер кладёт шорт в `short_pos_size`, а нетто = 0) — берём нетто
        // ног (шорт отрицательный). Иначе реальный фьючер-шорт с `pos_size=0` не проходит
        // `is_position` в UI и пропадает из «Активов» (баланса монеты у деривати­ва нет).
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
        });
    }
    let g = balances.global();
    // `btc_balance_*` исторически в БАЗОВОЙ валюте аккаунта (для USDT-бота это уже USDT,
    // курс=1; для BTC-бота — BTC, курс=BTCUSDT). Курс берём по базовой валюте сервера.
    let rate = base_rate(markets, base_currency);
    // Итог/свободно из global. ФОЛБЭК на `btc_total`, когда `btc_full` не заполнен: у
    // некоторых спот-аккаунтов (напр. Binance spot USDC) биржа не отдаёт «full», и весь
    // баланс лежит в «available» (`btc_total`) — иначе карта ядра показывала бы 0.
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
    // COIN-M: global в USDT-эквиваленте бесполезен (деноминирован в монете, курс не тот),
    // но кошельки монет мы уже просуммировали с дедупом. Если global почти нулевой, а
    // монетные кошельки есть — берём их (это и есть эквити квартального/COIN-M аккаунта).
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

/// Снимок transfer-активов ядра по кошелькам (Spot/Futures/Quarterly) для дерева переноса.
/// USDT-стоимость каждой строки считаем по рынку `<currency>USDT` (для веток в USDT).
pub(super) fn build_transfer_assets(
    markets: &MarketsState,
    st: &TransferAssetsState,
) -> TransferAssetsSnapshot {
    // Индекс «базовая монета рынка (UPPER) → цена». Покрывает Hyperliquid-спот: там рынок назван
    // ИНДЕКСОМ («@206»), а кошелёк отдаёт токен по ИМЕНИ («UENA»), и конкатенация `UENA+quote`
    // рынка не находит → стоимость 0 → холдинг прячется фильтром пыли. Строим ОДИН раз (скан на
    // каждую монету дал бы O(рынки×монеты) — дорого по CPU); первый рынок монеты выигрывает.
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
mod tests {
    use super::source_priced;

    /// A global-sourced account needs a usable base-currency rate.
    #[test]
    fn global_source_requires_a_finite_positive_rate() {
        assert!(source_priced(false, false, false, 1.0));
        assert!(!source_priced(false, false, false, 0.0));
        assert!(!source_priced(false, false, false, -1.0));
        assert!(!source_priced(false, false, false, f64::NAN));
        assert!(!source_priced(false, false, false, f64::INFINITY));
    }

    /// A corrupt `btc_full` disqualifies the global source even when the rate is perfect.
    ///
    /// The trap this guards: `NaN.abs() > 1e-9` is false, so a non-finite `btc_full` silently took
    /// the "field not published" fallback and reported available funds as though they were equity
    /// — locked balance and unrealized PnL dropped, at full rendering strength.
    #[test]
    fn corrupt_full_disqualifies_the_global_source() {
        assert!(!source_priced(false, false, true, 1.0));
        assert!(!source_priced(false, true, true, 1.0));
    }

    /// A coin-margined account is judged by its wallets, which `btc_full` never fed — so the same
    /// corruption must NOT disqualify it, or a healthy COIN-M account would read as unpriced.
    #[test]
    fn coin_margined_source_is_judged_by_its_wallets() {
        assert!(source_priced(true, true, true, 0.0));
        assert!(!source_priced(true, false, false, 1.0));
    }
}
