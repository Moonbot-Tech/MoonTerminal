//! Owner-thread notification tick, with synthetic trades and links.
//!
//! The host below never starts a bot and never opens the report database. A
//! missing override panics before it can touch a live session.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use chrono::NaiveDate;
use moon_core::config::{AppConfig, TelegramConfig};
use moon_core::telegram::notify::{ChatNotify, NotifyFile, Pending};
use moon_core::telegram::runtime::NotifyStore;

use super::InjectedReads;
use crate::notify::down::Link;
use crate::notify::trades::ClosedTrade;
use crate::{HostKind, Job, TelegramState, TgHost};

/// Chat every fixture pairs as the owner.
const CHAT: i64 = 42;

/// What the test host does after the job reads and before finish runs.
enum DuringJob {
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
struct TempRoot(PathBuf);

impl TempRoot {
    /// Create a private directory under the process temp dir.
    ///
    /// Args:
    ///     tag: Short name mixed into the directory so failures name the test.
    ///
    /// Returns:
    ///     The root. Its drop deletes the directory.
    fn new(tag: &str) -> Self {
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
    fn notifications(&self) -> PathBuf {
        self.0.join("notifications.json")
    }
}

impl Drop for TempRoot {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Station-shaped host whose session methods panic.
struct TickHost {
    config: AppConfig,
    state: TelegramState,
    jobs: u32,
    during: DuringJob,
    store_path: PathBuf,
}

impl TickHost {
    /// Open an empty notifications file and a service-less state.
    ///
    /// Args:
    ///     path: Notifications file. Its parent must already exist.
    ///
    /// Returns:
    ///     A host whose service is `None` and whose store override points at `path`.
    fn open(path: PathBuf) -> Self {
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
    fn admit(&mut self, chat: i64) {
        self.config.telegram.authorized_chat_ids = vec![chat];
        self.config.telegram.owner_chat_id = Some(chat);
    }

    /// Save one edit and keep it only when the write succeeds.
    ///
    /// Args:
    ///     edit: Mutation applied to a clone of the document.
    fn edit(&self, edit: impl FnOnce(&mut NotifyFile)) {
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
    fn file(&self) -> NotifyFile {
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
    fn outbox(&self) -> Vec<Pending> {
        self.file().outbox
    }

    /// HTML bodies in send order.
    ///
    /// Returns:
    ///     One string per outbox row.
    fn htmls(&self) -> Vec<String> {
        self.outbox().into_iter().map(|row| row.html).collect()
    }

    /// Record ids stored in this chat's seen ledger.
    ///
    /// Returns:
    ///     Every rec id, across cores. Empty when the chat is missing.
    fn seen_ids(&self) -> BTreeSet<i64> {
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
    fn tick(&mut self, now_utc: i64) {
        self.state.notify_clock_override = Some(now_utc);
        super::run(self, now_utc);
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

/// No rows. Installed so a mistaken job does not open the report database.
fn empty_reads() -> InjectedReads {
    InjectedReads {
        trades: Vec::new(),
        days: BTreeMap::new(),
    }
}

/// One closed trade on core 7.
///
/// Args:
///     rec_id: Report row id.
///     close_utc: Close time, UTC Unix seconds. The open time is ten seconds earlier.
///     coin: Market symbol the card must show.
///     profit_usd: Realised profit, or `None` when the row is unvalued.
///
/// Returns:
///     A trade whose name, strategy, and volume are fixed by this fixture.
fn closed_trade(rec_id: i64, close_utc: i64, coin: &str, profit_usd: Option<f64>) -> ClosedTrade {
    ClosedTrade {
        core: 7,
        rec_id,
        close_utc,
        coin: coin.to_string(),
        core_name: "CoreA".to_string(),
        strategy: String::new(),
        volume_usd: None,
        profit_usd,
        profit_pct: None,
        open_utc: close_utc - 10,
    }
}

/// The two closes the trade tests inject: one after enable, one before it.
///
/// Returns:
///     Rec 11 closed at 1500, and rec 12 closed at 900. No day rows.
fn closes_around_enable() -> InjectedReads {
    InjectedReads {
        trades: vec![
            closed_trade(11, 1_500, "NEWCOIN", None),
            closed_trade(12, 900, "OLDCOIN", None),
        ],
        days: BTreeMap::new(),
    }
}

/// Turn trade cards on at unix 1000 and record settings revision 1.
///
/// Args:
///     file: Document the host is about to save.
fn enable_trades(file: &mut NotifyFile) {
    let mut chat = ChatNotify {
        revision: 1,
        ..ChatNotify::default()
    };
    chat.settings.trades.on = true;
    chat.ledger.trades_enabled_utc = Some(1_000);
    file.chats.insert(CHAT, chat);
}

/// A chat with every switch off spawns nothing and writes nothing.
#[test]
fn all_off_chat_spawns_no_job_and_enqueues_nothing() {
    let root = TempRoot::new("all-off");
    let mut host = TickHost::open(root.notifications());
    host.admit(CHAT);
    host.edit(|file| {
        file.chats.insert(CHAT, ChatNotify::default());
    });
    host.state.injected_reads = Some(empty_reads());
    host.tick(2_000);
    assert_eq!(host.jobs, 0, "an all-off file must not spawn a read");
    assert!(host.outbox().is_empty(), "an all-off file must not enqueue");
}

/// A close after the enable moment is sent once. An older close is not, and the next tick adds nothing.
#[test]
fn enabled_chat_enqueues_closes_after_enable_once() {
    let _locale = crate::test_locale::force("en");
    let root = TempRoot::new("trades");
    let mut host = TickHost::open(root.notifications());
    host.admit(CHAT);
    host.edit(enable_trades);
    host.state.visible_override = Some(vec![7]);
    host.state.injected_reads = Some(closes_around_enable());
    host.tick(2_000);
    let rows = host.outbox();
    assert_eq!(rows.len(), 1, "only the close after enable is announced");
    assert_eq!(rows[0].chat, CHAT);
    assert_eq!(rows[0].created_utc, 2_000);
    assert_eq!(rows[0].cores, Some(vec![7]));
    assert!(rows[0].html.contains("NEWCOIN"), "{}", rows[0].html);
    assert!(
        !rows[0].html.contains("OLDCOIN"),
        "a close before enable must stay out of the card: {}",
        rows[0].html
    );
    assert_eq!(host.seen_ids(), BTreeSet::from([11]));
    assert_eq!(host.jobs, 1);
    host.tick(2_000);
    assert_eq!(host.jobs, 1, "the interval has not elapsed");
    assert_eq!(
        host.outbox().len(),
        1,
        "a second tick must not send the card again"
    );
    assert_eq!(host.seen_ids(), BTreeSet::from([11]));
}

/// A settings revision that moves while the job runs skips the chat.
#[test]
fn revision_changed_during_the_job_skips_the_chat() {
    let _locale = crate::test_locale::force("en");
    let root = TempRoot::new("revision");
    let mut host = TickHost::open(root.notifications());
    host.admit(CHAT);
    host.edit(enable_trades);
    host.state.visible_override = Some(vec![7]);
    host.state.injected_reads = Some(closes_around_enable());
    host.during = DuringJob::BumpRevision(CHAT);
    host.tick(2_000);
    assert_eq!(host.jobs, 1);
    assert!(
        host.outbox().is_empty(),
        "a moved revision must not enqueue"
    );
    assert!(
        host.seen_ids().is_empty(),
        "a skipped chat must not mark seen"
    );
    assert!(!host.state.notify_busy, "finish must clear the busy flag");
    assert_eq!(
        host.file().chats.get(&CHAT).map(|chat| chat.revision),
        Some(2),
        "the in-flight settings save is the revision finish compares against"
    );
}

/// A fresh process with the same file does not send a close that is already in the ledger.
#[test]
fn restart_does_not_resend_a_seen_close() {
    let _locale = crate::test_locale::force("en");
    let root = TempRoot::new("restart");
    let path = root.notifications();
    let first_html = {
        let mut host = TickHost::open(path.clone());
        host.admit(CHAT);
        host.edit(enable_trades);
        host.state.visible_override = Some(vec![7]);
        host.state.injected_reads = Some(closes_around_enable());
        host.tick(2_000);
        let html = host.htmls();
        assert_eq!(html.len(), 1);
        assert!(html[0].contains("NEWCOIN"));
        html
    };
    let mut host = TickHost::open(path);
    host.admit(CHAT);
    host.state.visible_override = Some(vec![7]);
    host.state.injected_reads = Some(closes_around_enable());
    host.tick(2_000);
    assert_eq!(host.jobs, 1, "the restarted tick still reads");
    assert_eq!(
        host.htmls(),
        first_html,
        "a seen close must not be queued again"
    );
    assert_eq!(host.seen_ids(), BTreeSet::from([11]));
}

/// One down notice after the delay, one back notice, and no repeats.
#[test]
fn down_after_the_delay_is_enqueued_once() {
    let _locale = crate::test_locale::force("en");
    let root = TempRoot::new("down");
    let mut host = TickHost::open(root.notifications());
    host.admit(CHAT);
    host.edit(|file| {
        let mut chat = ChatNotify::default();
        chat.settings.down.on = true;
        chat.settings.down.after_minutes = 1;
        file.chats.insert(CHAT, chat);
    });
    host.state.visible_override = Some(vec![1]);
    host.state.injected_reads = Some(empty_reads());
    host.state.down_links_override = Some(vec![(1, "AlphaCore".to_string(), Link::Lost)]);

    host.tick(0);
    assert!(
        host.outbox().is_empty(),
        "the loss is still inside the delay"
    );
    assert_eq!(host.jobs, 0);

    host.tick(60);
    let down = host.outbox();
    assert_eq!(down.len(), 1);
    assert_eq!(down[0].created_utc, 60);
    assert_eq!(down[0].chat, CHAT);
    assert_eq!(down[0].cores, Some(vec![1]));
    assert!(down[0].html.contains("AlphaCore"), "{}", down[0].html);
    assert!(
        down[0].html.contains("lost connection since"),
        "{}",
        down[0].html
    );

    host.tick(120);
    assert_eq!(
        host.outbox().len(),
        1,
        "a still-lost core is not announced twice"
    );

    host.state.down_links_override = Some(vec![(1, "AlphaCore".to_string(), Link::Up)]);
    host.tick(180);
    let both = host.outbox();
    assert_eq!(both.len(), 2);
    assert!(
        both[0].html.contains("lost connection since"),
        "{}",
        both[0].html
    );
    assert!(
        both[1].html.contains("connection restored"),
        "{}",
        both[1].html
    );
    assert_eq!(both[1].created_utc, 180);
    assert_eq!(both[1].chat, CHAT);
    assert_eq!(both[1].cores, Some(vec![1]));

    host.tick(240);
    assert_eq!(
        host.outbox().len(),
        2,
        "a second up observation adds nothing"
    );
    assert_eq!(host.jobs, 0, "a down-only chat does not read the replica");
}

/// Unpairing while the job runs skips the chat.
#[test]
fn unpaired_chat_is_skipped_when_the_job_finishes() {
    let _locale = crate::test_locale::force("en");
    let root = TempRoot::new("unpair");
    let mut host = TickHost::open(root.notifications());
    host.admit(CHAT);
    host.edit(enable_trades);
    host.state.visible_override = Some(vec![7]);
    host.state.injected_reads = Some(closes_around_enable());
    host.during = DuringJob::Unpair;
    host.tick(2_000);
    assert_eq!(host.jobs, 1);
    assert!(
        host.outbox().is_empty(),
        "an unpaired chat must not enqueue"
    );
    assert!(
        host.seen_ids().is_empty(),
        "an unpaired chat must not mark seen"
    );
    assert!(!host.state.notify_busy);
}

/// A store pointer that changed while the job ran skips every chat.
#[test]
fn replaced_store_skips_every_chat() {
    let _locale = crate::test_locale::force("en");
    let root = TempRoot::new("replace");
    let mut host = TickHost::open(root.notifications());
    host.admit(CHAT);
    host.edit(enable_trades);
    host.state.visible_override = Some(vec![7]);
    host.state.injected_reads = Some(closes_around_enable());
    host.during = DuringJob::ReplaceStore;
    host.tick(2_000);
    assert_eq!(host.jobs, 1);
    assert!(
        host.outbox().is_empty(),
        "a replaced store must not be written"
    );
    assert!(host.seen_ids().is_empty());
    assert!(!host.state.notify_busy);
}

/// The summary for 1970-01-01 is sent once, from the injected day rather than the trade list.
#[test]
fn daily_summary_is_sent_once_on_the_due_day() {
    let _locale = crate::test_locale::force("en");
    let root = TempRoot::new("daily");
    let date = NaiveDate::from_ymd_opt(1970, 1, 1).expect("epoch date");
    let mut days = BTreeMap::new();
    days.insert(date, vec![closed_trade(21, 1_500, "DAYCOIN", Some(1.5))]);
    let mut host = TickHost::open(root.notifications());
    host.admit(CHAT);
    host.edit(|file| {
        let mut chat = ChatNotify {
            revision: 1,
            ..ChatNotify::default()
        };
        chat.settings.daily.on = true;
        chat.settings.daily.hour = 0;
        chat.settings.daily.minute = 0;
        file.chats.insert(CHAT, chat);
    });
    host.state.visible_override = Some(vec![7]);
    host.state.injected_reads = Some(InjectedReads {
        trades: Vec::new(),
        days,
    });
    host.tick(2_000);
    let rows = host.outbox();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].chat, CHAT);
    assert_eq!(rows[0].created_utc, 2_000);
    assert_eq!(rows[0].cores, Some(vec![7]));
    assert!(
        rows[0].html.contains("Daily summary 1970-01-01"),
        "{}",
        rows[0].html
    );
    assert!(rows[0].html.contains("DAYCOIN"), "{}", rows[0].html);
    assert_eq!(
        host.file()
            .chats
            .get(&CHAT)
            .and_then(|chat| chat.ledger.daily_last),
        Some(date)
    );
    assert_eq!(host.jobs, 1);
    host.state.last_notify_run = None;
    host.tick(2_000);
    assert_eq!(host.jobs, 1, "today's summary is already recorded");
    assert_eq!(host.outbox().len(), 1);
}

/// One queued row for a viewer fixture.
///
/// Args:
///     id: Outbox id. Tests pick ids so the remaining rows are obvious.
///     cores: Cores the row discloses. `None` is an unknown legacy row.
///         `Some([])` discloses no core and is not unknown.
///
/// Returns:
///     A row addressed to [`CHAT`].
fn queued(id: u64, cores: Option<Vec<u64>>) -> Pending {
    Pending {
        id,
        chat: CHAT,
        html: format!("row {id}"),
        created_utc: 1,
        cores,
    }
}

/// Seed three outbox rows and an all-off chat so the tick purges and does not send.
///
/// Args:
///     host: Host whose store will hold the rows.
fn seed_mixed_outbox(host: &TickHost) {
    host.edit(|file| {
        file.outbox = vec![
            queued(1, Some(vec![7])),
            queued(2, Some(vec![8])),
            queued(3, None),
        ];
        file.next_id = 4;
        file.chats.insert(CHAT, ChatNotify::default());
    });
}

/// A viewer drops a revoked core and an unknown row before anything is sent.
#[test]
fn viewer_outbox_drops_revoked_and_unknown_rows_before_send() {
    let root = TempRoot::new("viewer-purge");
    let mut host = TickHost::open(root.notifications());
    host.config.telegram.authorized_chat_ids = vec![CHAT];
    host.config.telegram.owner_chat_id = None;
    host.config.telegram.chat_profile_mut(CHAT).core_uids = vec![8];
    seed_mixed_outbox(&host);
    host.tick(2_000);
    let ids: Vec<u64> = host.outbox().into_iter().map(|row| row.id).collect();
    assert_eq!(ids, vec![2]);
    assert_eq!(host.jobs, 0, "an all-off chat must not send");
}

/// The owner keeps an unknown row and drops a hidden core before anything is sent.
#[test]
fn owner_outbox_keeps_unknown_rows_and_drops_a_hidden_core() {
    let root = TempRoot::new("owner-purge");
    let mut host = TickHost::open(root.notifications());
    host.admit(CHAT);
    host.state.visible_override = Some(vec![8]);
    seed_mixed_outbox(&host);
    host.tick(2_000);
    let ids: Vec<u64> = host.outbox().into_iter().map(|row| row.id).collect();
    assert_eq!(ids, vec![2, 3]);
    assert_eq!(host.jobs, 0, "an all-off chat must not send");
}

/// A viewer's empty-day summary discloses no core and survives the next purge.
#[test]
fn viewer_empty_day_summary_survives_the_tick_purge() {
    let _locale = crate::test_locale::force("en");
    let root = TempRoot::new("empty-day");
    let date = NaiveDate::from_ymd_opt(1970, 1, 1).expect("epoch date");
    let mut host = TickHost::open(root.notifications());
    host.config.telegram.authorized_chat_ids = vec![CHAT];
    host.config.telegram.owner_chat_id = None;
    host.config.telegram.chat_profile_mut(CHAT).core_uids = vec![8];
    host.edit(|file| {
        let mut chat = ChatNotify {
            revision: 1,
            ..ChatNotify::default()
        };
        chat.settings.daily.on = true;
        chat.settings.daily.hour = 0;
        chat.settings.daily.minute = 0;
        file.chats.insert(CHAT, chat);
    });
    host.state.visible_override = Some(vec![8]);
    host.state.injected_reads = Some(InjectedReads {
        trades: Vec::new(),
        days: BTreeMap::new(),
    });
    host.tick(2_000);
    let rows = host.outbox();
    assert_eq!(rows.len(), 1, "an empty day still sends the summary");
    assert_eq!(rows[0].cores, Some(Vec::new()));
    assert!(
        rows[0].html.contains("Daily summary 1970-01-01"),
        "{}",
        rows[0].html
    );
    assert_eq!(
        host.file()
            .chats
            .get(&CHAT)
            .and_then(|chat| chat.ledger.daily_last),
        Some(date)
    );
    let queued_id = rows[0].id;
    host.tick(2_000);
    let kept = host.outbox();
    assert_eq!(kept.len(), 1, "a quiet-day summary must survive the purge");
    assert_eq!(kept[0].cores, Some(Vec::new()));
    assert_eq!(kept[0].id, queued_id);
}

/// `seen` and `daily_last` written after the down snapshot must survive the write.
#[test]
fn write_down_keeps_seen_and_daily_last_set_after_the_snapshot() {
    let root = TempRoot::new("down-partial");
    let store = Mutex::new(NotifyStore::open(root.notifications()).expect("open"));
    let date = NaiveDate::from_ymd_opt(2026, 3, 1).expect("date");
    {
        let mut guard = store
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        guard
            .update(|file| {
                let mut chat = ChatNotify::default();
                chat.ledger.down_announced.insert(1);
                file.chats.insert(CHAT, chat);
            })
            .expect("seed");
    }
    let mut stepped = {
        let guard = store
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        guard.file.chats[&CHAT].ledger.clone()
    };
    {
        let mut guard = store
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        guard
            .update(|file| {
                let entry = file.chats.get_mut(&CHAT).expect("chat");
                entry.ledger.seen.insert(9, BTreeMap::from([(3, 50)]));
                entry.ledger.daily_last = Some(date);
            })
            .expect("live edit");
    }
    stepped.down_announced.insert(4);
    let wrote = super::write_down(&store, CHAT, stepped.clone(), Vec::new(), 80);
    assert!(matches!(wrote, Ok(true)), "the down write must land");
    let ledger = store
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .file
        .chats[&CHAT]
        .ledger
        .clone();
    assert_eq!(ledger.down_announced, stepped.down_announced);
    assert_eq!(ledger.daily_last, Some(date));
    assert_eq!(
        ledger.seen.get(&9).and_then(|rows| rows.get(&3)).copied(),
        Some(50)
    );
}
