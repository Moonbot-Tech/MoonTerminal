//! Core manual trading: placing, moving, and canceling orders.
//! Translates domain trading `CoreCmd`s into high-level moonproto handles
//! (`client.trade()` / `client.orders()`). Some `MoonOrders` mutations update the local
//! `Orders` model before sending (including replace throttling and send-if-changed). New,
//! join, split, close, and sell operations are queued or sent without inserting them into
//! the local model; this layer only invokes the handle.
//!
//! moonproto is the API source of truth: `MoonTrade::new_order` (TNewOrderCommand, CmdId=3),
//! `MoonOrders::move_order` (TOrderReplaceCommand, CmdId=6),
//! `MoonOrders::cancel` (TOrderCancelCommand, CmdId=10).

use moonproto::{
    BulkMoveKind, ClosePositionParams, MoonClient, MoveAllBuysParams, MoveAllSellsParams,
    NewOrderParams, OrderSide, OrderWorkerStatus, PositionFilter, SellOrderParams,
    SplitOrderParams, VStopParams,
};

use crate::feed::{OrderLinePriceKind, OrderStopKind};

/// Logs a trading call outcome consistently: `Ok` → `info` with `ctx`; `Err` → the same
/// context plus a `warn` with the error. Context text matches the former per-function logs
/// so log searches remain compatible.
pub(super) fn report<T, E: std::fmt::Display>(
    server_id: u64,
    ctx: impl std::fmt::Display,
    r: Result<T, E>,
) {
    match r {
        Ok(_) => log::info!("core {} {ctx}", crate::feed::core_label(server_id)),
        Err(error) => log::warn!(
            "core {} {ctx} failed: {error}",
            crate::feed::core_label(server_id)
        ),
    }
}

/// Places a new order (TNewOrderCommand). `short` is the POSITION side
/// (Long/Short, mirroring `is_short`); `strategy_id=None` sends `StratID=0`, marking a manual
/// order that may still be governed by the core's configured manual strategy. `size` is expressed
/// in the base coin, while `use_market_stop` comes from the order's confirmed group settings.
pub(super) fn place_order(
    client: &MoonClient,
    server_id: u64,
    market: String,
    short: bool,
    price: f64,
    size: f64,
    strategy_id: Option<u64>,
    use_market_stop: bool,
    planned_sell: f64,
) {
    let params = new_order_params(
        market.clone(),
        short,
        price,
        size,
        strategy_id,
        use_market_stop,
        planned_sell,
    );
    match client.trade().new_order(params) {
        Ok(_ticket) => log::info!(
            "core {} place order {market} short={short} price={price} size={size} strat={strategy_id:?}",
            crate::feed::core_label(server_id)
        ),
        Err(error) => {
            log::warn!(
                "core {} place order {market} failed: {error}",
                crate::feed::core_label(server_id)
            )
        }
    }
}

/// Build complete MoonProto order parameters, including per-order stop-market semantics.
fn new_order_params(
    market: String,
    short: bool,
    price: f64,
    size: f64,
    strategy_id: Option<u64>,
    use_market_stop: bool,
    planned_sell: f64,
) -> NewOrderParams {
    let side = if short {
        OrderSide::Short
    } else {
        OrderSide::Long
    };
    let mut params =
        NewOrderParams::new(market, side, price, size).with_market_stop(use_market_stop);
    if let Some(id) = strategy_id {
        params = params.with_strategy_id(id);
    }
    // A positive target only: the wire's zero means "no planned sell", and the core then applies
    // its own settings or the order's strategy, which is exactly what an absent target must do.
    if planned_sell.is_finite() && planned_sell > 0.0 {
        params = params.with_planned_sell_price(planned_sell);
    }
    params
}

/// Moves/replaces an existing core order identified by `uid` to a new price, as when dragging
/// its line. The runtime throttles repeats (`replace_sent_time`) and derives side/market from
/// the local order.
pub(super) fn move_order(client: &MoonClient, server_id: u64, uid: u64, new_price: f64) {
    report(
        server_id,
        format!("move order {uid} -> {new_price}"),
        client.orders().move_order(uid, new_price),
    );
}

/// Cancels a core order by `uid` (TOrderCancelCommand). The runtime derives the current status
/// from the local order; for pending (OS_None), it repeats replace-then-cancel.
pub(super) fn cancel_order(client: &MoonClient, server_id: u64, uid: u64) {
    report(
        server_id,
        format!("cancel order {uid}"),
        client.orders().cancel(uid),
    );
}

/// Toggles panic sell for a market, matching the chart button's market-level semantics.
/// This maps to `orders().switch_panic_sell_by_market`; the runtime applies the toggle to
/// the market's orders and sends the required packets.
pub(super) fn panic_sell_market(client: &MoonClient, server_id: u64, market: String, on: bool) {
    report(
        server_id,
        format!("panic sell market {market} on={on}"),
        client
            .orders()
            .switch_panic_sell_by_market(market.clone(), on),
    );
}

/// Toggles panic sell for one SPECIFIC order (`TTurnPanicSellCommand` by `uid`). The runtime
/// gates repeats through send-if-changed against its live order model.
pub(super) fn turn_order_panic_sell(client: &MoonClient, server_id: u64, uid: u64, on: bool) {
    report(
        server_id,
        format!("turn order panic sell {uid} on={on}"),
        client.orders().turn_panic_sell(uid, on),
    );
}

/// Closes a market POSITION (`TDoClosePositionCommand`, market_sell=true), as triggered by the
/// Assets row's `Market sell` button for an open position. The runtime determines the side.
pub(super) fn market_sell_position(client: &MoonClient, server_id: u64, market: String) {
    report(
        server_id,
        format!("market close position {market}"),
        client
            .trade()
            // `::new()`/`limit_orders()` performs a LIMIT close (market_sell=false), which might
            // remain unfilled and make `Market sell` appear ineffective. The button must close AT
            // MARKET, so use `market_order()` (market_sell=true); the runtime determines the side.
            .close_position(ClosePositionParams::market_order(market)),
    );
}

/// Sells a market's SPOT TOKEN at market (`TDoSellOrderCommand`), as triggered by the Assets
/// holding row's `Market sell` button. `price=0` means a market order; `size` is the base-coin
/// amount.
pub(super) fn market_sell_token(client: &MoonClient, server_id: u64, market: String, size: f64) {
    report(
        server_id,
        format!("market sell token {market} size={size}"),
        client
            .trade()
            .sell_order(SellOrderParams::new(market, 0.0, size)),
    );
}

/// Cancels a market's pending buy orders (`Cancel Buy`). Takes an OWNED snapshot, selects
/// uncanceled orders for this market in a pre-fill buy phase (`OS_None` means not yet on the
/// exchange; `BuySet` means a limit buy awaits a fill), and calls `orders().cancel(uid)` for each.
/// Filled positions (`BuyDone`/sell phases) and terminal orders remain untouched.
pub(super) fn cancel_market_buys(client: &MoonClient, server_id: u64, market: &str) {
    let Some(snap) = client.snapshot() else {
        log::warn!(
            "core {} cancel market buys {market}: no snapshot yet",
            crate::feed::core_label(server_id)
        );
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
        "core {} cancel market buys {market}: {} pending",
        crate::feed::core_label(server_id),
        uids.len()
    );
    for uid in uids {
        if let Err(error) = client.orders().cancel(uid) {
            log::warn!(
                "core {} cancel market buys {market} uid {uid} failed: {error}",
                crate::feed::core_label(server_id)
            );
        }
    }
}

/// Joins all sell orders for a market (the sell-line context-menu action). `short` is the
/// POSITION side (mirroring `is_short`) and determines `OrderSide`. Maps to `trade().join_orders`.
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

/// Shift a market's live orders of one side by a percent of their price.
///
/// The core picks the orders by their own phase — `SellSet` for sells, `BuySet` for buys, checked
/// in moonproto before the send — so no local "is it filled yet" guess is involved, and it does the
/// arithmetic. `percent` is whole percent, signed. Three consequences worth knowing:
///
/// - the percent variant deliberately INCLUDES click-immune orders, unlike the replace-kind and
///   price-zone ones (moonproto `docs/trade_actions.md`), matching the core;
/// - the phase is not the exchange side: on a short position the sell phase holds exchange buys;
/// - nothing moves locally on the press. Per-order `move_order` used to rewrite the retained price
///   on its way out, so the line jumped at once; a bulk move waits for the core's echo, like every
///   other market-level command here.
///
/// As with the other bulk actions, the log line means the intent was QUEUED: moonproto drops it
/// later and silently when the market holds no order of that phase.
pub(super) fn shift_orders_percent(
    client: &MoonClient,
    server_id: u64,
    market: String,
    sell: bool,
    percent: f64,
) {
    let side = if sell { "sells" } else { "buys" };
    let ctx = format!("shift {side} {market} by {percent:+}%");
    if sell {
        report(
            server_id,
            ctx,
            client
                .trade()
                .move_all_sells(market, MoveAllSellsParams::percent(percent)),
        );
    } else {
        report(
            server_id,
            ctx,
            client
                .trade()
                .move_all_buys(market, MoveAllBuysParams::percent(percent)),
        );
    }
}

/// Move a market's orders of one side onto a clicked price — Moonbot's Move Open / Move TP.
///
/// Everything about WHICH orders move and where each one lands is the core's: the wire carries the
/// arrangement (`ReplaceMultiKind`), one destination price and the position side. That is the whole
/// reason this is a bulk command rather than a per-order replace — Moonbot's "Parallel Shift to
/// cursor" puts the nearest line on the click and keeps the spacing of the rest, arithmetic the
/// terminal has no orders list precise enough to reproduce.
///
/// Like every other bulk send here, the log line says the intent was QUEUED, not that the core
/// acted on it.
pub(super) fn move_orders_to_price(
    client: &MoonClient,
    server_id: u64,
    market: String,
    sell: bool,
    kind: crate::config::MoveKind,
    price: f64,
    side: crate::config::MoveSide,
) {
    use crate::config::{MoveKind, MoveSide};
    let move_kind = match kind {
        // The caller drops a `None` kind before it gets here: Moonbot leaves such a gesture inert
        // rather than sending a command with no arrangement.
        MoveKind::None => return,
        MoveKind::ParallelShift => BulkMoveKind::Shift,
        MoveKind::TopVolume => BulkMoveKind::TopVolume,
        MoveKind::LowVolume => BulkMoveKind::LowVolume,
        MoveKind::TopProfit => BulkMoveKind::TopProfit,
        MoveKind::AllToOnePrice => BulkMoveKind::All,
        MoveKind::LastSet => BulkMoveKind::LastSet,
        MoveKind::LastMoved => BulkMoveKind::LastMoved,
    };
    let filter = match side {
        MoveSide::Long => PositionFilter::Long,
        MoveSide::Short => PositionFilter::Short,
        MoveSide::Both => PositionFilter::Both,
    };
    let what = if sell { "sells" } else { "buys" };
    let ctx = format!("move {what} {market} to {price:.8} kind={kind:?} side={side:?}");
    if sell {
        report(
            server_id,
            ctx,
            client.trade().move_all_sells(
                market,
                MoveAllSellsParams::replace_kind(move_kind, price, filter),
            ),
        );
    } else {
        report(
            server_id,
            ctx,
            client.trade().move_all_buys(
                market,
                MoveAllBuysParams::replace_kind(move_kind, price, filter),
            ),
        );
    }
}

/// Spread a market's sell orders evenly across a price zone — Moonbot's "sells to rectangle".
///
/// The core does the spreading; this only names the market and the two prices.
///
/// The log line says the intent was QUEUED, not that the core acted: `move_all_sells` returns as
/// soon as the intent is in, and moonproto drops it later and silently when the domain is not ready
/// or the market holds no non-immune `SellSet` order (`client/domain_trade.rs`). An empty market is
/// therefore harmless but still logs a send.
pub(super) fn sells_to_zone(
    client: &MoonClient,
    server_id: u64,
    market: String,
    min_price: f64,
    max_price: f64,
    short: bool,
) {
    // The side comes from the market's own position, exactly as `join_sells` above takes it: a zone
    // drawn on a short's chart addresses that short's exits, not a long's that happens to share the
    // market. moonproto's PriceZone candidate check ignores the side, but the wire carries it and
    // the core reads it.
    let side = if short {
        PositionFilter::Short
    } else {
        PositionFilter::Long
    };
    report(
        server_id,
        format!("sells to zone {market} {min_price:.8}..{max_price:.8} short={short}"),
        client.trade().move_all_sells(
            market,
            MoveAllSellsParams::price_zone(min_price, max_price, side),
        ),
    );
}

/// Which order a market-level split (hotkey) should target.
#[derive(Debug, PartialEq, Eq)]
enum SplitTarget {
    /// Exactly one order on the market has a live sell leg.
    One(u64),
    /// Zero or several candidates — never guess, so send nothing.
    Ambiguous,
}

/// Resolve the sole active sell order on `market`.
///
/// A market-level split hotkey carries no specific order, but protocol v4
/// addresses a split by server order uid, so the terminal must choose one.
/// Policy: target the order on `market` in the `SellSet` phase only when
/// EXACTLY ONE exists; zero or several yields `Ambiguous`, so an ambiguous
/// split is never sent to the wrong order. `candidates` are
/// `(market_name, uid, has_live_sell_leg)`.
fn resolve_market_split_target<'a>(
    candidates: impl Iterator<Item = (&'a str, u64, bool)>,
    market: &str,
) -> SplitTarget {
    let mut found: Option<u64> = None;
    for (name, uid, has_live_sell_leg) in candidates {
        if name == market && has_live_sell_leg {
            if found.is_some() {
                return SplitTarget::Ambiguous;
            }
            found = Some(uid);
        }
    }
    found.map_or(SplitTarget::Ambiguous, SplitTarget::One)
}

/// Split a specific sell order selected from a line or row into `parts`.
/// Protocol v4 addresses the order by its server `uid`.
pub(super) fn split_order(client: &MoonClient, server_id: u64, uid: u64, parts: i32) {
    report(
        server_id,
        format!("split order uid={uid} parts={parts}"),
        client
            .trade()
            .split_order(SplitOrderParams::new(uid, parts)),
    );
}

/// Split the sole active sell order for a market-level hotkey.
///
/// [`resolve_market_split_target`] selects the order because no row or line is
/// targeted. Zero or multiple candidates send nothing rather than guessing.
pub(super) fn split_order_for_market(
    client: &MoonClient,
    server_id: u64,
    market: String,
    parts: i32,
) {
    let Some(snap) = client.snapshot() else {
        log::warn!(
            "core {} split order {market}: no snapshot yet",
            crate::feed::core_label(server_id)
        );
        return;
    };
    let target = resolve_market_split_target(
        snap.orders().iter().map(|o| {
            (
                o.market_name.as_str(),
                o.uid,
                o.status == OrderWorkerStatus::SellSet,
            )
        }),
        &market,
    );
    match target {
        SplitTarget::One(uid) => report(
            server_id,
            format!("split order {market} uid={uid} parts={parts}"),
            client
                .trade()
                .split_order(SplitOrderParams::new(uid, parts)),
        ),
        SplitTarget::Ambiguous => log::warn!(
            "core {} split order {market}: not exactly one active sell order — sending nothing",
            crate::feed::core_label(server_id)
        ),
    }
}

/// Enables or disables an order's stop (SL/TS/VStop) by `uid`.
///
/// Moonbot MODEL (confirmed live on 2026-07-08): an order may have NO per-order stop of its
/// own; the stop then comes from its effective strategy, or from ClientSettings for a manual
/// order without a strategy, and will trigger even though per-order wire fields are empty
/// (zeroes). The wire does NOT distinguish `explicitly disabled` from `not set`.
/// This gives three rules:
/// - effective state (for the UI and packet assembly) = our explicit override (terminal clicks,
///   [`stop_override`]), otherwise the per-order flag, effective-strategy flag, or a nonzero
///   ClientSettings default for a manual order without a strategy;
/// - `update_stops` carries the ENTIRE StopSettings: the neighboring group (SL↔TS) is assembled
///   from EFFECTIVE parameters, or toggling SL would overwrite strategy TS with zeroes and vice
///   versa;
/// - an enabled stop's level comes from wire (≠0) → disable memory → strategy field
///   ("StopLoss"/"TrailingStop", whose strategy percentages are negative → abs) → ClientSettings
///   default (SL `price_drop_level` / TS `trailing_drop`). VStop uses wire/memory only.
/// The runtime compares against the live model through send-if-changed. SL/TS → `update_stops`,
/// VStop → `update_vstop`.
pub(super) fn set_order_stop(
    client: &MoonClient,
    server_id: u64,
    uid: u64,
    kind: OrderStopKind,
    on: bool,
) {
    let Some(snap) = client.snapshot() else {
        log::warn!(
            "core {} set order stop {uid} {kind:?}->{on}: no snapshot yet",
            crate::feed::core_label(server_id)
        );
        return;
    };
    let Some(o) = snap.orders().iter().find(|o| o.uid == uid) else {
        log::warn!(
            "core {} set order stop {uid} {kind:?}->{on}: order not tracked",
            crate::feed::core_label(server_id)
        );
        return;
    };
    // Effective order strategy: its own OR the core settings' `manual strategy`, which governs
    // manual orders with strat_id=0. A 0 means no strategy at all, so stops use ClientSettings
    // defaults. Keep this synchronized with feed/live/convert.rs (display).
    let strat_id = super::strategies::effective_strat_id(&snap, o.strat_id);
    let has_strat = snap.strats().snapshot(strat_id).is_some();
    let cs = snap.settings().client_settings.as_ref();
    let (cs_sl, cs_ts) = cs
        .map(|c| (f64::from(c.price_drop_level), f64::from(c.trailing_drop)))
        .unwrap_or((0.0, 0.0));
    log::info!(
        "core {} set order stop {uid} {kind:?}->{on}: found order emulator={} sl={} ts={} vstop={} \
         strat_id={} eff_strat={} has_strat={} cs={} price_drop={cs_sl} trailing_drop={cs_ts}",
        crate::feed::core_label(server_id),
        o.emulator_mode,
        o.stops.stop_loss_enabled(),
        o.stops.trailing_enabled(),
        o.vstop_on,
        o.strat_id,
        strat_id,
        has_strat,
        cs.is_some()
    );
    let result = match kind {
        OrderStopKind::StopLoss | OrderStopKind::Trailing => {
            let stops = o.stops;
            // The `enabled by default` flag comes from the strategy field, or, without a strategy
            // (manual order), from a nonzero core-settings default. Settings `drop` percentages
            // are NEGATIVE (price_drop_level=-1.1 → SL 1.1%): nonzero means enabled, and level
            // resolution below takes the absolute value.
            //
            // It applies only while the order still AWAITS its stops: the core materializes them at
            // the fill, and from then on the order's own flags are the state. Resolved here, at the
            // single place it is read, so the rule stays out of every resolve call below — and a
            // click on one group cannot revive the neighbour the trader switched off.
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
            let strat_ts_level =
                super::strategies::strat_field_double(&snap, strat_id, "TrailingStop")
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
            // TS: no level was found (trailing_drop is often 0 for manual orders without a
            // manual strategy), but MB enables trailing without a configured level and lets the
            // core supply its own default. Send enable with level=0. SL CANNOT do this because the
            // core rejects enable-with-zero, as confirmed by the 14:05 TAG log. If the core rejects
            // TS too, the log will show it and wire ts will remain false.
            if kind == OrderStopKind::Trailing && on && ts.is_none() {
                log::info!(
                    "core {} set order stop {uid} Trailing->on: уровень не найден — пробуем enable с level=0 (дефолт ядра)",
                    crate::feed::core_label(server_id)
                );
                ts = Some((true, false, 0.0, 0.0));
            }
            // The toggled group must resolve, or nothing is sent. If the neighboring group has no
            // level, preserve its wire state (no worse than before) but emit a warning.
            let (target, other) = if kind == OrderStopKind::StopLoss {
                (&sl, &ts)
            } else {
                (&ts, &sl)
            };
            if on && target.is_none() {
                log::warn!(
                    "core {} set order stop {uid} {kind:?}->on: нет уровня (провод/память/стратегия/дефолт пусты), не шлём",
                    crate::feed::core_label(server_id)
                );
                return;
            }
            if other.is_none() {
                log::warn!(
                    "core {} set order stop {uid} {kind:?}: у соседнего стопа нет уровня — его страта может погаснуть",
                    crate::feed::core_label(server_id)
                );
            }
            let apply_sl = |s: moonproto::StopSettings, g: &Option<(bool, bool, f64, f64)>| match g
            {
                Some((true, true, level, spread)) => s.with_stop_loss_fixed(*level, *spread),
                Some((true, false, level, spread)) => s.with_stop_loss_percent(*level, *spread),
                Some((false, ..)) => s.without_stop_loss(),
                None => s, // No level found: preserve the wire state.
            };
            let apply_ts = |s: moonproto::StopSettings, g: &Option<(bool, bool, f64, f64)>| match g
            {
                Some((true, true, level, spread)) => s.with_trailing_fixed(*level, *spread),
                Some((true, false, level, spread)) => s.with_trailing_percent(*level, *spread),
                Some((false, ..)) => s.without_trailing(),
                None => s,
            };
            let next = apply_ts(apply_sl(stops, &sl), &ts);
            // First-OFF PRIMER for a default stop: per-order wire fields are empty (zeroes), and
            // `disable` (without_*) is bit-for-bit identical to the local model. moonproto's
            // send_stops_if_changed suppresses the packet, the core receives NOTHING, and the stop
            // stays enabled by strategy/settings default. First materialize the effective per-order
            // stop (enable at the current level is behaviorally a no-op because it is already armed),
            // then send the ordinary OFF; both packets differ from the model and reach the core.
            let target_wire_on = if kind == OrderStopKind::StopLoss {
                stops.stop_loss_enabled()
            } else {
                stops.trailing_enabled()
            };
            if !on && !target_wire_on {
                let enable = if kind == OrderStopKind::StopLoss {
                    sl_resolve(Some(true))
                } else {
                    // TS primer: without a level, enable at level=0 (the core default; see above).
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
                        "core {} set order stop {uid} {kind:?}->off: праймер без уровня — первый OFF может заглушиться send-if-changed",
                        crate::feed::core_label(server_id)
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
                // VStop has no default; only wire state or memory can provide its parameters.
                let Some((fixed, level, vol)) = restore_from_wire_or_memory(
                    server_id,
                    uid,
                    kind,
                    o.vstop_fixed,
                    o.vstop_level,
                    o.vstop_vol,
                ) else {
                    log::warn!(
                        "core {} set order stop {uid} {kind:?}->on: нет уровня (провод/память пусты), не шлём",
                        crate::feed::core_label(server_id)
                    );
                    return;
                };
                if fixed {
                    VStopParams::fixed(level, vol)
                } else {
                    VStopParams::percent(level, vol)
                }
            } else {
                // Primer as for SL/TS: send_vstop_if_changed suppresses disable when the wire is
                // empty (the model is already all zeroes). A primer is impossible without a level,
                // so warn in that case.
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
                            "core {} set order stop {uid} {kind:?}->off: праймер без уровня — первый OFF может заглушиться send-if-changed",
                            crate::feed::core_label(server_id)
                        );
                    }
                }
                remember_stop_params(
                    server_id,
                    uid,
                    kind,
                    o.vstop_fixed,
                    o.vstop_level,
                    o.vstop_vol,
                );
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

/// Resolves one stop group's effective `(enabled, fixed, level, spread)` parameters for a complete
/// StopSettings. `forced=Some(x)` identifies the toggled group and its click target; `None`
/// identifies the neighboring group, whose state is preserved so that toggling SL does not wipe TS
/// and vice versa.
///
/// `strat_on` is what the strategy would give an order that still awaits one, so callers clear it
/// once the entry has filled ([`crate::feed::order_entry_filled`]): the core materializes the stop
/// at the fill, and from then on the order's own flag is the state. Inheriting past that
/// point is how a click on the NEIGHBOUR revived a stop-loss the trader had switched off — the
/// packet carries the whole StopSettings, so a neighbour resolved as `wire_on || strat_on` re-armed
/// SL in the core while the table, reading the wire, still showed it OFF.
///
/// Returns `None` when the group should be enabled but no level can be found.
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
    // Level precedence: wire → disable memory → strategy (strategy percentages are negative,
    // `drop by N%` → abs) → ClientSettings default.
    if wire_level != 0.0 && wire_level.is_finite() {
        return Some((true, wire_fixed, wire_level, wire_spread));
    }
    if let Some((fixed, level, spread)) = stop_memory_get(server_id, uid, kind) {
        return Some((true, fixed, level, spread));
    }
    if let Some(level) = strat_level.filter(|l| *l != 0.0 && l.is_finite()) {
        return Some((true, false, level.abs(), 0.0));
    }
    // Settings defaults are also negative (`drop by N%`), so take the absolute value.
    let pct = default_pct.filter(|p| *p != 0.0 && p.is_finite())?;
    Some((true, false, pct.abs(), 0.0))
}

/// Restores parameters from wire → memory without strategy/default fallbacks (VStop).
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

/// Explicit per-order stop overrides made FROM THE TERMINAL, scoped to the session.
/// The wire does not distinguish `explicitly disabled` from `not set` (both are zeroes), so the
/// strategy flag would otherwise mask our OFF in the table.
/// Key: (core, uid) → [Option<target flag>; 3].
fn stop_overrides_map()
-> &'static std::sync::Mutex<std::collections::HashMap<(u64, u64), [Option<bool>; 3]>> {
    static MEM: std::sync::OnceLock<
        std::sync::Mutex<std::collections::HashMap<(u64, u64), [Option<bool>; 3]>>,
    > = std::sync::OnceLock::new();
    MEM.get_or_init(Default::default)
}

/// Records an override when a table toggle or form edit is sent.
pub(super) fn note_stop_override(server_id: u64, uid: u64, kind: OrderStopKind, on: bool) {
    stop_overrides_map()
        .lock()
        .unwrap()
        .entry((server_id, uid))
        .or_default()[stop_kind_tag(kind) as usize] = Some(on);
}

/// Reads an override for the UI or packet assembly as the effective SL/TS/VStop flag.
/// Expiration invariant: the override lives while the wire AGREES with its target
/// (`target == wire_now`; our own sends immediately move the local model to the target).
/// A conflicting wire value means the per-order stop changed ON THE OTHER SIDE (core/MB),
/// so the override is discarded and wire/strategy takes effect. Without this, a server-side ON
/// would be masked forever by our old OFF.
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

/// Remembers stop parameters erased by disabling. Key: (core, order uid, stop kind) →
/// (fixed, level, spread|vol). It lives until process exit and holds only a handful of entries
/// per session because writes occur only when a stop is manually disabled from the order table
/// or the order-edit form.
fn stop_memory()
-> &'static std::sync::Mutex<std::collections::HashMap<(u64, u64, u8), (bool, f64, f64)>> {
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

/// Remembers stop parameters before disabling when the level is meaningful.
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

/// Remembers an SL/TS group from StopSettings before disabling it.
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

/// Moves an order's stop/take line (dragged on the chart) to absolute `price`. SL/TS become FIXED
/// price stops through `with_stop_loss_fixed`/`with_trailing_fixed`, preserving the current spread;
/// take-profit uses `with_take_profit_price`. Other order stops remain unchanged because each
/// StopSettings builder touches only its own field group. The runtime gates the send through
/// send-if-changed against the live model.
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
        log::warn!(
            "core {} move order stop price {uid} {kind:?}: no snapshot yet",
            crate::feed::core_label(server_id)
        );
        return;
    };
    let Some(o) = snap.orders().iter().find(|o| o.uid == uid) else {
        log::warn!(
            "core {} move order stop price {uid} {kind:?}: order not tracked",
            crate::feed::core_label(server_id)
        );
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

#[cfg(test)]
mod tests;
