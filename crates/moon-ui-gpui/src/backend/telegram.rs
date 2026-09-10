//! Telegram application adapter: localization, durable authorization, and pairing.
use crate::Backend;
use gpui::Context;
use moon_core::config::TelegramConfig;
use moon_core::telegram::{
    TelegramService, TelegramStatus,
    api::{
        InlineKeyboardButton, InlineKeyboardMarkup, KeyboardButton, ReplyKeyboardMarkup,
        ReplyMarkup,
    },
    commands::ParsedCommand,
    runtime::mini_app::MiniAppStatus,
    runtime::{Response, Work},
    web::*,
};
use rust_i18n::t;
use std::sync::mpsc::SyncSender;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

mod reports;
#[cfg(test)]
mod tests;

/// Process-only service state; no credential is rendered by Debug.
pub(crate) struct TelegramState {
    /// At most one database report is computed at a time, including timed-out requests.
    report_pending: bool,
    pub(crate) service: Option<TelegramService>,
    pub(crate) status: TelegramStatus,
    pub(crate) mini_status: MiniAppStatus,
    pub(crate) pairing: Option<(String, Instant)>,
    pub(crate) revision: u64,
    /// Retry a briefly contended worker configuration publication on the next owner tick.
    configuration_pending: bool,
    /// Retirement runs off GPUI; the next saved service starts only after the old one joins.
    retiring: Option<JoinHandle<()>>,
    /// Same-token service restarts must still clear menus for previously revoked chats.
    retired_menu_chats: Vec<i64>,
}
impl TelegramState {
    /// Construct optional transport only from saved configuration.
    pub(crate) fn new(config: &TelegramConfig) -> Self {
        Self::new_with_menu_cleanup(config, Vec::new())
    }

    /// Start transport with cleanup-only identities retained from the same bot credential.
    fn new_with_menu_cleanup(config: &TelegramConfig, retired_menu_chats: Vec<i64>) -> Self {
        let service = TelegramService::start_localized_with_menu_cleanup(
            config,
            telegram_labels(),
            &retired_menu_chats,
        );
        let status = if config.token.is_empty() {
            TelegramStatus::Disabled
        } else if service.is_some() {
            TelegramStatus::Starting
        } else {
            TelegramStatus::Unavailable
        };
        Self {
            report_pending: false,
            service,
            status,
            mini_status: MiniAppStatus::Stopped,
            pairing: None,
            revision: 0,
            configuration_pending: false,
            retiring: None,
            retired_menu_chats,
        }
    }

    /// Replace joined transport without forgetting pending cleanup for the same bot.
    fn start_saved(&mut self, config: &TelegramConfig) {
        let retired = std::mem::take(&mut self.retired_menu_chats);
        let report_pending = self.report_pending;
        *self = Self::new_with_menu_cleanup(config, retired);
        self.report_pending = report_pending;
    }

    /// Retain only removed identities, and never transfer them to a different bot token.
    fn remember_menu_cleanup(&mut self, before: &TelegramConfig, saved: &TelegramConfig) {
        if before.token.expose() != saved.token.expose() {
            self.retired_menu_chats.clear();
        } else {
            for &chat in &before.authorized_chat_ids {
                if !saved.authorized_chat_ids.contains(&chat)
                    && !self.retired_menu_chats.contains(&chat)
                {
                    self.retired_menu_chats.push(chat);
                }
            }
        }
    }
    /// Stop and join transport before their owners disappear.
    pub(crate) fn stop(&mut self) {
        if let Some(join) = self.retiring.take() {
            let _ = join.join();
        }
        if let Some(mut service) = self.service.take() {
            service.stop();
        }
        self.status = TelegramStatus::Stopped;
        self.mini_status = MiniAppStatus::Stopped;
        self.pairing = None;
    }

    /// Revoke liveness without joining, so a caller can do useful work while transports wind down.
    ///
    /// The blocking half stays in [`Self::stop`]: quit signals here first, persists, then joins.
    pub(crate) fn request_stop(&mut self) {
        if let Some(service) = self.service.as_mut() {
            service.request_stop();
        }
    }

    /// Retire the current transport without blocking the coordination loop.
    fn restart(&mut self) {
        self.pairing = None;
        self.status = TelegramStatus::Stopping;
        self.mini_status = MiniAppStatus::Stopped;
        if let Some(mut service) = self.service.take() {
            service.request_stop();
            self.retiring = Some(std::thread::spawn(move || {
                service.stop();
            }));
        }
    }
}
impl Backend {
    /// Reconcile a successfully persisted token or identity change; retain saved failures.
    pub(crate) fn reconcile_telegram(&mut self, before: &TelegramConfig) {
        let saved = &self.config.telegram;
        self.telegram.remember_menu_cleanup(before, saved);
        if before.token.expose() != saved.token.expose()
            || before.authorized_chat_ids != saved.authorized_chat_ids
        {
            self.telegram.restart();
            if self.telegram.retiring.is_none() {
                self.telegram.start_saved(saved);
            }
        } else {
            if let Some(service) = self.telegram.service.as_ref() {
                self.telegram.configuration_pending |= !service.configure(saved);
                self.telegram.configuration_pending |= !service.set_labels(telegram_labels());
            }
        }
        self.telegram.revision = self.telegram.revision.wrapping_add(1);
    }
    /// Issue a fresh ten-minute pairing code from the live transport ledger.
    pub(crate) fn issue_telegram_pairing(&mut self) {
        self.telegram.pairing = self
            .telegram
            .service
            .as_ref()
            .and_then(TelegramService::pairing_code)
            .map(|code| (code, Instant::now() + Duration::from_secs(600)));
        self.telegram.revision = self.telegram.revision.wrapping_add(1);
    }
    /// Persist revocation before displaying an empty paired set or restarting service.
    pub(crate) fn reset_telegram_pairing(&mut self) {
        let mut candidate = self.config.clone();
        candidate.telegram.authorized_chat_ids.clear();
        if candidate.save_telegram().is_err() {
            self.telegram.status = TelegramStatus::Unavailable;
            return;
        }
        self.telegram
            .remember_menu_cleanup(&self.config.telegram, &candidate.telegram);
        self.config = candidate;
        if let Some(preview) = self.preview.as_mut() {
            preview.telegram.authorized_chat_ids.clear();
        }
        self.telegram.restart();
        if self.telegram.retiring.is_none() {
            self.telegram.start_saved(&self.config.telegram);
        }
    }
    /// Drain bounded transport work on the 100 ms owner loop.
    pub(crate) fn tick_telegram(&mut self, cx: &mut Context<Self>) {
        if self
            .telegram
            .retiring
            .as_ref()
            .is_some_and(|join| join.is_finished())
        {
            if let Some(join) = self.telegram.retiring.take() {
                let _ = join.join();
            }
            self.telegram.start_saved(&self.config.telegram);
            cx.notify();
        }
        if self.telegram.service.is_none() {
            return;
        }
        if self.telegram.configuration_pending {
            if let Some(service) = self.telegram.service.as_ref() {
                let configured = service.configure(&self.config.telegram);
                let localized = service.set_labels(telegram_labels());
                self.telegram.configuration_pending = !configured || !localized;
            }
        }
        let mut changed = false;
        for _ in 0..64 {
            let work = self
                .telegram
                .service
                .as_ref()
                .and_then(TelegramService::try_recv);
            let Some(work) = work else {
                break;
            };
            changed = true;
            match work {
                Work::Status(status) => self.telegram.status = status,
                Work::MiniStatus(status) => self.telegram.mini_status = status,
                Work::Pair { chat_id, reply } => {
                    let mut candidate = self.config.clone();
                    if !candidate.telegram.authorized_chat_ids.contains(&chat_id) {
                        candidate.telegram.authorized_chat_ids.push(chat_id);
                    }
                    let saved = candidate.save_telegram().is_ok();
                    if saved {
                        self.config = candidate;
                        if let Some(preview) = self.preview.as_mut() {
                            preview.telegram.authorized_chat_ids =
                                self.config.telegram.authorized_chat_ids.clone();
                        }
                        if let Some(service) = self.telegram.service.as_ref() {
                            self.telegram.configuration_pending |=
                                !service.configure(&self.config.telegram);
                        }
                        self.telegram.pairing = None;
                    }
                    let text = if saved {
                        t!("telegram.pair_success")
                    } else {
                        t!("telegram.refusal")
                    }
                    .to_string();
                    let keyboard = saved.then(navigation_keyboard);
                    let _ = reply.try_send(Response::PairSaved {
                        saved,
                        text,
                        keyboard,
                    });
                }
                Work::Command {
                    chat_id,
                    command,
                    reply,
                } => {
                    if matches!(command, ParsedCommand::Pair { .. }) {
                        answer(&reply, t!("telegram.refusal").to_string());
                        continue;
                    }
                    if !self.config.telegram.authorized_chat_ids.contains(&chat_id) {
                        answer(&reply, t!("telegram.refusal").to_string());
                        continue;
                    }
                    match command {
                        ParsedCommand::Start => self.telegram_report(
                            chat_id,
                            moon_core::telegram::report::ReportRequest::new(
                                moon_core::telegram::report::Period::Today,
                                false,
                            ),
                            reply,
                            cx,
                        ),
                        ParsedCommand::Report(request) => {
                            self.telegram_report(chat_id, request, reply, cx)
                        }
                        ParsedCommand::Help => {
                            let zone = crate::chrome::clock::resolved_header_clock_zone(
                                self.header_clock_zone(),
                            );
                            let _ = reply.try_send(reports::help(&zone.to_string()));
                        }
                        ParsedCommand::MiniApp => {
                            let keyboard = match &self.telegram.mini_status {
                                MiniAppStatus::Tunneling { url, .. }
                                    if self.config.telegram.mini_app_enabled =>
                                {
                                    Some(InlineKeyboardMarkup::from_rows(vec![vec![
                                        InlineKeyboardButton::web_app(
                                            t!("telegram.open").to_string(),
                                            url.clone(),
                                        ),
                                    ]]))
                                }
                                _ => None,
                            };
                            let text = if keyboard.is_some() {
                                t!("telegram.bot_ready").to_string()
                            } else if !self.config.telegram.mini_app_enabled {
                                t!("telegram.bot_mini_disabled").to_string()
                            } else {
                                t!("telegram.bot_mini_wait").to_string()
                            };
                            let keyboard = keyboard
                                .map(ReplyMarkup::Inline)
                                .or_else(|| Some(navigation_keyboard()));
                            let _ = reply.try_send(Response::Text { text, keyboard });
                        }
                        _ => {
                            let _ = reply.try_send(Response::Text {
                                text: format!(
                                    "{}\n\n{}",
                                    t!("telegram.invalid"),
                                    t!("telegram.report_help")
                                ),
                                keyboard: Some(navigation_keyboard()),
                            });
                        }
                    }
                }
                Work::MiniApp(request) => self.telegram_mini_request(request),
            }
        }
        if self
            .telegram
            .pairing
            .as_ref()
            .is_some_and(|(_, expiry)| Instant::now() >= *expiry)
        {
            self.telegram.pairing = None;
            changed = true;
        }
        if changed {
            self.telegram.revision = self.telegram.revision.wrapping_add(1);
            cx.notify();
        }
    }
    /// The service's own state, without the per-core roster.
    ///
    /// A settings surface must show only whether the service itself is up.
    pub(crate) fn telegram_service_status_text(&self) -> String {
        status_text(&self.telegram.status)
    }

    /// Recheck pairing and Mini App enablement on every authenticated HTTP request.
    fn telegram_mini_request(&mut self, request: MiniAppApiRequest) {
        match request {
            MiniAppApiRequest::Session { chat_id, reply, .. } => {
                if !self.config.telegram.mini_app_enabled
                    || !self.config.telegram.authorized_chat_ids.contains(&chat_id)
                {
                    let _ = reply.try_send(Err(MiniAppApiError::Rejected));
                    return;
                }
                let _ = reply.try_send(Ok(()));
            }
        }
    }
}
/// Nonblocking localized response; a disconnected requester cannot stall Backend.
fn answer(reply: &SyncSender<Response>, text: String) {
    let _ = reply.try_send(Response::Text {
        text,
        keyboard: None,
    });
}
/// Render service health through the Telegram locale domain.
fn status_text(status: &TelegramStatus) -> String {
    match status {
        TelegramStatus::Stopping => t!("telegram.stopping"),
        TelegramStatus::Disabled => t!("telegram.state.disabled"),
        TelegramStatus::Starting => t!("telegram.state.starting"),
        TelegramStatus::Unpaired | TelegramStatus::Paired { .. } => t!("telegram.state.connected"),
        TelegramStatus::RateLimited { retry_after_secs } => {
            t!("telegram.rate_limited", seconds = retry_after_secs)
        }
        TelegramStatus::Unavailable => t!("telegram.state.unavailable"),
        TelegramStatus::Stopped => t!("telegram.stopped"),
    }
    .to_string()
}

/// Global navigation owns periods and help; report actions remain inline.
fn navigation_keyboard() -> ReplyMarkup {
    let locale = rust_i18n::locale();
    let buttons: Vec<_> = navigation_buttons()
        .iter()
        .map(|(name, icon)| {
            let key = format!("telegram.button_{name}");
            KeyboardButton {
                text: format!("{icon} {}", t!(&key, locale = locale.as_ref())),
                style: None,
            }
        })
        .collect();
    ReplyMarkup::Reply(ReplyKeyboardMarkup {
        keyboard: buttons.chunks(2).map(|row| row.to_vec()).collect(),
        resize_keyboard: true,
        is_persistent: true,
    })
}

/// Stable glyphs are kept outside localization dictionaries and shared by rendering and aliases.
fn navigation_buttons() -> [(&'static str, &'static str); 5] {
    [
        ("today", "\u{1f4c5}"),
        ("yesterday", "\u{23ee}"),
        ("month", "\u{1f5d3}"),
        ("lastmonth", "\u{1f4c6}"),
        ("help", "\u{2139}\u{fe0f}"),
    ]
}

/// Compose Mini App shell labels and all reply-button aliases in the UI locale domain.
fn telegram_labels() -> std::collections::BTreeMap<String, String> {
    let mut labels: std::collections::BTreeMap<String, String> = [
        ("menu_miniapp".to_string(), t!("telegram.open").to_string()),
        (
            "mini_shell_checking".to_string(),
            t!("telegram.mini_shell_checking").to_string(),
        ),
        (
            "mini_shell_connected".to_string(),
            t!("telegram.mini_shell_connected").to_string(),
        ),
        (
            "mini_shell_denied".to_string(),
            t!("telegram.mini_shell_denied").to_string(),
        ),
        (
            "mini_shell_unreachable".to_string(),
            t!("telegram.mini_shell_unreachable").to_string(),
        ),
        ("refusal".to_string(), t!("telegram.refusal").to_string()),
        ("locale".to_string(), rust_i18n::locale().to_string()),
    ]
    .into_iter()
    .collect();
    labels.insert(
        "report_delivery_failed".into(),
        t!("telegram.report_delivery_failed").to_string(),
    );
    // Keep old keyboard labels usable after the desktop locale changes.
    for locale in ["ru", "en", "es"] {
        for (name, icon) in [("home", "\u{1f4ca}"), ("help", "\u{2753}")] {
            let key = format!("telegram.button_{name}");
            labels.insert(
                format!("button_{name}_legacy_emoji_{locale}"),
                format!("{icon} {}", t!(&key, locale = locale)),
            );
        }
        for (name, icon) in navigation_buttons() {
            let key = format!("telegram.button_{name}");
            labels.insert(
                format!("button_{name}_emoji_{locale}"),
                format!("{icon} {}", t!(&key, locale = locale)),
            );
        }
        for name in ["today", "yesterday", "month", "lastmonth", "daily", "home"] {
            let key = format!("telegram.button_{name}");
            labels.insert(
                format!("button_{name}_{locale}"),
                t!(&key, locale = locale).to_string(),
            );
        }
        labels.insert(
            format!("button_miniapp_{locale}"),
            t!("telegram.mini_open", locale = locale).to_string(),
        );
        labels.insert(
            format!("button_help_{locale}"),
            t!("telegram.button_help", locale = locale).to_string(),
        );
    }
    labels
}
