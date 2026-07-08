//! Применение формы правок стопов из окна «Активный ордер» (диалог редактирования ордера).
//! Родственно [`trade::set_order_stop`] (тогл одного стопа кликом в таблице), но применяет
//! ЦЕЛЕВОЕ состояние сразу нескольких групп: SL/TS/TP собираются в ОДИН `update_stops`
//! (нетронутые `None`-группы — из ЭФФЕКТИВНЫХ параметров, чтобы не затереть стратегийный
//! стоп нулями), VStop — отдельным `update_vstop`. Иерархия истины уровня та же, что в
//! `set_order_stop`: явная правка формы → провод → память выключения → стратегия → дефолт
//! ClientSettings. Праймер первого OFF по дефолт-стопу — тоже как там (send-if-changed
//! глушит «выключить» при пустом проводе, сначала материализуем эффективный стоп).

use moonproto::{MoonClient, VStopParams};

use super::trade;
use super::{OrderStopKind, OrderStopsForm, StopGroupEdit};

/// Целевые параметры stop-группы `(enabled, fixed, level, spread)` по правке формы.
/// `edit=None` (группа не менялась) → эффективное сохранение (как соседняя группа в
/// `set_order_stop`). `None`-результат = уровень не разрешился — группу на проводе не трогаем.
#[allow(clippy::too_many_arguments)]
fn target_group(
    server_id: u64,
    uid: u64,
    kind: OrderStopKind,
    edit: Option<StopGroupEdit>,
    wire_on: bool,
    wire_fixed: bool,
    wire_level: f64,
    wire_spread: f64,
    strat_on: bool,
    strat_level: Option<f64>,
    cs_default: f64,
    // TS можно включать с level=0 (ядро дефолтит уровень само); SL — нельзя (отвергает).
    allow_zero_level: bool,
) -> Option<(bool, bool, f64, f64)> {
    let resolve = |forced: Option<bool>| {
        trade::resolve_stop_group(
            server_id,
            uid,
            kind,
            forced,
            wire_on,
            wire_fixed,
            wire_level,
            wire_spread,
            strat_on,
            strat_level,
            Some(cs_default),
        )
    };
    let Some(edit) = edit else {
        return resolve(None);
    };
    if !edit.on {
        return Some((false, false, 0.0, 0.0));
    }
    if edit.fixed {
        if edit.price > 0.0 && edit.price.is_finite() {
            return Some((true, true, edit.price, wire_spread));
        }
        // Невалидная фиксированная цена — обычное включение (провод/память/стратегия).
        return resolve(Some(true));
    }
    // Глобальный (процентный) режим.
    if wire_fixed {
        // Возврат fixed → глобальный: провод хранит ЦЕНУ, процент берём из стратегии/дефолта.
        if let Some(level) = strat_level.filter(|l| *l != 0.0 && l.is_finite()) {
            return Some((true, false, level.abs(), 0.0));
        }
        if cs_default != 0.0 && cs_default.is_finite() {
            return Some((true, false, cs_default.abs(), 0.0));
        }
        return if allow_zero_level {
            Some((true, false, 0.0, 0.0))
        } else {
            log::warn!(
                "core {server_id} order form {uid} {kind:?}: возврат к глобальному без уровня стратегии/дефолта — оставляем fixed"
            );
            resolve(Some(true))
        };
    }
    resolve(Some(true)).or_else(|| allow_zero_level.then_some((true, false, 0.0, 0.0)))
}

/// Применить форму правок стопов ордера `uid` (окно «Активный ордер»).
pub(super) fn update_order_stops_form(
    client: &MoonClient,
    server_id: u64,
    uid: u64,
    form: OrderStopsForm,
) {
    if form.is_empty() {
        return;
    }
    let Some(snap) = client.snapshot() else {
        log::warn!("core {server_id} order form {uid}: no snapshot yet");
        return;
    };
    let Some(o) = snap.orders().iter().find(|o| o.uid == uid) else {
        log::warn!("core {server_id} order form {uid}: order not tracked");
        return;
    };
    log::info!("core {server_id} order form {uid}: {form:?}");

    if form.sl.is_some() || form.ts.is_some() || form.tp.is_some() {
        let stops = o.stops;
        // Контекст стратегии/дефолтов — как в trade::set_order_stop.
        let strat_id = super::strategies::effective_strat_id(&snap, o.strat_id);
        let has_strat = snap.strats().snapshot(strat_id).is_some();
        let (cs_sl, cs_ts) = snap
            .settings()
            .client_settings
            .as_ref()
            .map(|c| (f64::from(c.price_drop_level), f64::from(c.trailing_drop)))
            .unwrap_or((0.0, 0.0));
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

        let sl = target_group(
            server_id,
            uid,
            OrderStopKind::StopLoss,
            form.sl,
            stops.stop_loss_enabled(),
            stops.stop_loss_fixed(),
            stops.stop_loss_level(),
            stops.stop_loss_spread(),
            strat_sl_on,
            strat_sl_level,
            cs_sl,
            false,
        );
        let ts = target_group(
            server_id,
            uid,
            OrderStopKind::Trailing,
            form.ts,
            stops.trailing_enabled(),
            stops.trailing_fixed(),
            stops.trailing_level(),
            stops.trailing_spread(),
            strat_ts_on,
            strat_ts_level,
            cs_ts,
            true,
        );
        let apply_sl = |s: moonproto::StopSettings, g: &Option<(bool, bool, f64, f64)>| match g {
            Some((true, true, level, spread)) => s.with_stop_loss_fixed(*level, *spread),
            Some((true, false, level, spread)) => s.with_stop_loss_percent(*level, *spread),
            Some((false, ..)) => s.without_stop_loss(),
            None => s, // уровень не разрешился — провод как есть
        };
        let apply_ts = |s: moonproto::StopSettings, g: &Option<(bool, bool, f64, f64)>| match g {
            Some((true, true, level, spread)) => s.with_trailing_fixed(*level, *spread),
            Some((true, false, level, spread)) => s.with_trailing_percent(*level, *spread),
            Some((false, ..)) => s.without_trailing(),
            None => s,
        };
        let apply_tp = |s: moonproto::StopSettings| match form.tp {
            Some(tp) if tp.on && tp.price > 0.0 && tp.price.is_finite() => {
                s.with_take_profit_price(tp.price)
            }
            Some(tp) if !tp.on => s.without_take_profit(),
            _ => s, // не менялся / on с невалидной ценой — провод как есть
        };

        // ПРАЙМЕР первого OFF по дефолт-стопу (см. set_order_stop): per-order поля на
        // проводе пустые и «выключить» совпадает с локальной моделью — send-if-changed
        // глушит пакет. Материализуем эффективный стоп per-order, затем обычный OFF.
        let primer_groups: [(OrderStopKind, Option<StopGroupEdit>, bool); 2] = [
            (OrderStopKind::StopLoss, form.sl, stops.stop_loss_enabled()),
            (OrderStopKind::Trailing, form.ts, stops.trailing_enabled()),
        ];
        for (kind, edit, wire_on) in primer_groups {
            let turning_off = edit.is_some_and(|e| !e.on);
            if !(turning_off && !wire_on) {
                continue;
            }
            let enable = target_group(
                server_id,
                uid,
                kind,
                Some(StopGroupEdit {
                    on: true,
                    fixed: false,
                    price: 0.0,
                }),
                wire_on,
                if kind == OrderStopKind::StopLoss {
                    stops.stop_loss_fixed()
                } else {
                    stops.trailing_fixed()
                },
                if kind == OrderStopKind::StopLoss {
                    stops.stop_loss_level()
                } else {
                    stops.trailing_level()
                },
                if kind == OrderStopKind::StopLoss {
                    stops.stop_loss_spread()
                } else {
                    stops.trailing_spread()
                },
                if kind == OrderStopKind::StopLoss {
                    strat_sl_on
                } else {
                    strat_ts_on
                },
                if kind == OrderStopKind::StopLoss {
                    strat_sl_level
                } else {
                    strat_ts_level
                },
                if kind == OrderStopKind::StopLoss {
                    cs_sl
                } else {
                    cs_ts
                },
                kind == OrderStopKind::Trailing,
            );
            if let Some((true, ..)) = enable {
                let primer = if kind == OrderStopKind::StopLoss {
                    apply_tp(apply_ts(apply_sl(stops, &enable), &ts))
                } else {
                    apply_tp(apply_ts(apply_sl(stops, &sl), &enable))
                };
                trade::report(
                    server_id,
                    format!("order form {uid} {kind:?} primer(on)"),
                    client.orders().update_stops(uid, primer),
                );
            } else {
                log::warn!(
                    "core {server_id} order form {uid} {kind:?}->off: праймер без уровня — первый OFF может заглушиться send-if-changed"
                );
            }
        }

        // Память уровня перед выключением + явный override терминала для правленых групп.
        for (kind, edit) in [
            (OrderStopKind::StopLoss, form.sl),
            (OrderStopKind::Trailing, form.ts),
        ] {
            if let Some(e) = edit {
                if !e.on {
                    trade::remember_stop_group(server_id, uid, kind, &stops);
                }
                trade::note_stop_override(server_id, uid, kind, e.on);
            }
        }

        let next = apply_tp(apply_ts(apply_sl(stops, &sl), &ts));
        trade::report(
            server_id,
            format!("order form {uid} stops"),
            client.orders().update_stops(uid, next),
        );
    }

    if let Some(v) = form.vstop {
        // Выключение и включение-без-уровня — через set_order_stop (праймер против
        // send-if-changed + память уровня). Явный уровень формы — прямым update_vstop.
        if !v.on {
            trade::set_order_stop(client, server_id, uid, OrderStopKind::VStop, false);
        } else if v.level > 0.0 && v.level.is_finite() {
            let params = if v.fixed {
                VStopParams::fixed(v.level, v.vol)
            } else {
                VStopParams::percent(v.level, v.vol)
            };
            trade::note_stop_override(server_id, uid, OrderStopKind::VStop, true);
            trade::report(
                server_id,
                format!(
                    "order form {uid} vstop fixed={} level={} vol={}",
                    v.fixed, v.level, v.vol
                ),
                client.orders().update_vstop(uid, params),
            );
        } else {
            trade::set_order_stop(client, server_id, uid, OrderStopKind::VStop, true);
        }
    }
}
