//! Bounded application bridge and joined Telegram transport ownership.
use super::{
    TelegramStatus, api::ReplyMarkup, auth::Authorization, commands::ParsedCommand,
    web::MiniAppApiRequest,
};
use crate::config::TelegramConfig;
use std::path::PathBuf;
use std::sync::{
    Arc, Mutex, Weak,
    mpsc::{self, Receiver, SyncSender},
};
use std::thread::{self, JoinHandle};
use std::time::Instant;
mod bot;
mod history;
mod menu;
pub mod mini_app;
mod notify;
mod work;
pub(crate) use notify::drop_chart;
pub use notify::{
    NotifyStore, chart_spool_dir, cores_kept, drop_redraws, purge_outbox, purge_outbox_where,
    push_auto_report, push_edit, push_outbox, push_photo, push_redraw, push_trade_card,
};
use work::WorkSender;
pub use work::WorkWaker;
/// Authenticated work drained by the application's coordination loop.
// `MiniApp` carries the settings document, so this variant is the large one.
// Boxing it would allocate on every Mini App request.
#[allow(clippy::large_enum_variant)]
pub enum Work {
    Command {
        chat_id: i64,
        command: ParsedCommand,
        /// The message whose button sent the command, which its answer replaces; `None` for a
        /// typed command, answered with a new message.
        message: Option<i64>,
        reply: SyncSender<Response>,
    },
    Pair {
        chat_id: i64,
        reply: SyncSender<Response>,
    },
    MiniApp(MiniAppApiRequest),
    Status(TelegramStatus),
    MiniStatus(mini_app::MiniAppStatus),
}
/// Localized result; transport inserts protocol fields but never invents prose.
pub enum Response {
    /// One complete report or Help message; never split inside markup.
    Rich {
        html: String,
        keyboard: ReplyMarkup,
        /// Persistent navigation is installed on its own welcome message, never on an editable report.
        navigation: (String, ReplyMarkup),
    },
    Text {
        text: String,
        keyboard: Option<ReplyMarkup>,
    },
    PairSaved {
        saved: bool,
        text: String,
        keyboard: Option<ReplyMarkup>,
    },
}
/// In-memory pairing ledger shared by the bot and the Mini App session check.
pub type SharedAuthorization = Arc<Mutex<Authorization>>;

/// Result of loading the notification document on its owner thread.
#[derive(Default)]
struct NotificationLoad {
    /// Whether loading has either succeeded or failed.
    done: bool,
    /// Fully loaded store, or `None` when the document could not be read.
    store: Option<Arc<Mutex<NotifyStore>>>,
}

/// Shared handoff that lets service construction finish before notification I/O completes.
struct NotificationSlot {
    /// Completed load result.
    state: Mutex<NotificationLoad>,
    /// Stable sender wake shared with the store once it loads.
    wake: Arc<notify::NotifyWake>,
}

impl NotificationSlot {
    /// Create a pending result with the wake that shutdown can ring before loading finishes.
    fn new() -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(NotificationLoad::default()),
            wake: Arc::default(),
        })
    }

    /// Build an already-loaded slot for a service fixture with no loader thread.
    #[cfg(test)]
    fn loaded(store: Arc<Mutex<NotifyStore>>) -> Arc<Self> {
        let wake = notify::lock_store(&store).wake.clone();
        let slot = Arc::new(Self {
            state: Mutex::new(NotificationLoad::default()),
            wake,
        });
        slot.complete(Some(store));
        slot
    }

    /// Publish one terminal load result for later nonblocking host polls.
    fn complete(&self, store: Option<Arc<Mutex<NotifyStore>>>) {
        let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
        state.store = store;
        state.done = true;
    }

    /// Return the completed store, or `None` while the owner is still loading or after a failure.
    fn get(&self) -> Option<Arc<Mutex<NotifyStore>>> {
        let state = self.state.lock().unwrap_or_else(|p| p.into_inner());
        state.done.then(|| state.store.clone()).flatten()
    }
}

/// Optional owner constructed only after the saved credential passes the off switch.
pub struct TelegramService {
    alive: Option<Arc<()>>,
    joins: Vec<JoinHandle<()>>,
    events: Receiver<Work>,
    authorization: SharedAuthorization,
    mini_config: Arc<Mutex<TelegramConfig>>,
    labels: Arc<Mutex<std::collections::BTreeMap<String, String>>>,
    notifications: Option<Arc<NotificationSlot>>,
    /// The sender's wake, rung on stop so it leaves its wait at once.
    notify_wake: Option<Arc<notify::NotifyWake>>,
    /// Causal owner wake shared with every bounded work producer.
    work_waker: work::SharedWaker,
}
impl TelegramService {
    /// Start background transport only for a non-empty saved token.
    ///
    /// Args:
    ///     config: Saved token and chats. An empty token starts nothing.
    ///     notifications_path: Durable outbox. `None` starts no sender and keeps no store.
    pub fn start(config: &TelegramConfig, notifications_path: Option<PathBuf>) -> Option<Self> {
        Self::start_localized(
            config,
            std::collections::BTreeMap::new(),
            notifications_path,
        )
    }

    /// Start transport carrying localized application strings without interpreting them.
    ///
    /// Args:
    ///     config: Saved token and chats. An empty token starts nothing.
    ///     labels: Localized button text. The transport does not interpret it.
    ///     notifications_path: Durable outbox. `None` starts no sender and keeps no store.
    pub fn start_localized(
        config: &TelegramConfig,
        labels: std::collections::BTreeMap<String, String>,
        notifications_path: Option<PathBuf>,
    ) -> Option<Self> {
        Self::start_localized_with_menu_cleanup(config, labels, &[], notifications_path)
    }

    /// Restart the same bot with retired chat IDs solely for clearing their native menus.
    ///
    /// Args:
    ///     config: Saved token and chats. An empty token starts nothing.
    ///     labels: Localized button text. The transport does not interpret it.
    ///     retired_chats: Chats whose native menu is cleared and then forgotten.
    ///     notifications_path: Durable outbox. `None` starts no sender and keeps no store.
    ///         A file that cannot be parsed disables notifications and leaves the file untouched.
    pub fn start_localized_with_menu_cleanup(
        config: &TelegramConfig,
        labels: std::collections::BTreeMap<String, String>,
        retired_chats: &[i64],
        notifications_path: Option<PathBuf>,
    ) -> Option<Self> {
        if config.token.is_empty() {
            return None;
        }
        let alive = Arc::new(());
        let authorization = Arc::new(Mutex::new(Authorization::from_authorized_chats(
            config.authorized_chat_ids.clone(),
        )));
        let (tx, events) = mpsc::sync_channel(64);
        let work_waker = Arc::default();
        let tx = WorkSender {
            tx,
            waker: Arc::clone(&work_waker),
        };
        let mini_config = Arc::new(Mutex::new(config.clone()));
        let labels = Arc::new(Mutex::new(labels));
        let menu = Arc::new(Mutex::new(menu::MenuIntent::stopped(
            &config.authorized_chat_ids,
        )));
        let weak = Arc::downgrade(&alive);
        let auth = authorization.clone();
        let bot_tx = tx.clone();
        let flood = super::api::FloodWait::default();
        let bot_api = super::api::BotApi::with_flood_wait(config.token.clone(), flood.clone());
        let bot_labels = labels.clone();
        let bot_menu = menu.clone();
        let menu_sync = menu::MenuSync::with_cleanup(retired_chats);
        let join = thread::Builder::new()
            .name("telegram-bot".into())
            .spawn(move || bot::run(bot_api, weak, auth, bot_labels, bot_menu, menu_sync, bot_tx))
            .ok()?;
        let weak = Arc::downgrade(&alive);
        let cfg = mini_config.clone();
        let mini_labels = labels.clone();
        let failure_tx = tx.clone();
        let mini_join = thread::Builder::new()
            .name("telegram-miniapp-owner".into())
            .spawn(move || mini_app::run_service(cfg, mini_labels, menu, weak, tx));
        let mut joins = vec![join];
        if let Ok(join) = mini_join {
            joins.push(join);
        } else {
            let _ = failure_tx.try_send(Work::MiniStatus(mini_app::MiniAppStatus::Failed {
                reason: mini_app::MiniAppFailReason::Server,
            }));
        }
        let notifications = match notifications_path {
            Some(path) => {
                let slot = NotificationSlot::new();
                let thread_slot = slot.clone();
                let store_wake = slot.wake.clone();
                let authorized = config.authorized_chat_ids.clone();
                let api = super::api::BotApi::with_flood_wait(config.token.clone(), flood);
                let alive = Arc::downgrade(&alive);
                let spawned = thread::Builder::new()
                    .name("telegram-notify-load".into())
                    .spawn(move || match notify::NotifyStore::open(path) {
                        Ok(mut store) => {
                            if let Err(error) = store.forget_unpaired(&authorized) {
                                log::warn!("telegram notifications kept unpaired chats: {error}");
                            }
                            store.wake = store_wake;
                            let store = Arc::new(Mutex::new(store));
                            let sender = notify::spawn_sender(api, alive, Arc::clone(&store));
                            thread_slot.complete(Some(store));
                            let _ = sender.join();
                        }
                        Err(error) => {
                            log::warn!("telegram notifications disabled: {error}");
                            thread_slot.complete(None);
                        }
                    });
                match spawned {
                    Ok(join) => joins.push(join),
                    Err(error) => {
                        log::warn!("telegram notification loader did not start: {error}");
                        slot.complete(None);
                    }
                }
                Some(slot)
            }
            None => None,
        };
        let notify_wake = notifications.as_ref().map(|slot| Arc::clone(&slot.wake));
        Some(Self {
            alive: Some(alive),
            joins,
            events,
            authorization,
            mini_config,
            labels,
            notifications,
            notify_wake,
            work_waker,
        })
    }

    /// The durable outbox when the notification owner has finished loading it.
    ///
    /// Returns:
    ///     The store the moon-tg host edits, or `None` while loading, when no path was given,
    ///     or after a load failure. This getter never waits on the UI thread.
    pub fn notify_store(&self) -> Option<Arc<Mutex<NotifyStore>>> {
        self.notifications.as_ref().and_then(|slot| slot.get())
    }
    /// Drain one event without waiting on transport.
    pub fn try_recv(&self) -> Option<Work> {
        self.events.try_recv().ok()
    }
    /// Install the host's causal wake and wake once for work published before registration.
    pub fn set_work_waker(&self, waker: WorkWaker) {
        *self
            .work_waker
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(waker);
        work::wake(&self.work_waker);
    }
    /// Issue a ten-minute code without waiting on network operations.
    pub fn pairing_code(&self) -> Option<String> {
        self.authorization
            .try_lock()
            .ok()?
            .issue_pairing_code(Instant::now())
            .ok()
    }
    /// Publish persisted authorization and Mini App intent independently of bot reply delivery.
    pub fn configure(&self, config: &TelegramConfig) -> bool {
        let Ok(mut saved) = self.mini_config.try_lock() else {
            return false;
        };
        let Ok(mut ledger) = self.authorization.try_lock() else {
            return false;
        };
        ledger.sync_persisted_chats(&config.authorized_chat_ids);
        *saved = config.clone();
        true
    }

    /// Publish a locale change without restarting bot polling.
    pub fn set_labels(&self, labels: std::collections::BTreeMap<String, String>) -> bool {
        let Ok(mut current) = self.labels.try_lock() else {
            return false;
        };
        *current = labels;
        true
    }
    /// Revoke liveness, join every transport, and checkpoint receipts before owners disappear.
    pub fn stop(&mut self) {
        self.request_stop();
        for join in self.joins.drain(..) {
            let _ = join.join();
        }
        if let Some(store) = self.notifications.as_ref().and_then(|slot| slot.get()) {
            if let Err(error) = notify::lock_store(&store).checkpoint() {
                log::warn!("telegram notify shutdown checkpoint failed: {error}");
            }
        }
    }

    /// Signal shutdown immediately; the application can join on a background retirement thread.
    pub fn request_stop(&mut self) {
        self.alive = None;
        if let Some(wake) = &self.notify_wake {
            wake.bump();
        }
    }
}
impl Drop for TelegramService {
    /// Stop and join transports when the owning service is dropped.
    fn drop(&mut self) {
        self.stop();
    }
}
/// Wait for application response while observing shutdown.
///
/// Args:
///     slow: The answer reads the report database off the owner thread first, so it gets two
///         minutes instead of ten seconds. Status uses completed background discovery results.
fn response(rx: Receiver<Response>, alive: &Weak<()>, slow: bool) -> Option<Response> {
    let deadline = Instant::now() + std::time::Duration::from_secs(if slow { 120 } else { 10 });
    while alive.upgrade().is_some() && Instant::now() < deadline {
        match rx.recv_timeout(std::time::Duration::from_millis(100)) {
            Ok(result) => return Some(result),
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
    }
    None
}

#[cfg(test)]
mod tests;
