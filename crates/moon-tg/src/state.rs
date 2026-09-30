//! The bot's process-only state and the service's lifecycle.

use std::collections::HashMap;
use std::thread::JoinHandle;
use std::time::Instant;

use moon_core::config::TelegramConfig;
use moon_core::session::CoreId;
use moon_core::telegram::runtime::mini_app::MiniAppStatus;
use moon_core::telegram::{TelegramService, TelegramStatus};

use crate::HostKind;
use crate::labels::telegram_labels;
use crate::mini_app::cache::{CachedReport, CachedTrades};

/// Process-only service state; no credential is rendered by Debug.
pub struct TelegramState {
    /// At most one database report is computed at a time, including timed-out requests.
    pub(crate) report_pending: bool,
    /// At most one Mini App report is computed at a time, including timed-out requests.
    pub(crate) mini_report_pending: bool,
    /// Last finished Mini App report for one chat, period, and admission grant.
    ///
    /// A read that outlives the 5 s HTTP wait stays here so the page retry can still receive it,
    /// and past the TTL it answers while its inputs are unchanged (`mini_app::cache`). A hit is
    /// served only when the stored grant still equals the chat's current admission. Service
    /// restart, a failed admission recheck, and a grant mismatch all clear it.
    pub(crate) mini_report_last: Option<CachedReport>,
    /// At most one Mini App trades read is computed at a time, including timed-out requests.
    pub(crate) mini_trades_pending: bool,
    /// Last finished Mini App trades read for one chat and admission grant.
    ///
    /// Same lifetime rules as `mini_report_last`: served only to the same grant, cleared on
    /// service restart.
    pub(crate) mini_trades_last: Option<CachedTrades>,
    /// Unconfirmed Mini App strategy toggles by `(core, strategy id)`.
    ///
    /// Value: `(wanted, sent_at, strategies_ack_rev before, strategies_rev before)`. Cleared on
    /// service restart.
    pub(crate) mini_strategy_wanted: HashMap<(CoreId, u64), (bool, Instant, u64, u64)>,
    pub service: Option<TelegramService>,
    pub status: TelegramStatus,
    pub mini_status: MiniAppStatus,
    pub pairing: Option<(String, Instant)>,
    pub revision: u64,
    /// Retry a briefly contended worker configuration publication on the next owner tick.
    pub(crate) configuration_pending: bool,
    /// Retirement runs off the owner thread; the next saved service starts only after the old one
    /// joins.
    pub(crate) retiring: Option<JoinHandle<()>>,
    /// Same-token service restarts must still clear menus for previously revoked chats.
    retired_menu_chats: Vec<i64>,
    /// The bot is being handed over to a server: no transport starts, whatever the saved
    /// configuration says, until [`Self::resume`].
    suspended: bool,
    /// Which process runs the bot: the Mini App's own texts name what it depends on.
    kind: HostKind,
}

impl TelegramState {
    /// Construct optional transport only from saved configuration.
    ///
    /// Args:
    ///     config: The saved Telegram configuration.
    ///     kind: Which process runs the bot.
    pub fn new(config: &TelegramConfig, kind: HostKind) -> Self {
        Self::new_with_menu_cleanup(config, kind, Vec::new())
    }

    /// Start transport with cleanup-only identities retained from the same bot credential.
    fn new_with_menu_cleanup(
        config: &TelegramConfig,
        kind: HostKind,
        retired_menu_chats: Vec<i64>,
    ) -> Self {
        let service = TelegramService::start_localized_with_menu_cleanup(
            config,
            telegram_labels(kind),
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
            mini_report_pending: false,
            mini_report_last: None,
            mini_trades_pending: false,
            mini_trades_last: None,
            mini_strategy_wanted: HashMap::new(),
            service,
            status,
            mini_status: MiniAppStatus::Stopped,
            pairing: None,
            revision: 0,
            configuration_pending: false,
            retiring: None,
            retired_menu_chats,
            suspended: false,
            kind,
        }
    }

    /// Which process runs the bot.
    pub(crate) fn kind(&self) -> HostKind {
        self.kind
    }

    /// Replace joined transport without forgetting pending cleanup for the same bot.
    pub(crate) fn start_saved(&mut self, config: &TelegramConfig) {
        if self.suspended {
            self.status = TelegramStatus::Stopped;
            return;
        }
        let retired = std::mem::take(&mut self.retired_menu_chats);
        let report_pending = self.report_pending;
        let mini_report_pending = self.mini_report_pending;
        let mini_trades_pending = self.mini_trades_pending;
        *self = Self::new_with_menu_cleanup(config, self.kind, retired);
        self.report_pending = report_pending;
        self.mini_report_pending = mini_report_pending;
        self.mini_trades_pending = mini_trades_pending;
        // `mini_report_last` and `mini_trades_last` stay clear: a restarted service must not
        // replay the previous grant.
    }

    /// Retain only removed identities, and never transfer them to a different bot token.
    pub(crate) fn remember_menu_cleanup(
        &mut self,
        before: &TelegramConfig,
        saved: &TelegramConfig,
    ) {
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
    pub fn stop(&mut self) {
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
    pub fn request_stop(&mut self) {
        if let Some(service) = self.service.as_mut() {
            service.request_stop();
        }
    }

    /// Retire the transport without blocking and keep it down, the saved configuration untouched:
    /// the bot's token is being handed to a server, and one token has one poller.
    pub fn suspend(&mut self) {
        self.suspended = true;
        self.restart();
    }

    /// End [`Self::suspend`]: start `saved` now, or on the owner tick once the retired transport
    /// has joined. A `saved` without a token starts nothing.
    pub fn resume(&mut self, saved: &TelegramConfig) {
        self.suspended = false;
        if self.retiring.is_none() && self.service.is_none() {
            self.start_saved(saved);
        }
    }

    /// Whether the bot is held down by [`Self::suspend`].
    pub fn suspended(&self) -> bool {
        self.suspended
    }

    /// Retire the current transport and start the saved one now, unless the old one is still
    /// joining; then the owner tick starts it once the join finishes.
    pub(crate) fn restart_saved(&mut self, saved: &TelegramConfig) {
        self.restart();
        if self.retiring.is_none() {
            self.start_saved(saved);
        }
    }

    /// Retire the current transport without blocking the coordination loop.
    pub(crate) fn restart(&mut self) {
        self.mini_report_last = None;
        self.mini_trades_last = None;
        self.mini_strategy_wanted.clear();
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

#[cfg(test)]
mod tests;
