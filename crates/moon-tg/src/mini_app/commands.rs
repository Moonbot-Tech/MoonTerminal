//! The owner's money and control commands from the Mini App, through the shared command module
//! ([`crate::control`]), which makes the session calls the desktop uses. This file only turns
//! the page's requests and the module's answers into the page's documents.

use moon_core::session::RunSwitch;
use moon_core::telegram::web::MiniAppApiError;
use moon_core::telegram::web::dto::{
    CommandErrorDto, CommandResultDto, CoreSwitchDto, ScopeResultDto,
};

use super::dto::{command_hit, command_miss};
use super::mini_owner;
use crate::TgHost;
use crate::control::{self, Refusal};

/// The page's error for a refused command. The owner gate ran first, so `NotOwner` cannot reach
/// here; it reads as unavailable if it ever did.
fn miss(refusal: Refusal) -> CommandResultDto {
    command_miss(match refusal {
        Refusal::NotFound => CommandErrorDto::NotFound,
        Refusal::Offline => CommandErrorDto::Offline,
        Refusal::Unavailable | Refusal::NotOwner | Refusal::NotReady | Refusal::NoList => {
            CommandErrorDto::Unavailable
        }
    })
}

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
    Ok(match control::cancel_order(host, chat_id, core, uid) {
        Ok(()) => command_hit(None),
        Err(refusal) => miss(refusal),
    })
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
    Ok(
        match control::panic_market(host, chat_id, core, &market, on) {
            Ok(armed) => command_hit(Some(armed)),
            Err(refusal) => miss(refusal),
        },
    )
}

/// Flip one core's trading or auto-detect switch through the desktop's gated session call.
///
/// Args:
///     chat_id: Paired chat that sent the command.
///     core: Core id.
///     switch: Which switch to flip.
///     on: State the page asked for.
///
/// Returns:
///     `Ok` with `NotFound` for a core that is not configured and `Offline` for one that is not
///     connected; nothing is sent for either. A core already in the asked state is a hit without
///     a send. A refused send is `Unavailable`. `Err` is only the owner gate.
pub(super) fn mini_core_switch(
    host: &mut dyn TgHost,
    chat_id: i64,
    core: u64,
    switch: CoreSwitchDto,
    on: bool,
) -> Result<CommandResultDto, MiniAppApiError> {
    mini_owner(host, chat_id)?;
    Ok(
        match control::run_one(host, chat_id, core, run_switch(switch), on) {
            Ok(_) => command_hit(None),
            Err(refusal) => miss(refusal),
        },
    )
}

/// Flip one switch on several cores with one gated scope call.
///
/// Args:
///     chat_id: Paired chat that sent the command.
///     cores: Requested core ids; unknown ids and repeats are dropped before sending.
///     switch: Which switch to flip.
///     on: State the page asked for.
///
/// Returns:
///     `Ok` with how many known cores were sent, already in the asked state, or skipped as not
///     connected; `ok` only when none was skipped or refused. No known core is `NotFound` and
///     sends nothing. `Err` is only the owner gate.
pub(super) fn mini_cores_switch(
    host: &mut dyn TgHost,
    chat_id: i64,
    cores: &[u64],
    switch: CoreSwitchDto,
    on: bool,
) -> Result<ScopeResultDto, MiniAppApiError> {
    mini_owner(host, chat_id)?;
    let (targets, outcome) = match control::run_many(host, chat_id, cores, run_switch(switch), on) {
        Ok(done) => done,
        Err(_) => {
            return Ok(ScopeResultDto {
                ok: false,
                sent: 0,
                requested: 0,
                already: 0,
                offline: 0,
                error: Some(CommandErrorDto::NotFound),
            });
        }
    };
    let count = |n: usize| u32::try_from(n).unwrap_or(u32::MAX);
    let error = if outcome.refused() > 0 {
        Some(CommandErrorDto::Unavailable)
    } else if outcome.offline > 0 {
        Some(CommandErrorDto::Offline)
    } else {
        None
    };
    Ok(ScopeResultDto {
        ok: error.is_none(),
        sent: count(outcome.sent.len()),
        requested: count(targets.len()),
        already: count(outcome.already),
        offline: count(outcome.offline),
        error,
    })
}

/// The session's run switch for the page's switch name.
fn run_switch(switch: CoreSwitchDto) -> RunSwitch {
    match switch {
        CoreSwitchDto::Trading => RunSwitch::Trading,
        CoreSwitchDto::AutoDetect => RunSwitch::AutoDetect,
    }
}

/// Cancel every open order of one core through the desktop's session call.
///
/// Args:
///     chat_id: Paired chat that sent the command.
///     core: Core id.
///
/// Returns:
///     `Ok` with `NotFound` for a core that is not configured and `Offline` for one that is not
///     connected; nothing is sent for either. A refused send is `Unavailable`. `Err` is only the
///     owner gate.
pub(super) fn mini_cancel_all(
    host: &mut dyn TgHost,
    chat_id: i64,
    core: u64,
) -> Result<CommandResultDto, MiniAppApiError> {
    mini_owner(host, chat_id)?;
    Ok(match control::cancel_all(host, chat_id, core) {
        Ok(()) => command_hit(None),
        Err(refusal) => miss(refusal),
    })
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
    let result = mini_owner(host, chat_id).map(|()| {
        match control::strategy_toggle(host, chat_id, core, id, on) {
            Ok(()) => command_hit(None),
            Err(refusal) => miss(refusal),
        }
    });
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
    Ok(match control::reconnect(host, chat_id, core) {
        Ok(()) => command_hit(None),
        Err(refusal) => miss(refusal),
    })
}
