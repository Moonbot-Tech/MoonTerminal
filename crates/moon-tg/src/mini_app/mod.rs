//! Mini App routes: report, core status, open orders, owner money commands, and notification
//! settings.
//!
//! The session check stays in `dispatch.rs` so its authorization text stays byte-identical.
//! Every route here goes through [`mini_access`]. Only the report and trades reads leave the owner
//! thread.

use std::collections::HashMap;

use moon_core::config::telegram_access::TelegramReportAccess;
use moon_core::session::CoreId;
use moon_core::session::core_order::{CoreOrder, exchange_sections};
use moon_core::telegram::web::{MiniAppApiError, MiniAppApiRequest};
use moon_core::venue::CoreVenue;

use crate::TgHost;
use crate::labels::section_label;

pub(crate) mod cache;
mod commands;
mod dto;
mod reads;
mod settings;

pub(crate) use settings::{chat_notify, save_chat_notify};

/// Answer one Mini App request. The live session check is handled by the caller.
pub(crate) fn dispatch(host: &mut dyn TgHost, request: MiniAppApiRequest) {
    match request {
        MiniAppApiRequest::Report {
            chat_id,
            period,
            reply,
            ..
        } => reads::mini_report(host, chat_id, period, reply),
        MiniAppApiRequest::Cores { chat_id, reply, .. } => {
            let _ = reply.try_send(reads::mini_cores(host, chat_id));
        }
        MiniAppApiRequest::Orders { chat_id, reply, .. } => {
            let _ = reply.try_send(reads::mini_orders(host, chat_id));
        }
        MiniAppApiRequest::CancelOrder {
            chat_id,
            core,
            uid,
            reply,
            ..
        } => {
            let _ = reply.try_send(commands::mini_cancel_order(host, chat_id, core, uid));
        }
        MiniAppApiRequest::PanicSell {
            chat_id,
            core,
            market,
            on,
            reply,
            ..
        } => {
            let _ = reply.try_send(commands::mini_panic_sell(host, chat_id, core, market, on));
        }
        MiniAppApiRequest::CoreSwitch {
            chat_id,
            core,
            switch,
            on,
            reply,
            ..
        } => {
            let _ = reply.try_send(commands::mini_core_switch(host, chat_id, core, switch, on));
        }
        MiniAppApiRequest::CoresSwitch {
            chat_id,
            cores,
            switch,
            on,
            reply,
            ..
        } => {
            let _ = reply.try_send(commands::mini_cores_switch(
                host, chat_id, &cores, switch, on,
            ));
        }
        MiniAppApiRequest::CancelAllOrders {
            chat_id,
            core,
            reply,
            ..
        } => {
            let _ = reply.try_send(commands::mini_cancel_all(host, chat_id, core));
        }
        MiniAppApiRequest::Trades { chat_id, reply, .. } => {
            reads::mini_trades(host, chat_id, reply);
        }
        MiniAppApiRequest::Strategies { chat_id, reply, .. } => {
            let _ = reply.try_send(reads::mini_strategies(host, chat_id));
        }
        MiniAppApiRequest::StrategyToggle {
            chat_id,
            core,
            id,
            on,
            reply,
            ..
        } => {
            let _ = reply.try_send(commands::mini_strategy_toggle(host, chat_id, core, id, on));
        }
        MiniAppApiRequest::CoreReconnect {
            chat_id,
            core,
            reply,
            ..
        } => {
            let _ = reply.try_send(commands::mini_core_reconnect(host, chat_id, core));
        }
        MiniAppApiRequest::Session { reply, .. } => {
            let _ = reply.try_send(Err(MiniAppApiError::Rejected));
        }
        MiniAppApiRequest::Notify { chat_id, reply, .. } => {
            let _ = reply.try_send(settings::mini_notify(host, chat_id));
        }
        MiniAppApiRequest::NotifySave {
            chat_id,
            settings,
            revision,
            reply,
            ..
        } => {
            let _ = reply.try_send(settings::mini_notify_save(
                host, chat_id, settings, revision,
            ));
        }
    }
}

/// Live Mini App gate: the feature is on, and `report_access` still pairs this chat.
/// Pairing is that function's own check.
///
/// `None` means the request is refused. A viewer grant may list no cores; that is still
/// `Some`, and the reads then return empty data instead of every core.
fn mini_access(host: &dyn TgHost, chat_id: i64) -> Option<TelegramReportAccess> {
    if !host.config().telegram.mini_app_enabled {
        return None;
    }
    host.config().telegram.report_access(chat_id)
}

/// Owner gate for a money command.
///
/// Args:
///     chat_id: Paired chat that sent the command.
///
/// Returns:
///     `Ok(())` for the owner. A viewer is `Forbidden`. No grant is `Rejected`.
fn mini_owner(host: &dyn TgHost, chat_id: i64) -> Result<(), MiniAppApiError> {
    match mini_access(host, chat_id) {
        Some(TelegramReportAccess::Owner) => Ok(()),
        Some(TelegramReportAccess::Viewer(_)) => Err(MiniAppApiError::Forbidden),
        None => Err(MiniAppApiError::Rejected),
    }
}

/// Sessions this grant may see, in Mini App order. An empty viewer list keeps none.
///
/// Returns `(id, name, exchange section caption)` ordered by [`by_section`].
fn visible_cores(host: &dyn TgHost, access: &TelegramReportAccess) -> Vec<(u64, String, String)> {
    let order = CoreOrder::new(host.config());
    let cores: Vec<(u64, String)> = order
        .from_sessions(host.session().sessions(), |session| match access {
            TelegramReportAccess::Owner => true,
            TelegramReportAccess::Viewer(ids) => ids.contains(&session.id),
        })
        .into_iter()
        .collect();
    by_section(
        cores,
        host.session().core_venues(),
        |(id, _)| *id,
        |(_, name)| name,
    )
    .into_iter()
    .map(|(section, (id, name))| (id, name, section))
    .collect()
}

/// Order per-core rows the way every Mini App list shows them.
///
/// Rows are grouped into the terminal's exchange sections, in the terminal's section order
/// ([`exchange_sections`]), and sorted by name inside each section with numbers compared as
/// numbers ([`natural_cmp`]), so "Account 9" comes before "Account 10".
///
/// Args:
///     rows: Per-core rows in any order.
///     venues: Live venue of each core id.
///     id: Core id of one row.
///     name: Display name of one row.
///
/// Returns:
///     Each row paired with its section caption, in display order.
pub(crate) fn by_section<T>(
    rows: Vec<T>,
    venues: &HashMap<CoreId, CoreVenue>,
    id: impl Fn(&T) -> u64,
    name: impl Fn(&T) -> &str,
) -> Vec<(String, T)> {
    let sections: Vec<(String, Vec<usize>)> = exchange_sections(
        rows.iter()
            .enumerate()
            .map(|(index, row)| (index, venues.get(&id(row)))),
    )
    .into_iter()
    .map(|(venue, mut members)| {
        members.sort_by(|&a, &b| natural_cmp(name(&rows[a]), name(&rows[b])));
        (section_label(venue), members)
    })
    .collect();
    let mut slots: Vec<Option<T>> = rows.into_iter().map(Some).collect();
    let mut out = Vec::with_capacity(slots.len());
    for (label, members) in sections {
        for index in members {
            if let Some(row) = slots[index].take() {
                out.push((label.clone(), row));
            }
        }
    }
    out
}

/// Compare two names case-insensitively, reading each run of ASCII digits as one number.
///
/// Leading zeros do not change a number's value; names equal under that reading fall back to
/// the raw text, so the order is total.
///
/// Args:
///     a: First name.
///     b: Second name.
///
/// Returns:
///     The natural order of `a` against `b`.
fn natural_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    use std::iter::Peekable;
    use std::str::Chars;

    /// Consume one digit run and return it without leading zeros.
    fn number(chars: &mut Peekable<Chars<'_>>) -> String {
        let mut digits = String::new();
        while let Some(c) = chars.next_if(char::is_ascii_digit) {
            digits.push(c);
        }
        digits.trim_start_matches('0').to_string()
    }

    let (mut x, mut y) = (a.chars().peekable(), b.chars().peekable());
    loop {
        match (x.peek().copied(), y.peek().copied()) {
            (None, None) => return a.cmp(b),
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(p), Some(q)) if p.is_ascii_digit() && q.is_ascii_digit() => {
                let (m, n) = (number(&mut x), number(&mut y));
                let by_value = m.len().cmp(&n.len()).then_with(|| m.cmp(&n));
                if by_value != Ordering::Equal {
                    return by_value;
                }
            }
            (Some(p), Some(q)) => {
                let by_char = p.to_lowercase().cmp(q.to_lowercase());
                if by_char != Ordering::Equal {
                    return by_char;
                }
                x.next();
                y.next();
            }
        }
    }
}

/// Requested cores that are visible, without repeats, in `visible` order.
///
/// Args:
///     requested: Core ids the page asked for.
///     visible: Cores the chat may command, in canonical order.
///
/// Returns:
///     The ids to send; unknown ids never appear.
fn scope_targets(requested: &[u64], visible: &[CoreId]) -> Vec<CoreId> {
    let mut targets: Vec<CoreId> = Vec::new();
    for id in visible {
        if requested.contains(id) && !targets.contains(id) {
            targets.push(*id);
        }
    }
    targets
}

#[cfg(test)]
mod tests;
