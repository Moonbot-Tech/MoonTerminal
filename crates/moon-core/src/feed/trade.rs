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
    ClosePositionParams, MoonClient, NewOrderParams, OrderSide, OrderWorkerStatus, SellOrderParams,
    SplitOrderParams, VStopParams,
};

use crate::feed::{OrderLinePriceKind, OrderStopKind};

/// Единый лог исхода торгового вызова: `Ok` → `info` с контекстом `ctx`, `Err` → тот же
/// контекст + `warn` с ошибкой. Контекст совпадает по тексту с прежними per-функция логами,
/// чтобы грепы по логам не сломались.
pub(super) fn report<T, E: std::fmt::Display>(
    server_id: u64,
    ctx: impl std::fmt::Display,
    r: Result<T, E>,
) {
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

/// Паник-селл КОНКРЕТНОГО ордера (`TTurnPanicSellCommand` по `uid`). Рантайм гейтит
/// повтор (send-if-changed) против своей живой модели ордеров.
pub(super) fn turn_order_panic_sell(client: &MoonClient, server_id: u64, uid: u64, on: bool) {
    report(
        server_id,
        format!("turn order panic sell {uid} on={on}"),
        client.orders().turn_panic_sell(uid, on),
    );
}

/// Закрыть ПОЗИЦИЮ рынка по маркету (`TDoClosePositionCommand`, market_sell=true) — кнопка
/// «Market sell» в Активах у строки с открытой позицией. Рантайм сам определяет сторону.
pub(super) fn market_sell_position(client: &MoonClient, server_id: u64, market: String) {
    report(
        server_id,
        format!("market close position {market}"),
        client
            .trade()
            // `::new()`/`limit_orders()` = ЛИМИТНОЕ закрытие (market_sell=false) — лимитка могла
            // не исполниться, из-за чего «Market sell» не срабатывал. Кнопка обязана закрывать
            // ПО МАРКЕТУ → `market_order()` (market_sell=true). Сторону рантайм определит сам.
            .close_position(ClosePositionParams::market_order(market)),
    );
}

/// Продать СПОТ-ТОКЕН рынка по маркету (`TDoSellOrderCommand`) — кнопка «Market sell» в
/// Активах у строки-холдинга. `price=0` = рыночный ордер; `size` — количество в базовой монете.
pub(super) fn market_sell_token(client: &MoonClient, server_id: u64, market: String, size: f64) {
    report(
        server_id,
        format!("market sell token {market} size={size}"),
        client
            .trade()
            .sell_order(SellOrderParams::new(market, 0.0, size)),
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

/// Включить/выключить стоп (SL/TS/VStop) ордера по `uid`.
///
/// МОДЕЛЬ Moonbot (подтверждена вживую 2026-07-08): у ордера может НЕ БЫТЬ «своего»
/// per-order стопа — тогда стоп берётся ИЗ СТРАТЕГИИ и реально сработает, хотя на проводе
/// per-order поля пустые (нули). Провод НЕ отличает «стоп явно выключен» от «не задан».
/// Отсюда три правила:
/// - эффективное состояние (для UI и для сборки пакета) = наш явный override (клики
///   терминала, [`stop_override`]) → иначе `per-order флаг ИЛИ флаг стратегии`;
/// - `update_stops` несёт ВЕСЬ StopSettings: соседняя группа (SL↔TS) собирается из
///   ЭФФЕКТИВНЫХ параметров — иначе тогл SL затирал нулями стратегийный TS (и наоборот);
/// - уровень включаемого стопа: провод (≠0) → память выключения → поле стратегии
///   ("StopLoss"/"TrailingStop", в стратегии проценты отрицательные → abs) → дефолт
///   ClientSettings (SL `price_drop_level` / TS `trailing_drop`). VStop — провод/память.
/// Рантайм сам сравнивает с живой моделью (send-if-changed). SL/TS → `update_stops`,
/// VStop → `update_vstop`.
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
    // Эффективная стратегия ордера: своя ЛИБО «ручная стратегия» настроек ядра (ручные
    // ордера strat_id=0 ведутся по ней); 0 = совсем без стратегии, стопы — из дефолтов
    // ClientSettings. Держать синхронно с convert.rs (отображение).
    let strat_id = super::strategies::effective_strat_id(&snap, o.strat_id);
    let has_strat = snap.strats().snapshot(strat_id).is_some();
    let cs = snap.settings().client_settings.as_ref();
    let (cs_sl, cs_ts) = cs
        .map(|c| (f64::from(c.price_drop_level), f64::from(c.trailing_drop)))
        .unwrap_or((0.0, 0.0));
    log::info!(
        "core {server_id} set order stop {uid} {kind:?}->{on}: found order emulator={} sl={} ts={} vstop={} \
         strat_id={} eff_strat={} has_strat={} cs={} price_drop={cs_sl} trailing_drop={cs_ts}",
        o.emulator_mode,
        o.stops.stop_loss_enabled(),
        o.stops.trailing_enabled(),
        o.vstop_on,
        o.strat_id,
        strat_id,
        has_strat,
        cs.is_some(),
    );
    let result = match kind {
        OrderStopKind::StopLoss | OrderStopKind::Trailing => {
            let stops = o.stops;
            // Флаг «включён по умолчанию»: поле стратегии, а без стратегии (ручной ордер)
            // — ненулевой дефолт настроек ядра.
            // Проценты «падения» в настройках ОТРИЦАТЕЛЬНЫЕ (price_drop_level=-1.1 →
            // SL 1.1%): включён = ненулевое, уровень дальше берётся по модулю.
            let strat_sl_on = if has_strat {
                super::strategies::strat_field_bool(&snap, strat_id, "UseStopLoss")
            } else {
                o.strat_id == 0 && cs_sl != 0.0
            };
            let strat_ts_on = if has_strat {
                super::strategies::strat_field_bool(&snap, strat_id, "UseTrailing")
            } else {
                o.strat_id == 0 && cs_ts != 0.0
            };
            let strat_sl_level = super::strategies::strat_field_double(&snap, strat_id, "StopLoss");
            let strat_ts_level = super::strategies::strat_field_double(&snap, strat_id, "TrailingStop")
                .or_else(|| super::strategies::strat_field_double(&snap, strat_id, "Trailing"));
            let sl_resolve = |forced: Option<bool>| {
                resolve_stop_group(
                    server_id,
                    uid,
                    OrderStopKind::StopLoss,
                    forced,
                    stops.stop_loss_enabled(),
                    stops.stop_loss_fixed(),
                    stops.stop_loss_level(),
                    stops.stop_loss_spread(),
                    strat_sl_on,
                    strat_sl_level,
                    Some(cs_sl),
                )
            };
            let ts_resolve = |forced: Option<bool>| {
                resolve_stop_group(
                    server_id,
                    uid,
                    OrderStopKind::Trailing,
                    forced,
                    stops.trailing_enabled(),
                    stops.trailing_fixed(),
                    stops.trailing_level(),
                    stops.trailing_spread(),
                    strat_ts_on,
                    strat_ts_level,
                    Some(cs_ts),
                )
            };
            let sl = sl_resolve((kind == OrderStopKind::StopLoss).then_some(on));
            let mut ts = ts_resolve((kind == OrderStopKind::Trailing).then_some(on));
            // TS: уровень не нашли (у ручных без manual-стратегии trailing_drop часто 0),
            // но MB включает трейлинг и без настроенного уровня — ядро дефолтит его само.
            // Шлём enable с level=0 (для SL так НЕЛЬЗЯ: ядро отвергает enable-с-нулём —
            // проверено логом 14:05 TAG). Если ядро отвергнет и тут — увидим по логу
            // (wire ts останется false).
            if kind == OrderStopKind::Trailing && on && ts.is_none() {
                log::info!(
                    "core {server_id} set order stop {uid} Trailing->on: уровень не найден — пробуем enable с level=0 (дефолт ядра)"
                );
                ts = Some((true, false, 0.0, 0.0));
            }
            // Тогаемая группа обязана разрешиться (иначе не шлём вовсе); соседняя без
            // уровня — оставляем как на проводе (хуже не станет), но предупреждаем.
            let (target, other) = if kind == OrderStopKind::StopLoss {
                (&sl, &ts)
            } else {
                (&ts, &sl)
            };
            if on && target.is_none() {
                log::warn!(
                    "core {server_id} set order stop {uid} {kind:?}->on: нет уровня (провод/память/стратегия/дефолт пусты), не шлём"
                );
                return;
            }
            if other.is_none() {
                log::warn!(
                    "core {server_id} set order stop {uid} {kind:?}: у соседнего стопа нет уровня — его страта может погаснуть"
                );
            }
            let apply_sl = |s: moonproto::StopSettings, g: &Option<(bool, bool, f64, f64)>| match g {
                Some((true, true, level, spread)) => s.with_stop_loss_fixed(*level, *spread),
                Some((true, false, level, spread)) => s.with_stop_loss_percent(*level, *spread),
                Some((false, ..)) => s.without_stop_loss(),
                None => s, // уровень не найден — оставить провод как есть
            };
            let apply_ts = |s: moonproto::StopSettings, g: &Option<(bool, bool, f64, f64)>| match g {
                Some((true, true, level, spread)) => s.with_trailing_fixed(*level, *spread),
                Some((true, false, level, spread)) => s.with_trailing_percent(*level, *spread),
                Some((false, ..)) => s.without_trailing(),
                None => s,
            };
            let next = apply_ts(apply_sl(stops, &sl), &ts);
            // ПРАЙМЕР первого OFF по дефолт-стопу: per-order поля на проводе пустые (нули),
            // и «выключить» (without_*) бит-в-бит совпадает с локальной моделью — moonproto
            // send_stops_if_changed глушит пакет, ядро НИЧЕГО не получает, стоп остаётся
            // включённым по дефолту (стратегия/настройки). Сначала материализуем эффективный
            // стоп per-order (enable с текущим уровнем — поведенчески no-op, стоп и так
            // вооружён), затем обычный OFF: оба пакета отличаются от модели и доходят.
            let target_wire_on = if kind == OrderStopKind::StopLoss {
                stops.stop_loss_enabled()
            } else {
                stops.trailing_enabled()
            };
            if !on && !target_wire_on {
                let enable = if kind == OrderStopKind::StopLoss {
                    sl_resolve(Some(true))
                } else {
                    // TS-праймер: без уровня — enable с level=0 (дефолт ядра, см. выше).
                    ts_resolve(Some(true)).or(Some((true, false, 0.0, 0.0)))
                };
                if let Some((true, ..)) = enable {
                    let primer = if kind == OrderStopKind::StopLoss {
                        apply_ts(apply_sl(stops, &enable), &ts)
                    } else {
                        apply_ts(apply_sl(stops, &sl), &enable)
                    };
                    report(
                        server_id,
                        format!("set order {uid} {kind:?} primer(on)"),
                        client.orders().update_stops(uid, primer),
                    );
                } else {
                    log::warn!(
                        "core {server_id} set order stop {uid} {kind:?}->off: праймер без уровня — первый OFF может заглушиться send-if-changed"
                    );
                }
            }
            if !on {
                remember_stop_group(server_id, uid, kind, &stops);
            }
            note_stop_override(server_id, uid, kind, on);
            client.orders().update_stops(uid, next)
        }
        OrderStopKind::VStop => {
            let params = if on {
                // Для VStop дефолта нет — только провод/память.
                let Some((fixed, level, vol)) = restore_from_wire_or_memory(
                    server_id,
                    uid,
                    kind,
                    o.vstop_fixed,
                    o.vstop_level,
                    o.vstop_vol,
                ) else {
                    log::warn!(
                        "core {server_id} set order stop {uid} {kind:?}->on: нет уровня (провод/память пусты), не шлём"
                    );
                    return;
                };
                if fixed {
                    VStopParams::fixed(level, vol)
                } else {
                    VStopParams::percent(level, vol)
                }
            } else {
                // Праймер как у SL/TS: выключение при пустом проводе глушится
                // send_vstop_if_changed (модель и так нулевая). Без уровня праймер
                // невозможен — предупреждаем.
                if !o.vstop_on {
                    if let Some((fixed, level, vol)) = restore_from_wire_or_memory(
                        server_id,
                        uid,
                        kind,
                        o.vstop_fixed,
                        o.vstop_level,
                        o.vstop_vol,
                    ) {
                        let primer = if fixed {
                            VStopParams::fixed(level, vol)
                        } else {
                            VStopParams::percent(level, vol)
                        };
                        report(
                            server_id,
                            format!("set order {uid} {kind:?} primer(on)"),
                            client.orders().update_vstop(uid, primer),
                        );
                    } else {
                        log::warn!(
                            "core {server_id} set order stop {uid} {kind:?}->off: праймер без уровня — первый OFF может заглушиться send-if-changed"
                        );
                    }
                }
                remember_stop_params(server_id, uid, kind, o.vstop_fixed, o.vstop_level, o.vstop_vol);
                VStopParams::disabled()
            };
            note_stop_override(server_id, uid, kind, on);
            client.orders().update_vstop(uid, params)
        }
    };
    report(
        server_id,
        format!("set order {uid} {kind:?} -> {on}"),
        result,
    );
}

/// Эффективные параметры одной stop-группы `(enabled, fixed, level, spread)` для сборки
/// полного StopSettings. `forced=Some(x)` — тогаемая группа (целевое состояние клика);
/// `None` — соседняя, сохраняем её ЭФФЕКТИВНОЕ состояние (override → провод|страта).
/// Возврат `None` = группа должна быть включена, но уровень найти не удалось.
#[allow(clippy::too_many_arguments)]
pub(super) fn resolve_stop_group(
    server_id: u64,
    uid: u64,
    kind: OrderStopKind,
    forced: Option<bool>,
    wire_on: bool,
    wire_fixed: bool,
    wire_level: f64,
    wire_spread: f64,
    strat_on: bool,
    strat_level: Option<f64>,
    default_pct: Option<f64>,
) -> Option<(bool, bool, f64, f64)> {
    let enabled = forced
        .or_else(|| stop_override(server_id, uid, kind, wire_on))
        .unwrap_or(wire_on || strat_on);
    if !enabled {
        return Some((false, false, 0.0, 0.0));
    }
    // Уровень: провод → память выключения → стратегия (проценты в стратегии
    // отрицательные, «падение на N%» → abs) → дефолт ClientSettings.
    if wire_level != 0.0 && wire_level.is_finite() {
        return Some((true, wire_fixed, wire_level, wire_spread));
    }
    if let Some((fixed, level, spread)) = stop_memory_get(server_id, uid, kind) {
        return Some((true, fixed, level, spread));
    }
    if let Some(level) = strat_level.filter(|l| *l != 0.0 && l.is_finite()) {
        return Some((true, false, level.abs(), 0.0));
    }
    // Дефолты настроек тоже отрицательные («падение на N%») → модуль.
    let pct = default_pct.filter(|p| *p != 0.0 && p.is_finite())?;
    Some((true, false, pct.abs(), 0.0))
}

/// Провод → память: восстановление параметров без стратегии/дефолтов (VStop).
fn restore_from_wire_or_memory(
    server_id: u64,
    uid: u64,
    kind: OrderStopKind,
    fixed: bool,
    level: f64,
    extra: f64,
) -> Option<(bool, f64, f64)> {
    if level != 0.0 && level.is_finite() {
        return Some((fixed, level, extra));
    }
    stop_memory_get(server_id, uid, kind).map(|(f, l, s)| (f, l, s))
}

/// Явные per-order переопределения стопов, сделанные ИЗ ТЕРМИНАЛА (session-scoped).
/// Провод не отличает «стоп явно выключен» от «не задан» (оба — нули), а флаг стратегии
/// иначе маскировал бы наш OFF в таблице. Ключ (ядро, uid) → [Option<целевой_флаг>; 3].
fn stop_overrides_map()
-> &'static std::sync::Mutex<std::collections::HashMap<(u64, u64), [Option<bool>; 3]>> {
    static MEM: std::sync::OnceLock<
        std::sync::Mutex<std::collections::HashMap<(u64, u64), [Option<bool>; 3]>>,
    > = std::sync::OnceLock::new();
    MEM.get_or_init(Default::default)
}

/// Записать переопределение (вызывается при отправке тогла).
pub(super) fn note_stop_override(server_id: u64, uid: u64, kind: OrderStopKind, on: bool) {
    stop_overrides_map()
        .lock()
        .unwrap()
        .entry((server_id, uid))
        .or_default()[stop_kind_tag(kind) as usize] = Some(on);
}

/// Прочитать переопределение (UI и сборка пакетов: эффективный флаг SL/TS/Vstop).
/// Инвариант протухания: override живёт, пока провод СОГЛАСЕН с его целью
/// (`target == wire_now`; наши же отправки сразу приводят локальную модель к цели).
/// Провод противоречит цели → значит per-order стоп изменили С ТОЙ СТОРОНЫ (ядро/MB)
/// — override отбрасывается, действует провод/стратегия. Без этого серверный ON
/// навсегда маскировался бы нашим старым OFF.
pub(super) fn stop_override(
    server_id: u64,
    uid: u64,
    kind: OrderStopKind,
    wire_now: bool,
) -> Option<bool> {
    stop_overrides_map()
        .lock()
        .unwrap()
        .get(&(server_id, uid))
        .and_then(|f| f[stop_kind_tag(kind) as usize])
        .filter(|on| *on == wire_now)
}

/// Память параметров стопов, стираемых выключением: ключ (ядро, uid ордера, вид стопа) →
/// (fixed, level, spread|vol). Живёт до конца процесса; объём — единицы записей на сессию
/// (пишется только при ручном выключении стопа из таблицы ордеров).
fn stop_memory() -> &'static std::sync::Mutex<std::collections::HashMap<(u64, u64, u8), (bool, f64, f64)>>
{
    static MEM: std::sync::OnceLock<
        std::sync::Mutex<std::collections::HashMap<(u64, u64, u8), (bool, f64, f64)>>,
    > = std::sync::OnceLock::new();
    MEM.get_or_init(Default::default)
}

fn stop_memory_get(server_id: u64, uid: u64, kind: OrderStopKind) -> Option<(bool, f64, f64)> {
    stop_memory()
        .lock()
        .unwrap()
        .get(&(server_id, uid, stop_kind_tag(kind)))
        .copied()
}

fn stop_kind_tag(kind: OrderStopKind) -> u8 {
    match kind {
        OrderStopKind::StopLoss => 0,
        OrderStopKind::Trailing => 1,
        OrderStopKind::VStop => 2,
    }
}

/// Запомнить параметры стопа перед выключением (только осмысленный уровень).
fn remember_stop_params(
    server_id: u64,
    uid: u64,
    kind: OrderStopKind,
    fixed: bool,
    level: f64,
    extra: f64,
) {
    if level != 0.0 && level.is_finite() {
        stop_memory()
            .lock()
            .unwrap()
            .insert((server_id, uid, stop_kind_tag(kind)), (fixed, level, extra));
    }
}

/// Запомнить SL/TS группу StopSettings перед выключением.
pub(super) fn remember_stop_group(
    server_id: u64,
    uid: u64,
    kind: OrderStopKind,
    stops: &moonproto::StopSettings,
) {
    match kind {
        OrderStopKind::StopLoss => remember_stop_params(
            server_id,
            uid,
            kind,
            stops.stop_loss_fixed(),
            stops.stop_loss_level(),
            stops.stop_loss_spread(),
        ),
        OrderStopKind::Trailing => remember_stop_params(
            server_id,
            uid,
            kind,
            stops.trailing_fixed(),
            stops.trailing_level(),
            stops.trailing_spread(),
        ),
        OrderStopKind::VStop => {}
    }
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
