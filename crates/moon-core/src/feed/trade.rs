//! Ручная торговля ядра: постановка / переставление / отмена ордера.
//! Транслирует доменные торговые `CoreCmd` в high-level хендлы moonproto
//! (`client.trade()` / `client.orders()`). Рантайм moonproto сам применяет
//! действие к локальной модели `Orders` ДО отправки пакета (Delphi-гейты:
//! throttle replace, send-if-changed) — здесь мы только вызываем хендл.
//!
//! Истина по API — moonproto: `MoonTrade::new_order` (TNewOrderCommand, CmdId=3),
//! `MoonOrders::move_order` (TOrderReplaceCommand, CmdId=6),
//! `MoonOrders::cancel` (TOrderCancelCommand, CmdId=10).

use std::collections::HashSet;
use std::time::{Duration, Instant};

use moonproto::{
    MoonClient, NewOrderParams, OrderSide, OrderWorkerStatus, SplitOrderParams, StopSettings,
    VStopParams,
};

use crate::feed::{OrderLinePriceKind, OrderStopKind};

/// Отложенная простановка селл/стопа РУЧНОГО ордера. `new_order` не несёт уровней выхода, а его
/// тикет (`client_order_id`) не равен серверному `uid` (см. moonproto docs/trade_actions.md), —
/// поэтому абсолютные цены селл/стопа считаем при постановке (от цены входа по экранным
/// настройкам ядра) и дослыаем их `update_stops` НОВОМУ ордеру, как только он появится в снимке.
pub(super) struct PendingManualStops {
    market: String,
    short: bool,
    /// Абсолютная цена селла/тейка (всегда, если экранный TP > 0).
    tp_price: Option<f64>,
    /// Абсолютная цена стопа (только если включён тогл SL / `panic_if_price_drop`).
    sl_price: Option<f64>,
    /// Uid'ы ордеров этого рынка+стороны ДО постановки — новый ордер = uid не из набора.
    known_uids: HashSet<u64>,
    placed_at: Instant,
}

/// Подготовить отложенную простановку стопов для ручного ордера. Абсолютные цены селл/стопа
/// УЖЕ посчитаны в UI по экранным значениям (с оптимистичными оверлеями — то, что видит юзер),
/// поэтому здесь их НЕ пересчитываем (раньше читали снимок ядра — он отставал от экрана). Только
/// фиксируем текущие uid'ы рынка+стороны, чтобы отличить новый ордер. `None`, если нечего ставить.
pub(super) fn prepare_manual_stops(
    client: &MoonClient,
    market: &str,
    short: bool,
    tp_price: Option<f64>,
    sl_price: Option<f64>,
) -> Option<PendingManualStops> {
    let tp_price = tp_price.filter(|v| v.is_finite() && *v > 0.0);
    let sl_price = sl_price.filter(|v| v.is_finite() && *v > 0.0);
    if tp_price.is_none() && sl_price.is_none() {
        return None;
    }
    let snap = client.snapshot()?;
    let known_uids = snap
        .orders()
        .iter()
        .filter(|o| o.market_name == market && o.is_short == short)
        .map(|o| o.uid)
        .collect();
    log::info!(
        "manual stops prepare {market} short={short} -> tp_price={tp_price:?} sl_price={sl_price:?}"
    );
    Some(PendingManualStops {
        market: market.to_string(),
        short,
        tp_price,
        sl_price,
        known_uids,
        placed_at: Instant::now(),
    })
}

/// Дослать отложенные селл/стоп тем новым ручным ордерам, что уже появились в снимке (uid не из
/// `known_uids`). Абсолютными ценами (`with_take_profit_price`/`with_stop_loss_fixed`), чтобы ядро
/// не пересчитывало их через плечо/ROE. Истекает через 15с, если ордер так и не пришёл.
pub(super) fn apply_pending_manual_stops(
    client: &MoonClient,
    server_id: u64,
    pending: &mut Vec<PendingManualStops>,
) {
    if pending.is_empty() {
        return;
    }
    let Some(snap) = client.snapshot() else {
        return;
    };
    pending.retain_mut(|p| {
        if p.placed_at.elapsed() > Duration::from_secs(600) {
            log::warn!(
                "core {server_id} manual stops timeout: new order {} short={} not filled in 600s",
                p.market,
                p.short
            );
            return false;
        }
        let found = snap
            .orders()
            .iter()
            .find(|o| o.market_name == p.market && o.is_short == p.short && !p.known_uids.contains(&o.uid));
        let Some(o) = found else {
            return true; // ордер ещё не появился — ждём
        };
        // Дослыаем стопы ТОЛЬКО после исполнения входа (есть позиция). По pending-ордеру ядро
        // при филле ставит/перетирает свой выход (ROE), поэтому ждём `fill>0` и кладём абсолютные
        // цены поверх уже на позицию — тогда они не перетираются.
        let fill_pct =
            (o.buy_order.quantity - o.buy_order.quantity_remaining) / o.buy_order.quantity.max(1e-12) * 100.0;
        if !(fill_pct > 0.0) {
            return true; // ордер ещё не залит — ждём фила
        }
        let uid = o.uid;
        log::info!(
            "manual stops apply uid={uid} {} short={} fill={fill_pct}% sell_price={} take_profit_en={} sl_en={} -> set tp={:?} sl={:?}",
            p.market,
            p.short,
            o.sell_price,
            o.stops.take_profit_enabled(),
            o.stops.stop_loss_enabled(),
            p.tp_price,
            p.sl_price
        );
        let mut stops = StopSettings::disabled();
        if let Some(tp) = p.tp_price {
            stops = stops.with_take_profit_price(tp);
        }
        if let Some(sl) = p.sl_price {
            stops = stops.with_stop_loss_fixed(sl, 0.0);
        }
        match client.orders().update_stops(uid, stops) {
            Ok(_) => log::info!(
                "core {server_id} manual stops applied uid={uid} tp={:?} sl={:?}",
                p.tp_price,
                p.sl_price
            ),
            Err(error) => {
                log::warn!("core {server_id} manual stops uid={uid} failed: {error}")
            }
        }
        false
    });
}

/// Единый лог исхода торгового вызова: `Ok` → `info` с контекстом `ctx`, `Err` → тот же
/// контекст + `warn` с ошибкой. Контекст совпадает по тексту с прежними per-функция логами,
/// чтобы грепы по логам не сломались.
fn report<T, E: std::fmt::Display>(server_id: u64, ctx: impl std::fmt::Display, r: Result<T, E>) {
    match r {
        Ok(_) => log::info!("core {server_id} {ctx}"),
        Err(error) => log::warn!("core {server_id} {ctx} failed: {error}"),
    }
}

/// Поставить новый ордер (TNewOrderCommand). `short` — сторона ПОЗИЦИИ
/// (Long/Short, зеркало `is_short`); `strategy_id=None` шлёт `StratID=0` —
/// штатный ручной ордер без стратегии. `size` — размер в базовой монете.
pub(super) fn place_order(
    client: &MoonClient,
    server_id: u64,
    market: String,
    short: bool,
    price: f64,
    size: f64,
    strategy_id: Option<u64>,
) {
    let side = if short {
        OrderSide::Short
    } else {
        OrderSide::Long
    };
    let mut params = NewOrderParams::new(market.clone(), side, price, size);
    if let Some(id) = strategy_id {
        params = params.with_strategy_id(id);
    }
    match client.trade().new_order(params) {
        Ok(_ticket) => log::info!(
            "core {server_id} place order {market} short={short} price={price} size={size} strat={strategy_id:?}"
        ),
        Err(error) => {
            log::warn!("core {server_id} place order {market} failed: {error}")
        }
    }
}

/// Переставить (move/replace) существующий ордер ядра по `uid` на новую цену —
/// «потянуть за линию». Рантайм троттлит повторы (`replace_sent_time`) и сам
/// выводит сторону/рынок из локального ордера.
pub(super) fn move_order(client: &MoonClient, server_id: u64, uid: u64, new_price: f64) {
    report(
        server_id,
        format!("move order {uid} -> {new_price}"),
        client.orders().move_order(uid, new_price),
    );
}

/// Отменить ордер ядра по `uid` (TOrderCancelCommand). Рантайм выводит текущий
/// статус из локального ордера; для pending (OS_None) повторяет replace-then-cancel.
pub(super) fn cancel_order(client: &MoonClient, server_id: u64, uid: u64) {
    report(
        server_id,
        format!("cancel order {uid}"),
        client.orders().cancel(uid),
    );
}

/// «Паник-селл» по рынку (кнопка на чарте): market-level panic sell button semantics.
/// Транслируется в `orders().switch_panic_sell_by_market`. Рантайм сам применяет тоггл
/// к ордерам рынка и шлёт нужные пакеты.
pub(super) fn panic_sell_market(client: &MoonClient, server_id: u64, market: String, on: bool) {
    report(
        server_id,
        format!("panic sell market {market} on={on}"),
        client.orders().switch_panic_sell_by_market(market.clone(), on),
    );
}

/// Отменить ожидающие buy-ордера рынка («Cancel Buy»). Берём УДЕРЖАННЫЙ снимок, отбираем
/// ордера этого рынка в buy-фазе ДО исполнения (`OS_None` — ещё не на бирже, или `BuySet` —
/// лимит-бай ждёт налива), не помеченные на отмену, и шлём по каждому `orders().cancel(uid)`.
/// Исполненные позиции (`BuyDone`/sell-фазы) и терминальные ордера не трогаем.
pub(super) fn cancel_market_buys(client: &MoonClient, server_id: u64, market: &str) {
    let Some(snap) = client.snapshot() else {
        log::warn!("core {server_id} cancel market buys {market}: no snapshot yet");
        return;
    };
    let uids: Vec<u64> = snap
        .orders()
        .iter()
        .filter(|o| {
            o.market_name == market
                && !o.pending_cancel
                && (o.status == OrderWorkerStatus::None || o.status == OrderWorkerStatus::BuySet)
        })
        .map(|o| o.uid)
        .collect();
    log::info!(
        "core {server_id} cancel market buys {market}: {} pending",
        uids.len()
    );
    for uid in uids {
        if let Err(error) = client.orders().cancel(uid) {
            log::warn!("core {server_id} cancel market buys {market} uid {uid} failed: {error}");
        }
    }
}

/// «Join all sells» (ПКМ по линии sell): объединить sell-ордера рынка. `short` — сторона
/// ПОЗИЦИИ (зеркало `is_short`), задаёт `OrderSide`. Транслируется в `trade().join_orders`.
pub(super) fn join_sells(client: &MoonClient, server_id: u64, market: String, short: bool) {
    let side = if short {
        OrderSide::Short
    } else {
        OrderSide::Long
    };
    report(
        server_id,
        format!("join sells {market} short={short}"),
        client.trade().join_orders(market, side),
    );
}

/// «Split order» (ПКМ по линии sell): разбить выбранный sell-ордер рынка на `parts` частей.
/// Транслируется в `trade().split_order(SplitOrderParams::new(market, parts))`.
pub(super) fn split_order(client: &MoonClient, server_id: u64, market: String, parts: i32) {
    report(
        server_id,
        format!("split order {market} parts={parts}"),
        client.trade().split_order(SplitOrderParams::new(market, parts)),
    );
}

/// Включить/выключить стоп-флаг (SL/TS/VStop) ордера по `uid`. Берём УДЕРЖАННЫЙ снимок
/// ордера и флипаем только нужный флаг, СОХРАНЯЯ уровень/spread/режим (percent|fixed) —
/// чтобы повторное включение восстановило ровно тот стоп, что был настроен. Рантайм
/// сам сравнивает с живой моделью (send-if-changed) и не шлёт пакет, если ничего не
/// изменилось. SL/TS → `update_stops`, VStop → `update_vstop`.
pub(super) fn set_order_stop(
    client: &MoonClient,
    server_id: u64,
    uid: u64,
    kind: OrderStopKind,
    on: bool,
) {
    let Some(snap) = client.snapshot() else {
        log::warn!("core {server_id} set order stop {uid} {kind:?}->{on}: no snapshot yet");
        return;
    };
    let Some(o) = snap.orders().iter().find(|o| o.uid == uid) else {
        log::warn!("core {server_id} set order stop {uid} {kind:?}->{on}: order not tracked");
        return;
    };
    log::info!(
        "core {server_id} set order stop {uid} {kind:?}->{on}: found order emulator={} sl={} ts={} vstop={}",
        o.emulator_mode,
        o.stops.stop_loss_enabled(),
        o.stops.trailing_enabled(),
        o.vstop_on
    );
    let result = match kind {
        OrderStopKind::StopLoss => {
            let stops = o.stops;
            let next = if on {
                let (level, spread) = (stops.stop_loss_level(), stops.stop_loss_spread());
                if stops.stop_loss_fixed() {
                    stops.with_stop_loss_fixed(level, spread)
                } else {
                    stops.with_stop_loss_percent(level, spread)
                }
            } else {
                stops.without_stop_loss()
            };
            client.orders().update_stops(uid, next)
        }
        OrderStopKind::Trailing => {
            let stops = o.stops;
            let next = if on {
                let (level, spread) = (stops.trailing_level(), stops.trailing_spread());
                if stops.trailing_fixed() {
                    stops.with_trailing_fixed(level, spread)
                } else {
                    stops.with_trailing_percent(level, spread)
                }
            } else {
                stops.without_trailing()
            };
            client.orders().update_stops(uid, next)
        }
        OrderStopKind::VStop => {
            let params = if on {
                if o.vstop_fixed {
                    VStopParams::fixed(o.vstop_level, o.vstop_vol)
                } else {
                    VStopParams::percent(o.vstop_level, o.vstop_vol)
                }
            } else {
                VStopParams::disabled()
            };
            client.orders().update_vstop(uid, params)
        }
    };
    report(server_id, format!("set order {uid} {kind:?} -> {on}"), result);
}

/// Передвинуть цену стоп/тейк-линии ордера (перетаскивание линии на чарте) на абсолютную
/// `price`. SL/TS ставим ФИКСИРОВАННЫМ стопом по цене (`with_stop_loss_fixed`/
/// `with_trailing_fixed`, сохраняя текущий spread), take-profit — `with_take_profit_price`.
/// Остальные стопы ордера сохраняем (билдеры StopSettings трогают только свою группу полей).
/// Рантайм сам гейтит отправку (send-if-changed) против живой модели.
pub(super) fn move_order_stop_price(
    client: &MoonClient,
    server_id: u64,
    uid: u64,
    kind: OrderLinePriceKind,
    price: f64,
) {
    if !(price.is_finite() && price > 0.0) {
        return;
    }
    let Some(snap) = client.snapshot() else {
        log::warn!("core {server_id} move order stop price {uid} {kind:?}: no snapshot yet");
        return;
    };
    let Some(o) = snap.orders().iter().find(|o| o.uid == uid) else {
        log::warn!("core {server_id} move order stop price {uid} {kind:?}: order not tracked");
        return;
    };
    let stops = o.stops;
    let next = match kind {
        OrderLinePriceKind::StopLoss => stops.with_stop_loss_fixed(price, stops.stop_loss_spread()),
        OrderLinePriceKind::Trailing => stops.with_trailing_fixed(price, stops.trailing_spread()),
        OrderLinePriceKind::TakeProfit => stops.with_take_profit_price(price),
    };
    report(
        server_id,
        format!("move order stop price {uid} {kind:?} -> {price}"),
        client.orders().update_stops(uid, next),
    );
}
