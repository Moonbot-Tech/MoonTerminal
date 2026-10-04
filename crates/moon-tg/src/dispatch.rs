//! The owner loop: transport work in, localized answers out, pairing and the saved configuration.

use std::sync::mpsc::SyncSender;
use std::time::{Duration, Instant};

use moon_core::config::TelegramConfig;
use moon_core::config::telegram_access::TelegramReportAccess;
use moon_core::telegram::api::{InlineKeyboardButton, InlineKeyboardMarkup, ReplyMarkup};
use moon_core::telegram::commands::ParsedCommand;
use moon_core::telegram::menu_action::MenuAction;
use moon_core::telegram::report::{Period, ReportRequest};
use moon_core::telegram::runtime::mini_app::MiniAppStatus;
use moon_core::telegram::runtime::{Response, Work};
use moon_core::telegram::web::{MiniAppApiError, MiniAppApiRequest};
use moon_core::telegram::{TelegramService, TelegramStatus};
use rust_i18n::t;

use crate::labels::{answer, navigation_keyboard, telegram_labels};
use crate::{TelegramState, TgHost, mini_app, report};

/// Reconcile a successfully persisted token or identity change; retain saved failures.
///
/// Args:
///     state: The bot's state.
///     saved: The Telegram configuration as it is now saved.
///     before: The Telegram configuration before the save.
pub fn reconcile(state: &mut TelegramState, saved: &TelegramConfig, before: &TelegramConfig) {
    state.remember_menu_cleanup(before, saved);
    let same_token = before.token.expose() == saved.token.expose();
    if !same_token || !saved.only_adds_chats_to(before) {
        // Cancel the old API's liveness before retiring it: a queued report or a pending
        // navigation send must not continue with grants that have just been revoked.
        state.restart_saved(saved);
    } else if let Some(service) = state.service.as_ref() {
        // Every earlier chat keeps its grant and chats may be added: the live transport takes them
        // in place, and the Mini App keeps its tunnel address.
        state.configuration_pending |= !service.configure(saved);
        state.configuration_pending |= !service.set_labels(telegram_labels(state.kind()));
        // The bot republishes its status only after its current long poll; show the new count now.
        if matches!(
            state.status,
            TelegramStatus::Unpaired | TelegramStatus::Paired { .. }
        ) {
            state.status = match saved.authorized_chat_ids.len() {
                0 => TelegramStatus::Unpaired,
                chat_count => TelegramStatus::Paired { chat_count },
            };
        }
    }
    state.revision = state.revision.wrapping_add(1);
}

/// Issue a fresh ten-minute pairing code from the live transport ledger.
pub fn issue_pairing(state: &mut TelegramState) {
    state.pairing = state
        .service
        .as_ref()
        .and_then(TelegramService::pairing_code)
        .map(|code| (code, Instant::now() + Duration::from_secs(600)));
    state.revision = state.revision.wrapping_add(1);
}

/// Persist revocation before displaying an empty paired set or restarting service.
pub fn reset_pairing(host: &mut dyn TgHost) {
    let before = host.config().telegram.clone();
    if !host.save_cleared_pairing() {
        host.state_mut().status = TelegramStatus::Unavailable;
        return;
    }
    let saved = host.config().telegram.clone();
    let state = host.state_mut();
    state.remember_menu_cleanup(&before, &saved);
    state.restart_saved(&saved);
}

/// Drain bounded transport work on the owner loop (the terminal's 100 ms tick).
///
/// The notification tick also runs without a service. Production then has no store and returns;
/// a test store override can still exercise enqueue decisions without a live poller.
pub fn tick(host: &mut dyn TgHost) {
    if host
        .state()
        .retiring
        .as_ref()
        .is_some_and(|join| join.is_finished())
    {
        let saved = host.config().telegram.clone();
        let state = host.state_mut();
        if let Some(join) = state.retiring.take() {
            let _ = join.join();
        }
        state.start_saved(&saved);
        host.repaint();
    }
    if host.state().service.is_some() {
        drain_service(host);
    }
    crate::menu::control_tick(host);
    let now_utc = i64::try_from(moon_core::util::time::now_unix_secs()).unwrap_or(i64::MAX);
    crate::notify::tick::run(host, now_utc);
}

/// Apply queued transport work while a service is running.
fn drain_service(host: &mut dyn TgHost) {
    if host.state().configuration_pending {
        let saved = host.config().telegram.clone();
        let state = host.state_mut();
        let kind = state.kind();
        if let Some(service) = state.service.as_ref() {
            let configured = service.configure(&saved);
            let localized = service.set_labels(telegram_labels(kind));
            state.configuration_pending = !configured || !localized;
        }
    }
    let mut changed = false;
    for _ in 0..64 {
        let work = host
            .state()
            .service
            .as_ref()
            .and_then(TelegramService::try_recv);
        let Some(work) = work else {
            break;
        };
        changed = true;
        match work {
            Work::Status(status) => host.state_mut().status = status,
            Work::MiniStatus(status) => host.state_mut().mini_status = status,
            Work::Pair { chat_id, reply } => pair(host, chat_id, reply),
            Work::Command {
                chat_id,
                command,
                message,
                reply,
            } => run_command(host, chat_id, command, message, reply),
            Work::MiniApp(request) => mini_request(host, request),
        }
    }
    let state = host.state_mut();
    if state
        .pairing
        .as_ref()
        .is_some_and(|(_, expiry)| Instant::now() >= *expiry)
    {
        state.pairing = None;
        changed = true;
    }
    if changed {
        state.revision = state.revision.wrapping_add(1);
        host.repaint();
    }
}

/// Save a chat that sent a valid pairing code and answer it.
fn pair(host: &mut dyn TgHost, chat_id: i64, reply: SyncSender<Response>) {
    let saved = host.save_paired_chat(chat_id);
    if saved {
        let telegram = host.config().telegram.clone();
        let state = host.state_mut();
        if let Some(service) = state.service.as_ref() {
            state.configuration_pending |= !service.configure(&telegram);
        }
        state.pairing = None;
    }
    let text = if saved {
        t!("telegram.pair_success")
    } else {
        t!("telegram.refusal")
    }
    .to_string();
    let keyboard = saved.then(|| {
        let telegram = &host.config().telegram;
        navigation_keyboard(
            host.kind(),
            telegram.report_access(chat_id) == Some(TelegramReportAccess::Owner),
            telegram,
        )
    });
    let _ = reply.try_send(Response::PairSaved {
        saved,
        text,
        keyboard,
    });
}

/// Answer paired-chat commands; station status and updates require the current owner grant.
fn run_command(
    host: &mut dyn TgHost,
    chat_id: i64,
    command: ParsedCommand,
    message: Option<i64>,
    reply: SyncSender<Response>,
) {
    if matches!(command, ParsedCommand::Pair { .. }) {
        answer(&reply, t!("telegram.refusal").to_string());
        return;
    }
    if !host
        .config()
        .telegram
        .authorized_chat_ids
        .contains(&chat_id)
    {
        answer(&reply, t!("telegram.refusal").to_string());
        return;
    }
    let owner = host.config().telegram.report_access(chat_id) == Some(TelegramReportAccess::Owner);
    if matches!(
        command,
        ParsedCommand::StationStatus
            | ParsedCommand::StationUpdate
            | ParsedCommand::Menu(MenuAction::Settings(_))
            | ParsedCommand::Menu(MenuAction::Control(_))
    ) && !owner
    {
        answer(&reply, t!("telegram.refusal").to_string());
        return;
    }
    match command {
        ParsedCommand::Start => {
            report::telegram_report(host, chat_id, ReportRequest::preset(Period::Today), reply)
        }
        ParsedCommand::Report(request) => report::telegram_report(host, chat_id, request, reply),
        ParsedCommand::Menu(action) => {
            crate::menu::run(host, chat_id, action, owner, message, reply)
        }
        ParsedCommand::Text(text) => {
            if !crate::menu::answer_text(host, chat_id, &text, &reply) {
                cannot_run(host, chat_id, &reply);
            }
        }
        ParsedCommand::Help => {
            let zone = host.report_zone();
            let navigation = navigation_keyboard(host.kind(), owner, &host.config().telegram);
            let _ = reply.try_send(report::help(
                &zone.to_string(),
                host.kind(),
                owner,
                navigation,
            ));
        }
        ParsedCommand::MiniApp => {
            let mini_app_enabled = host.config().telegram.mini_app_enabled;
            let keyboard = match &host.state().mini_status {
                MiniAppStatus::Tunneling { url, .. } if mini_app_enabled => {
                    Some(InlineKeyboardMarkup::from_rows(vec![vec![
                        InlineKeyboardButton::web_app(t!("telegram.open").to_string(), url.clone()),
                    ]]))
                }
                _ => None,
            };
            // Where the Mini App is switched on differs: the terminal's Settings, or the station's
            // administrator.
            let station = host.kind() == crate::HostKind::Station;
            let text = if keyboard.is_some() {
                t!("telegram.bot_ready").to_string()
            } else if !mini_app_enabled {
                match station {
                    true => t!("telegram.bot_mini_disabled_station"),
                    false => t!("telegram.bot_mini_disabled"),
                }
                .to_string()
            } else {
                match station {
                    true => t!("telegram.bot_mini_wait_station"),
                    false => t!("telegram.bot_mini_wait"),
                }
                .to_string()
            };
            let keyboard = keyboard.map(ReplyMarkup::Inline).or_else(|| {
                Some(navigation_keyboard(
                    host.kind(),
                    owner,
                    &host.config().telegram,
                ))
            });
            let _ = reply.try_send(Response::Text { text, keyboard });
        }
        ParsedCommand::StationStatus => {
            if !host.station_status(reply.clone(), false) {
                cannot_run(host, chat_id, &reply);
            }
        }
        ParsedCommand::StationUpdate => {
            let text = match host.request_station_update() {
                None => return cannot_run(host, chat_id, &reply),
                Some(Ok(())) => t!("telegram.station.update_requested"),
                Some(Err(reason)) => t!(
                    "telegram.station.update_not_requested",
                    reason = reason.text()
                ),
            };
            let telegram = &host.config().telegram;
            let _ = reply.try_send(Response::Text {
                text: text.to_string(),
                keyboard: Some(navigation_keyboard(
                    host.kind(),
                    telegram.report_access(chat_id) == Some(TelegramReportAccess::Owner),
                    telegram,
                )),
            });
        }
        _ => cannot_run(host, chat_id, &reply),
    }
}

/// Explain an unsupported request with navigation limited to the chat's current role.
fn cannot_run(host: &dyn TgHost, chat_id: i64, reply: &SyncSender<Response>) {
    let telegram = &host.config().telegram;
    let _ = reply.try_send(Response::Text {
        text: format!(
            "{}\n\n{}",
            t!("telegram.invalid"),
            crate::labels::report_help(host.kind())
        ),
        keyboard: Some(navigation_keyboard(
            host.kind(),
            telegram.report_access(chat_id) == Some(TelegramReportAccess::Owner),
            telegram,
        )),
    });
}

/// Recheck pairing and Mini App enablement on every authenticated HTTP request.
fn mini_request(host: &mut dyn TgHost, request: MiniAppApiRequest) {
    match request {
        MiniAppApiRequest::Session { chat_id, reply, .. } => {
            let telegram = &host.config().telegram;
            if !telegram.mini_app_enabled || !telegram.authorized_chat_ids.contains(&chat_id) {
                let _ = reply.try_send(Err(MiniAppApiError::Rejected));
                return;
            }
            let _ = reply.try_send(Ok(()));
        }
        other => mini_app::dispatch(host, other),
    }
}

#[cfg(test)]
mod tests;
