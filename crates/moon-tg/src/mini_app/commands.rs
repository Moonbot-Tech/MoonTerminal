//! The owner's money and control commands, through the same session calls the desktop uses.

use std::time::Instant;

use moon_core::session::CoreId;
use moon_core::telegram::web::MiniAppApiError;
use moon_core::telegram::web::dto::{
    CommandErrorDto, CommandResultDto, CoreSwitchDto, ScopeResultDto, StrategyPendingDto,
};

use super::dto::{command_hit, command_miss, strategy_pending};
use super::{mini_owner, scope_targets, visible_cores};
use crate::TgHost;
use moon_core::config::telegram_access::TelegramReportAccess;

/// Cancel one open order through the same session call the desktop uses.
///
/// Args:
///     chat_id: Paired chat that sent the command.
///     core: Core id. `CoreId` is a `u64`.
///     uid: Order uid from the open-order list.
///
/// Returns:
///     `Ok` with `ok: false` and `NotFound` when the uid is absent, and nothing is sent.
///     A refused send is `Unavailable`. `Err` is only the owner gate.
pub(super) fn mini_cancel_order(
    host: &mut dyn TgHost,
    chat_id: i64,
    core: u64,
    uid: u64,
) -> Result<CommandResultDto, MiniAppApiError> {
    mini_owner(host, chat_id)?;
    let listed = host
        .session()
        .store()
        .core(core)
        .is_some_and(|data| data.orders.iter().any(|order| order.uid == uid));
    if !listed {
        return Ok(command_miss(CommandErrorDto::NotFound));
    }
    match host.session_mut().cancel_order(core, uid) {
        Ok(()) => Ok(command_hit(None)),
        Err(_) => Ok(command_miss(CommandErrorDto::Unavailable)),
    }
}

/// Arm or disarm Panic Sell only when the market is not already in the asked state.
///
/// Args:
///     chat_id: Paired chat that sent the command.
///     core: Core id. `CoreId` is a `u64`.
///     market: Market key.
///     on: Armed state the page asked for.
///
/// Returns:
///     `Ok` with the armed state after an accepted toggle, or the asked state when no toggle
///     was needed. An absent core, or a market that is not on that core's open orders, is
///     `NotFound` and sends nothing. A refused toggle is `Unavailable`. `Err` is only the
///     owner gate.
pub(super) fn mini_panic_sell(
    host: &mut dyn TgHost,
    chat_id: i64,
    core: u64,
    market: String,
    on: bool,
) -> Result<CommandResultDto, MiniAppApiError> {
    mini_owner(host, chat_id)?;
    if host.session().store().core(core).is_none() {
        return Ok(command_miss(CommandErrorDto::NotFound));
    }
    let listed = host
        .session()
        .store()
        .core(core)
        .is_some_and(|data| data.orders.iter().any(|order| order.market == market));
    if !listed {
        return Ok(command_miss(CommandErrorDto::NotFound));
    }
    if host.is_panic_armed(core, &market) == on {
        return Ok(command_hit(Some(on)));
    }
    if !host.toggle_panic_sell(core, market.clone()) {
        return Ok(command_miss(CommandErrorDto::Unavailable));
    }
    Ok(command_hit(Some(host.is_panic_armed(core, &market))))
}

/// Flip one core's trading or auto-detect switch through the desktop's session call.
///
/// Args:
///     chat_id: Paired chat that sent the command.
///     core: Core id.
///     switch: Which switch to flip.
///     on: State the page asked for.
///
/// Returns:
///     `Ok` with `NotFound` for a core that is not configured, and nothing is sent. A refused
///     send is `Unavailable`. `Err` is only the owner gate.
pub(super) fn mini_core_switch(
    host: &mut dyn TgHost,
    chat_id: i64,
    core: u64,
    switch: CoreSwitchDto,
    on: bool,
) -> Result<CommandResultDto, MiniAppApiError> {
    mini_owner(host, chat_id)?;
    if !mini_core_known(host, core) {
        return Ok(command_miss(CommandErrorDto::NotFound));
    }
    let sent = match switch {
        CoreSwitchDto::Trading => host.session_mut().set_trading(core, on),
        CoreSwitchDto::AutoDetect => host.session_mut().set_auto_detect(core, on),
    };
    match sent {
        Ok(()) => Ok(command_hit(None)),
        Err(_) => Ok(command_miss(CommandErrorDto::Unavailable)),
    }
}

/// Flip one switch on several cores with one scope call.
///
/// Args:
///     chat_id: Paired chat that sent the command.
///     cores: Requested core ids; unknown ids and repeats are dropped before sending.
///     switch: Which switch to flip.
///     on: State the page asked for.
///
/// Returns:
///     `Ok` with `sent` of `requested` known cores accepted; `ok` only when all were.
///     No known core is `NotFound` and sends nothing. `Err` is only the owner gate.
pub(super) fn mini_cores_switch(
    host: &mut dyn TgHost,
    chat_id: i64,
    cores: &[u64],
    switch: CoreSwitchDto,
    on: bool,
) -> Result<ScopeResultDto, MiniAppApiError> {
    mini_owner(host, chat_id)?;
    let visible = mini_owner_core_ids(host);
    let targets = scope_targets(cores, &visible);
    if targets.is_empty() {
        return Ok(ScopeResultDto {
            ok: false,
            sent: 0,
            requested: 0,
            error: Some(CommandErrorDto::NotFound),
        });
    }
    let accepted = match switch {
        CoreSwitchDto::Trading => host.session_mut().set_trading_many(&targets, on),
        CoreSwitchDto::AutoDetect => host.session_mut().set_auto_detect_many(&targets, on),
    };
    let sent = u32::try_from(accepted.len()).unwrap_or(u32::MAX);
    let requested = u32::try_from(targets.len()).unwrap_or(u32::MAX);
    let ok = sent == requested;
    Ok(ScopeResultDto {
        ok,
        sent,
        requested,
        error: (!ok).then_some(CommandErrorDto::Unavailable),
    })
}

/// Cancel every open order of one core through the desktop's session call.
///
/// Args:
///     chat_id: Paired chat that sent the command.
///     core: Core id.
///
/// Returns:
///     `Ok` with `NotFound` for a core that is not configured, and nothing is sent. A refused
///     send is `Unavailable`. `Err` is only the owner gate.
pub(super) fn mini_cancel_all(
    host: &mut dyn TgHost,
    chat_id: i64,
    core: u64,
) -> Result<CommandResultDto, MiniAppApiError> {
    mini_owner(host, chat_id)?;
    if !mini_core_known(host, core) {
        return Ok(command_miss(CommandErrorDto::NotFound));
    }
    match host.session_mut().cancel_all_orders(core) {
        Ok(()) => Ok(command_hit(None)),
        Err(_) => Ok(command_miss(CommandErrorDto::Unavailable)),
    }
}

/// Whether `core` is one of the configured sessions the owner sees.
fn mini_core_known(host: &dyn TgHost, core: u64) -> bool {
    mini_owner_core_ids(host).contains(&core)
}

/// Ids of the configured sessions the owner sees.
fn mini_owner_core_ids(host: &dyn TgHost) -> Vec<CoreId> {
    visible_cores(host, &TelegramReportAccess::Owner)
        .into_iter()
        .map(|(id, _, _)| id)
        .collect()
}

/// Turn one strategy on or off without touching the core's strategy engine.
///
/// Args:
///     chat_id: Paired chat that sent the command.
///     core: Core id.
///     id: Strategy id on that core.
///     on: Checked state the page asked for.
///
/// Returns:
///     `Ok` with `NotFound` for an unknown core or strategy, and nothing is sent. A toggle
///     already pending for the same state is a hit without a second send. A refused send is
///     `Unavailable`. `Err` is only the owner gate.
pub(super) fn mini_strategy_toggle(
    host: &mut dyn TgHost,
    chat_id: i64,
    core: u64,
    id: u64,
    on: bool,
) -> Result<CommandResultDto, MiniAppApiError> {
    let result = mini_strategy_toggle_inner(host, chat_id, core, id, on);
    let outcome = match &result {
        Ok(dto) if dto.ok => "sent",
        Ok(dto) => match dto.error {
            Some(CommandErrorDto::NotFound) => "not found",
            _ => "unavailable",
        },
        Err(_) => "rejected",
    };
    log::info!("mini app strategy toggle: core={core} strategy={id} on={on} -> {outcome}");
    result
}

/// Body of [`Self::mini_strategy_toggle`], which logs the outcome.
fn mini_strategy_toggle_inner(
    host: &mut dyn TgHost,
    chat_id: i64,
    core: u64,
    id: u64,
    on: bool,
) -> Result<CommandResultDto, MiniAppApiError> {
    mini_owner(host, chat_id)?;
    if !mini_core_known(host, core) {
        return Ok(command_miss(CommandErrorDto::NotFound));
    }
    let now = Instant::now();
    let listed = host.session().store().core(core).and_then(|data| {
        data.strategies
            .iter()
            .find(|row| row.id == id)
            .map(|row| (data.strategies_ack_rev, data.strategies_rev, row.checked))
    });
    let Some((ack_before, rev_before, checked)) = listed else {
        return Ok(command_miss(CommandErrorDto::NotFound));
    };
    let already = host
        .state()
        .mini_strategy_wanted
        .get(&(core, id))
        .is_some_and(|entry| {
            entry.0 == on
                && strategy_pending(*entry, ack_before, rev_before, checked, now)
                    == Some(StrategyPendingDto::Pending)
        });
    if already {
        return Ok(command_hit(None));
    }
    match host
        .session_mut()
        .apply_strategies(core, vec![(id, on)], None)
    {
        Ok(()) => {
            host.state_mut()
                .mini_strategy_wanted
                .insert((core, id), (on, now, ack_before, rev_before));
            Ok(command_hit(None))
        }
        Err(_) => Ok(command_miss(CommandErrorDto::Unavailable)),
    }
}

/// Queue one core for the host's reconnect path.
///
/// Args:
///     chat_id: Paired chat that sent the command.
///     core: Core id.
///
/// Returns:
///     `Ok` with `NotFound` for a core that is not configured, and nothing is queued. A hit
///     only means the request was queued; the next core status read shows the outcome.
///     `Err` is only the owner gate.
pub(super) fn mini_core_reconnect(
    host: &mut dyn TgHost,
    chat_id: i64,
    core: u64,
) -> Result<CommandResultDto, MiniAppApiError> {
    mini_owner(host, chat_id)?;
    if !mini_core_known(host, core) {
        return Ok(command_miss(CommandErrorDto::NotFound));
    }
    host.request_reconnect(core);
    Ok(command_hit(None))
}
