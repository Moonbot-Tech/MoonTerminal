//! Ручная торговля ядра: постановка / переставление / отмена ордера.
//! Транслирует доменные торговые `CoreCmd` в high-level хендлы moonproto
//! (`client.trade()` / `client.orders()`). Рантайм moonproto сам применяет
//! действие к локальной модели `Orders` ДО отправки пакета (Delphi-гейты:
//! throttle replace, send-if-changed) — здесь мы только вызываем хендл.
//!
//! Истина по API — moonproto: `MoonTrade::new_order` (TNewOrderCommand, CmdId=3),
//! `MoonOrders::move_order` (TOrderReplaceCommand, CmdId=6),
//! `MoonOrders::cancel` (TOrderCancelCommand, CmdId=10).

use moonproto::{
    MoonClient, NewOrderParams, OrderSide, OrderWorkerStatus, SplitOrderParams, VStopParams,
};

use crate::feed::{OrderLinePriceKind, OrderStopKind};

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
        client
            .orders()
            .switch_panic_sell_by_market(market.clone(), on),
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
        client
            .trade()
            .split_order(SplitOrderParams::new(market, parts)),
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
    report(
        server_id,
        format!("set order {uid} {kind:?} -> {on}"),
        result,
    );
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
