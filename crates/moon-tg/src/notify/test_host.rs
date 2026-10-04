//! The notification tests' host: a station-shaped [`TgHost`] over a temp notifications file
//! that never starts a bot and whose session methods panic.

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use moon_core::config::{AppConfig, TelegramConfig};
use moon_core::telegram::notify::{NotifyFile, Pending};
use moon_core::telegram::runtime::NotifyStore;

use crate::{HostKind, Job, TelegramState, TgHost};

/// Chat every fixture pairs as the owner.
pub(crate) const CHAT: i64 = 42;

/// What the test host does after the job reads and before finish runs.
pub(crate) enum DuringJob {
    /// Leave pairing, revision, and the store pointer alone.
    None,
    /// Settings save bumped this chat's revision while the read was in flight.
    BumpRevision(i64),
    /// The chat was unpaired while the read was in flight.
    Unpair,
    /// A new store was opened on the same path while the read was in flight.
    ReplaceStore,
}

/// Temp directory removed when the test drops it, including after a panic.
pub(crate) struct TempRoot(PathBuf);

impl TempRoot {
    /// Create a private directory under the process temp dir.
    ///
    /// Args:
    ///     tag: Short name mixed into the directory so failures name the test.
    ///
    /// Returns:
    ///     The root. Its drop deletes the directory.
    pub(crate) fn new(tag: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "moon-tg-notify-tick-{}-{tag}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("temp root");
        Self(root)
    }

    /// Path of the notifications file inside this root.
    ///
    /// Returns:
    ///     `notifications.json`. The parent already exists, so the first save can create it.
    pub(crate) fn notifications(&self) -> PathBuf {
        self.0.join("notifications.json")
    }
}

impl Drop for TempRoot {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Station-shaped host whose session methods panic.
pub(crate) struct TickHost {
    pub(crate) config: AppConfig,
    pub(crate) state: TelegramState,
    pub(crate) jobs: u32,
    pub(crate) during: DuringJob,
    pub(crate) store_path: PathBuf,
}

impl TickHost {
    /// Open an empty notifications file and a service-less state.
    ///
    /// Args:
    ///     path: Notifications file. Its parent must already exist.
    ///
    /// Returns:
    ///     A host whose service is `None` and whose store override points at `path`.
    pub(crate) fn open(path: PathBuf) -> Self {
        let config = AppConfig::headless(Vec::new());
        let store = NotifyStore::open(path.clone()).expect("open notifications");
        let mut state = TelegramState::new(&TelegramConfig::default(), HostKind::Station);
        assert!(
            state.service.is_none(),
            "an empty token must not start transport"
        );
        state.notify_store_override = Some(Arc::new(Mutex::new(store)));
        Self {
            config,
            state,
            jobs: 0,
            during: DuringJob::None,
            store_path: path,
        }
    }

    /// Pair `chat` as the sole owner.
    ///
    /// Args:
    ///     chat: Chat id stored in both the authorized list and `owner_chat_id`.
    pub(crate) fn admit(&mut self, chat: i64) {
        self.config.telegram.authorized_chat_ids = vec![chat];
        self.config.telegram.owner_chat_id = Some(chat);
    }

    /// Save one edit and keep it only when the write succeeds.
    ///
    /// Args:
    ///     edit: Mutation applied to a clone of the document.
    pub(crate) fn edit(&self, edit: impl FnOnce(&mut NotifyFile)) {
        let store = self
            .state
            .notify_store_override
            .clone()
            .expect("notifications store");
        let mut guard = store
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        guard.update(edit).expect("save notifications");
    }

    /// Clone the document the override currently holds.
    ///
    /// Returns:
    ///     Settings, ledger, and outbox after the last successful save.
    pub(crate) fn file(&self) -> NotifyFile {
        let store = self
            .state
            .notify_store_override
            .clone()
            .expect("notifications store");
        let guard = store
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        guard.file.clone()
    }

    /// Outbox rows in send order.
    ///
    /// Returns:
    ///     The pending rows stored for every chat.
    pub(crate) fn outbox(&self) -> Vec<Pending> {
        self.file().outbox
    }

    /// HTML bodies in send order.
    ///
    /// Returns:
    ///     One string per outbox row.
    pub(crate) fn htmls(&self) -> Vec<String> {
        self.outbox().into_iter().map(|row| row.html).collect()
    }

    /// Record ids stored in this chat's seen ledger.
    ///
    /// Returns:
    ///     Every rec id, across cores. Empty when the chat is missing.
    pub(crate) fn seen_ids(&self) -> BTreeSet<i64> {
        self.file()
            .chats
            .get(&CHAT)
            .map(|chat| {
                chat.ledger
                    .seen
                    .values()
                    .flat_map(|rows| rows.keys().copied())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Run one owner tick at `now_utc`.
    ///
    /// Args:
    ///     now_utc: UTC Unix seconds passed into the tick. Outbox rows store this value.
    pub(crate) fn tick(&mut self, now_utc: i64) {
        self.state.notify_clock_override = Some(now_utc);
        crate::notify::tick::run(self, now_utc);
    }

    /// Apply the in-flight mutation once, then forget it.
    fn prepare_finish(&mut self) {
        match std::mem::replace(&mut self.during, DuringJob::None) {
            DuringJob::None => {}
            DuringJob::BumpRevision(chat) => {
                self.edit(|file| {
                    if let Some(entry) = file.chats.get_mut(&chat) {
                        entry.revision = entry.revision.saturating_add(1);
                    }
                });
            }
            DuringJob::Unpair => {
                self.config.telegram.authorized_chat_ids.clear();
                self.config.telegram.owner_chat_id = None;
            }
            DuringJob::ReplaceStore => {
                let opened =
                    NotifyStore::open(self.store_path.clone()).expect("reopen notifications");
                self.state.notify_store_override = Some(Arc::new(Mutex::new(opened)));
            }
        }
    }
}

impl TgHost for TickHost {
    /// This fixture stands in for the station.
    fn kind(&self) -> HostKind {
        HostKind::Station
    }

    /// Saved pairing and grants.
    fn config(&self) -> &AppConfig {
        &self.config
    }

    /// The tick tests inject links and never read a live session.
    fn session(&self) -> &moon_core::session::SessionManager {
        panic!("notify tick test must not call session()")
    }

    /// The tick tests inject links and never mutate a live session.
    fn session_mut(&mut self) -> &mut moon_core::session::SessionManager {
        panic!("notify tick test must not call session()")
    }

    /// Process-only state, including the store override.
    fn state(&self) -> &TelegramState {
        &self.state
    }

    /// Process-only state, for the busy flag and the overrides.
    fn state_mut(&mut self) -> &mut TelegramState {
        &mut self.state
    }

    /// Report times are UTC, so unix 2000 is 1970-01-01 00:33:20.
    fn report_zone(&self) -> chrono_tz::Tz {
        chrono_tz::UTC
    }

    /// The temp notifications file. Constructing the path does not create it.
    fn notifications_path(&self) -> PathBuf {
        self.store_path.clone()
    }

    /// Pairing changes in these tests are applied directly, not through a save.
    fn save_paired_chat(&mut self, _chat: i64) -> bool {
        panic!("notify tick test must not save pairing")
    }

    /// Pairing changes in these tests are applied directly, not through a save.
    fn save_cleared_pairing(&mut self) -> bool {
        panic!("notify tick test must not clear pairing")
    }

    /// The bot's settings are not edited from a notify tick.
    fn save_bot_settings(&mut self, _: moon_core::config::telegram_menu::BotSettings) -> bool {
        panic!("notify tick test must not save bot settings")
    }

    /// Money commands are outside this fixture.
    fn is_panic_armed(&self, _core: u64, _market: &str) -> bool {
        panic!("notify tick test must not read panic state")
    }

    /// Money commands are outside this fixture.
    fn toggle_panic_sell(&mut self, _core: u64, _market: String) -> bool {
        panic!("notify tick test must not toggle panic")
    }

    /// Reconnects are outside this fixture.
    fn request_reconnect(&mut self, _core: u64) {
        panic!("notify tick test must not reconnect")
    }

    /// Run the job and its finish on this thread, after the in-flight mutation.
    fn spawn(&mut self, job: Job) {
        self.jobs += 1;
        let finish = job();
        self.prepare_finish();
        finish(self);
    }

    /// No window exists in this fixture.
    fn repaint(&mut self) {}
}
