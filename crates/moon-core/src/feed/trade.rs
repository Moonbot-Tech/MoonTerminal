//! Ручная торговля ядра: постановка / переставление / отмена ордера.
//! Транслирует доменные торговые `CoreCmd` в high-level хендлы moonproto
//! (`client.trade()` / `client.orders()`). Рантайм moonproto сам применяет
//! действие к локальной модели `Orders` ДО отправки пакета (Delphi-гейты:
//! throttle replace, send-if-changed) — здесь мы только вызываем хендл.
//!
//! Истина по API — moonproto: `MoonTrade::new_order` (TNewOrderCommand, CmdId=3),
//! `MoonOrders::move_order` (TOrderReplaceCommand, CmdId=6),
//! `MoonOrders::cancel` (TOrderCancelCommand, CmdId=10).

use moonproto::{MoonClient, NewOrderParams, OrderSide};

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
    match client.orders().move_order(uid, new_price) {
        Ok(()) => log::info!("core {server_id} move order {uid} -> {new_price}"),
        Err(error) => {
            log::warn!("core {server_id} move order {uid} -> {new_price} failed: {error}")
        }
    }
}

/// Отменить ордер ядра по `uid` (TOrderCancelCommand). Рантайм выводит текущий
/// статус из локального ордера; для pending (OS_None) повторяет replace-then-cancel.
pub(super) fn cancel_order(client: &MoonClient, server_id: u64, uid: u64) {
    match client.orders().cancel(uid) {
        Ok(()) => log::info!("core {server_id} cancel order {uid}"),
        Err(error) => log::warn!("core {server_id} cancel order {uid} failed: {error}"),
    }
}
