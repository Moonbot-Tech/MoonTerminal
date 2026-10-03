//! The cores' own Telegram reports: detects and trade entries their strategies mark for Telegram
//! (`ReportToTelegram`, `ReportTradesToTelegram`), relayed to the chats that switched them on.
//!
//! The feed produces only flagged events and the session keeps the last few per core
//! (`CoreData::tg_events`). At most every [`BATCH`] this reads what arrived since its cursor and
//! queues ONE message per chat: a detect storm becomes a list, Telegram's per-chat pace holds,
//! and the outbox file is rewritten once per batch, not once per detect.
//!
//! A relay of what happens now, not a history: the cursor starts at the newest event the first
//! time a core is seen, and an event older than [`MAX_AGE_MS`] when the batch goes is dropped —
//! after a restart, or after every switch in the file was off a while, nothing old floods in.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use moon_core::db::CoreNames;
use moon_core::feed::CoreTgEvent;
use moon_core::session::TgEventRow;
use moon_core::telegram::notify::EventRule;
use moon_core::telegram::reply::utf16_len;
use moon_core::telegram::runtime::{NotifyStore, push_outbox};
use rust_i18n::t;

use crate::TgHost;
use crate::html::escape;

/// Shortest gap between two batches.
const BATCH: Duration = Duration::from_secs(10);

/// Oldest event a batch still relays: the session's own window for an entry
/// (`tg_open_is_fresh`, 120 s) plus a batch and a second of rounding, so an entry it filed at the
/// edge is not dropped here.
const MAX_AGE_MS: i64 = 135_000;

/// A chat whose oldest queued row waits longer than this is not given more: Telegram is not
/// taking its messages, or is far behind, and a backlog of "now" would arrive hours late. Well
/// past what the ordinary pace (3 s a message in a group) builds up.
const STUCK_SECS: i64 = 300;

/// UTF-16 budget of one message's lines, under Telegram's 4096 with room for the tail line.
const MESSAGE_BUDGET: usize = 3_600;

/// Longest detect line kept on a message, in Unicode scalars.
const MSG_CHARS: usize = 120;

/// Longest coin, core or strategy name kept, in Unicode scalars.
const NAME_CHARS: usize = 48;

/// One event to relay, with its core.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Fresh {
    pub core: u64,
    pub row: TgEventRow,
}

/// Queue this batch's messages. Called on every owner tick; returns at once inside [`BATCH`].
///
/// Args:
///     host: Session (the event rings), pairing, grants and the cursors.
///     store: Notifications file.
///     now_utc: Current UTC Unix seconds.
pub(crate) fn run(host: &mut dyn TgHost, store: &Arc<Mutex<NotifyStore>>, now_utc: i64) {
    // Nobody listening: nothing is read, and the cursors go, so a chat that switches on later
    // starts at what happens from then on.
    let listening = super::tick::lock_store(store)
        .file
        .chats
        .values()
        .any(|entry| entry.settings.events.any());
    if !listening {
        host.state_mut().events_cursor.clear();
        return;
    }
    if host
        .state()
        .events_flush
        .is_some_and(|at| at.elapsed() < BATCH)
    {
        return;
    }
    host.state_mut().events_flush = Some(Instant::now());
    let (fresh, missed) = drain(host, now_utc.saturating_mul(1_000));
    if fresh.is_empty() && missed.is_empty() {
        return;
    }
    let names = CoreNames::from_servers(&host.config().servers);
    let targets = targets(host, store);
    if targets.is_empty() {
        return;
    }
    let messages: Vec<(i64, String, Vec<u64>)> = targets
        .into_iter()
        .filter_map(|(chat, rule, visible)| {
            let picked: Vec<&Fresh> = fresh
                .iter()
                .filter(|event| visible.contains(&event.core) && wanted(rule, &event.row.event))
                .collect();
            // Only a detect storm overflows a ring; a chat that hears no detects is not told.
            let lost: u64 = if rule.detects {
                missed
                    .iter()
                    .filter(|(core, _)| visible.contains(core))
                    .map(|(_, count)| *count)
                    .sum()
            } else {
                0
            };
            let (html, cores) = render(&picked, &names, lost)?;
            Some((chat, html, cores))
        })
        .collect();
    if messages.is_empty() {
        return;
    }
    let stuck_before = now_utc.saturating_sub(STUCK_SECS);
    let saved = super::tick::lock_store(store).update(|file| {
        for (chat, html, cores) in messages {
            let stuck = file
                .outbox
                .iter()
                .any(|row| row.chat == chat && row.created_utc < stuck_before);
            if stuck {
                log::debug!("telegram core events for chat {chat} dropped: its outbox is stuck");
                continue;
            }
            push_outbox(file, chat, html, Some(cores), now_utc);
        }
    });
    if let Err(error) = saved {
        log::warn!("telegram core events not queued: {error}");
    }
}

/// Every event each core produced since its cursor, oldest first, younger than [`MAX_AGE_MS`],
/// and per core how many fell off its ring before this read; the cursors move to each core's
/// newest. A core seen for the first time, or whose numbering went back (its store was made
/// anew), starts at its newest and gives nothing.
///
/// Args:
///     host: Session and cursors.
///     now_ms: Current UTC Unix milliseconds.
fn drain(host: &mut dyn TgHost, now_ms: i64) -> (Vec<Fresh>, HashMap<u64, u64>) {
    let mut fresh = Vec::new();
    let mut missed: HashMap<u64, u64> = HashMap::new();
    let mut cursors: HashMap<u64, u64> = HashMap::new();
    {
        let session = host.session();
        let store = session.store();
        for core in session.sessions().iter().map(|row| row.id) {
            let Some(data) = store.core(core) else {
                continue;
            };
            let newest = data.tg_events_seq;
            cursors.insert(core, newest);
            let Some(&cursor) = host.state().events_cursor.get(&core) else {
                continue;
            };
            if newest < cursor {
                continue;
            }
            // The ring keeps the newest; rows numbered between the cursor and its oldest are gone.
            let oldest = data.tg_events.front().map_or(newest + 1, |row| row.seq);
            let lost = oldest.saturating_sub(cursor.saturating_add(1));
            if lost > 0 {
                missed.insert(core, lost);
            }
            fresh.extend(
                data.tg_events
                    .iter()
                    .filter(|row| {
                        row.seq > cursor && now_ms.saturating_sub(row.at_utc_ms) <= MAX_AGE_MS
                    })
                    .map(|row| Fresh {
                        core,
                        row: row.clone(),
                    }),
            );
        }
    }
    host.state_mut().events_cursor = cursors;
    fresh.sort_by_key(|event| event.row.at_utc_ms);
    (fresh, missed)
}

/// Paired, admitted chats with an event switch on, with what they may see now.
fn targets(host: &dyn TgHost, store: &Mutex<NotifyStore>) -> Vec<(i64, EventRule, Vec<u64>)> {
    let telegram = &host.config().telegram;
    let wanting: Vec<(i64, EventRule)> = super::tick::lock_store(store)
        .file
        .chats
        .iter()
        .filter(|(chat, entry)| {
            entry.settings.events.any() && telegram.authorized_chat_ids.contains(chat)
        })
        .map(|(chat, entry)| (*chat, entry.settings.events))
        .collect();
    wanting
        .into_iter()
        .filter_map(|(chat, rule)| {
            let access = telegram.report_access(chat)?;
            Some((chat, rule, super::tick::visible_ids(host, &access)))
        })
        .collect()
}

/// Whether `rule` relays `event`.
fn wanted(rule: EventRule, event: &CoreTgEvent) -> bool {
    match event {
        CoreTgEvent::Detect { .. } => rule.detects,
        CoreTgEvent::Opened { .. } => rule.opened,
    }
}

/// One message for `events`, one line each, until the budget is spent; the rest, and `lost`
/// events that fell off a ring before they could be read, are counted on a last line. `None`
/// when there is nothing to tell.
///
/// Args:
///     events: What one chat is told, oldest first.
///     names: Configured core names.
///     lost: Events of the chat's cores the rings dropped unread.
///
/// Returns:
///     The HTML and the cores it discloses — those of the lines it printed.
pub(crate) fn render(
    events: &[&Fresh],
    names: &CoreNames,
    lost: u64,
) -> Option<(String, Vec<u64>)> {
    if events.is_empty() && lost == 0 {
        return None;
    }
    let mut lines: Vec<String> = Vec::new();
    let mut used = 0usize;
    let mut cores: Vec<u64> = Vec::new();
    let mut rest = 0usize;
    for (shown, event) in events.iter().enumerate() {
        let line = line(event, names);
        let cost = utf16_len(&line) + 1;
        if shown > 0 && used + cost > MESSAGE_BUDGET {
            rest = events.len() - shown;
            break;
        }
        used += cost;
        lines.push(line);
        if !cores.contains(&event.core) {
            cores.push(event.core);
        }
    }
    let more = (rest as u64).saturating_add(lost);
    if more > 0 {
        lines.push(escape(&t!("telegram.notify_events_more", n = more)));
    }
    Some((lines.join("\n"), cores))
}

/// One event's line: mark, coin, core, strategy, and the detect's own text or the entry word.
fn line(event: &Fresh, names: &CoreNames) -> String {
    let fallback = format!("core {}", event.core);
    let core = cut(names.resolve(event.core, &fallback), NAME_CHARS);
    match &event.row.event {
        CoreTgEvent::Detect {
            market,
            msg,
            strat_name,
            ..
        } => {
            let mut text = format!(
                "\u{1f514} <b>{}</b> \u{00b7} {} \u{00b7} <i>{}</i>",
                cut(market, NAME_CHARS),
                core,
                cut(strat_name, NAME_CHARS)
            );
            let msg = msg.trim();
            if !msg.is_empty() {
                text.push_str(" \u{2014} ");
                text.push_str(&cut(msg, MSG_CHARS));
            }
            text
        }
        CoreTgEvent::Opened {
            coin,
            strat_name,
            emulator,
            ..
        } => {
            let mut word = t!("telegram.notify_opened").to_string();
            if *emulator {
                word = format!("{word} ({})", t!("telegram.notify_emulator"));
            }
            format!(
                "\u{1f7e6} <b>{}</b> \u{00b7} {} \u{00b7} <i>{}</i> \u{2014} {}",
                cut(coin, NAME_CHARS),
                core,
                cut(strat_name, NAME_CHARS),
                escape(&word)
            )
        }
    }
}

/// First `chars` Unicode scalars, then HTML-escaped.
fn cut(value: &str, chars: usize) -> String {
    escape(&value.chars().take(chars).collect::<String>())
}

#[cfg(test)]
mod tests;
