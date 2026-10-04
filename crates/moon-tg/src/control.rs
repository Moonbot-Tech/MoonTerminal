//! The owner's commands to the cores, in one place for the Mini App and the bot's Control
//! section: the owner check, the choice of cores the owner sees, and the session calls the
//! desktop makes for the same buttons.
//!
//! Every function here checks the owner first and sends nothing to a core that is not configured;
//! the run switches and "cancel all" go only to connected cores (`dispatch_run`, K1). Callers turn
//! a [`Refusal`] into their own words: the Mini App's error codes, the chat's lines.

use std::time::Instant;

use moon_core::config::telegram_access::TelegramReportAccess;
use moon_core::feed::OrderRow;
use moon_core::session::{CoreId, RunDispatch, RunSwitch};
use moon_core::telegram::menu_action::OrderBan;
use moon_core::telegram::web::dto::StrategyPendingDto;

use crate::TgHost;

/// Why a command was not carried out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Refusal {
    /// The chat is not the owner.
    NotOwner,
    /// No such core, order or market among what the owner sees.
    NotFound,
    /// The core is not connected; nothing was sent.
    Offline,
    /// The core's command channel refused the send.
    Unavailable,
}

/// Whether `chat` is the owner: the one check every command passes first.
pub(crate) fn is_owner(host: &dyn TgHost, chat: i64) -> bool {
    host.config().telegram.report_access(chat) == Some(TelegramReportAccess::Owner)
}

/// [`is_owner`] as a gate.
fn owner(host: &dyn TgHost, chat: i64) -> Result<(), Refusal> {
    if is_owner(host, chat) {
        Ok(())
    } else {
        Err(Refusal::NotOwner)
    }
}

/// The cores the owner sees, with their names, in the order the bot lists them.
pub(crate) fn owner_cores(host: &dyn TgHost) -> Vec<(CoreId, String)> {
    crate::mini_app::visible_cores(host, &TelegramReportAccess::Owner)
        .into_iter()
        .map(|(id, name, _)| (id, name))
        .collect()
}

/// Whether `core` is one of the cores the owner sees.
fn known(host: &dyn TgHost, core: CoreId) -> bool {
    owner_cores(host).iter().any(|(id, _)| *id == core)
}

/// Set one run switch on one core: sent only when the core is connected and not already there.
///
/// Returns:
///     `Ok` once sent, or when the core already is in the asked state. A core the owner does not
///     see is `NotFound`; one not connected, `Offline`; a refused send, `Unavailable`.
pub(crate) fn run_one(
    host: &dyn TgHost,
    chat: i64,
    core: CoreId,
    switch: RunSwitch,
    on: bool,
) -> Result<(), Refusal> {
    owner(host, chat)?;
    if !known(host, core) {
        return Err(Refusal::NotFound);
    }
    let outcome = host.session().dispatch_run(&[core], switch, on);
    if outcome.offline > 0 {
        return Err(Refusal::Offline);
    }
    if outcome.refused() > 0 {
        return Err(Refusal::Unavailable);
    }
    Ok(())
}

/// Set one run switch on several cores in one gated call.
///
/// Args:
///     cores: Requested ids; unknown ids and repeats are dropped before sending.
///
/// Returns:
///     The cores addressed and what happened to each. None of them known is `NotFound`.
pub(crate) fn run_many(
    host: &dyn TgHost,
    chat: i64,
    cores: &[CoreId],
    switch: RunSwitch,
    on: bool,
) -> Result<(Vec<CoreId>, RunDispatch), Refusal> {
    owner(host, chat)?;
    let visible: Vec<CoreId> = owner_cores(host).into_iter().map(|(id, _)| id).collect();
    let targets = crate::mini_app::scope_targets(cores, &visible);
    if targets.is_empty() {
        return Err(Refusal::NotFound);
    }
    let outcome = host.session().dispatch_run(&targets, switch, on);
    Ok((targets, outcome))
}

/// Cancel every open order of one connected core.
pub(crate) fn cancel_all(host: &mut dyn TgHost, chat: i64, core: CoreId) -> Result<(), Refusal> {
    owner(host, chat)?;
    if !known(host, core) {
        return Err(Refusal::NotFound);
    }
    // The session refuses an unconnected core too; checking first names the reason.
    if !host.session().core_run_state(core).online {
        return Err(Refusal::Offline);
    }
    host.session_mut()
        .cancel_all_orders(core)
        .map_err(|_| Refusal::Unavailable)
}

/// Cancel one open order.
///
/// Returns:
///     `NotFound` when the order is not on the core's open orders; nothing is sent then.
pub(crate) fn cancel_order(
    host: &mut dyn TgHost,
    chat: i64,
    core: CoreId,
    uid: u64,
) -> Result<(), Refusal> {
    owner(host, chat)?;
    let listed = host
        .session()
        .store()
        .core(core)
        .is_some_and(|data| data.orders.iter().any(|order| order.uid == uid));
    if !listed {
        return Err(Refusal::NotFound);
    }
    host.session_mut()
        .cancel_order(core, uid)
        .map_err(|_| Refusal::Unavailable)
}

/// Arm or disarm Panic Sell on one market, toggled only when it is not already in the asked state.
///
/// Returns:
///     The armed state after the call. A core that is not there, or a market not on its open
///     orders, is `NotFound`; a refused toggle, `Unavailable`.
pub(crate) fn panic_market(
    host: &mut dyn TgHost,
    chat: i64,
    core: CoreId,
    market: &str,
    on: bool,
) -> Result<bool, Refusal> {
    owner(host, chat)?;
    let Some(data) = host.session().store().core(core) else {
        return Err(Refusal::NotFound);
    };
    if !data.orders.iter().any(|order| order.market == market) {
        return Err(Refusal::NotFound);
    }
    if host.is_panic_armed(core, market) == on {
        return Ok(on);
    }
    if !host.toggle_panic_sell(core, market.to_string()) {
        return Err(Refusal::Unavailable);
    }
    Ok(host.is_panic_armed(core, market))
}

/// What [`panic_all`] did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct PanicAll {
    /// Markets armed now.
    pub armed: usize,
    /// Markets that already were.
    pub already: usize,
    /// Cores skipped as not connected.
    pub offline: usize,
    /// Markets whose toggle was refused.
    pub refused: usize,
}

/// Panic-sell every market with an open position on each of `cores`, one market at a time, as
/// the desktop's Panic Sell button does.
///
/// Args:
///     cores: Requested ids; unknown ids and repeats are dropped.
///
/// Returns:
///     What happened. None of the cores known is `NotFound`.
pub(crate) fn panic_all(
    host: &mut dyn TgHost,
    chat: i64,
    cores: &[CoreId],
) -> Result<PanicAll, Refusal> {
    owner(host, chat)?;
    let visible: Vec<CoreId> = owner_cores(host).into_iter().map(|(id, _)| id).collect();
    let targets = crate::mini_app::scope_targets(cores, &visible);
    if targets.is_empty() {
        return Err(Refusal::NotFound);
    }
    let mut done = PanicAll::default();
    for core in targets {
        if !host.session().core_run_state(core).online {
            done.offline += 1;
            continue;
        }
        for market in open_position_markets(host, core) {
            if host.is_panic_armed(core, &market) {
                done.already += 1;
            } else if host.toggle_panic_sell(core, market) {
                done.armed += 1;
            } else {
                done.refused += 1;
            }
        }
    }
    Ok(done)
}

/// Markets where `core` holds an open position — an entry filled and not yet closed — each once.
pub(crate) fn open_position_markets(host: &dyn TgHost, core: CoreId) -> Vec<String> {
    let mut markets: Vec<String> = Vec::new();
    if let Some(data) = host.session().store().core(core) {
        for order in &data.orders {
            if order.filled && !order.job_is_done && !markets.contains(&order.market) {
                markets.push(order.market.clone());
            }
        }
    }
    markets
}

/// Queue one core for the host's reconnect path; the next status read shows the outcome.
pub(crate) fn reconnect(host: &mut dyn TgHost, chat: i64, core: CoreId) -> Result<(), Refusal> {
    owner(host, chat)?;
    if !known(host, core) {
        return Err(Refusal::NotFound);
    }
    host.request_reconnect(core);
    Ok(())
}

/// Turn one strategy on or off without touching the core's strategy engine.
///
/// A toggle already on its way to the same state is not sent twice: the pending entry lives
/// until the core's echo or its timeout (`mini_app::dto::strategy_pending`).
///
/// Returns:
///     `Ok` once sent or already pending. A core or strategy the owner does not see is
///     `NotFound`; a refused send, `Unavailable`.
pub(crate) fn strategy_toggle(
    host: &mut dyn TgHost,
    chat: i64,
    core: CoreId,
    id: u64,
    on: bool,
) -> Result<(), Refusal> {
    owner(host, chat)?;
    if !known(host, core) {
        return Err(Refusal::NotFound);
    }
    let now = Instant::now();
    let listed = host.session().store().core(core).and_then(|data| {
        data.strategies
            .iter()
            .find(|row| row.id == id)
            .map(|row| (data.strategies_ack_rev, data.strategies_rev, row.checked))
    });
    let Some((ack_before, rev_before, checked)) = listed else {
        return Err(Refusal::NotFound);
    };
    let already = host
        .state()
        .mini_strategy_wanted
        .get(&(core, id))
        .is_some_and(|entry| {
            entry.0 == on
                && crate::mini_app::dto::strategy_pending(
                    *entry, ack_before, rev_before, checked, now,
                ) == Some(StrategyPendingDto::Pending)
        });
    if already {
        return Ok(());
    }
    host.session_mut()
        .apply_strategies(core, vec![(id, on)], None)
        .map_err(|_| Refusal::Unavailable)?;
    host.state_mut()
        .mini_strategy_wanted
        .insert((core, id), (on, now, ack_before, rev_before));
    Ok(())
}

/// Whether a toggle of `core`'s strategy `id` is still on its way: sent and neither echoed by the
/// core nor timed out.
pub(crate) fn strategy_waiting(host: &dyn TgHost, core: CoreId, id: u64) -> bool {
    let Some(entry) = host.state().mini_strategy_wanted.get(&(core, id)).copied() else {
        return false;
    };
    let Some(data) = host.session().store().core(core) else {
        return false;
    };
    let Some(row) = data.strategies.iter().find(|row| row.id == id) else {
        return false;
    };
    crate::mini_app::dto::strategy_pending(
        entry,
        data.strategies_ack_rev,
        data.strategies_rev,
        row.checked,
        Instant::now(),
    ) == Some(StrategyPendingDto::Pending)
}

/// Put one coin on the core's own blacklist, or with `lift` take it off.
///
/// Returns:
///     Whether a command was sent; `false` when the list already said so.
pub(crate) fn core_blacklist(
    host: &dyn TgHost,
    chat: i64,
    core: CoreId,
    coin: &str,
    lift: bool,
) -> Result<bool, Refusal> {
    owner(host, chat)?;
    if !known(host, core) {
        return Err(Refusal::NotFound);
    }
    if !host.session().core_run_state(core).online {
        return Err(Refusal::Offline);
    }
    host.session()
        .write_core_blacklist(core, coin, lift)
        .map_err(|_| Refusal::Unavailable)
}

/// `core`'s own blacklist as it last sent it: whether it is on, and its coins.
pub(crate) fn core_blacklist_state(host: &dyn TgHost, core: CoreId) -> Option<(bool, Vec<String>)> {
    let settings = host
        .session()
        .store()
        .core(core)?
        .client_settings
        .as_ref()?;
    let coins = settings
        .blacklist_text
        .split(',')
        .map(str::trim)
        .filter(|coin| !coin.is_empty())
        .map(str::to_string)
        .collect();
    Some((settings.use_blacklist, coins))
}

/// `core`'s open positions — entries filled and not yet closed — newest first.
pub(crate) fn open_orders(host: &dyn TgHost, core: CoreId) -> Vec<OrderRow> {
    let mut orders: Vec<OrderRow> = host
        .session()
        .store()
        .core(core)
        .map(|data| {
            data.orders
                .iter()
                .filter(|order| order.filled && !order.job_is_done)
                .cloned()
                .collect()
        })
        .unwrap_or_default();
    orders.sort_by_key(|order| std::cmp::Reverse(order.uid));
    orders
}

/// One of `core`'s open positions by its uid.
pub(crate) fn open_order(host: &dyn TgHost, core: CoreId, uid: u64) -> Option<OrderRow> {
    open_orders(host, core)
        .into_iter()
        .find(|order| order.uid == uid)
}

/// An open position of a connected core the owner sees, for a command on it.
fn order_for_command(
    host: &dyn TgHost,
    chat: i64,
    core: CoreId,
    uid: u64,
) -> Result<OrderRow, Refusal> {
    owner(host, chat)?;
    if !known(host, core) {
        return Err(Refusal::NotFound);
    }
    let order = open_order(host, core, uid).ok_or(Refusal::NotFound)?;
    if !host.session().core_run_state(core).online {
        return Err(Refusal::Offline);
    }
    Ok(order)
}

/// Panic-sell one open position.
pub(crate) fn order_panic(
    host: &mut dyn TgHost,
    chat: i64,
    core: CoreId,
    uid: u64,
) -> Result<(), Refusal> {
    order_for_command(host, chat, core, uid)?;
    host.session()
        .turn_order_panic_sell(core, uid, true)
        .map_err(|_| Refusal::Unavailable)
}

/// Put an open position's coin on a blacklist: the core's own, its strategy's `CoinsBlackList`,
/// or the core's temporary list for a span, keyed by the order's market.
///
/// Returns:
///     Whether anything was sent; `false` when the coin already was listed. A manual order, or a
///     strategy whose kind has no coin list, is `NotFound` for the strategy's list.
pub(crate) fn order_ban(
    host: &mut dyn TgHost,
    chat: i64,
    core: CoreId,
    uid: u64,
    ban: OrderBan,
) -> Result<bool, Refusal> {
    let order = order_for_command(host, chat, core, uid)?;
    let sent = match ban {
        OrderBan::Core => host
            .session()
            .write_core_blacklist(core, &order.coin, false),
        OrderBan::Strategy => {
            let listed =
                order.strat_id != 0 && host.session().strategy_has_blacklist(core, order.strat_id);
            if !listed {
                return Err(Refusal::NotFound);
            }
            host.session()
                .write_strategy_blacklist(core, order.strat_id, &order.coin, false)
        }
        OrderBan::Temp(span) => host
            .session()
            .set_temp_ban(core, order.market.clone(), Some(span.duration()))
            .map(|()| true),
    };
    sent.map_err(|_| Refusal::Unavailable)
}
