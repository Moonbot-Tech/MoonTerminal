//! Automatic reports: at its slot, the period that just ended as the same rich report a button
//! opens, queued in the chat's outbox.
//!
//! What is due is decided against each chat's ledger: a report is due when its latest slot
//! ([`auto_window`]) is after the slot it last recorded. After a restart that is the latest slot
//! only, so a missed run sends one report per kind, never a batch. A report is recorded when it is
//! queued; a read that failed is not, and the next read retries it while its slot is the latest.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::t;
use chrono::Offset;
use chrono_tz::Tz;
use moon_core::config::telegram_access::TelegramReportAccess;
use moon_core::db::{self, CoreNames};
use moon_core::session::core_order::CoreOrder;
use moon_core::telegram::notify::{AutoReport, AutoRow, NotifyFile};
use moon_core::telegram::report::{AutoWindow, auto_window};
use moon_core::telegram::runtime::{NotifyStore, push_auto_report};
use moon_core::util::display_time;

use crate::notify::tick::{current_store, lock_store};
use crate::report::{AutoCaption, AutoInputs, AutoPage, read_auto_report};
use crate::{Finish, Job, TgHost};

/// Least time between two automatic-report reads: a read that failed is retried after it.
const AUTO_INTERVAL: Duration = Duration::from_secs(60);

/// One report due for one chat.
#[derive(Clone, Debug)]
struct Due {
    chat: i64,
    kind: AutoReport,
    window: AutoWindow,
    /// The chat's grant when the read was spawned; finish compares it again.
    access: TelegramReportAccess,
}

/// What a test hands the read instead of the report database: one page for every due report.
#[derive(Clone)]
pub(crate) struct InjectedAuto {
    pub(crate) html: String,
    pub(crate) cores: Option<Vec<u64>>,
}

/// A due report's read: the page, `None` when there is nothing to report to the chat, or `Err`
/// when the database could not be read.
type Read = Result<Option<AutoPage>, ()>;

/// Spawn one read of every automatic report that is due, unless one is in flight or the last
/// started less than [`AUTO_INTERVAL`] ago.
///
/// Args:
///     host: Pairing, grants, the bot's view and basis, the report zone.
///     store: Notifications file of the running bot.
///     now_utc: Current UTC Unix seconds.
pub(crate) fn run(host: &mut dyn TgHost, store: &Arc<Mutex<NotifyStore>>, now_utc: i64) {
    let state = host.state();
    if state.auto_busy
        || state
            .last_auto_run
            .is_some_and(|at| at.elapsed() < AUTO_INTERVAL)
    {
        return;
    }
    let due = due_reports(host, &lock_store(store).file, now_utc);
    if due.is_empty() {
        return;
    }
    let injected = host.state().injected_auto.clone();
    let bot = &host.config().telegram.bot;
    let inputs = AutoInputs {
        zone: host.report_zone(),
        basis: bot.period_basis,
        view: bot.report_view,
        order: CoreOrder::new(host.config()),
        names: CoreNames::from_servers(&host.config().servers),
        // Injected pages read no database, so a test host needs no session.
        venues: match injected {
            Some(_) => Default::default(),
            None => host.session().core_venues().clone(),
        },
        groups: host.config().core_groups.clone(),
    };
    let store_ptr = Arc::as_ptr(store).addr();
    let state = host.state_mut();
    state.auto_busy = true;
    state.last_auto_run = Some(Instant::now());
    let job: Job = Box::new(move || {
        let reads = read_all(&due, &inputs, injected.as_ref());
        let finish: Finish = Box::new(move |host| finish(host, store_ptr, due, reads, now_utc));
        finish
    });
    host.spawn(job);
}

/// Every automatic report a paired chat has on whose latest slot it has not recorded.
///
/// Args:
///     host: Pairing, grants and the report zone.
///     file: Notifications document, read under the caller's lock.
///     now_utc: Current UTC Unix seconds.
fn due_reports(host: &dyn TgHost, file: &NotifyFile, now_utc: i64) -> Vec<Due> {
    let telegram = &host.config().telegram;
    let zone = host.report_zone();
    let mut due = Vec::new();
    for (&chat, entry) in &file.chats {
        if !telegram.authorized_chat_ids.contains(&chat) || !entry.settings.reports.any() {
            continue;
        }
        let Some(access) = telegram.report_access(chat) else {
            continue;
        };
        for kind in AutoReport::ALL {
            if !entry.settings.reports.on(kind) {
                continue;
            }
            let Some(window) = auto_window(kind, now_utc, zone) else {
                continue;
            };
            let recorded = entry.ledger.reports.slot(kind).slot_utc;
            if recorded.is_none_or(|slot| slot < window.at) {
                due.push(Due {
                    chat,
                    kind,
                    window,
                    access: access.clone(),
                });
            }
        }
    }
    due
}

/// Read every due report from one database connection, off the owner thread.
fn read_all(due: &[Due], inputs: &AutoInputs, injected: Option<&InjectedAuto>) -> Vec<Read> {
    if let Some(injected) = injected {
        return due
            .iter()
            .map(|_| {
                Ok(Some(AutoPage {
                    html: injected.html.clone(),
                    keyboard: moon_core::telegram::api::ReplyMarkup::Inline(
                        moon_core::telegram::api::InlineKeyboardMarkup {
                            inline_keyboard: Vec::new(),
                        },
                    ),
                    cores: injected.cores.clone(),
                }))
            })
            .collect();
    }
    let conn = match db::open_reader() {
        Ok(conn) => conn,
        Err(error) => {
            log::warn!("telegram auto reports not read: {error}");
            return due.iter().map(|_| Err(())).collect();
        }
    };
    due.iter()
        .map(|item| {
            let caption = caption(item.kind, &item.window, inputs.zone);
            read_auto_report(&conn, &item.window, caption, inputs, &item.access).map_err(|error| {
                log::warn!(
                    "telegram auto report {:?} for chat {} not read: {error}",
                    item.kind,
                    item.chat
                );
            })
        })
        .collect()
}

/// What heads an automatic report: what it is, in the reply keyboard's short words where they
/// fit, and the zone its period is in. The table marks a period read by open time itself.
fn caption(kind: AutoReport, window: &AutoWindow, zone: Tz) -> AutoCaption {
    let title = match kind {
        AutoReport::Hourly => t!("telegram.auto.hourly"),
        AutoReport::Today => t!("telegram.button_today"),
        AutoReport::Month => t!("telegram.button_month"),
    };
    AutoCaption {
        title: format!("\u{1f4ca} {title}"),
        zone: offset_label(window.at, zone),
    }
}

/// `UTC+3`, `UTC−5`, `UTC+5:45`: the zone's offset at `at`.
fn offset_label(at: i64, zone: Tz) -> String {
    let Some(local) = display_time::at(at, zone) else {
        return zone.to_string();
    };
    let seconds = local.offset().fix().local_minus_utc();
    let sign = if seconds < 0 { '\u{2212}' } else { '+' };
    let (hours, minutes) = (seconds.abs() / 3600, seconds.abs() % 3600 / 60);
    match (hours, minutes) {
        (0, 0) => "UTC".to_string(),
        (hours, 0) => format!("UTC{sign}{hours}"),
        (hours, minutes) => format!("UTC{sign}{hours}:{minutes:02}"),
    }
}

/// Queue what was read, on the owner thread, and always clear the busy flag.
///
/// A report is queued only while its chat is still paired with the same grant, still has the
/// report on, and has not recorded its slot meanwhile; queuing records the slot. A chat with
/// nothing to report records it without a message. A failed read records nothing.
fn finish(host: &mut dyn TgHost, store_ptr: usize, due: Vec<Due>, reads: Vec<Read>, now_utc: i64) {
    host.state_mut().auto_busy = false;
    let Some(store) = current_store(host) else {
        return;
    };
    if Arc::as_ptr(&store).addr() != store_ptr {
        log::debug!("telegram auto reports skipped: notification store was replaced");
        return;
    }
    let current: Vec<(Due, Read)> = due
        .into_iter()
        .zip(reads)
        .filter(|(item, _)| {
            let telegram = &host.config().telegram;
            let same = telegram.authorized_chat_ids.contains(&item.chat)
                && telegram.report_access(item.chat).as_ref() == Some(&item.access);
            if !same {
                log::debug!(
                    "telegram auto report skipped for chat {}: grant changed",
                    item.chat
                );
            }
            same
        })
        .collect();
    if current.iter().all(|(_, read)| read.is_err()) {
        return;
    }
    let saved = lock_store(&store).update(|file| queue_reports(file, current, now_utc));
    if let Err(error) = saved {
        log::warn!("telegram auto reports not queued: {error}");
    }
}

/// Queue each read report into `file` and record its slot. See [`finish`].
fn queue_reports(file: &mut NotifyFile, reads: Vec<(Due, Read)>, now_utc: i64) {
    for (item, read) in reads {
        let Ok(page) = read else {
            continue;
        };
        let Some(entry) = file.chats.get(&item.chat) else {
            continue;
        };
        let recorded = entry.ledger.reports.slot(item.kind).slot_utc;
        if !entry.settings.reports.on(item.kind)
            || recorded.is_some_and(|slot| slot >= item.window.at)
        {
            continue;
        }
        if let Some(page) = page {
            let auto = AutoRow {
                kind: item.kind,
                keyboard: page.keyboard,
            };
            push_auto_report(file, item.chat, page.html, page.cores, auto, now_utc);
        }
        if let Some(entry) = file.chats.get_mut(&item.chat) {
            entry.ledger.reports.slot_mut(item.kind).slot_utc = Some(item.window.at);
        }
    }
}

#[cfg(test)]
mod tests;
