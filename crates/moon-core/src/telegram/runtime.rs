//! Bounded application bridge and joined Telegram transport ownership.
use super::{
    TelegramStatus, api::ReplyMarkup, auth::Authorization, commands::ParsedCommand,
    web::MiniAppApiRequest,
};
use crate::config::TelegramConfig;
use std::sync::{
    Arc, Mutex, Weak,
    mpsc::{self, Receiver, SyncSender},
};
use std::thread::{self, JoinHandle};
use std::time::Instant;
mod bot;
mod menu;
pub mod mini_app;
/// Authenticated work drained by the application's coordination loop.
pub enum Work {
    Command {
        chat_id: i64,
        command: ParsedCommand,
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
/// Optional owner constructed only after the saved credential passes the off switch.
pub struct TelegramService {
    alive: Option<Arc<()>>,
    joins: Vec<JoinHandle<()>>,
    events: Receiver<Work>,
    authorization: SharedAuthorization,
    mini_config: Arc<Mutex<TelegramConfig>>,
    labels: Arc<Mutex<std::collections::BTreeMap<String, String>>>,
}
impl TelegramService {
    /// Start background transport only for a non-empty saved token.
    pub fn start(config: &TelegramConfig) -> Option<Self> {
        Self::start_localized(config, std::collections::BTreeMap::new())
    }

    /// Start transport carrying localized application strings without interpreting them.
    pub fn start_localized(
        config: &TelegramConfig,
        labels: std::collections::BTreeMap<String, String>,
    ) -> Option<Self> {
        Self::start_localized_with_menu_cleanup(config, labels, &[])
    }

    /// Restart the same bot with retired chat IDs solely for clearing their native menus.
    pub fn start_localized_with_menu_cleanup(
        config: &TelegramConfig,
        labels: std::collections::BTreeMap<String, String>,
        retired_chats: &[i64],
    ) -> Option<Self> {
        if config.token.is_empty() {
            return None;
        }
        let alive = Arc::new(());
        let authorization = Arc::new(Mutex::new(Authorization::from_authorized_chats(
            config.authorized_chat_ids.clone(),
        )));
        let (tx, events) = mpsc::sync_channel(64);
        let mini_config = Arc::new(Mutex::new(config.clone()));
        let labels = Arc::new(Mutex::new(labels));
        let menu = Arc::new(Mutex::new(menu::MenuIntent::stopped(
            &config.authorized_chat_ids,
        )));
        let weak = Arc::downgrade(&alive);
        let auth = authorization.clone();
        let bot_tx = tx.clone();
        let token = config.token.clone();
        let bot_labels = labels.clone();
        let bot_menu = menu.clone();
        let menu_sync = menu::MenuSync::with_cleanup(retired_chats);
        let join = thread::Builder::new()
            .name("telegram-bot".into())
            .spawn(move || bot::run(token, weak, auth, bot_labels, bot_menu, menu_sync, bot_tx))
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
        Some(Self {
            alive: Some(alive),
            joins,
            events,
            authorization,
            mini_config,
            labels,
        })
    }
    /// Drain one event without waiting on transport.
    pub fn try_recv(&self) -> Option<Work> {
        self.events.try_recv().ok()
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
    /// Revoke liveness and join every transport before application owners disappear.
    pub fn stop(&mut self) {
        self.request_stop();
        for join in self.joins.drain(..) {
            let _ = join.join();
        }
    }

    /// Signal shutdown immediately; the application can join on a background retirement thread.
    pub fn request_stop(&mut self) {
        self.alive = None;
    }
}
impl Drop for TelegramService {
    /// Stop and join transports when the owning service is dropped.
    fn drop(&mut self) {
        self.stop();
    }
}
/// Wait for application response while observing shutdown.
fn response(rx: Receiver<Response>, alive: &Weak<()>) -> Option<Response> {
    let deadline = Instant::now() + std::time::Duration::from_secs(10);
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
