//! Telegram application adapter: localization, durable authorization, and pairing.
use crate::Backend;
use gpui::Context;
use moon_core::config::TelegramConfig;
use moon_core::telegram::{
    TelegramService, TelegramStatus,
    api::{InlineKeyboardButton, InlineKeyboardMarkup},
    commands::ParsedCommand,
    runtime::mini_app::MiniAppStatus,
    runtime::{Response, Work},
    web::*,
};
use rust_i18n::t;
use std::sync::mpsc::SyncSender;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// Process-only service state; no credential is rendered by Debug.
pub(crate) struct TelegramState {
    pub(crate) service: Option<TelegramService>,
    pub(crate) status: TelegramStatus,
    pub(crate) mini_status: MiniAppStatus,
    pub(crate) pairing: Option<(String, Instant)>,
    pub(crate) revision: u64,
    /// Retry a briefly contended worker configuration publication on the next owner tick.
    configuration_pending: bool,
    /// Retirement runs off GPUI; the next saved service starts only after the old one joins.
    retiring: Option<JoinHandle<()>>,
}
impl TelegramState {
    /// Construct optional transport only from saved configuration.
    pub(crate) fn new(config: &TelegramConfig) -> Self {
        let service = TelegramService::start_localized(config, telegram_labels());
        let status = if config.token.is_empty() {
            TelegramStatus::Disabled
        } else if service.is_some() {
            TelegramStatus::Starting
        } else {
            TelegramStatus::Unavailable
        };
        Self {
            service,
            status,
            mini_status: MiniAppStatus::Stopped,
            pairing: None,
            revision: 0,
            configuration_pending: false,
            retiring: None,
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
        if before.token.expose() != saved.token.expose()
            || before.authorized_chat_ids != saved.authorized_chat_ids
        {
            self.telegram.restart();
            if self.telegram.retiring.is_none() {
                self.telegram = TelegramState::new(saved);
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
        self.config = candidate;
        if let Some(preview) = self.preview.as_mut() {
            preview.telegram.authorized_chat_ids.clear();
        }
        self.telegram.restart();
        if self.telegram.retiring.is_none() {
            self.telegram = TelegramState::new(&self.config.telegram);
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
            self.telegram = TelegramState::new(&self.config.telegram);
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
                    let _ = reply.try_send(Response::PairSaved { saved, text });
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
                        ParsedCommand::MiniApp => {
                            let keyboard = match &self.telegram.mini_status {
                                MiniAppStatus::Tunneling { url, .. } => {
                                    Some(InlineKeyboardMarkup::from_rows(vec![vec![
                                        InlineKeyboardButton::web_app(
                                            t!("telegram.mini_open").to_string(),
                                            url.clone(),
                                        ),
                                    ]]))
                                }
                                _ => None,
                            };
                            let text = if keyboard.is_some() {
                                t!("telegram.mini_open")
                            } else {
                                t!("telegram.state.unavailable")
                            }
                            .to_string();
                            let _ = reply.try_send(Response::Text { text, keyboard });
                        }
                        _ => answer(&reply, t!("telegram.invalid").to_string()),
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
        TelegramStatus::Unpaired => t!("telegram.paired_none"),
        TelegramStatus::Paired { chat_count } => t!("telegram.paired_count", count = chat_count),
        TelegramStatus::RateLimited { retry_after_secs } => {
            t!("telegram.rate_limited", seconds = retry_after_secs)
        }
        TelegramStatus::Unavailable => t!("telegram.state.unavailable"),
        TelegramStatus::Stopped => t!("telegram.stopped"),
    }
    .to_string()
}

/// Compose Mini App shell labels in the UI locale domain.
fn telegram_labels() -> std::collections::BTreeMap<String, String> {
    [
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
    .collect()
}
