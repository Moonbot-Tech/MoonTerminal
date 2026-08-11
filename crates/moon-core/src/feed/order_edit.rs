//! Applies stop edits from the Active Order window's order-edit dialog.
//! This resembles [`trade::set_order_stop`] (toggling one stop from the table), but applies the
//! TARGET state of several groups at once: SL/TS/TP are assembled into ONE `update_stops`
//! call (untouched `None` groups come from EFFECTIVE parameters so a strategy stop is not
//! overwritten with zeroes), while VStop uses a separate `update_vstop`. Level resolution uses
//! the same precedence as `set_order_stop`: explicit form edit → wire → disable memory → strategy
//! → ClientSettings default. The first-OFF primer for a default stop is also the same: when the
//! wire is empty, send-if-changed suppresses `disable`, so the effective stop is materialized first.

use moonproto::{MoonClient, VStopParams};

use super::trade;
use super::{OrderStopKind, OrderStopsForm, StopGroupEdit};

/// Resolves target `(enabled, fixed, level, spread)` parameters for a stop-group form edit.
/// `edit=None` (the group was unchanged) preserves its effective state, like the neighboring
/// group in `set_order_stop`. A `None` result means no level was resolved, so the wire group stays.
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
    // TS can be enabled with level=0 (the core supplies its own default); SL rejects level=0.
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
        // An invalid fixed price falls back to normal enable resolution
        // (wire/memory/strategy/ClientSettings).
        return resolve(Some(true));
    }
    // Global (percentage) mode.
    if wire_fixed {
        // Switching fixed → global: the wire stores a PRICE, so take the percentage from
        // strategy/default.
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
                "core {} order form {uid} {kind:?}: возврат к глобальному без уровня стратегии/дефолта — оставляем fixed"
            , crate::feed::core_label(server_id));
            resolve(Some(true))
        };
    }
    resolve(Some(true)).or_else(|| allow_zero_level.then_some((true, false, 0.0, 0.0)))
}

/// Applies stop edits for order `uid` from the Active Order window.
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
        log::warn!(
            "core {} order form {uid}: no snapshot yet",
            crate::feed::core_label(server_id)
        );
        return;
    };
    let Some(o) = snap.orders().iter().find(|o| o.uid == uid) else {
        log::warn!(
            "core {} order form {uid}: order not tracked",
            crate::feed::core_label(server_id)
        );
        return;
    };
    log::info!(
        "core {} order form {uid}: {form:?}",
        crate::feed::core_label(server_id)
    );

    if form.sl.is_some() || form.ts.is_some() || form.tp.is_some() {
        let stops = o.stops;
        // Strategy/default context matches trade::set_order_stop.
        let strat_id = super::strategies::effective_strat_id(&snap, o.strat_id);
        let has_strat = snap.strats().snapshot(strat_id).is_some();
        let (cs_sl, cs_ts) = snap
            .settings()
            .client_settings
            .as_ref()
            .map(|c| (f64::from(c.price_drop_level), f64::from(c.trailing_drop)))
            .unwrap_or((0.0, 0.0));
        // Strategy inheritance ends where the position begins: from the fill on, the order's own
        // stops are the state, so an untouched group in this form goes back as the core holds it —
        // not re-armed from the strategy behind an unrelated take-profit edit. Resolved once, here,
        // rather than carried down into every group resolution.
        let entry_filled = crate::feed::order_entry_filled(o);
        let inherited =
            |strat_on| crate::feed::stop_inherited_from_strategy(entry_filled, strat_on);
        let strat_sl_on = inherited(if has_strat {
            super::strategies::strat_field_bool(&snap, strat_id, "UseStopLoss")
        } else {
            o.strat_id == 0 && cs_sl != 0.0
        });
        let strat_ts_on = inherited(if has_strat {
            super::strategies::strat_field_bool(&snap, strat_id, "UseTrailing")
        } else {
            o.strat_id == 0 && cs_ts != 0.0
        });
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
            None => s, // No level resolved: preserve the wire state.
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
            _ => s, // Unchanged, or enabled with an invalid price: preserve the wire state.
        };

        // First-OFF PRIMER for a default stop (see set_order_stop): per-order wire fields are
        // empty, so `disable` matches the local model and send-if-changed suppresses the packet.
        // Materialize the effective per-order stop first, then send the ordinary OFF.
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
                    "core {} order form {uid} {kind:?}->off: праймер без уровня — первый OFF может заглушиться send-if-changed"
                , crate::feed::core_label(server_id));
            }
        }

        // Remember the level before disabling and record explicit terminal overrides for
        // edited groups.
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
        // Disable and enable-without-level go through set_order_stop (the send-if-changed primer
        // plus level memory). An explicit form level goes directly through update_vstop.
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
