//! Чистые проекции moonproto → терминальные снимки (license/client-settings/lev/runtime),
//! точечные правки удержанных снимков настроек и сборка строк ордеров (`OrderRow`).

use std::sync::Arc;

use moonproto::state::{Order, OrderTraceChartPoint, OrderTraceLine};
use moonproto::{Event, MoonClient};

use crate::feed::report::{OrderIndex, OrderMeta};
use crate::feed::strategies::strat_kind_name;
use crate::feed::{
    ClientSettings, ClientSettingsEdit, LevManageEdit, LevManageState, LicenseState, OrderRow,
    OrderTrace, OrderTracePoint, RuntimeState,
};

fn trace_point(p: OrderTraceChartPoint) -> OrderTracePoint {
    OrderTracePoint {
        time_ms: p.unix_millis() as f64,
        price: p.price,
    }
}

fn valid_trace_point(p: &OrderTracePoint) -> bool {
    p.time_ms > 1.0 && p.price.is_finite() && p.price > 0.0
}

fn valid_trace_tmp_point(p: OrderTraceChartPoint) -> Option<OrderTracePoint> {
    let time_ms = p.unix_millis() as f64;
    (time_ms > 1.0 && p.price.is_finite() && p.price > 0.0).then_some(OrderTracePoint {
        time_ms,
        price: p.price,
    })
}

fn moon_time_to_unix_seconds(time: moonproto::MoonTime) -> Option<i64> {
    let millis = time.unix_millis();
    (millis > 0).then_some(millis.div_euclid(1000))
}

fn moon_time_to_unix_millis_f64(time: moonproto::MoonTime) -> f64 {
    let millis = time.unix_millis();
    if millis > 0 {
        millis as f64
    } else {
        0.0
    }
}

pub(super) fn license_state_from_proto(
    license: moonproto::KernelLicenseStateCommand,
) -> LicenseState {
    LicenseState {
        paid_version: license.paid_version,
        reg_id: license.reg_id,
        moon_credits: license.moon_credits,
        moon_credits_hold: license.moon_credits_hold,
        moon_credits_auction: license.moon_credits_auction,
        can_use_watcher: license.can_use_watcher,
    }
}

/// Плоская проекция moonproto `ClientSettings` → терминальный снимок. Raw-поля
/// (`s_price`/`sb_num`/…) в проде `pub(crate)`, поэтому читаем ТОЛЬКО через хелперы.
/// «Свой» TP кнопки (из `x_sell`/scalp), ИГНОРИРУЯ `fixed_sell_mode` — это ветка
/// `effective_take_profit_percent` без fixed-sell, чтобы выбор S-слота не подменял отображаемый TP.
fn main_take_profit_percent(c: &moonproto::ClientSettingsCommand) -> f64 {
    if c.x_sell > 0 {
        let mut value = f64::from(c.x_sell);
        if c.x_tmode {
            value *= 10.0;
        }
        value.min(900.0)
    } else {
        f64::from(c.x_sell_scalp) / 50.0
    }
}

pub(super) fn client_settings_from_proto(c: &moonproto::ClientSettingsCommand) -> ClientSettings {
    let fixed_sell_pcts =
        std::array::from_fn(|i| c.fixed_sell_preset_percent(i + 1).unwrap_or(0.0));
    ClientSettings {
        take_profit_pct: c.effective_take_profit_percent(),
        take_profit_main_pct: main_take_profit_percent(c),
        take_profit_extended: c.x_tmode,
        fixed_sell_mode: c.fixed_sell_mode,
        stop_loss_pct: c.price_drop_level,
        trailing_drop_pct: c.trailing_drop,
        use_global_take_profit: c.use_g_take_profit,
        global_take_profit_pct: c.g_take_profit,
        panic_if_price_drop: c.panic_if_price_drop,
        emu_mode: c.emu_mode,
        buy_iceberg: c.buy_iceberg,
        sell_iceberg: c.sell_iceberg,
        sign_orders: c.sign_orders,
        use_stop_market: c.use_stop_market,
        fixed_sell_pcts,
        fixed_sell_slot: c.selected_fixed_sell_slot(),
    }
}

pub(super) fn lev_manage_from_proto(l: &moonproto::LevManage) -> LevManageState {
    LevManageState {
        auto_max_order: l.auto_max_order,
        auto_lev_up: l.auto_lev_up,
        auto_isolated: l.auto_isolated,
        auto_cross: l.auto_cross,
        auto_fix_lev: l.auto_fix_lev,
        fix_lev: l.fix_lev,
        tlg_report: l.tlg_report,
        lev_control: l.lev_control.clone(),
    }
}

pub(super) fn runtime_state_from_proto(s: &moonproto::RuntimeStateCommand) -> RuntimeState {
    RuntimeState {
        is_started: s.is_started,
        auto_detect_active: s.auto_detect_active,
    }
}

/// Снимок настроек ядра по событию: тянем из snapshot ТОЛЬКО когда в пачке есть
/// соответствующее `Settings`-событие (как license/client_settings/lev/runtime),
/// иначе None — снимок дёшев, но дёргать его без события незачем.
pub(super) fn settings_event_snapshot<T>(
    events: &[Event],
    client: &MoonClient,
    matched: impl Fn(&Event) -> bool,
    extract: impl FnOnce(Arc<moonproto::MoonStateSnapshot>) -> Option<T>,
) -> Option<T> {
    events
        .iter()
        .any(matched)
        .then(|| client.snapshot())
        .flatten()
        .and_then(extract)
}

/// Применяет точечную правку тулбара к удержанному снимку настроек ЧЕРЕЗ хелперы команды
/// (raw-поля `s_price`/`sb_num` в проде `pub(crate)`; `price_drop_level` — pub).
pub(super) fn apply_client_settings_edit(
    s: &mut moonproto::ClientSettingsCommand,
    edit: ClientSettingsEdit,
) {
    match edit {
        ClientSettingsEdit::TakeProfit { pct, extended } => {
            // x_tmode/«s9»: on → x_sell хранит pct/10 (видимые 100..900%); off → x_sell=pct
            // напрямую (1..100%). Ядро без флага само режет TP до 100, поэтому пишем оба поля.
            s.fixed_sell_mode = false;
            if extended {
                s.x_tmode = true;
                s.x_sell = (pct / 10.0).round().clamp(10.0, 90.0) as i32;
            } else {
                s.x_tmode = false;
                s.x_sell = pct.round().clamp(1.0, 100.0) as i32;
            }
        }
        ClientSettingsEdit::StopLossPct(pct) => s.price_drop_level = pct,
        ClientSettingsEdit::ScalpTakeProfit(pct) => s.set_scalp_take_profit_percent(pct),
        ClientSettingsEdit::SelectFixedSellSlot(slot) => {
            // Включаем fixed-sell режим — иначе effective TP остаётся на x_sell и не меняется.
            // С ним effective_take_profit_percent() = процент выбранного пресета → TP в тулбаре
            // становится равным значению S-кнопки.
            s.fixed_sell_mode = true;
            s.set_selected_fixed_sell_slot(slot);
        }
        ClientSettingsEdit::EngageMainTakeProfit => {
            // Возврат к главному TP: гасим fixed-sell, значение TP (x_sell/scalp) не трогаем.
            s.fixed_sell_mode = false;
        }
        ClientSettingsEdit::SetFixedSellPct { slot, pct } => {
            // Видимый процент = s_price · (x_tmode? 10 : 1); пишем s_price обратным пересчётом.
            let price = if s.x_tmode {
                (pct / 10.0) as f32
            } else {
                pct as f32
            };
            s.set_fixed_sell_preset_price(slot, price);
        }
    }
}

pub(super) fn apply_lev_manage_edit(l: &mut moonproto::LevManage, edit: LevManageEdit) {
    match edit {
        LevManageEdit::FixLev(n) => {
            l.auto_fix_lev = true;
            l.fix_lev = n;
        }
    }
}

fn order_trace(line: &OrderTraceLine) -> Option<OrderTrace> {
    let points: Vec<OrderTracePoint> = line.points.iter().copied().map(trace_point).collect();
    if !points.iter().any(valid_trace_point) {
        return None;
    }
    Some(OrderTrace {
        points,
        tmp_point: line.tmp_point.and_then(valid_trace_tmp_point),
        stop_price: line.stop_price.filter(|p| p.is_finite() && *p > 0.0),
        stop_time_ms: line
            .stop_time
            .map(|time| time.unix_millis() as f64)
            .filter(|time_ms| *time_ms > 1.0),
    })
}

fn build_order_row(
    snap: &moonproto::MoonStateSnapshot,
    o: &Order,
    remember_reports: bool,
    orders_index: &mut OrderIndex,
) -> OrderRow {
    // Полные данные ордера по uid (есть с открытия); db_id→uid —
    // когда db_id появился (перед закрытием). close-SQL этих полей
    // не несёт.
    if remember_reports {
        orders_index.remember(
            o.uid,
            OrderMeta {
                coin: o.market_name.clone(),
                isshort: o.is_short,
                buyprice: o.buy_price,
                sellprice: o.sell_price,
                quantity: o.buy_order.quantity,
                spentbtc: o.buy_order.spent_btc,
                gainedbtc: o.sell_order.total_btc,
                lev: o.buy_order.leverage as i64,
                strategyid: o.strat_id as i64,
                taskid: o.uid as i64,
                exorderid: (o.buy_order.int_id != 0).then(|| o.buy_order.int_id.to_string()),
                emulator: o.emulator_mode,
                buydate: moon_time_to_unix_seconds(o.buy_order.open_time()),
                sellsetdate: moon_time_to_unix_seconds(o.sell_order.create_time()),
                closedate: moon_time_to_unix_seconds(o.sell_order.close_time()),
            },
        );
        if o.db_id != 0 {
            orders_index.map_dbid(o.db_id, o.uid);
        }
    }

    let strat = match snap.strats().snapshot(o.strat_id) {
        Some(s) => strat_kind_name(s.kind().ordinal()).to_string(),
        None => o.strat_id.to_string(),
    };
    // Входная нога ВСЕГДА `buy_order` — и для лонга, и для шорта (статус-машина фазовая:
    // вход = «Buy*», выход = «Sell*»; у шорта вход тоже лежит в buy_order/buy_price, а
    // sell_order — пустая выходная нога). Раньше для шорта брали sell_order → fill_pct=0.
    let leg = &o.buy_order;
    let fill_pct = if leg.quantity > 0.0 {
        ((leg.quantity - leg.quantity_remaining) / leg.quantity * 100.0) as f32
    } else {
        0.0
    };
    let pick = |qb: f64, q: f64, qr: f64| {
        if qb != 0.0 {
            qb
        } else if q != 0.0 {
            q
        } else {
            qr
        }
    };
    let bs = pick(
        o.buy_order.quantity_base,
        o.buy_order.quantity,
        o.buy_order.quantity_remaining,
    );
    let ss = pick(
        o.sell_order.quantity_base,
        o.sell_order.quantity,
        o.sell_order.quantity_remaining,
    );
    let size = if bs.abs() >= ss.abs() { bs } else { ss };

    let mkt = snap.markets().price(&o.market_name);
    let last = mkt.as_ref().map(|p| p.p_last as f32).unwrap_or(0.0);
    // Цена входа для линии входа и расчёта стоп/тейк-уровней.
    // ВАЖНО (баг «линия выше реального бая»): ПОСЛЕ исполнения ядро кладёт в `buy_price` И в
    // `buy_order.actual_price` цену БЕЗУБЫТКА (= реальный филл + комиссия круга ≈ +0.1%), а не
    // сырой вход. Реальная цена входа исполненного ордера = средняя цена ПОЗИЦИИ (`pos_price`,
    // с биржи, без надбавки). Пока ордер НЕ залит (fill=0) — позиции ещё нет, берём цену
    // выставленного лимита (`buy_price`).
    let pos_price = snap
        .markets()
        .get(&o.market_name)
        .map(|h| h.balance_position().pos_price)
        .unwrap_or(0.0);
    let entry = if fill_pct > 0.0 && pos_price > 0.0 {
        pos_price
    } else {
        o.buy_price
    };
    let valid_entry = entry.is_finite() && entry > 0.0;
    let fin = |v: f64| (v.is_finite() && v > 0.0).then_some(v);
    let pct_stop = |level: f64| {
        if o.is_short {
            entry * (1.0 + level / 100.0)
        } else {
            entry * (1.0 - level / 100.0)
        }
    };
    // SL и trailing считаются одинаково: fixed → абсолютный уровень как есть; иначе
    // (если вход валиден) уровень-процент от входа; выключен/нет входа → None.
    let stop = |enabled: bool, fixed: bool, level: f64| {
        if !enabled {
            None
        } else if fixed {
            fin(level)
        } else if valid_entry {
            fin(pct_stop(level))
        } else {
            None
        }
    };
    let stop_loss = stop(
        o.stops.stop_loss_enabled(),
        o.stops.stop_loss_fixed(),
        o.stops.stop_loss_level(),
    );
    let trailing = stop(
        o.stops.trailing_enabled(),
        o.stops.trailing_fixed(),
        o.stops.trailing_level(),
    );
    let take_profit = o
        .stops
        .take_profit_enabled()
        .then(|| fin(o.stops.take_profit()))
        .flatten();
    let vstop = o.vstop_on.then(|| fin(o.vstop_level)).flatten();
    let pending_cond = o.pending_buy_cond_price.and_then(fin);
    let liq = snap.markets().get(&o.market_name).and_then(|h| {
        let bp = h.balance_position();
        let v = if o.is_short {
            bp.short_liq_price
        } else {
            bp.long_liq_price
        };
        fin(v).or_else(|| fin(bp.liq_price))
    });
    let pending = o.pending_buy_cond_price.is_some();
    let filled = fill_pct > 0.0;
    let create_time_ms = moon_time_to_unix_millis_f64(o.buy_order.create_time());
    // Фолбэк-индикатор: SL/TS ядра (ClientSettings) применяются ко всем его позициям, даже если
    // у самого ордера per-order флаг не выставлен. Колонки SL/TS показывают это как «унаследовано»,
    // чтобы не вводить в заблуждение («выкл», хотя на ядре включено). price_drop_level/trailing_drop
    // > 0 = включено в основной панели ядра.
    let (sl_strat, ts_strat) = snap
        .settings()
        .client_settings
        .as_ref()
        .map(|c| (c.price_drop_level > 0.0, c.trailing_drop > 0.0))
        .unwrap_or((false, false));
    OrderRow {
        market: o.market_name.clone(),
        is_short: o.is_short,
        size,
        sl_on: o.stops.stop_loss_enabled(),
        ts_on: o.stops.trailing_enabled(),
        sl_strat,
        ts_strat,
        vstop_on: o.vstop_on,
        buy_price: entry,
        sell_price: o.sell_price,
        create_time_ms,
        price: last,
        fill_pct,
        strat,
        strat_id: o.strat_id,
        status: o.status.name().to_string(),
        uid: o.uid,
        emulator: o.emulator_mode,
        job_is_done: o.job_is_done,
        pending,
        filled,
        stop_loss,
        trailing,
        take_profit,
        vstop,
        pending_cond,
        liq,
        panic_sell: o.panic_sell,
        is_moon_shot: o.is_moon_shot,
        corridor_price_down: o.corridor_price_down,
        corridor_price_up: o.corridor_price_up,
        buy_trace: o.buy_trace_line.as_ref().and_then(order_trace),
        sell_trace: o.sell_trace_line.as_ref().and_then(order_trace),
    }
}

pub(super) fn build_order_rows(
    snap: &moonproto::MoonStateSnapshot,
    events: &[Event],
    remember_reports: bool,
    orders_index: &mut OrderIndex,
) -> Vec<OrderRow> {
    let mut order_rows = Vec::new();
    for o in snap.orders().iter() {
        order_rows.push(build_order_row(snap, o, remember_reports, orders_index));
    }

    // Snapshot is the live view. Terminal statuses can be removed from that view
    // before the app drains the event queue, so MoonProto carries an Arc<Order>
    // on Created/Updated/Removed. Overlay those captured rows onto the full live
    // snapshot: OrderLineStore still receives a complete seen-set, while closed
    // rows cannot vanish into BackstopMissing.
    for ev in events {
        let Event::Order(order_event) = ev else {
            continue;
        };
        let Some(order) = order_event.order() else {
            continue;
        };
        let row = build_order_row(snap, order, remember_reports, orders_index);
        if let Some(existing) = order_rows.iter_mut().find(|r| r.uid == row.uid) {
            *existing = row;
        } else {
            order_rows.push(row);
        }
    }

    order_rows
}
