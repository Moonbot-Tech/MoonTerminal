//! The bot's process-only state and the service's lifecycle.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Instant;

use moon_core::config::TelegramConfig;
use moon_core::config::telegram_access::TelegramReportAccess;
use moon_core::session::CoreId;
use moon_core::telegram::runtime::mini_app::MiniAppStatus;
use moon_core::telegram::runtime::{NotifyStore, cores_kept, purge_outbox_where};
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
    /// At most one closed-trade notification read is in flight.
    pub(crate) notify_busy: bool,
    /// Report revision captured at spawn and retained after that notification read saved.
    ///
    /// `None` is a host that cannot name a revision, the value after a service restart, and the
    /// value after a read that did not save. A later tick spawns again only when this differs
    /// from the host's revision, and only after the interval.
    pub(crate) last_report_revision: Option<crate::ReportRevision>,
    /// When the last notification read was spawned. `None` allows the first run.
    pub(crate) last_notify_run: Option<Instant>,
    /// Each core's newest Telegram event already relayed or passed over
    /// (`notify::events`); a core missing here starts at its newest.
    pub(crate) events_cursor: HashMap<CoreId, u64>,
    /// When the last batch of core events went. `None` allows the first.
    pub(crate) events_flush: Option<Instant>,
    /// Chats asked for a coin for a core's blacklist (`menu::control`): the core, whether to take
    /// the coin off, and until when the question stands.
    pub(crate) awaiting_coin: HashMap<i64, (CoreId, bool, Instant)>,
    /// The confirmed press each chat was asked for (`menu::control`), and until when it counts.
    pub(crate) awaiting_confirm:
        HashMap<i64, (moon_core::telegram::menu_action::ControlAction, Instant)>,
    /// At most one automatic-report read is in flight.
    pub(crate) auto_busy: bool,
    /// When the last automatic-report read was spawned. `None` allows the first run.
    pub(crate) last_auto_run: Option<Instant>,
    /// Injected automatic-report pages. `None` in production, which reads the report database.
    pub(crate) injected_auto: Option<crate::notify::reports::InjectedAuto>,
    /// In-memory down timers, one per chat. Empty after a restart on purpose: the ledger on disk
    /// is what stops a second down notice.
    pub(crate) down_trackers: BTreeMap<i64, crate::notify::down::DownTracker>,
    /// When the down machine last attempted a step. `None` after a restart.
    ///
    /// A step inside the following second is skipped unless `notify_clock_override` is set. An
    /// empty chat list does not move this instant.
    pub(crate) last_down_step: Option<Instant>,
    /// Frozen UTC Unix seconds for notification tests. `None` in production.
    ///
    /// When set, the one-second down gate does not apply.
    pub(crate) notify_clock_override: Option<i64>,
    /// Store used when no service is running. Production leaves this `None` and reads the service.
    pub(crate) notify_store_override: Option<Arc<Mutex<moon_core::telegram::runtime::NotifyStore>>>,
    /// Injected core links. `None` in production, which reads the live links.
    pub(crate) down_links_override: Option<Vec<(u64, String, crate::notify::down::Link)>>,
    /// Injected visible core ids. `None` in production, which uses the grant.
    pub(crate) visible_override: Option<Vec<u64>>,
    /// Injected closed trades. `None` in production, which opens the report database.
    pub(crate) injected_reads: Option<crate::notify::tick::InjectedReads>,
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
    /// Run switches the chat's Control section sent and the core has not reported yet, by
    /// `(core, switch)`: the asked state and when. Cleared on service restart.
    pub(crate) run_wanted: HashMap<(CoreId, moon_core::session::RunSwitch), (bool, Instant)>,
    /// Control messages to redraw once the core answers their press, by `(chat, message)`.
    /// Cleared on service restart.
    pub(crate) control_redraws: crate::menu::ControlRedraws,
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
    /// Notifications file for later restarts. `None` starts no store.
    ///
    /// A failed parent-directory creation still keeps this path so the next start retries.
    notifications_path: Option<PathBuf>,
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
        let notifications = match kind {
            HostKind::Terminal => Some(moon_core::config::paths::telegram_notifications()),
            HostKind::Station => None,
        };
        Self::new_with_notifications(config, kind, notifications)
    }

    /// Construct state that restarts onto `notifications`.
    ///
    /// Args:
    ///     config: The saved Telegram configuration.
    ///     kind: Which process runs the bot.
    ///     notifications: Notifications file for this process. `None` starts no store.
    ///         The parent directory is created only when `config` has a token.
    ///
    /// Returns:
    ///     State whose service is `None` when the token is empty.
    pub fn new_with_notifications(
        config: &TelegramConfig,
        kind: HostKind,
        notifications: Option<PathBuf>,
    ) -> Self {
        Self::new_with_menu_cleanup(config, kind, Vec::new(), notifications)
    }

    /// Start transport with cleanup-only identities retained from the same bot credential.
    fn new_with_menu_cleanup(
        config: &TelegramConfig,
        kind: HostKind,
        retired_menu_chats: Vec<i64>,
        notifications_path: Option<PathBuf>,
    ) -> Self {
        let service = TelegramService::start_localized_with_menu_cleanup(
            config,
            telegram_labels(kind),
            &retired_menu_chats,
            prepare_notifications_dir(config, notifications_path.as_deref()),
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
            notify_busy: false,
            last_report_revision: None,
            last_notify_run: None,
            events_cursor: HashMap::new(),
            events_flush: None,
            awaiting_coin: HashMap::new(),
            awaiting_confirm: HashMap::new(),
            auto_busy: false,
            last_auto_run: None,
            injected_auto: None,
            down_trackers: BTreeMap::new(),
            last_down_step: None,
            notify_clock_override: None,
            notify_store_override: None,
            down_links_override: None,
            visible_override: None,
            injected_reads: None,
            mini_trades_last: None,
            mini_strategy_wanted: HashMap::new(),
            run_wanted: HashMap::new(),
            control_redraws: HashMap::new(),
            service,
            status,
            mini_status: MiniAppStatus::Stopped,
            pairing: None,
            revision: 0,
            configuration_pending: false,
            retiring: None,
            retired_menu_chats,
            suspended: false,
            notifications_path,
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
        let notify_busy = self.notify_busy;
        let auto_busy = self.auto_busy;
        let notifications_path = self.notifications_path.clone();
        *self = Self::new_with_menu_cleanup(config, self.kind, retired, notifications_path);
        self.report_pending = report_pending;
        self.mini_report_pending = mini_report_pending;
        self.mini_trades_pending = mini_trades_pending;
        self.notify_busy = notify_busy;
        self.auto_busy = auto_busy;
        // `mini_report_last` and `mini_trades_last` stay clear: a restarted service must not
        // replay the previous grant. Down timers and the read gate stay clear on purpose; the
        // ledger on disk stops a restart from replaying announcements.
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
        self.forget_live_unpaired(saved);
        self.restart();
        if self.retiring.is_none() {
            self.start_saved(saved);
        }
    }

    /// Drop settings for chats `saved` no longer pairs, and drop a viewer's queued rows that
    /// name a core it can no longer see.
    ///
    /// A save failure is logged and the in-memory file stays as it was. The allow map is
    /// published from `saved` before the viewer purge, so a failed purge still leaves it.
    /// The owner is not purged here: this restart has no session, so the next tick applies
    /// the owner's filter. An unpaired chat's outbox row stays. Nothing is written when
    /// every stored chat is still paired and no viewer row would be dropped. A row with
    /// `cores: None` is unknown and is dropped for a viewer. `Some([])` discloses no core
    /// and stays.
    ///
    /// Args:
    ///     saved: Configuration about to replace the running one.
    fn forget_live_unpaired(&mut self, saved: &TelegramConfig) {
        let Some(store) = self
            .service
            .as_ref()
            .and_then(TelegramService::notify_store)
        else {
            return;
        };
        {
            let mut guard = store
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if let Err(error) = guard.forget_unpaired(&saved.authorized_chat_ids) {
                log::warn!("telegram notification settings kept an unpaired chat: {error}");
            }
            publish_saved_allowed(&mut guard, saved);
        }
        purge_viewer_rows(&store, saved);
    }

    /// Retire the current transport without blocking the coordination loop.
    pub(crate) fn restart(&mut self) {
        self.mini_report_last = None;
        self.mini_trades_last = None;
        self.mini_strategy_wanted.clear();
        self.run_wanted.clear();
        self.control_redraws.clear();
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

/// Publish the allow map from `saved` without reading a session.
///
/// An owner is `None` (any core). A viewer is `Some` of that chat's grant ids.
/// The file is not written.
///
/// Args:
///     store: Notifications file the sender still holds.
///     saved: Configuration about to replace the running one.
fn publish_saved_allowed(store: &mut NotifyStore, saved: &TelegramConfig) {
    let mut next = BTreeMap::new();
    for &chat in &saved.authorized_chat_ids {
        let Some(access) = saved.report_access(chat) else {
            continue;
        };
        let grant = match access {
            TelegramReportAccess::Owner => None,
            TelegramReportAccess::Viewer(ids) => Some(ids.into_iter().collect()),
        };
        next.insert(chat, grant);
    }
    store.publish_allowed(next);
}

/// Drop queued rows a still-paired viewer can no longer be shown.
///
/// The owner is skipped: this restart has no session, so an owner's visible cores are not known
/// here. The next tick purges the owner. A viewer with nothing to drop does not save. A row
/// with `cores: None` is unknown and is dropped for a viewer. `Some([])` discloses no core
/// and stays.
///
/// Args:
///     store: Notifications file the sender still holds.
///     saved: Configuration about to replace the running one.
fn purge_viewer_rows(store: &Mutex<NotifyStore>, saved: &TelegramConfig) {
    let mut plans = Vec::new();
    for &chat in &saved.authorized_chat_ids {
        let Some(TelegramReportAccess::Viewer(ids)) = saved.report_access(chat) else {
            continue;
        };
        let visible: BTreeSet<u64> = ids.into_iter().collect();
        plans.push((chat, visible));
    }
    if plans.is_empty() {
        return;
    }
    let mut guard = store
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Err(error) = purge_outbox_where(&mut guard, |row| {
        let Some((_, visible)) = plans.iter().find(|(chat, _)| *chat == row.chat) else {
            return true;
        };
        cores_kept(&row.cores, visible, false)
    }) {
        log::warn!("telegram notify kept a row for a core the chat can no longer see: {error}");
    }
}

/// Parent directory for `path`, created only when the bot will actually start.
///
/// Args:
///     config: Saved token. An empty token creates nothing and opens nothing.
///     path: Notifications file stored on the state. `None` starts no store.
///
/// Returns:
///     The path to open, or `None` when there is no token, no path, or the parent directory
///     could not be created. The state still keeps `path` so a later start retries.
fn prepare_notifications_dir(config: &TelegramConfig, path: Option<&Path>) -> Option<PathBuf> {
    if config.token.is_empty() {
        return None;
    }
    let path = path?;
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
        && let Err(error) = std::fs::create_dir_all(parent)
    {
        log::warn!(
            "telegram notifications directory {} not created: {error}",
            parent.display()
        );
        return None;
    }
    Some(path.to_path_buf())
}

#[cfg(test)]
mod tests;
