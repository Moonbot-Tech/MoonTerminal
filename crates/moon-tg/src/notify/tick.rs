//! Owner-thread notification tick.
//!
//! `decide`, `due`, and `DownTracker::step` stay pure. This module reads the
//! report replica and writes the durable outbox.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use chrono::NaiveDate;
use chrono_tz::Tz;
use moon_core::config::telegram_access::TelegramReportAccess;
use moon_core::db::CoreNames;
use moon_core::telegram::TelegramService;
use moon_core::telegram::notify::{ChatNotify, DownRule, NotifyFile, NotifyLedger, NotifySettings};
use moon_core::telegram::runtime::{NotifyStore, cores_kept, purge_outbox_where, push_outbox};

use crate::notify::daily::{due, summarize};
use crate::notify::down::{DownEvent, Link, link_of};
use crate::notify::render::{back_line, daily_summary, down_line, trade_card};
use crate::notify::trades::{ClosedTrade, decide, read_from_utc};
use crate::{Finish, Job, ReportRevision, TelegramState, TgHost};

/// Minimum gap between notification reads. Down and back notices are not gated by it.
const NOTIFY_INTERVAL: Duration = Duration::from_secs(15);

/// Closed trades a test supplies so the job does not open the report database.
#[derive(Clone, Debug)]
pub(crate) struct InjectedReads {
    /// Rows `decide` may announce. Ignored when no chat has a trade read floor.
    pub(crate) trades: Vec<ClosedTrade>,
    /// Rows for one local day, keyed by that day. A missing key is an empty day.
    pub(crate) days: BTreeMap<NaiveDate, Vec<ClosedTrade>>,
}

/// One chat captured when a read is spawned.
struct ChatShot {
    chat: i64,
    revision: u64,
    access: TelegramReportAccess,
    daily: Option<NaiveDate>,
    read_from: Option<i64>,
}

/// Settings and ledger copied out of the store before admission is checked.
struct ChatSnap {
    chat: i64,
    revision: u64,
    settings: NotifySettings,
    ledger: NotifyLedger,
}

/// A chat whose grant still matches, with the cores it may see at finish time.
struct ChatApply {
    chat: i64,
    revision: u64,
    daily: Option<NaiveDate>,
    visible: Vec<u64>,
}

/// What one notification read is going to load.
struct ReadPlan {
    names: CoreNames,
    zone: Tz,
    from_utc: Option<i64>,
    days: Vec<NaiveDate>,
    shots: Vec<ChatShot>,
    now_utc: i64,
}

/// Trades and day rows a finished read hands back to the owner thread.
struct Loaded {
    trades: Vec<ClosedTrade>,
    days: BTreeMap<NaiveDate, Vec<ClosedTrade>>,
}

/// The report replica could not be read. The job does not log this; the owner does.
struct ReadMiss;

/// The notifications file could not be saved.
struct SaveMiss;

/// Run down/back notices, then at most one closed-trade and daily read, then at most one
/// automatic-report read.
///
/// Args:
///     host: The process that owns the bot. Down links come from its session.
///     now_utc: Current UTC Unix seconds. The caller reads the clock.
///
/// A missing store returns before any session call. An empty outbox is purged
/// without a session call. A file where every switch is off returns after that
/// purge and before a down step or a read.
pub(crate) fn run(host: &mut dyn TgHost, now_utc: i64) {
    let Some(store) = current_store(host) else {
        return;
    };
    purge_revoked_rows(host, &store);
    if !anything_on(&store) {
        return;
    }
    step_down(host, &store, now_utc);
    maybe_spawn(host, &store, now_utc);
    crate::notify::reports::run(host, &store, now_utc);
}

/// Drop queued rows that name a core the chat can no longer see.
///
/// The allow map is published first, including when the outbox is empty, and
/// replaced only when it changed. An empty outbox then returns before any
/// session call. The file is rewritten only when at least one row would go.
/// An unpaired chat is left for the sender. A failed save leaves the published
/// map in place.
///
/// Args:
///     host: Current grants and the visible-core override.
///     store: Notifications file.
fn purge_revoked_rows(host: &dyn TgHost, store: &Mutex<NotifyStore>) {
    publish_allowed(host, store);
    if lock_store(store).file.outbox.is_empty() {
        return;
    }
    let chats = outbox_chats(store);
    let mut plans = Vec::new();
    for chat in chats {
        let Some(access) = host.config().telegram.report_access(chat) else {
            continue;
        };
        let keep_unknown = matches!(access, TelegramReportAccess::Owner);
        let visible: BTreeSet<u64> = visible_ids(host, &access).into_iter().collect();
        plans.push((chat, visible, keep_unknown));
    }
    if plans.is_empty() {
        return;
    }
    let saved = {
        let mut guard = lock_store(store);
        purge_outbox_where(&mut guard, |row| {
            let Some((_, visible, keep_unknown)) =
                plans.iter().find(|(chat, _, _)| *chat == row.chat)
            else {
                return true;
            };
            cores_kept(&row.cores, visible, *keep_unknown)
        })
    };
    if let Err(error) = saved {
        log::warn!("telegram notify kept a row for a core the chat can no longer see: {error}");
    }
}

/// Publish who may receive a queued row.
///
/// An owner is stored as `None` (any core) and does not read the session.
/// A viewer is stored as the cores [`visible_ids`] already returns. The map
/// is replaced only when it differs. The outbox file is not written.
///
/// Args:
///     host: Pairing, grants, and the visible-core override.
///     store: Notifications file whose in-memory allow map is updated.
fn publish_allowed(host: &dyn TgHost, store: &Mutex<NotifyStore>) {
    let mut next = BTreeMap::new();
    let telegram = &host.config().telegram;
    for &chat in &telegram.authorized_chat_ids {
        let Some(access) = telegram.report_access(chat) else {
            continue;
        };
        let grant = match &access {
            TelegramReportAccess::Owner => None,
            TelegramReportAccess::Viewer(_) => {
                Some(visible_ids(host, &access).into_iter().collect())
            }
        };
        next.insert(chat, grant);
    }
    lock_store(store).publish_allowed(next);
}

/// Distinct chats that still have an outbox row, in first-seen order.
///
/// Args:
///     store: Notifications file.
///
/// Returns:
///     Chat ids. The lock is not held after this returns.
fn outbox_chats(store: &Mutex<NotifyStore>) -> Vec<i64> {
    let guard = lock_store(store);
    let mut chats = Vec::new();
    for row in &guard.file.outbox {
        if !chats.contains(&row.chat) {
            chats.push(row.chat);
        }
    }
    chats
}

/// The service store, or the test override when no service is running.
///
/// Args:
///     host: Owner of the bot state.
///
/// Returns:
///     The store the tick edits, or `None` when this process has no file.
pub(super) fn current_store(host: &dyn TgHost) -> Option<Arc<Mutex<NotifyStore>>> {
    service_store(host).or_else(|| test_store(host))
}

/// Store attached to a running service.
///
/// Args:
///     host: Owner of the bot state.
///
/// Returns:
///     The service store, or `None` when the service is absent or has no file.
fn service_store(host: &dyn TgHost) -> Option<Arc<Mutex<NotifyStore>>> {
    host.state()
        .service
        .as_ref()
        .and_then(TelegramService::notify_store)
}

/// Lock the store, recovering a poisoned mutex.
///
/// Args:
///     store: The notifications file.
///
/// Returns:
///     The guard. A poisoned lock still yields the inner store.
pub(super) fn lock_store(store: &Mutex<NotifyStore>) -> MutexGuard<'_, NotifyStore> {
    store
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// `true` when any chat in the file has a switch on, paired or not.
///
/// Args:
///     store: The notifications file.
///
/// Returns:
///     `false` when the file has no chat, or every chat is all-off.
fn anything_on(store: &Mutex<NotifyStore>) -> bool {
    lock_store(store).file.chats.values().any(chat_enabled)
}

/// `true` when this chat asked for trades, down/back, a daily summary or an automatic report.
///
/// Args:
///     chat: One stored chat.
///
/// Returns:
///     Whether any of its switches is on.
fn chat_enabled(chat: &ChatNotify) -> bool {
    chat.settings.trades.on
        || chat.settings.down.on
        || chat.settings.daily.on
        || chat.settings.reports.any()
}

/// Step every authorized chat whose down rule is on.
///
/// Trackers for chats that are no longer in that set are dropped, including
/// when the set is empty. A step runs at most once per second unless the host
/// has a frozen notify clock. The ledger is cloned, stepped once, and written
/// only when the step emitted a notice or `down_announced` changed. A failed
/// save leaves the previous tracker in place.
///
/// Args:
///     host: Session, admission, and the in-memory trackers.
///     store: Notifications file.
///     now_utc: Current UTC Unix seconds.
fn step_down(host: &mut dyn TgHost, store: &Arc<Mutex<NotifyStore>>, now_utc: i64) {
    let chats = down_chat_ids(host, store);
    host.state_mut()
        .down_trackers
        .retain(|chat, _| chats.contains(chat));
    if chats.is_empty() || down_step_waiting(host) {
        return;
    }
    host.state_mut().last_down_step = Some(Instant::now());
    let links = configured_links(host);
    let zone = host.report_zone();
    for chat in chats {
        step_one_down(host, store, chat, &links, zone, now_utc);
    }
}

/// Whether this tick should skip the down step.
///
/// A frozen notify clock always steps, so tests that jump the logical clock
/// still emit. Otherwise a step inside the last second is skipped.
///
/// Args:
///     host: Last step instant and the optional frozen clock.
///
/// Returns:
///     `true` when the step must not run.
fn down_step_waiting(host: &dyn TgHost) -> bool {
    if host.state().notify_clock_override.is_some() {
        return false;
    }
    host.state()
        .last_down_step
        .is_some_and(|at| at.elapsed() < Duration::from_secs(1))
}

/// Authorized chats with the down rule on.
///
/// Args:
///     host: Saved pairing. Only `authorized_chat_ids` is read.
///     store: Notifications file.
///
/// Returns:
///     Chat ids, in file order. Empty when nobody is both paired and enabled,
///     and then the caller does not touch the session.
fn down_chat_ids(host: &dyn TgHost, store: &Mutex<NotifyStore>) -> Vec<i64> {
    let authorized = &host.config().telegram.authorized_chat_ids;
    lock_store(store)
        .file
        .chats
        .iter()
        .filter_map(|(&chat, entry)| {
            (entry.settings.down.on && authorized.contains(&chat)).then_some(chat)
        })
        .collect()
}

/// Live core links, or the test override when one is installed.
///
/// Args:
///     host: Session host. The override skips the session entirely.
///
/// Returns:
///     `(core id, configured name, link)` for cores the down machine can see.
fn configured_links(host: &dyn TgHost) -> Vec<(u64, String, Link)> {
    if let Some(links) = down_override(host) {
        return links;
    }
    live_links(host)
}

/// Core id, name, and link for every session core that has store data.
///
/// Args:
///     host: Live sessions. The id list is copied before the store borrow.
///
/// Returns:
///     One row per core `link_of` understands. A core with no store row is skipped.
///     No enabled-flag filter: every listed session core is a configured core.
fn live_links(host: &dyn TgHost) -> Vec<(u64, String, Link)> {
    let listed: Vec<(u64, String)> = host
        .session()
        .sessions()
        .iter()
        .map(|row| (row.id, row.name.clone()))
        .collect();
    let store = host.session().store();
    let mut links = Vec::new();
    for (id, name) in listed {
        let Some(data) = store.core(id) else {
            continue;
        };
        let Some(link) = link_of(&data.status) else {
            continue;
        };
        links.push((id, name, link));
    }
    links
}

/// Step one chat on a cloned ledger and save only when the disk state moved.
///
/// The in-memory tracker is replaced when only `lost_since` moved, and also
/// after a successful save. A missing chat drops the tracker. A failed save
/// leaves the previous tracker.
///
/// Args:
///     host: Trackers. The tracker is replaced only after a successful decision.
///     store: Notifications file.
///     chat: Chat being stepped.
///     links: Core id, name, and link observed now.
///     zone: Host report zone, for the down clock.
///     now_utc: Current UTC Unix seconds, stored on each outbox row.
fn step_one_down(
    host: &mut dyn TgHost,
    store: &Mutex<NotifyStore>,
    chat: i64,
    links: &[(u64, String, Link)],
    zone: Tz,
    now_utc: i64,
) {
    let Some(access) = host.config().telegram.report_access(chat) else {
        return;
    };
    let visible = visible_ids(host, &access);
    let Some((rule, ledger)) = chat_down_snapshot(store, chat) else {
        host.state_mut().down_trackers.remove(&chat);
        return;
    };
    let tracker = host
        .state()
        .down_trackers
        .get(&chat)
        .cloned()
        .unwrap_or_default();
    let mut next = tracker.clone();
    let mut stepped = ledger.clone();
    let observed = visible_links(links, &visible);
    let events = next.step(&rule, &mut stepped, &observed, now_utc);
    let announced_changed = stepped.down_announced != ledger.down_announced;
    if events.is_empty() && !announced_changed {
        host.state_mut().down_trackers.insert(chat, next);
        return;
    }
    let messages: Vec<(String, Option<Vec<u64>>)> = events
        .iter()
        .copied()
        .map(|event| {
            (
                event_html(event, links, zone),
                Some(vec![event_core(event)]),
            )
        })
        .collect();
    match write_down(store, chat, stepped, messages, now_utc) {
        Ok(true) => {
            host.state_mut().down_trackers.insert(chat, next);
        }
        Ok(false) => {
            host.state_mut().down_trackers.remove(&chat);
        }
        Err(SaveMiss) => {
            log::warn!("telegram notify down save failed; the outage was not recorded");
        }
    }
}

/// Copy the down rule and ledger for `chat`.
///
/// Args:
///     store: Notifications file.
///     chat: Chat being stepped.
///
/// Returns:
///     The rule and a clone of the ledger, or `None` when the chat is gone.
fn chat_down_snapshot(store: &Mutex<NotifyStore>, chat: i64) -> Option<(DownRule, NotifyLedger)> {
    let guard = lock_store(store);
    let entry = guard.file.chats.get(&chat)?;
    Some((entry.settings.down, entry.ledger.clone()))
}

/// Write `down_announced` from the stepped ledger and any notices in one save.
///
/// `seen` and `daily_last` on the stored chat stay as they are. The step only
/// edits `down_announced`, so copying that field loses nothing the step wrote.
///
/// Args:
///     store: Notifications file.
///     chat: Chat being written.
///     ledger: Ledger after one step. Only `down_announced` is copied back.
///     messages: HTML and the cores each notice discloses.
///     now_utc: UTC Unix seconds stored on each outbox row.
///
/// Returns:
///     `Ok(true)` after the save. `Ok(false)` when the chat disappeared
///     before the write.
///
/// Errors:
///     [`SaveMiss`] when the atomic write failed. The in-memory file stays
///     as it was.
fn write_down(
    store: &Mutex<NotifyStore>,
    chat: i64,
    ledger: NotifyLedger,
    messages: Vec<(String, Option<Vec<u64>>)>,
    now_utc: i64,
) -> Result<bool, SaveMiss> {
    lock_store(store)
        .update(|file| {
            let Some(entry) = file.chats.get_mut(&chat) else {
                return false;
            };
            entry.ledger.down_announced = ledger.down_announced;
            enqueue_all(file, chat, messages, now_utc);
            true
        })
        .map_err(|_| SaveMiss)
}

/// Links whose core is in `visible`, without the display name.
///
/// Args:
///     links: Full observation, including names.
///     visible: Cores this chat may see.
///
/// Returns:
///     `(core, link)` pairs in `links` order.
fn visible_links(links: &[(u64, String, Link)], visible: &[u64]) -> Vec<(u64, Link)> {
    links
        .iter()
        .filter(|(id, _, _)| visible.contains(id))
        .map(|(id, _, link)| (*id, *link))
        .collect()
}

/// Render one down or back notice.
///
/// Args:
///     event: Notice from [`DownTracker::step`].
///     links: Observations, used for the core's configured name.
///     zone: Host report zone. Only the down clock uses it.
///
/// Returns:
///     One plain HTML line. An unknown core renders with an empty name.
fn event_html(event: DownEvent, links: &[(u64, String, Link)], zone: Tz) -> String {
    let name = link_name(links, event_core(event));
    match event {
        DownEvent::Down { since_utc, .. } => down_line(name, since_utc, zone),
        DownEvent::Back { down_for_secs, .. } => back_line(name, down_for_secs),
    }
}

/// Core id carried by either notice.
///
/// Args:
///     event: Notice from [`DownTracker::step`].
///
/// Returns:
///     The core the notice names.
fn event_core(event: DownEvent) -> u64 {
    match event {
        DownEvent::Down { core, .. } | DownEvent::Back { core, .. } => core,
    }
}

/// Configured name for `core`, or empty when this observation did not include it.
///
/// Args:
///     links: Observations from this tick.
///     core: Core the notice names.
///
/// Returns:
///     The name stored beside that core, borrowed from `links`.
fn link_name(links: &[(u64, String, Link)], core: u64) -> &str {
    links
        .iter()
        .find(|(id, _, _)| *id == core)
        .map(|(_, name, _)| name.as_str())
        .unwrap_or("")
}

/// Spawn one read when the interval is open and a chat needs trades or a daily summary.
///
/// Args:
///     host: Admission, zone, and the busy flag.
///     store: Notifications file. The job does not lock it; finish does.
///     now_utc: Current UTC Unix seconds.
fn maybe_spawn(host: &mut dyn TgHost, store: &Arc<Mutex<NotifyStore>>, now_utc: i64) {
    if host.state().notify_busy {
        return;
    }
    let shots = notification_shots(host, store, now_utc);
    if shots.is_empty() {
        return;
    }
    let daily_due = shots.iter().any(|shot| shot.daily.is_some());
    if !reads_are_due(host, daily_due) {
        return;
    }
    let plan = read_plan(host, shots, now_utc);
    arm_and_spawn(host, store, plan);
}

/// Chats that need a trade read or a daily summary on this tick.
///
/// Args:
///     host: Pairing and report zone. Admission is checked after the store lock drops.
///     store: Notifications file.
///     now_utc: Current UTC Unix seconds.
///
/// Returns:
///     One shot per paired chat that has trades on or a due daily rule.
///     Unpaired chats and `report_access` of `None` are left out.
fn notification_shots(
    host: &dyn TgHost,
    store: &Mutex<NotifyStore>,
    now_utc: i64,
) -> Vec<ChatShot> {
    let snaps = paired_snapshots(host, store);
    let zone = host.report_zone();
    shots_from(host, snaps, zone, now_utc)
}

/// Copy paired chats that have trades or daily switched on.
///
/// Args:
///     host: Saved `authorized_chat_ids`.
///     store: Notifications file. The guard ends before this returns.
///
/// Returns:
///     Snapshots. The lock is not held.
fn paired_snapshots(host: &dyn TgHost, store: &Mutex<NotifyStore>) -> Vec<ChatSnap> {
    let authorized = &host.config().telegram.authorized_chat_ids;
    lock_store(store)
        .file
        .chats
        .iter()
        .filter_map(|(&chat, entry)| snap_if_reading(authorized, chat, entry))
        .collect()
}

/// Snapshot `entry` when it is paired and reads trades or a daily summary.
///
/// Args:
///     authorized: Paired chat ids.
///     chat: Chat id.
///     entry: Stored settings and ledger.
///
/// Returns:
///     `None` when the chat is unpaired, or both the trade rule and the daily
///     rule are off. Down-only chats are not read from the replica.
fn snap_if_reading(authorized: &[i64], chat: i64, entry: &ChatNotify) -> Option<ChatSnap> {
    if !authorized.contains(&chat) || !reads_trades_or_daily(entry) {
        return None;
    }
    Some(ChatSnap {
        chat,
        revision: entry.revision,
        settings: entry.settings.clone(),
        ledger: entry.ledger.clone(),
    })
}

/// `true` when this chat wants trade cards or a daily summary.
///
/// Args:
///     entry: Stored settings. The daily clock is not consulted here.
///
/// Returns:
///     Whether a later shot might need a replica read.
fn reads_trades_or_daily(entry: &ChatNotify) -> bool {
    entry.settings.trades.on || entry.settings.daily.on
}

/// Turn snapshots into shots, dropping chats that are no longer admitted.
///
/// Args:
///     host: Current pairing and grants.
///     snaps: Chats copied under the lock.
///     zone: Host report zone, for [`due`].
///     now_utc: Current UTC Unix seconds.
///
/// Returns:
///     Shots for chats `report_access` still admits, and only when trades are
///     on or today's summary is due. `read_from` stays `None` when the trade
///     rule has never been enabled.
fn shots_from(host: &dyn TgHost, snaps: Vec<ChatSnap>, zone: Tz, now_utc: i64) -> Vec<ChatShot> {
    let mut shots = Vec::new();
    for snap in snaps {
        let Some(access) = host.config().telegram.report_access(snap.chat) else {
            continue;
        };
        let daily = due(now_utc, zone, &snap.settings.daily, snap.ledger.daily_last);
        if !snap.settings.trades.on && daily.is_none() {
            continue;
        }
        shots.push(shot_for(snap, access, daily, now_utc));
    }
    shots
}

/// One shot. The trade floor is recorded only while the trade rule is on.
///
/// Args:
///     snap: Settings captured under the lock.
///     access: Grant at spawn. Finish compares it again.
///     daily: Local date whose summary is due, if any.
///     now_utc: Current UTC Unix seconds.
///
/// Returns:
///     The shot. `read_from` is `None` when trades are off or were never enabled.
fn shot_for(
    snap: ChatSnap,
    access: TelegramReportAccess,
    daily: Option<NaiveDate>,
    now_utc: i64,
) -> ChatShot {
    let read_from = snap
        .settings
        .trades
        .on
        .then(|| read_from_utc(&snap.ledger, now_utc))
        .flatten();
    ChatShot {
        chat: snap.chat,
        revision: snap.revision,
        access,
        daily,
        read_from,
    }
}

/// Whether this tick may start a read.
///
/// Args:
///     host: Busy flag, last run, and report revision.
///     daily_due: A captured shot has a summary due today.
///
/// Returns:
///     `false` while a read is in flight, before the interval, or when neither
///     the revision nor a due daily rule asks for another read.
fn reads_are_due(host: &dyn TgHost, daily_due: bool) -> bool {
    let revision = host.report_revision();
    interval_open(host.state(), revision, daily_due)
}

/// The 15-second gate.
///
/// Args:
///     state: Busy flag and the previous spawn's stamp and revision.
///     revision: Host revision now. `None` matches a previous `None`.
///     daily_due: A daily rule is due, which opens the gate after the interval
///         even when the revision did not move.
///
/// Returns:
///     `true` for the first run (`last_notify_run` is `None`). Afterwards, only
///     when the interval has elapsed and the revision changed or `daily_due`.
fn interval_open(state: &TelegramState, revision: Option<ReportRevision>, daily_due: bool) -> bool {
    if state.notify_busy {
        return false;
    }
    match state.last_notify_run {
        None => true,
        Some(at) => {
            at.elapsed() >= NOTIFY_INTERVAL && (state.last_report_revision != revision || daily_due)
        }
    }
}

/// Names, zone, and the union of what the shots need to read.
///
/// Args:
///     host: Configured core names and report zone.
///     shots: Chats captured for this spawn.
///     now_utc: Current UTC Unix seconds, stored for finish.
///
/// Returns:
///     The plan. `from_utc` is the earliest trade floor, or `None` when no
///     shot has one. Days are sorted and deduplicated.
fn read_plan(host: &dyn TgHost, shots: Vec<ChatShot>, now_utc: i64) -> ReadPlan {
    let from_utc = shots.iter().filter_map(|shot| shot.read_from).min();
    let mut days: Vec<NaiveDate> = shots.iter().filter_map(|shot| shot.daily).collect();
    days.sort();
    days.dedup();
    ReadPlan {
        names: CoreNames::from_servers(&host.config().servers),
        zone: host.report_zone(),
        from_utc,
        days,
        shots,
        now_utc,
    }
}

/// Mark the read in flight, then spawn it.
///
/// `last_notify_run` moves before the read returns so a failed read does not
/// hot-loop the owner tick. `last_report_revision` stays until a save lands.
///
/// Args:
///     host: Busy flag and spawn hook.
///     store: Store whose pointer finish compares. Not moved into the job.
///     plan: What the job reads, moved into the job.
fn arm_and_spawn(host: &mut dyn TgHost, store: &Arc<Mutex<NotifyStore>>, plan: ReadPlan) {
    let store_ptr = Arc::as_ptr(store).addr();
    let injected = injected_reads(host);
    let revision = host.report_revision();
    let state = host.state_mut();
    state.notify_busy = true;
    state.last_notify_run = Some(Instant::now());
    let job: Job = Box::new(move || finish_job(plan, injected, store_ptr, revision));
    host.spawn(job);
}

/// Read off the owner thread and build the finish callback.
///
/// Args:
///     plan: Names, window, and the chats captured at spawn.
///     injected: Test rows. `None` reads the report replica.
///     store_ptr: Address of the store Arc at spawn.
///     revision: Host revision captured at spawn. Stamped only after a save.
///
/// Returns:
///     A finish callback. It does not run here.
fn finish_job(
    plan: ReadPlan,
    injected: Option<InjectedReads>,
    store_ptr: usize,
    revision: Option<ReportRevision>,
) -> Finish {
    let loaded = load_reads(&plan, injected.as_ref());
    let ReadPlan { shots, now_utc, .. } = plan;
    Box::new(move |host| apply_finish(host, store_ptr, loaded, &shots, now_utc, revision))
}

/// Load trades and due days, or fail the whole read.
///
/// Args:
///     plan: Window and days. `from_utc` of `None` skips the trade read.
///     injected: Test rows. `Some` never opens the replica.
///
/// Returns:
///     Every requested row. A missing injected day is an empty vector.
///
/// Errors:
///     [`ReadMiss`] when any replica read fails. Partial rows are discarded
///     and this function does not log.
fn load_reads(plan: &ReadPlan, injected: Option<&InjectedReads>) -> Result<Loaded, ReadMiss> {
    if let Some(injected) = injected {
        return Ok(from_injection(plan, injected));
    }
    from_replica(plan)
}

/// Take the injected rows the plan actually asked for.
///
/// Args:
///     plan: `from_utc` of `None` yields no trades. Days absent from the map
///         are empty, not an error.
///     injected: Rows the test installed.
///
/// Returns:
///     Trades and day rows. Trade filtering is left to `decide`.
fn from_injection(plan: &ReadPlan, injected: &InjectedReads) -> Loaded {
    let trades = if plan.from_utc.is_some() {
        injected.trades.clone()
    } else {
        Vec::new()
    };
    let mut days = BTreeMap::new();
    for date in &plan.days {
        days.insert(*date, injected.days.get(date).cloned().unwrap_or_default());
    }
    Loaded { trades, days }
}

/// Read the report replica. Any failure aborts the whole load.
///
/// Args:
///     plan: Names, zone, trade floor, and due days.
///
/// Returns:
///     Trades with `close_utc >= from_utc`, and one vector per due day.
///
/// Errors:
///     [`ReadMiss`] from the first failed read. Nothing is logged here.
fn from_replica(plan: &ReadPlan) -> Result<Loaded, ReadMiss> {
    Ok(Loaded {
        trades: read_trades(plan)?,
        days: read_days(plan)?,
    })
}

/// Closed trades at or after the plan floor.
///
/// Args:
///     plan: `from_utc` of `None` reads nothing.
///
/// Returns:
///     The replica rows, or an empty vector when no chat has a floor.
///
/// Errors:
///     [`ReadMiss`] when the replica read fails.
fn read_trades(plan: &ReadPlan) -> Result<Vec<ClosedTrade>, ReadMiss> {
    let Some(from_utc) = plan.from_utc else {
        return Ok(Vec::new());
    };
    crate::report::read_closed_since(plan.zone, plan.names.clone(), from_utc).map_err(|_| ReadMiss)
}

/// One replica read per due day.
///
/// Args:
///     plan: Days, zone, and core names.
///
/// Returns:
///     Rows keyed by the local date.
///
/// Errors:
///     [`ReadMiss`] when any day fails. Earlier days are dropped with it.
fn read_days(plan: &ReadPlan) -> Result<BTreeMap<NaiveDate, Vec<ClosedTrade>>, ReadMiss> {
    let mut days = BTreeMap::new();
    for date in &plan.days {
        let rows =
            crate::report::read_day(plan.zone, plan.names.clone(), *date).map_err(|_| ReadMiss)?;
        days.insert(*date, rows);
    }
    Ok(days)
}

/// Apply a finished read on the owner thread and always clear the busy flag.
///
/// A save stamps `last_report_revision` with the revision captured at spawn.
/// A replaced store, a failed read, an empty apply list, or a failed save sets
/// that revision to `None` and does not advance the ledgers.
///
/// Args:
///     host: Store, grants, and the busy flag.
///     store_ptr: Address captured at spawn. A different store skips every chat.
///     loaded: Rows, or [`ReadMiss`] when the replica read failed.
///     shots: Chats and grants captured at spawn.
///     now_utc: UTC Unix seconds captured at spawn, stored on outbox rows and
///         passed to `decide`. Today's date is read again at finish.
///     revision: Host revision captured at spawn. Stamped only after a save.
fn apply_finish(
    host: &mut dyn TgHost,
    store_ptr: usize,
    loaded: Result<Loaded, ReadMiss>,
    shots: &[ChatShot],
    now_utc: i64,
    revision: Option<ReportRevision>,
) {
    let Some(store) = current_store(host) else {
        skip_replaced(host);
        return;
    };
    if Arc::as_ptr(&store).addr() != store_ptr {
        skip_replaced(host);
        return;
    }
    let Ok(loaded) = loaded else {
        log::warn!("telegram notify read failed; the ledger was not advanced");
        reject_finish(host);
        return;
    };
    let applies = prepare_applies(host, shots);
    if applies.is_empty() {
        reject_finish(host);
        return;
    }
    let today = finish_today(host, now_utc);
    match save_applies(&store, &applies, &loaded, today, now_utc) {
        Ok(skipped) => {
            log_skips(&skipped);
            host.state_mut().last_report_revision = revision;
            clear_busy(host);
        }
        Err(SaveMiss) => {
            log::warn!("telegram notify save failed; the ledger was not advanced");
            reject_finish(host);
        }
    }
}

/// UTC seconds used to name today's local date at finish.
///
/// A frozen notify clock wins, so a test that passes unix 2000 still lands on
/// 1970-01-01. Otherwise the wall clock is read now. A value that does not fit
/// in `i64` falls back to the spawn instant.
///
/// Args:
///     host: Optional frozen clock.
///     spawned_utc: UTC Unix seconds captured when the read was spawned.
///
/// Returns:
///     Seconds passed to the zone conversion.
fn finish_clock(host: &dyn TgHost, spawned_utc: i64) -> i64 {
    if let Some(frozen) = host.state().notify_clock_override {
        return frozen;
    }
    i64::try_from(moon_core::util::time::now_unix_secs()).unwrap_or(spawned_utc)
}

/// Local calendar date at finish, in the host report zone.
///
/// Args:
///     host: Report zone and the optional frozen clock.
///     spawned_utc: Fallback when the wall clock does not fit in `i64`.
///
/// Returns:
///     Today's date, or `None` when the instant cannot be shown in the zone.
fn finish_today(host: &dyn TgHost, spawned_utc: i64) -> Option<NaiveDate> {
    let now = finish_clock(host, spawned_utc);
    moon_core::util::display_time::at(now, host.report_zone()).map(|local| local.date_naive())
}

/// Record that this read did not save, and allow the next one.
///
/// Args:
///     host: Revision stamp and busy flag. The store is not written.
fn reject_finish(host: &mut dyn TgHost) {
    host.state_mut().last_report_revision = None;
    clear_busy(host);
}

/// Log the replaced-store skip and reject the finish.
///
/// Args:
///     host: Busy flag and revision stamp. The store is not written.
fn skip_replaced(host: &mut dyn TgHost) {
    log::debug!("telegram notify skipped: notification store was replaced");
    reject_finish(host);
}

/// Allow the next read.
///
/// Args:
///     host: Busy flag.
fn clear_busy(host: &mut dyn TgHost) {
    host.state_mut().notify_busy = false;
}

/// Keep shots whose pairing and grant still match, and recompute visibility.
///
/// Args:
///     host: Current pairing, grants, and session. Visibility is computed here,
///         not at spawn and not while the store lock is held.
///     shots: Chats captured at spawn.
///
/// Returns:
///     Chats finish may edit. A mismatch is logged and omitted.
fn prepare_applies(host: &dyn TgHost, shots: &[ChatShot]) -> Vec<ChatApply> {
    let mut applies = Vec::new();
    for shot in shots {
        if !grant_matches(host, shot) {
            log_skip(shot.chat);
            continue;
        }
        applies.push(ChatApply {
            chat: shot.chat,
            revision: shot.revision,
            daily: shot.daily,
            visible: visible_ids(host, &shot.access),
        });
    }
    applies
}

/// `true` when `shot` is still paired and its grant is unchanged.
///
/// Args:
///     host: Current pairing and grants.
///     shot: Chat captured at spawn.
///
/// Returns:
///     Whether finish may edit this chat. The settings revision is checked
///     later, inside the save.
fn grant_matches(host: &dyn TgHost, shot: &ChatShot) -> bool {
    let telegram = &host.config().telegram;
    telegram.authorized_chat_ids.contains(&shot.chat)
        && telegram.report_access(shot.chat) == Some(shot.access.clone())
}

/// Cores `access` may see right now.
///
/// Args:
///     host: Session, for an owner with no test override.
///     access: Grant. A viewer does not consult the session.
///
/// Returns:
///     Core ids. A test override replaces both arms.
fn visible_ids(host: &dyn TgHost, access: &TelegramReportAccess) -> Vec<u64> {
    if let Some(ids) = visible_override(host) {
        return ids;
    }
    match access {
        TelegramReportAccess::Owner => host.session().sessions().iter().map(|row| row.id).collect(),
        TelegramReportAccess::Viewer(ids) => ids.clone(),
    }
}

/// Save every still-current chat in one write.
///
/// Args:
///     store: Notifications file.
///     applies: Chats whose grant matched.
///     loaded: Trades and day rows.
///     today: Local date at finish. A daily summary whose captured date differs
///         is dropped and does not stamp `daily_last`.
///     now_utc: UTC Unix seconds stored on outbox rows.
///
/// Returns:
///     Chats the save skipped because they disappeared or their settings
///     revision moved.
///
/// Errors:
///     [`SaveMiss`] when the atomic write failed. Nothing was advanced.
fn save_applies(
    store: &Mutex<NotifyStore>,
    applies: &[ChatApply],
    loaded: &Loaded,
    today: Option<NaiveDate>,
    now_utc: i64,
) -> Result<Vec<i64>, SaveMiss> {
    lock_store(store)
        .update(|file| apply_messages(file, applies, loaded, today, now_utc))
        .map_err(|_| SaveMiss)
}

/// Render each chat and push its messages. A revision mismatch is returned, not logged.
///
/// Args:
///     file: Document clone `update` will save.
///     applies: Chats whose grant matched.
///     loaded: Trades and day rows.
///     today: Local date at finish. Compared with each captured daily date.
///     now_utc: UTC Unix seconds for `decide` and the outbox.
///
/// Returns:
///     Chats skipped because they disappeared or their settings revision moved.
fn apply_messages(
    file: &mut NotifyFile,
    applies: &[ChatApply],
    loaded: &Loaded,
    today: Option<NaiveDate>,
    now_utc: i64,
) -> Vec<i64> {
    let mut skipped = Vec::new();
    for apply in applies {
        match messages_for(file, apply, loaded, today, now_utc) {
            Some(messages) => enqueue_all(file, apply.chat, messages, now_utc),
            None => skipped.push(apply.chat),
        }
    }
    skipped
}

/// Messages for one chat, or `None` when the chat or its revision no longer matches.
///
/// Args:
///     file: Document being edited.
///     apply: Chat, captured revision, and the cores it may see now.
///     loaded: Trades and day rows.
///     today: Local date at finish.
///     now_utc: UTC Unix seconds passed to `decide`.
///
/// Returns:
///     Rendered messages and the cores each one discloses. Empty is still a
///     successful apply: the ledger may have changed. `None` means skip this chat.
fn messages_for(
    file: &mut NotifyFile,
    apply: &ChatApply,
    loaded: &Loaded,
    today: Option<NaiveDate>,
    now_utc: i64,
) -> Option<Vec<(String, Option<Vec<u64>>)>> {
    let entry = file.chats.get_mut(&apply.chat)?;
    if entry.revision != apply.revision {
        return None;
    }
    Some(render_chat(entry, apply, loaded, today, now_utc))
}

/// Trade cards and, when the captured day is still today, one daily summary.
///
/// `decide` runs even when the trade rule is off, which clears `seen`.
/// A matching day sets `daily_last` even when the day has no visible row.
/// A stale day leaves `daily_last` unchanged and omits the summary; trade cards still apply.
///
/// Args:
///     entry: Chat settings and ledger, edited in place.
///     apply: Captured revision, due date, and visible cores.
///     loaded: Trades and day rows.
///     today: Local date at finish.
///     now_utc: UTC Unix seconds passed to `decide`.
///
/// Returns:
///     Messages in send order, each with the cores it discloses. May be empty.
///     The cores are `Some`, and `Some([])` when a summary names no core.
fn render_chat(
    entry: &mut ChatNotify,
    apply: &ChatApply,
    loaded: &Loaded,
    today: Option<NaiveDate>,
    now_utc: i64,
) -> Vec<(String, Option<Vec<u64>>)> {
    let mut messages = trade_html(entry, apply, loaded, now_utc);
    if let Some(date) = apply.daily
        && let Some(summary) = one_daily(entry, date, loaded, &apply.visible, today)
    {
        messages.push(summary);
    }
    messages
}

/// Cards for the trades `decide` announces.
///
/// Args:
///     entry: Trade rule and ledger. `seen` is updated in place.
///     apply: Visible cores.
///     loaded: Closed trades from the read. `decide` applies the floor.
///     now_utc: UTC Unix seconds.
///
/// Returns:
///     One card per announced trade, oldest close first. Each card discloses
///     that trade's core.
fn trade_html(
    entry: &mut ChatNotify,
    apply: &ChatApply,
    loaded: &Loaded,
    now_utc: i64,
) -> Vec<(String, Option<Vec<u64>>)> {
    decide(
        &entry.settings.trades,
        &mut entry.ledger,
        &apply.visible,
        &loaded.trades,
        now_utc,
    )
    .iter()
    .map(|trade| (trade_card(trade), Some(vec![trade.core])))
    .collect()
}

/// One daily summary when `date` is still today.
///
/// `daily_last` is stamped only when the finish-time local date equals `date`.
/// A mismatch, or a date that cannot be computed, drops the summary and leaves
/// the ledger alone. An empty visible day still stamps when the dates match.
///
/// Args:
///     entry: Ledger. `daily_last` becomes `date` only on a match.
///     date: Local day chosen at spawn.
///     loaded: Day rows. A missing key is an empty day.
///     visible: Cores this chat may see. Other cores are left out of the totals.
///     today: Local date at finish. `None` drops the summary.
///
/// Returns:
///     The summary HTML and `Some` of the cores the counted rows disclose,
///     including `Some([])` when the day has no visible row. `None` when the
///     captured day is no longer today.
fn one_daily(
    entry: &mut ChatNotify,
    date: NaiveDate,
    loaded: &Loaded,
    visible: &[u64],
    today: Option<NaiveDate>,
) -> Option<(String, Option<Vec<u64>>)> {
    if today != Some(date) {
        return None;
    }
    let rows = day_rows(loaded, date, visible);
    let summary = summarize(&rows);
    entry.ledger.daily_last = Some(date);
    Some((daily_summary(date, &summary), Some(disclosed_cores(&rows))))
}

/// Unique cores in the order the rows first mention them.
///
/// Args:
///     rows: Visible rows the summary counted.
///
/// Returns:
///     Core ids. Empty when the day has no visible row.
fn disclosed_cores(rows: &[ClosedTrade]) -> Vec<u64> {
    let mut cores = Vec::new();
    for trade in rows {
        if !cores.contains(&trade.core) {
            cores.push(trade.core);
        }
    }
    cores
}

/// Visible rows for one local day.
///
/// Args:
///     loaded: Day rows from the read.
///     date: Local day.
///     visible: Cores this chat may see.
///
/// Returns:
///     Matching rows. A day the read did not contain is empty.
fn day_rows(loaded: &Loaded, date: NaiveDate, visible: &[u64]) -> Vec<ClosedTrade> {
    loaded
        .days
        .get(&date)
        .map(|rows| {
            rows.iter()
                .filter(|trade| visible.contains(&trade.core))
                .cloned()
                .collect()
        })
        .unwrap_or_default()
}

/// Append every message. An empty list writes nothing.
///
/// Args:
///     file: Document to append to. `next_id` advances once per accepted message.
///     chat: Destination chat.
///     messages: Rendered bodies and the cores each one discloses, in send order.
///     now_utc: UTC Unix seconds stored on each row.
fn enqueue_all(
    file: &mut NotifyFile,
    chat: i64,
    messages: Vec<(String, Option<Vec<u64>>)>,
    now_utc: i64,
) {
    for (html, cores) in messages {
        push_outbox(file, chat, html, cores, now_utc);
    }
}

/// Log one skipped chat. The text is the grant mismatch and the revision mismatch.
///
/// Args:
///     chat: Chat that was not edited.
fn log_skip(chat: i64) {
    log::debug!("telegram notify skipped chat {chat}: grant or settings changed");
}

/// Log every chat a save skipped.
///
/// Args:
///     chats: Chats whose revision or presence did not match.
fn log_skips(chats: &[i64]) {
    for chat in chats {
        log_skip(*chat);
    }
}

/// Test store installed on the host, if any.
///
/// Args:
///     host: Bot state.
///
/// Returns:
///     The override, or `None` in production and when the test left it unset.
fn test_store(host: &dyn TgHost) -> Option<Arc<Mutex<NotifyStore>>> {
    host.state().notify_store_override.clone()
}

/// Injected down links, if the test installed any.
///
/// Args:
///     host: Bot state.
///
/// Returns:
///     The override. `None` means the live session is consulted.
fn down_override(host: &dyn TgHost) -> Option<Vec<(u64, String, Link)>> {
    host.state().down_links_override.clone()
}

/// Injected visible cores, if the test installed any.
///
/// Args:
///     host: Bot state.
///
/// Returns:
///     The override. `None` means visibility comes from the grant.
fn visible_override(host: &dyn TgHost) -> Option<Vec<u64>> {
    host.state().visible_override.clone()
}

/// Injected replica rows, if the test installed any.
///
/// Args:
///     host: Bot state.
///
/// Returns:
///     The override. `None` means the job opens the report database.
fn injected_reads(host: &dyn TgHost) -> Option<InjectedReads> {
    host.state().injected_reads.clone()
}

#[cfg(test)]
mod tests;
