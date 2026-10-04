//! Notification sender and the durable outbox it drains.
//!
//! The thread owns its own Bot API client, so a Telegram rate-limit wait never stalls command
//! polling. A message stays in the on-disk outbox until Telegram accepts it, or until the sender
//! drops it for a reason that will not change on retry. A timeout or a transport error is retried:
//! a rare duplicate is preferred to a lost notification. After `send_html` returns `Ok`, this
//! process does not send that id again; only the ack save is retried. A restart can still replay
//! a row whose ack never landed.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, Weak};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crate::config::Secret;
use crate::telegram::api::{BotApi, is_permanent_bad_request, is_unreachable_chat};
use crate::telegram::notify::{AutoReport, AutoRow, CardKey, NotifyFile, Pending};
use crate::telegram::reply::{TELEGRAM_MESSAGE_UTF16_LIMIT, utf16_len};

/// Minimum gap between two successful sends to one private chat.
const CHAT_GAP_PRIVATE: Duration = Duration::from_secs(1);
/// Minimum gap between two successful sends to a group or channel (a negative chat id).
const CHAT_GAP_GROUP: Duration = Duration::from_secs(3);
/// First extra wait after a retryable failure, on top of the wait `BotApi::post` already honored.
const CHAT_BACKOFF_START: Duration = Duration::from_secs(5);
/// Longest extra wait after repeated retryable failures for one chat.
const CHAT_BACKOFF_CAP: Duration = Duration::from_secs(600);
/// Successful sends recorded inside one rolling second, across every chat.
const GLOBAL_PER_SECOND: usize = 25;
/// Width of the global send window.
const GLOBAL_WINDOW: Duration = Duration::from_secs(1);
/// Idle and pace waits. Shutdown is observed at least this often.
const WAIT_SLICE: Duration = Duration::from_millis(500);
/// How long a queued redraw of a menu screen stays worth sending, in seconds.
const REDRAW_TTL_SECS: u64 = 60;

/// Whether `row` is a redraw queued longer ago than [`REDRAW_TTL_SECS`].
fn redraw_outlived(row: &Pending, now_utc: u64) -> bool {
    let queued = u64::try_from(row.created_utc).unwrap_or(0);
    row.redraw.is_some() && now_utc.saturating_sub(queued) > REDRAW_TTL_SECS
}

/// In-memory copy of one notifications file. Every enqueue, ack, and chat edit is saved before
/// it returns.
///
/// `file` is public so a host can edit paired chats on this same document. A chat edit is
/// persisted by [`Self::update`] before that call returns. Enqueue and ack use the same
/// clone-save-commit order. `allowed` is process memory only: [`NotifyFile::save`] does not
/// write it.
pub struct NotifyStore {
    /// On-disk document. Missing until the first successful save.
    pub path: PathBuf,
    /// Settings, ledger and outbox. Rows retain FIFO order within each chat.
    pub file: NotifyFile,
    /// Who may receive a queued row. `None` until the owner thread publishes.
    ///
    /// A chat mapped to `None` is the owner and may see any core. A chat mapped to
    /// `Some(set)` is a viewer limited to that set. A chat absent from a published
    /// map is unpaired. Not part of the on-disk document.
    pub allowed: Option<BTreeMap<i64, Option<BTreeSet<u64>>>>,
}

impl NotifyStore {
    /// Load `path`. A missing file is an empty document and is not created.
    ///
    /// Args:
    ///     path: Notifications file. The atomic writer creates its parent on the first save.
    ///
    /// Returns:
    ///     A store whose outbox is the file order.
    ///
    /// Errors:
    ///     The path is unreadable, or the body is not a [`NotifyFile`]. The file is left as it was.
    pub fn open(path: PathBuf) -> anyhow::Result<Self> {
        let file = NotifyFile::load(&path)?;
        Ok(Self {
            path,
            file,
            allowed: None,
        })
    }

    /// Replace the in-memory allow map when `next` differs from the published one.
    ///
    /// The first call publishes. A later call that builds the same map does not
    /// assign. This does not write the file.
    ///
    /// Args:
    ///     next: Chat id to that chat's grant. `None` is the owner (any core).
    ///         `Some(set)` is a viewer's visible cores.
    pub fn publish_allowed(&mut self, next: BTreeMap<i64, Option<BTreeSet<u64>>>) {
        if self.allowed.as_ref() == Some(&next) {
            return;
        }
        self.allowed = Some(next);
    }

    /// Append one HTML notification and save it before returning.
    ///
    /// A body longer than 4096 UTF-16 units is refused: this method logs the chat id and the
    /// length, writes nothing, and returns `Ok(())`. Renderers already bound every card. Cutting
    /// the body here would split a tag. `chat` does not have to be paired yet; the sender drops
    /// an unpaired chat when it reaches the row.
    ///
    /// Args:
    ///     chat: Destination chat id.
    ///     html: Telegram HTML. Taken by value because the store keeps the accepted copy.
    ///     cores: Cores the message discloses. `None` is a legacy row. `Some([])`
    ///         discloses no core.
    ///     now_utc: Unix seconds stored on the row. This method does not read the clock.
    ///
    /// Returns:
    ///     `Ok(())` after the atomic save, and also when the body was refused and nothing was
    ///     written. A save error leaves the in-memory outbox unchanged.
    ///
    /// Errors:
    ///     Serialization or the atomic write failed.
    pub fn enqueue(
        &mut self,
        chat: i64,
        html: String,
        cores: Option<Vec<u64>>,
        now_utc: i64,
    ) -> anyhow::Result<()> {
        let mut next = self.file.clone();
        if !push_outbox(&mut next, chat, html, cores, now_utc) {
            return Ok(());
        }
        next.save(&self.path)?;
        self.file = next;
        Ok(())
    }

    /// Remove `id` and save. A missing id is success and does not rewrite the file.
    ///
    /// An automatic report Telegram accepted, of a kind that replaces its previous one, is
    /// recorded as its kind's message in the chat's ledger in the same save, even when the row is
    /// already gone, and the message it replaces comes back for deletion. A trade card the chat
    /// waits to fill in with dollars gets its message recorded the same way.
    ///
    /// Args:
    ///     id: [`Pending::id`] Telegram has accepted, or that the sender is dropping.
    ///     chat: Chat that owns the row.
    ///     replaced: The accepted automatic report's kind and message id, if it is one.
    ///     card: The accepted trade card's trade and message id, if it is one.
    ///
    /// Returns:
    ///     The chat's previous message of that kind, to delete.
    ///
    /// Errors:
    ///     The save failed. The in-memory outbox still contains `id`.
    fn ack(
        &mut self,
        id: u64,
        chat: i64,
        replaced: Option<(AutoReport, i64)>,
        card: Option<(CardKey, i64)>,
    ) -> anyhow::Result<Option<i64>> {
        let replaced = replaced.filter(|(kind, _)| kind.replaces_previous());
        if replaced.is_none() && card.is_none() && !self.file.outbox.iter().any(|row| row.id == id)
        {
            return Ok(None);
        }
        let mut next = self.file.clone();
        next.outbox.retain(|row| row.id != id);
        // A card the chat stopped waiting for meanwhile has no entry and is not recorded.
        if let Some((key, message)) = card {
            let wait = next
                .chats
                .get_mut(&chat)
                .and_then(|entry| entry.ledger.cards.get_mut(&key.core))
                .and_then(|rows| rows.get_mut(&key.rec_id));
            if let Some(wait) = wait {
                wait.message = Some(message);
            }
        }
        // A report switched off meanwhile is not recorded: a later run must not delete it.
        let previous = replaced.and_then(|(kind, message)| {
            let entry = next.chats.get_mut(&chat)?;
            if !entry.settings.reports.on(kind) {
                return None;
            }
            let slot = entry.ledger.reports.slot_mut(kind);
            slot.message.replace(message).filter(|old| *old != message)
        });
        next.save(&self.path)?;
        self.file = next;
        Ok(previous)
    }

    /// Apply `edit` to a clone and save it before the in-memory file changes.
    ///
    /// Args:
    ///     edit: Mutation of the next document. Its return value is this method's return value.
    ///
    /// Returns:
    ///     Whatever `edit` returned, after the atomic save.
    ///
    /// Errors:
    ///     Serialization or the atomic write failed. The in-memory file is then unchanged.
    pub fn update<R>(&mut self, edit: impl FnOnce(&mut NotifyFile) -> R) -> anyhow::Result<R> {
        let mut next = self.file.clone();
        let result = edit(&mut next);
        next.save(&self.path)?;
        self.file = next;
        Ok(result)
    }

    /// Drop chats that are no longer paired. Outbox rows stay so the sender can ack them.
    ///
    /// Args:
    ///     paired: Chat ids that remain authorized. A chat absent from this slice is removed.
    ///
    /// Returns:
    ///     How many chat entries were removed. Zero means the file was not rewritten.
    ///
    /// Errors:
    ///     The save failed. The in-memory chats are then unchanged.
    pub fn forget_unpaired(&mut self, paired: &[i64]) -> anyhow::Result<usize> {
        if self.file.chats.keys().all(|chat| paired.contains(chat)) {
            return Ok(0);
        }
        self.update(|file| {
            let before = file.chats.len();
            file.chats.retain(|chat, _| paired.contains(chat));
            before.saturating_sub(file.chats.len())
        })
    }
}

/// Append one HTML notification without saving.
///
/// A body longer than [`TELEGRAM_MESSAGE_UTF16_LIMIT`] UTF-16 units is not appended. This
/// function logs the chat id and the length at error level and leaves `file` unchanged, including
/// `next_id`. Renderers already bound every card. Cutting the body here would split a tag. A
/// caller that also edits the ledger saves the document itself, so an accepted row and the ledger
/// move in one write.
///
/// Args:
///     file: Document to append to. `next_id` advances by one only when the row is accepted.
///     chat: Destination chat id.
///     html: Telegram HTML. Taken by value because the file keeps the accepted copy.
///     cores: Cores the message discloses. `None` is a legacy row. `Some([])`
///         discloses no core.
///     now_utc: Unix seconds stored on the row. This function does not read the clock.
///
/// Returns:
///     `true` when the row was appended. `false` when the body was refused.
pub fn push_outbox(
    file: &mut NotifyFile,
    chat: i64,
    html: String,
    cores: Option<Vec<u64>>,
    now_utc: i64,
) -> bool {
    push_row(file, chat, html, cores, None, now_utc)
}

/// [`push_outbox`] for a trade card, naming the trade when the chat waits to fill its dollar
/// value in: the sender records the accepted message against it.
///
/// Args:
///     file: Document to append to.
///     chat: Destination chat id.
///     html: The card.
///     cores: Cores the card discloses.
///     card: The trade, when [`crate::telegram::notify::NotifyLedger::cards`] waits for it.
///     now_utc: Unix seconds stored on the row.
///
/// Returns:
///     `true` when the row was appended. `false` when the body was refused.
pub fn push_trade_card(
    file: &mut NotifyFile,
    chat: i64,
    html: String,
    cores: Option<Vec<u64>>,
    card: Option<CardKey>,
    now_utc: i64,
) -> bool {
    let pushed = push_row(file, chat, html, cores, None, now_utc);
    if let Some(row) = file.outbox.last_mut().filter(|_| pushed) {
        row.card = card;
    }
    pushed
}

/// Queue an edit of a message already in the chat: the sender replaces its text with `html`.
///
/// Args:
///     file: Document to append to.
///     chat: Chat the message is in.
///     message: The message to edit.
///     html: Its new text, held to the plain-message cap.
///     cores: Cores the new text discloses.
///     now_utc: Unix seconds stored on the row.
///
/// Returns:
///     `true` when the row was appended. `false` when the body was refused.
pub fn push_edit(
    file: &mut NotifyFile,
    chat: i64,
    message: i64,
    html: String,
    cores: Option<Vec<u64>>,
    now_utc: i64,
) -> bool {
    let pushed = push_row(file, chat, html, cores, None, now_utc);
    if let Some(row) = file.outbox.last_mut().filter(|_| pushed) {
        row.edit = Some(message);
    }
    pushed
}

/// Queue a redraw of a screen the bot already shows: the message is replaced with `html` and its
/// buttons with `keyboard`. A redraw of the same message still queued is dropped first — only the
/// newest picture of a screen is worth sending. The row names no core: a screen is the owner's.
///
/// Args:
///     file: Document to append to.
///     chat: Chat the message is in.
///     message: The message to redraw.
///     html: The screen's rich-message HTML.
///     keyboard: The screen's buttons.
///     now_utc: Unix seconds stored on the row.
///
/// Returns:
///     `true` when the row was appended. `false` when the body was refused.
pub fn push_redraw(
    file: &mut NotifyFile,
    chat: i64,
    message: i64,
    html: String,
    keyboard: crate::telegram::api::ReplyMarkup,
    now_utc: i64,
) -> bool {
    drop_redraws(file, chat, message);
    let pushed = push_row(file, chat, html, None, None, now_utc);
    if let Some(row) = file.outbox.last_mut().filter(|_| pushed) {
        row.edit = Some(message);
        row.redraw = Some(keyboard);
    }
    pushed
}

/// Drop the redraws of `message` still queued: the screen it shows was just answered by a press,
/// and an older picture sent after that answer would put the press's screen back.
///
/// Returns:
///     How many rows were removed. The file is not saved; the caller writes it.
pub fn drop_redraws(file: &mut NotifyFile, chat: i64, message: i64) -> usize {
    let before = file.outbox.len();
    file.outbox
        .retain(|row| !(row.chat == chat && row.edit == Some(message) && row.redraw.is_some()));
    before - file.outbox.len()
}

/// Append one automatic report without saving. A kind that replaces its previous report
/// ([`AutoReport::replaces_previous`]) also drops its report still queued for `chat`: only the
/// newest total is worth sending.
///
/// The 4096-unit cap of [`push_outbox`] does not apply: a rich message has its own, larger
/// limits, which the renderer already checked (`moon_tg::report::rich_message_fits`).
///
/// Args:
///     file: Document to append to.
///     chat: Destination chat id.
///     html: Rich-message HTML of the report.
///     cores: Cores the report discloses.
///     auto: The report's kind and buttons.
///     now_utc: Unix seconds stored on the row.
///
/// Returns:
///     `true`: the row is always appended.
pub fn push_auto_report(
    file: &mut NotifyFile,
    chat: i64,
    html: String,
    cores: Option<Vec<u64>>,
    auto: AutoRow,
    now_utc: i64,
) -> bool {
    let kind = auto.kind;
    if kind.replaces_previous() {
        file.outbox
            .retain(|row| row.chat != chat || row.auto.as_ref().is_none_or(|a| a.kind != kind));
    }
    push_row(file, chat, html, cores, Some(auto), now_utc)
}

/// [`push_outbox`] with an optional automatic-report part; only a plain row is held to the
/// plain-message cap.
fn push_row(
    file: &mut NotifyFile,
    chat: i64,
    html: String,
    cores: Option<Vec<u64>>,
    auto: Option<AutoRow>,
    now_utc: i64,
) -> bool {
    let checked = match auto {
        Some(_) => Ok(html),
        None => accept_notification_html(html),
    };
    let html = match checked {
        Ok(html) => html,
        Err(len) => {
            log::error!(
                "telegram notification for chat {chat} refused: {len} utf-16 units exceeds {TELEGRAM_MESSAGE_UTF16_LIMIT}"
            );
            return false;
        }
    };
    let id = file.next_id;
    file.next_id = file.next_id.saturating_add(1);
    file.outbox.push(Pending {
        id,
        chat,
        html,
        created_utc: now_utc,
        cores,
        auto,
        card: None,
        edit: None,
        redraw: None,
    });
    true
}

/// Drop outbox rows for `chat` that disclose a core the chat can no longer see.
///
/// [`cores_kept`] decides each row. The file is not saved; the caller writes it.
/// `next_id` is left alone.
///
/// Args:
///     file: Document whose outbox is filtered in place.
///     chat: Chat whose rows are considered. Other chats stay.
///     visible: Core ids this chat may still be told about.
///     keep_unknown: Whether a row with `cores: None` stays.
///
/// Returns:
///     How many rows were removed.
pub fn purge_outbox(
    file: &mut NotifyFile,
    chat: i64,
    visible: &BTreeSet<u64>,
    keep_unknown: bool,
) -> usize {
    let before = file.outbox.len();
    file.outbox
        .retain(|row| row.chat != chat || cores_kept(&row.cores, visible, keep_unknown));
    before.saturating_sub(file.outbox.len())
}

/// Whether a row that discloses `cores` may stay for a chat that can see `visible`.
///
/// `Some(ids)` stays when every id is in `visible`, so `Some([])` always stays.
/// `None` stays only when `keep_unknown` is true.
///
/// Args:
///     cores: Disclosure on the row. `None` was stored before cores were recorded.
///     visible: Core ids the chat may still be told about.
///     keep_unknown: Whether a row with unknown cores stays.
///
/// Returns:
///     `true` when the row stays.
pub fn cores_kept(cores: &Option<Vec<u64>>, visible: &BTreeSet<u64>, keep_unknown: bool) -> bool {
    match cores {
        Some(ids) => ids.iter().all(|core| visible.contains(core)),
        None => keep_unknown,
    }
}

/// Drop outbox rows `keep` rejects, and save only when at least one would go.
///
/// The initial scan borrows the current outbox. When a row would go, [`NotifyStore::update`]
/// filters a clone and saves it before replacing the document. `keep` runs again on that
/// clone, so it must not depend on earlier calls.
///
/// Args:
///     store: Notifications file.
///     keep: `true` when the row stays.
///
/// Returns:
///     How many rows the save removed. Zero means the file was not rewritten.
///
/// Errors:
///     The save failed. The in-memory outbox is then unchanged. `allowed` is
///     left as it was either way, because [`NotifyStore::update`] replaces only
///     the document.
pub fn purge_outbox_where(
    store: &mut NotifyStore,
    keep: impl Fn(&Pending) -> bool,
) -> anyhow::Result<usize> {
    if !store.file.outbox.iter().any(|row| !keep(row)) {
        return Ok(0);
    }
    store.update(|file| {
        let before = file.outbox.len();
        file.outbox.retain(|row| keep(row));
        before.saturating_sub(file.outbox.len())
    })
}

/// Accept `html` when it fits the Bot API text limit.
///
/// Args:
///     html: Body about to be stored.
///
/// Returns:
///     The same body when its UTF-16 length is at most [`TELEGRAM_MESSAGE_UTF16_LIMIT`].
///
/// Errors:
///     The UTF-16 length, when the body is longer. The body is not shortened.
fn accept_notification_html(html: String) -> Result<String, usize> {
    let len = utf16_len(&html);
    if len <= TELEGRAM_MESSAGE_UTF16_LIMIT {
        Ok(html)
    } else {
        Err(len)
    }
}

/// What the loaded row is allowed to do on this pass.
#[derive(Debug)]
enum Held {
    /// Acked by a previous pass, or never present.
    Missing,
    /// The allow map has not been published. The row stays and is not acked.
    Waiting,
    /// The chat is absent from the published map, so the row will be dropped.
    Unpaired,
    /// The published map does not allow this row's cores. The row will be dropped.
    Revoked,
    /// The row to send: its HTML, and the automatic-report, card and edit parts it carries.
    Ready(Box<Pending>),
}

/// What Telegram accepted for one row, kept until its ack lands.
#[derive(Clone, Copy, Default)]
struct Accepted {
    /// The automatic report's kind and message id, when the row is one.
    auto: Option<(AutoReport, i64)>,
    /// The trade card's trade and message id, when the chat waits for its dollar value.
    card: Option<(CardKey, i64)>,
}

/// Start the sender. [`super::TelegramService::stop`] joins the handle.
///
/// Args:
///     token: Bot token. The thread builds its own [`BotApi`] and does not share the poller.
///     alive: Service liveness. Dropping it ends the loop and interrupts `BotApi` waits.
///     store: Shared outbox. Enqueue and ack both take this mutex.
///
/// Returns:
///     The `telegram-notify` thread.
///
/// Panics:
///     The operating system refused to spawn the thread.
pub(crate) fn spawn_sender(
    token: Secret,
    alive: Weak<()>,
    store: Arc<Mutex<NotifyStore>>,
) -> JoinHandle<()> {
    std::thread::Builder::new()
        .name("telegram-notify".into())
        .spawn(move || run_sender(token, alive, store))
        .expect("spawn telegram-notify")
}

/// Drain the outbox until `alive` is dropped.
///
/// Entries already in the file are sent in file order. A chat that is inside its gap or backoff
/// is skipped so a later chat can proceed; a later row for that same chat waits behind it.
/// `BotApi::post` honors `retry_after`. A retryable failure doubles that chat's extra wait,
/// from five seconds up to ten minutes, and leaves the row in the outbox. A successful send
/// clears the extra wait. After `send_html` returns `Ok`, the id stays in memory until the ack
/// save lands. A later pass retries that ack, starting the extra wait again at five seconds when
/// the ack save fails, and does not send the body again.
fn run_sender(token: Secret, alive: Weak<()>, store: Arc<Mutex<NotifyStore>>) {
    let mut api = BotApi::new(token);
    api.set_liveness(alive.clone());
    let mut last_sent: HashMap<i64, Instant> = HashMap::new();
    let mut ready_at: HashMap<i64, Instant> = HashMap::new();
    let mut backoff: HashMap<i64, Duration> = HashMap::new();
    let mut sent_at: VecDeque<Instant> = VecDeque::new();
    // Ids Telegram accepted and not yet acked, with what each one records on its ack.
    let mut delivered: HashMap<u64, Accepted> = HashMap::new();
    let mut stale: Vec<(i64, i64)> = Vec::new();

    while alive.upgrade().is_some() {
        let pending = snapshot(&store);
        if pending.is_empty() {
            last_sent.clear();
            ready_at.clear();
            backoff.clear();
            delivered.clear();
            wait_while_alive(&alive, WAIT_SLICE);
            continue;
        }
        let live: HashSet<i64> = pending.iter().map(|row| row.chat).collect();
        last_sent.retain(|chat, _| live.contains(chat));
        ready_at.retain(|chat, _| live.contains(chat));
        backoff.retain(|chat, _| live.contains(chat));
        let live_ids: HashSet<u64> = pending.iter().map(|row| row.id).collect();
        delivered.retain(|id, _| live_ids.contains(id));

        let mut progressed = false;
        let mut hit_global_cap = false;
        let mut blocked: HashSet<i64> = HashSet::new();
        for item in pending {
            if alive.upgrade().is_none() {
                return;
            }
            if blocked.contains(&item.chat) {
                continue;
            }
            let now = Instant::now();
            if chat_waiting(item.chat, now, &last_sent, &ready_at) {
                blocked.insert(item.chat);
                continue;
            }
            prune_window(&mut sent_at, now);
            if sent_at.len() >= GLOBAL_PER_SECOND {
                hit_global_cap = true;
                break;
            }
            let retry_ack = delivered.contains_key(&item.id);
            match hold(&store, item.id, item.chat) {
                Held::Missing => {
                    delivered.remove(&item.id);
                }
                Held::Waiting => {}
                Held::Unpaired => {
                    log::warn!(
                        "telegram notification dropped for chat {}: not paired",
                        item.chat
                    );
                    if !ack_saved(
                        &store,
                        item.id,
                        item.chat,
                        &mut ready_at,
                        &mut backoff,
                        &mut delivered,
                        &mut stale,
                    ) {
                        blocked.insert(item.chat);
                    }
                    progressed = true;
                }
                Held::Revoked => {
                    log::warn!(
                        "telegram notification dropped for chat {}: core no longer visible",
                        item.chat
                    );
                    if !ack_saved(
                        &store,
                        item.id,
                        item.chat,
                        &mut ready_at,
                        &mut backoff,
                        &mut delivered,
                        &mut stale,
                    ) {
                        blocked.insert(item.chat);
                    }
                    progressed = true;
                }
                // Telegram already accepted this id. Retry the ack; do not send again.
                Held::Ready(..) if retry_ack => {
                    if !ack_saved(
                        &store,
                        item.id,
                        item.chat,
                        &mut ready_at,
                        &mut backoff,
                        &mut delivered,
                        &mut stale,
                    ) {
                        blocked.insert(item.chat);
                    }
                    progressed = true;
                }
                // A menu screen's picture outlived by a backoff or a restart would cover whatever
                // the chat shows now with an old state: it goes unsent.
                Held::Ready(row) if redraw_outlived(&row, crate::util::time::now_unix_secs()) => {
                    log::info!(
                        "telegram redraw dropped for chat {}: older than {}s",
                        item.chat,
                        REDRAW_TTL_SECS
                    );
                    if !ack_saved(
                        &store,
                        item.id,
                        item.chat,
                        &mut ready_at,
                        &mut backoff,
                        &mut delivered,
                        &mut stale,
                    ) {
                        blocked.insert(item.chat);
                    }
                    progressed = true;
                }
                Held::Ready(row) => match send(&mut api, item.chat, &row) {
                    Ok(message) => {
                        let sent = Instant::now();
                        last_sent.insert(item.chat, sent);
                        sent_at.push_back(sent);
                        let accepted = Accepted {
                            auto: row
                                .auto
                                .as_ref()
                                .map(|auto| (auto.kind, message.message_id)),
                            card: row.card.map(|key| (key, message.message_id)),
                        };
                        delivered.insert(item.id, accepted);
                        backoff.remove(&item.chat);
                        ready_at.remove(&item.chat);
                        if !ack_saved(
                            &store,
                            item.id,
                            item.chat,
                            &mut ready_at,
                            &mut backoff,
                            &mut delivered,
                            &mut stale,
                        ) {
                            blocked.insert(item.chat);
                        }
                        progressed = true;
                    }
                    Err(error) if alive.upgrade().is_none() => {
                        log::warn!(
                            "telegram notification kept for chat {}: shutdown during send: {error}",
                            item.chat
                        );
                        return;
                    }
                    Err(error)
                        if is_unreachable_chat(&error) || is_permanent_bad_request(&error) =>
                    {
                        log::warn!(
                            "telegram notification dropped for chat {}: {error}",
                            item.chat
                        );
                        if !ack_saved(
                            &store,
                            item.id,
                            item.chat,
                            &mut ready_at,
                            &mut backoff,
                            &mut delivered,
                            &mut stale,
                        ) {
                            blocked.insert(item.chat);
                        }
                        progressed = true;
                    }
                    Err(error) => {
                        log::warn!(
                            "telegram notification deferred for chat {}: {error}",
                            item.chat
                        );
                        let wait = bump_backoff(&mut backoff, item.chat);
                        ready_at.insert(item.chat, Instant::now() + wait);
                        blocked.insert(item.chat);
                    }
                },
            }
        }
        tidy_replaced(&mut api, &mut stale);
        if !progressed || hit_global_cap {
            wait_while_alive(&alive, WAIT_SLICE);
        }
    }
}

/// Copy the outbox in file order. The lock is not held across the network call.
fn snapshot(store: &Mutex<NotifyStore>) -> Vec<Pending> {
    lock_store(store).file.outbox.clone()
}

/// Re-read one row so a concurrent ack is not sent again.
///
/// A missing allow map waits. A chat absent from the published map is unpaired.
/// An owner grant (`None`) may see any disclosure. A viewer grant must contain
/// every disclosed core; an unknown row (`cores: None`) is revoked for a viewer.
/// `Some([])` is allowed for every viewer.
fn hold(store: &Mutex<NotifyStore>, id: u64, chat: i64) -> Held {
    let guard = lock_store(store);
    let Some(row) = guard.file.outbox.iter().find(|row| row.id == id) else {
        return Held::Missing;
    };
    let Some(allowed) = guard.allowed.as_ref() else {
        return Held::Waiting;
    };
    let Some(grant) = allowed.get(&chat) else {
        return Held::Unpaired;
    };
    if grant_allows(&row.cores, grant) {
        Held::Ready(Box::new(row.clone()))
    } else {
        Held::Revoked
    }
}

/// Whether `grant` permits a row that discloses `cores`.
///
/// Args:
///     cores: Disclosure on the row.
///     grant: `None` is the owner. `Some` is the viewer's visible cores.
///
/// Returns:
///     `true` when the sender may deliver the row.
fn grant_allows(cores: &Option<Vec<u64>>, grant: &Option<BTreeSet<u64>>) -> bool {
    match grant {
        None => true,
        Some(visible) => cores_kept(cores, visible, false),
    }
}

/// Save the removal and forget `id` when that save lands.
///
/// A failed save leaves the id in `delivered` and backs the chat off, so a body Telegram already
/// accepted is not sent again on the next pass. The wait comes from [`bump_backoff`].
///
/// Args:
///     store: Shared outbox.
///     id: Row to remove.
///     chat: Chat that owns the row. A failed save backs this chat off.
///     ready_at: Per-chat instant before which the sender skips the chat.
///     backoff: Per-chat extra wait. A failed save stores the next doubled wait here.
///     delivered: Ids Telegram has accepted in this process and not yet acked, with the
///         automatic report each one is.
///     stale: Receives `(chat, message)` of an automatic report the acked one replaced.
///
/// Returns:
///     Whether the row is gone from the store.
fn ack_saved(
    store: &Mutex<NotifyStore>,
    id: u64,
    chat: i64,
    ready_at: &mut HashMap<i64, Instant>,
    backoff: &mut HashMap<i64, Duration>,
    delivered: &mut HashMap<u64, Accepted>,
    stale: &mut Vec<(i64, i64)>,
) -> bool {
    let accepted = delivered.get(&id).copied().unwrap_or_default();
    let Some(previous) = finish_ack(store, id, chat, accepted, ready_at, backoff) else {
        return false;
    };
    delivered.remove(&id);
    stale.extend(previous.map(|message| (chat, message)));
    true
}

/// Save the removal. On failure, back the chat off so a full disk does not spin the thread.
///
/// Args:
///     store: Shared outbox.
///     id: Row to remove.
///     chat: Chat that owns the row.
///     accepted: What Telegram accepted for the row, when it is a report or a waiting card.
///     ready_at: Per-chat instant before which the sender skips the chat.
///     backoff: Per-chat extra wait advanced by [`bump_backoff`] when the save fails.
///
/// Returns:
///     `Some` when the row is gone from the store, carrying the message the report replaced.
fn finish_ack(
    store: &Mutex<NotifyStore>,
    id: u64,
    chat: i64,
    accepted: Accepted,
    ready_at: &mut HashMap<i64, Instant>,
    backoff: &mut HashMap<i64, Duration>,
) -> Option<Option<i64>> {
    match lock_store(store).ack(id, chat, accepted.auto, accepted.card) {
        Ok(previous) => Some(previous),
        Err(error) => {
            log::warn!("telegram notification ack failed for id {id}: {error}");
            let wait = bump_backoff(backoff, chat);
            ready_at.insert(chat, Instant::now() + wait);
            None
        }
    }
}

/// Send one row: an automatic report as a rich message with its buttons, a redraw as an edit of
/// its message with its buttons, an edit as an edit of its message, anything else as HTML.
fn send(
    api: &mut BotApi,
    chat: i64,
    row: &Pending,
) -> Result<crate::telegram::api::Message, crate::telegram::api::ApiError> {
    match (&row.auto, row.edit) {
        (Some(auto), _) => api.rich_message(chat, None, &row.html, &auto.keyboard),
        (None, Some(message)) => match &row.redraw {
            Some(keyboard) => api.rich_message(chat, Some(message), &row.html, keyboard),
            None => api.edit_html(chat, message, &row.html),
        },
        (None, None) => api.send_html(chat, &row.html),
    }
}

/// Delete the messages automatic reports replaced. One attempt each: a message that cannot be
/// deleted stays in the chat, which is not an error.
fn tidy_replaced(api: &mut BotApi, stale: &mut Vec<(i64, i64)>) {
    for (chat, message) in stale.drain(..) {
        if let Err(error) = api.tidy_message(chat, message) {
            log::debug!("telegram auto report {message} in chat {chat} not deleted: {error}");
        }
    }
}

/// Whether this chat must wait for its gap or its failure backoff.
///
/// Args:
///     chat: Destination chat id. A negative id uses the group gap.
///     now: Instant of this pass.
///     last_sent: Instant of the previous successful send, per chat.
///     ready_at: Instant before which a failure backoff still holds, per chat.
///
/// Returns:
///     `true` when the chat must be skipped on this pass.
fn chat_waiting(
    chat: i64,
    now: Instant,
    last_sent: &HashMap<i64, Instant>,
    ready_at: &HashMap<i64, Instant>,
) -> bool {
    ready_at.get(&chat).is_some_and(|at| now < *at)
        || last_sent
            .get(&chat)
            .is_some_and(|at| now.saturating_duration_since(*at) < chat_gap(chat))
}

/// Next extra wait for `chat`, then store the doubled wait capped at [`CHAT_BACKOFF_CAP`].
///
/// The first failure returns [`CHAT_BACKOFF_START`]. A later failure returns the value the
/// previous call stored. A successful send removes the entry, so the next failure starts at
/// five seconds again.
///
/// Args:
///     backoff: Per-chat wait that the next failure will return.
///     chat: Chat that just failed.
///
/// Returns:
///     How long this chat should wait before the next attempt.
fn bump_backoff(backoff: &mut HashMap<i64, Duration>, chat: i64) -> Duration {
    let current = backoff.get(&chat).copied().unwrap_or(CHAT_BACKOFF_START);
    let doubled = current.saturating_mul(2).min(CHAT_BACKOFF_CAP);
    backoff.insert(chat, doubled);
    current
}

/// Minimum gap before the next successful send to `chat`.
///
/// Args:
///     chat: Destination chat id.
///
/// Returns:
///     Three seconds for a negative id (a group or channel), otherwise one second.
fn chat_gap(chat: i64) -> Duration {
    if chat < 0 {
        CHAT_GAP_GROUP
    } else {
        CHAT_GAP_PRIVATE
    }
}

/// Drop send timestamps that have left the rolling window.
fn prune_window(sent_at: &mut VecDeque<Instant>, now: Instant) {
    while sent_at
        .front()
        .is_some_and(|at| now.saturating_duration_since(*at) >= GLOBAL_WINDOW)
    {
        sent_at.pop_front();
    }
}

/// Sleep up to `total`, returning as soon as the service owner is dropped.
fn wait_while_alive(alive: &Weak<()>, total: Duration) {
    let start = Instant::now();
    while start.elapsed() < total {
        if alive.upgrade().is_none() {
            return;
        }
        let remaining = total.saturating_sub(start.elapsed());
        std::thread::sleep(remaining.min(Duration::from_millis(100)));
    }
}

/// Recover a poisoned outbox lock. The sender must keep draining after a caller panicked.
fn lock_store(store: &Mutex<NotifyStore>) -> std::sync::MutexGuard<'_, NotifyStore> {
    store
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[cfg(test)]
mod tests;
