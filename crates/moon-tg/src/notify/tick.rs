//! Owner-thread notification tick.
//!
//! `decide` and `DownTracker::step` stay pure. This module reads the
//! report replica and writes the durable outbox.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use chrono_tz::Tz;
use moon_core::config::telegram_access::TelegramReportAccess;
use moon_core::config::telegram_layout::CardLayout;
use moon_core::db::CoreNames;
use moon_core::telegram::TelegramService;
use moon_core::telegram::notify::{
    CardKey, CardWait, ChatNotify, DownRule, NotifyFile, NotifyLedger, NotifySettings,
};
use moon_core::telegram::runtime::{
    NotifyStore, cores_kept, purge_outbox_where, push_edit, push_outbox, push_trade_card,
};

use crate::notify::charts::Due;
use crate::notify::down::{DownEvent, Link, link_of};
use crate::notify::render::{back_line, down_line, shows_dollars, trade_card};
use crate::notify::trades::{
    Announced, ClosedTrade, decide, decide_charts, floor_from, held_until, hold_until,
    read_from_utc,
};
use crate::{Finish, Job, ReportRevision, TelegramState, TgHost};

/// Minimum gap between notification reads. Down and back notices are not gated by it. A read
/// still waits for the report replica to move, so a quiet replica costs nothing; a trade card
/// leaves within this of its row landing. Measured 03.10 on a 565 MB replica: one read of the
/// 72-hour window costs ~100 ms warm, ~80 ms of it fixed, so 2 s keeps a replica that moves all
/// the time at ~5 % of one thread. 5 s made a card arrive seconds after the core's own bot
/// (LinKvo, 04.10).
const NOTIFY_INTERVAL: Duration = Duration::from_secs(2);

/// How often a card waiting for its dollar value forces a read while the replica stands still:
/// the valuation may have landed before Telegram accepted the card.
const CARD_RECHECK: Duration = Duration::from_secs(60);

/// How long a card waits for its dollar value, in seconds.
const CARD_WAIT_SECS: i64 = 24 * 3600;

/// One row a read queues for a chat.
enum Outgoing {
    /// A plain notification.
    Plain(String, Option<Vec<u64>>),
    /// A trade card; the trade is named when the chat waits to fill its dollars in.
    Card(String, Option<Vec<u64>>, Option<CardKey>),
    /// New text for a card already in the chat.
    Edit(i64, String, Option<Vec<u64>>),
}

/// Closed trades a test supplies so the job does not open the report database.
#[derive(Clone, Debug)]
pub(crate) struct InjectedReads {
    /// Rows `decide` may announce. Ignored when no chat has a trade read floor.
    pub(crate) trades: Vec<ClosedTrade>,
}

/// One chat captured when a read is spawned.
struct ChatShot {
    chat: i64,
    revision: u64,
    access: TelegramReportAccess,
    read_from: Option<i64>,
}

/// Settings and ledger copied out of the store before admission is checked.
struct ChatSnap {
    chat: i64,
    revision: u64,
    settings: NotifySettings,
    ledger: NotifyLedger,
}

/// A chat whose grant still matches, with visible cores and the owner's captured card layout.
struct ChatApply {
    chat: i64,
    revision: u64,
    visible: Vec<u64>,
    layout: Arc<CardLayout>,
}

/// What one notification read is going to load.
struct ReadPlan {
    names: CoreNames,
    zone: Tz,
    from_utc: Option<i64>,
    shots: Vec<ChatShot>,
    now_utc: i64,
}

/// Trades a finished read hands back to the owner thread.
struct Loaded {
    trades: Vec<ClosedTrade>,
}

/// The report replica could not be read. The job does not log this; the owner does.
struct ReadMiss;

/// The notifications file could not be saved.
struct SaveMiss;

/// Run down/back notices, then at most one closed-trade read, then at most one automatic-report
/// read.
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
        // The core events' cursors and what waited for a batch go with the last switch: one
        // turned on later starts at what happens from then on.
        let state = host.state_mut();
        state.events_cursor.clear();
        state.events_waiting.clear();
        state.chart_queue.clear();
        return;
    }
    step_down(host, &store, now_utc);
    crate::notify::events::run(host, &store, now_utc);
    maybe_spawn(host, &store, now_utc);
    crate::notify::charts::run(host, &store, now_utc);
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
pub(crate) fn current_store(host: &dyn TgHost) -> Option<Arc<Mutex<NotifyStore>>> {
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
pub(crate) fn lock_store(store: &Mutex<NotifyStore>) -> MutexGuard<'_, NotifyStore> {
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

/// `true` when this chat asked for trades, down/back, an automatic report or the cores' own
/// events.
///
/// Args:
///     chat: One stored chat.
///
/// Returns:
///     Whether any of its switches is on.
fn chat_enabled(chat: &ChatNotify) -> bool {
    chat.settings.trades.on
        || chat.settings.down.on
        || chat.settings.reports.any()
        || chat.settings.events.any()
        || chat.settings.charts.on
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
/// `seen` and the rest of the stored ledger stay as they are. The step only
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
            let rows = messages
                .into_iter()
                .map(|(html, cores)| Outgoing::Plain(html, cores))
                .collect();
            enqueue_all(file, chat, rows, now_utc);
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

/// Spawn one read when the interval is open and a chat needs trades.
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
    let forced = waits_are_due(host, store, now_utc);
    if !reads_are_due(host, forced) {
        return;
    }
    let plan = read_plan(host, shots, now_utc);
    arm_and_spawn(host, store, plan);
}

/// Chats that need a trade read on this tick.
///
/// Args:
///     host: Pairing. Admission is checked after the store lock drops.
///     store: Notifications file.
///     now_utc: Current UTC Unix seconds.
///
/// Returns:
///     One shot per paired chat that has trades on.
///     Unpaired chats and `report_access` of `None` are left out.
fn notification_shots(
    host: &dyn TgHost,
    store: &Mutex<NotifyStore>,
    now_utc: i64,
) -> Vec<ChatShot> {
    let snaps = paired_snapshots(host, store);
    shots_from(host, snaps, now_utc)
}

/// Copy paired chats that have trades switched on.
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

/// Snapshot `entry` when it is paired and reads trades.
///
/// Args:
///     authorized: Paired chat ids.
///     chat: Chat id.
///     entry: Stored settings and ledger.
///
/// Returns:
///     `None` when the chat is unpaired or both the trade and the chart rule are off. Chats
///     without either are not read from the replica.
fn snap_if_reading(authorized: &[i64], chat: i64, entry: &ChatNotify) -> Option<ChatSnap> {
    let settings = &entry.settings;
    if !authorized.contains(&chat) || !(settings.trades.on || settings.charts.on) {
        return None;
    }
    Some(ChatSnap {
        chat,
        revision: entry.revision,
        settings: entry.settings.clone(),
        ledger: entry.ledger.clone(),
    })
}

/// Turn snapshots into shots, dropping chats that are no longer admitted.
///
/// Args:
///     host: Current pairing and grants.
///     snaps: Chats copied under the lock, each with trades on.
///     now_utc: Current UTC Unix seconds.
///
/// Returns:
///     Shots for chats `report_access` still admits. `read_from` stays `None` when the trade
///     rule has never been enabled.
fn shots_from(host: &dyn TgHost, snaps: Vec<ChatSnap>, now_utc: i64) -> Vec<ChatShot> {
    let mut shots = Vec::new();
    for snap in snaps {
        let Some(access) = host.config().telegram.report_access(snap.chat) else {
            continue;
        };
        shots.push(shot_for(snap, access, now_utc));
    }
    shots
}

/// One shot. The trade floor is recorded only while the trade rule is on.
///
/// Args:
///     snap: Settings captured under the lock.
///     access: Grant at spawn. Finish compares it again.
///     now_utc: Current UTC Unix seconds.
///
/// Returns:
///     The shot. `read_from` is `None` when trades are off or were never enabled.
fn shot_for(snap: ChatSnap, access: TelegramReportAccess, now_utc: i64) -> ChatShot {
    let trades = snap
        .settings
        .trades
        .on
        .then(|| read_from_utc(&snap.ledger, now_utc))
        .flatten();
    let charts = snap
        .settings
        .charts
        .on
        .then(|| floor_from(snap.ledger.charts.enabled_utc, now_utc))
        .flatten();
    let read_from = trades.into_iter().chain(charts).min();
    ChatShot {
        chat: snap.chat,
        revision: snap.revision,
        access,
        read_from,
    }
}

/// Whether this tick may start a read.
///
/// Args:
///     host: Busy flag, last run, and report revision.
///     forced: Something is due without the replica moving: a held trade whose wait ran out,
///         or a card waiting for its dollar value.
///
/// Returns:
///     `false` while a read is in flight, before the interval, or when neither
///     the revision nor `forced` asks for another read.
fn reads_are_due(host: &dyn TgHost, forced: bool) -> bool {
    let revision = host.report_revision();
    interval_open(host.state(), revision, forced)
}

/// Whether a held trade or a waiting card needs a read even though the replica did not move.
///
/// A held trade does once its wait has run out: it then goes unchecked. A card that Telegram
/// accepted does every [`CARD_RECHECK`]: its valuation may have landed before the card's message
/// id did, and nothing else would read the row again.
///
/// Args:
///     host: The previous spawn's stamp.
///     store: Notifications file; every chat's ledger is read under one lock.
///     now_utc: Current UTC Unix seconds.
fn waits_are_due(host: &dyn TgHost, store: &Mutex<NotifyStore>, now_utc: i64) -> bool {
    let recheck = host
        .state()
        .last_notify_run
        .is_none_or(|at| at.elapsed() >= CARD_RECHECK);
    lock_store(store).file.chats.values().any(|entry| {
        let ledger = &entry.ledger;
        if entry.settings.charts.on
            && held_until(&ledger.charts.held).is_some_and(|until| now_utc >= until)
        {
            return true;
        }
        if !entry.settings.trades.on {
            return false;
        }
        hold_until(ledger).is_some_and(|until| now_utc >= until)
            || (recheck
                && ledger
                    .cards
                    .values()
                    .flat_map(|rows| rows.values())
                    .any(|card| card.message.is_some()))
    })
}

/// The read gate.
///
/// Args:
///     state: Busy flag and the previous spawn's stamp and revision.
///     revision: Host revision now. `None` matches a previous `None`.
///     forced: A read is due without the revision moving, which opens the gate after the
///         interval.
///
/// Returns:
///     `true` for the first run (`last_notify_run` is `None`). Afterwards, only
///     when the interval has elapsed and the revision changed or `forced`.
fn interval_open(state: &TelegramState, revision: Option<ReportRevision>, forced: bool) -> bool {
    if state.notify_busy {
        return false;
    }
    match state.last_notify_run {
        None => true,
        Some(at) => {
            at.elapsed() >= NOTIFY_INTERVAL && (state.last_report_revision != revision || forced)
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
///     shot has one.
fn read_plan(host: &dyn TgHost, shots: Vec<ChatShot>, now_utc: i64) -> ReadPlan {
    let from_utc = shots.iter().filter_map(|shot| shot.read_from).min();
    ReadPlan {
        names: CoreNames::from_servers(&host.config().servers),
        zone: host.report_zone(),
        from_utc,
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

/// Load trades, or fail the whole read.
///
/// Args:
///     plan: Window. `from_utc` of `None` skips the trade read.
///     injected: Test rows. `Some` never opens the replica.
///
/// Returns:
///     Every requested row.
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
///     plan: `from_utc` of `None` yields no trades.
///     injected: Rows the test installed.
///
/// Returns:
///     Trades. Trade filtering is left to `decide`.
fn from_injection(plan: &ReadPlan, injected: &InjectedReads) -> Loaded {
    let trades = if plan.from_utc.is_some() {
        injected.trades.clone()
    } else {
        Vec::new()
    };
    Loaded { trades }
}

/// Read the report replica.
///
/// Args:
///     plan: Names, zone, and trade floor.
///
/// Returns:
///     Trades with `close_utc >= from_utc`.
///
/// Errors:
///     [`ReadMiss`] when the read failed. Nothing is logged here.
fn from_replica(plan: &ReadPlan) -> Result<Loaded, ReadMiss> {
    Ok(Loaded {
        trades: read_trades(plan)?,
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
///         passed to `decide`.
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
    match save_applies(&store, &applies, &loaded, now_utc) {
        Ok((skipped, charts)) => {
            log_skips(&skipped);
            let decided = Instant::now();
            let state = host.state_mut();
            state
                .chart_queue
                .extend(charts.into_iter().map(|(chat, trade)| Due {
                    chat,
                    trade,
                    decided,
                }));
            state.last_report_revision = revision;
            clear_busy(host);
        }
        Err(SaveMiss) => {
            log::warn!("telegram notify save failed; the ledger was not advanced");
            reject_finish(host);
        }
    }
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
    let layout = Arc::new(host.config().telegram.bot.message_layout.card.sanitized());
    let mut applies = Vec::new();
    for shot in shots {
        if !grant_matches(host, shot) {
            log_skip(shot.chat);
            continue;
        }
        applies.push(ChatApply {
            chat: shot.chat,
            revision: shot.revision,
            visible: visible_ids(host, &shot.access),
            layout: layout.clone(),
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
pub(crate) fn visible_ids(host: &dyn TgHost, access: &TelegramReportAccess) -> Vec<u64> {
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
///     loaded: Trades.
///     now_utc: UTC Unix seconds stored on outbox rows.
///
/// Returns:
///     Chats the save skipped because they disappeared or their settings
///     revision moved, and the deal charts the save decided, by chat.
///
/// Errors:
///     [`SaveMiss`] when the atomic write failed. Nothing was advanced.
fn save_applies(
    store: &Mutex<NotifyStore>,
    applies: &[ChatApply],
    loaded: &Loaded,
    now_utc: i64,
) -> Result<(Vec<i64>, Vec<(i64, ClosedTrade)>), SaveMiss> {
    lock_store(store)
        .update(|file| apply_messages(file, applies, loaded, now_utc))
        .map_err(|_| SaveMiss)
}

/// Render each chat and push its messages. A revision mismatch is returned, not logged.
///
/// Args:
///     file: Document clone `update` will save.
///     applies: Chats whose grant matched.
///     loaded: Trades.
///     now_utc: UTC Unix seconds for `decide` and the outbox.
///
/// Returns:
///     Chats skipped because they disappeared or their settings revision moved, and the deal
///     charts decided, by chat.
fn apply_messages(
    file: &mut NotifyFile,
    applies: &[ChatApply],
    loaded: &Loaded,
    now_utc: i64,
) -> (Vec<i64>, Vec<(i64, ClosedTrade)>) {
    let mut skipped = Vec::new();
    let mut charts = Vec::new();
    for apply in applies {
        match messages_for(file, apply, loaded, now_utc) {
            Some((messages, due)) => {
                enqueue_all(file, apply.chat, messages, now_utc);
                charts.extend(due.into_iter().map(|trade| (apply.chat, trade)));
            }
            None => skipped.push(apply.chat),
        }
    }
    (skipped, charts)
}

/// Messages for one chat, or `None` when the chat or its revision no longer matches.
///
/// Args:
///     file: Document being edited.
///     apply: Chat, captured revision, and the cores it may see now.
///     loaded: Trades.
///     now_utc: UTC Unix seconds passed to `decide`.
///
/// Returns:
///     Rendered rows, each with the cores it discloses, and the trades whose deal chart is due.
///     Empty is still a successful apply: the ledger may have changed. `None` means skip this
///     chat.
fn messages_for(
    file: &mut NotifyFile,
    apply: &ChatApply,
    loaded: &Loaded,
    now_utc: i64,
) -> Option<(Vec<Outgoing>, Vec<ClosedTrade>)> {
    let entry = file.chats.get_mut(&apply.chat)?;
    if entry.revision != apply.revision {
        return None;
    }
    let messages = render_chat(entry, apply, loaded, now_utc);
    let charts = decide_charts(
        &entry.settings.charts,
        &mut entry.ledger.charts,
        &apply.visible,
        &loaded.trades,
        now_utc,
    );
    Some((messages, charts))
}

/// Dollar fills for waiting cards, then trade cards.
///
/// `decide` runs even when the trade rule is off, which clears `seen`.
///
/// Args:
///     entry: Chat settings and ledger, edited in place.
///     apply: Captured revision and visible cores.
///     loaded: Trades.
///     now_utc: UTC Unix seconds passed to `decide`.
///
/// Returns:
///     Rows in send order, each with the cores it discloses. May be empty.
fn render_chat(
    entry: &mut ChatNotify,
    apply: &ChatApply,
    loaded: &Loaded,
    now_utc: i64,
) -> Vec<Outgoing> {
    let mut messages = card_fills(entry, apply, loaded, now_utc);
    messages.extend(trade_html(entry, apply, loaded, now_utc));
    messages
}

/// Cards for the trades `decide` announces.
///
/// A card printed before its trade's dollar value was known, outside a USD stablecoin, is
/// recorded in `ledger.cards` when the chat asks for the dollars to follow; the sender then
/// records its message against it.
///
/// Args:
///     entry: Trade rule and ledger. `seen`, `held` and `cards` are updated in place.
///     apply: Visible cores and the captured card layout.
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
) -> Vec<Outgoing> {
    let followup = entry.settings.trades.usd_followup;
    let announced = decide(
        &entry.settings.trades,
        &mut entry.ledger,
        &apply.visible,
        &loaded.trades,
        now_utc,
    );
    announced
        .into_iter()
        .map(|Announced { trade, unchecked }| {
            let waits = followup && trade.profit_usd.is_none() && shows_dollars(&trade);
            let key = waits.then(|| trade.key());
            if let Some(key) = key {
                entry.ledger.cards.entry(key.core).or_default().insert(
                    key.rec_id,
                    CardWait {
                        queued_utc: now_utc,
                        message: None,
                        unchecked,
                    },
                );
            }
            Outgoing::Card(
                trade_card(&trade, unchecked, &apply.layout),
                Some(vec![trade.core]),
                key,
            )
        })
        .collect()
}

/// New text for every waiting card whose trade now has its dollar value.
///
/// A card is filled once Telegram has accepted it and the read finds its trade valued; a card
/// whose trade left the chat's visible cores is dropped unfilled. A card older than
/// [`CARD_WAIT_SECS`] stops waiting.
///
/// Args:
///     entry: Ledger; `cards` loses every filled or expired card.
///     apply: Cores the chat may see now and the captured card layout.
///     loaded: Closed trades from the read.
///     now_utc: UTC Unix seconds.
///
/// Returns:
///     One edit per filled card.
fn card_fills(
    entry: &mut ChatNotify,
    apply: &ChatApply,
    loaded: &Loaded,
    now_utc: i64,
) -> Vec<Outgoing> {
    entry
        .ledger
        .prune_cards(now_utc.saturating_sub(CARD_WAIT_SECS));
    if entry.ledger.cards.is_empty() {
        return Vec::new();
    }
    let mut edits = Vec::new();
    for trade in &loaded.trades {
        let key = trade.key();
        let Some(card) = entry
            .ledger
            .cards
            .get(&key.core)
            .and_then(|rows| rows.get(&key.rec_id))
            .copied()
        else {
            continue;
        };
        if !apply.visible.contains(&key.core) {
            entry.ledger.drop_card(key);
            continue;
        }
        let (Some(message), Some(_)) = (card.message, trade.profit_usd) else {
            continue;
        };
        entry.ledger.drop_card(key);
        edits.push(Outgoing::Edit(
            message,
            trade_card(trade, card.unchecked, &apply.layout),
            Some(vec![key.core]),
        ));
    }
    edits
}

/// Append every row. An empty list writes nothing.
///
/// A card the outbox refuses is not waited for: no message will ever be recorded against it.
///
/// Args:
///     file: Document to append to. `next_id` advances once per accepted message.
///     chat: Destination chat.
///     messages: Rendered rows and the cores each one discloses, in send order.
///     now_utc: UTC Unix seconds stored on each row.
fn enqueue_all(file: &mut NotifyFile, chat: i64, messages: Vec<Outgoing>, now_utc: i64) {
    for message in messages {
        match message {
            Outgoing::Plain(html, cores) => {
                push_outbox(file, chat, html, cores, now_utc);
            }
            Outgoing::Card(html, cores, key) => {
                if !push_trade_card(file, chat, html, cores, key, now_utc)
                    && let Some(key) = key
                    && let Some(entry) = file.chats.get_mut(&chat)
                {
                    entry.ledger.drop_card(key);
                }
            }
            Outgoing::Edit(message, html, cores) => {
                push_edit(file, chat, message, html, cores, now_utc);
            }
        }
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
