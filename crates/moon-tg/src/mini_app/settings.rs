//! Read and save one chat's notification settings.
//!
//! Validation, the visible-core intersection, and a revision mismatch happen before the store
//! is asked to save. [`NotifyStore::update`] writes on every call, so a refusal must return first.

use std::sync::{Arc, Mutex};

use chrono_tz::Tz;
use moon_core::telegram::notify::{AutoReport, ChatNotify, CoreScope, NotifyFile, NotifySettings};
use moon_core::telegram::runtime::NotifyStore;
use moon_core::telegram::web::MiniAppApiError;
use moon_core::telegram::web::dto::{NotifyCoreDto, NotifyDto};
use moon_core::util::time::now_unix_secs;
use rust_i18n::t;

use crate::TgHost;

/// Why a save was refused before the file was touched.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum SaveFault {
    /// `Only` named no core this chat can see.
    Cores,
    /// [`NotifySettings::validate`] rejected the document.
    Invalid,
    /// The page's revision is not the revision stored for this chat.
    Stale,
}

/// Outcome of [`store_settings`].
#[derive(Debug, PartialEq, Eq)]
pub(super) enum SaveResult {
    /// The file and memory both hold the new settings.
    Saved,
    /// Validation, the core intersection, or a stale revision refused the edit. Nothing was saved.
    Refused(SaveFault),
    /// The atomic save failed. Memory is unchanged.
    Failed(String),
}

/// Keep `Only` ids this chat can see, in the submitted order, then validate.
///
/// `All` is not filtered. Duplicates that are visible stay duplicated. An `Only` that keeps
/// nothing, including one that arrived empty, is [`SaveFault::Cores`].
///
/// Args:
///     settings: Rules the page submitted.
///     visible: Core ids this chat may name.
///
/// Returns:
///     The settings to store, or the first refusal. The file is not touched.
pub(super) fn prepare_settings(
    mut settings: NotifySettings,
    visible: &[u64],
) -> Result<NotifySettings, SaveFault> {
    if let CoreScope::Only(ids) = &mut settings.trades.cores {
        ids.retain(|id| visible.contains(id));
        if ids.is_empty() {
            return Err(SaveFault::Cores);
        }
    }
    if settings.validate().is_err() {
        return Err(SaveFault::Invalid);
    }
    Ok(settings)
}

/// Replace `chat` and apply the ledger edges for a trades, down, or daily switch.
///
/// Enabling trades records `now_utc` and clears `seen` and `held`. Disabling trades clears
/// `trades_enabled_utc` and `held` and leaves `seen`. Cards waiting for their dollar value are
/// forgotten whenever trades or the follow-up end up off. The down set is cleared only on the true-to-false
/// edge. A chat that was not stored starts from the all-off default, so enabling it records
/// the timestamp. Saving again while trades stay on does not move that timestamp.
///
/// `daily_last` is set to today only when daily turns on, or its hour or minute changes while
/// it stays on, and today's target time in `zone` has already passed. An automatic report that
/// turns on records its current slot as done, so its first report comes at the next slot; one
/// that turns off forgets its last message, so a later run never deletes a report of an earlier
/// one.
/// [`crate::notify::daily::due`] is called with no previous send, so `Some` means that target
/// is already due. An ordinary re-save, and a clock that has not passed, leave `daily_last`
/// as it was.
///
/// Args:
///     file: Document the caller will save.
///     chat: Paired chat id.
///     settings: Rules already prepared.
///     now_utc: Unix seconds for a trades-on edge and the daily clock check. This function does
///         not read the clock.
///     zone: Host report zone. The daily clock is interpreted here.
pub(super) fn commit_settings(
    file: &mut NotifyFile,
    chat: i64,
    settings: NotifySettings,
    now_utc: i64,
    zone: Tz,
) {
    let previous = file.chats.get(&chat).cloned().unwrap_or_default();
    let mut ledger = previous.ledger;
    let was_trades = previous.settings.trades.on;
    let was_down = previous.settings.down.on;
    if !was_trades && settings.trades.on {
        ledger.trades_enabled_utc = Some(now_utc);
        ledger.seen.clear();
        ledger.held.clear();
    } else if was_trades && !settings.trades.on {
        ledger.trades_enabled_utc = None;
        ledger.held.clear();
    }
    // Nothing waits to be filled in with dollars once the chat stops asking for it.
    if !(settings.trades.on && settings.trades.usd_followup) {
        ledger.cards.clear();
    }
    if was_down && !settings.down.on {
        ledger.down_announced.clear();
    }
    let was_daily = previous.settings.daily.on;
    let clock_moved = previous.settings.daily.hour != settings.daily.hour
        || previous.settings.daily.minute != settings.daily.minute;
    let opened_or_moved =
        (!was_daily && settings.daily.on) || (was_daily && settings.daily.on && clock_moved);
    if opened_or_moved {
        // `None` means today's target has not passed, so the previous date stays.
        ledger.daily_last =
            crate::notify::daily::due(now_utc, zone, &settings.daily, None).or(ledger.daily_last);
    }
    for kind in AutoReport::ALL {
        let (was, is) = (
            previous.settings.reports.on(kind),
            settings.reports.on(kind),
        );
        let slot = ledger.reports.slot_mut(kind);
        if !was
            && is
            && let Some(window) = moon_core::telegram::report::auto_window(kind, now_utc, zone)
        {
            slot.slot_utc = Some(window.at);
        } else if was && !is {
            slot.message = None;
        }
    }
    let revision = previous.revision.saturating_add(1);
    file.chats.insert(
        chat,
        ChatNotify {
            settings,
            ledger,
            revision,
        },
    );
}

/// Prepare, then save. A refusal does not call [`NotifyStore::update`].
///
/// The revision is compared with the stored row before [`prepare_settings`], so a stale draft
/// that is also invalid is [`SaveFault::Stale`]. An absent chat is revision `0`.
///
/// Args:
///     store: Notifications file for this host.
///     chat: Paired chat id.
///     settings: Rules the page submitted.
///     visible: Core ids this chat may name.
///     now_utc: Unix seconds for a trades-on edge and the daily clock check.
///     zone: Host report zone. The daily clock is interpreted here.
///     revision: Revision the page loaded. It must equal the stored revision.
///
/// Returns:
///     [`SaveResult::Saved`] after the atomic save. [`SaveResult::Refused`] when the edit was
///     rejected first. [`SaveResult::Failed`] when the save failed; memory is then unchanged.
pub(super) fn store_settings(
    store: &mut NotifyStore,
    chat: i64,
    settings: NotifySettings,
    visible: &[u64],
    now_utc: i64,
    zone: Tz,
    revision: u64,
) -> SaveResult {
    let stored = store
        .file
        .chats
        .get(&chat)
        .map(|row| row.revision)
        .unwrap_or(0);
    if revision != stored {
        return SaveResult::Refused(SaveFault::Stale);
    }
    let settings = match prepare_settings(settings, visible) {
        Ok(settings) => settings,
        Err(fault) => return SaveResult::Refused(fault),
    };
    match store.update(|file| commit_settings(file, chat, settings, now_utc, zone)) {
        Ok(()) => SaveResult::Saved,
        Err(error) => SaveResult::Failed(error.to_string()),
    }
}

/// This chat's notification settings as stored, for the bot's own Settings section.
///
/// Returns:
///     The stored settings (the all-off default for a chat with none); `None` when the bot has
///     no notifications store.
pub(crate) fn chat_notify(host: &dyn TgHost, chat_id: i64) -> Option<NotifySettings> {
    let store = host
        .state()
        .service
        .as_ref()
        .and_then(moon_core::telegram::TelegramService::notify_store)?;
    let guard = store
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    Some(
        guard
            .file
            .chats
            .get(&chat_id)
            .map(|row| row.settings.clone())
            .unwrap_or_default(),
    )
}

/// Change this chat's notification settings from the bot's own Settings section, through the
/// same checks and ledger edges as the Mini App's save ([`store_settings`]), against the
/// revision stored right now. The section is the owner's, so every core is visible.
///
/// Returns:
///     The settings as stored after the attempt, or why nothing was saved — in words for the
///     chat.
pub(crate) fn save_chat_notify(
    host: &dyn TgHost,
    chat_id: i64,
    edit: impl FnOnce(&mut NotifySettings),
) -> Result<NotifySettings, String> {
    let store = host
        .state()
        .service
        .as_ref()
        .and_then(moon_core::telegram::TelegramService::notify_store)
        .ok_or_else(|| t!("telegram.mini_settings_err_save").to_string())?;
    let mut visible: Vec<u64> = super::visible_cores(
        host,
        &moon_core::config::telegram_access::TelegramReportAccess::Owner,
    )
    .into_iter()
    .map(|(id, _, _)| id)
    .collect();
    let now = i64::try_from(now_unix_secs()).unwrap_or(i64::MAX);
    let zone = host.report_zone();
    let mut guard = store
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let (mut settings, revision) = match guard.file.chats.get(&chat_id) {
        Some(row) => (row.settings.clone(), row.revision),
        None => (NotifySettings::default(), 0),
    };
    // A switch here never names cores: the stored choice stays whole, a core of it that is not
    // connected now included (the owner's grant is every core).
    if let CoreScope::Only(ids) = &settings.trades.cores {
        visible.extend(ids.iter().copied());
    }
    edit(&mut settings);
    match store_settings(&mut guard, chat_id, settings, &visible, now, zone, revision) {
        SaveResult::Saved => Ok(guard
            .file
            .chats
            .get(&chat_id)
            .map(|row| row.settings.clone())
            .unwrap_or_default()),
        SaveResult::Refused(fault) => Err(save_fault_text(fault)),
        SaveResult::Failed(error) => {
            log::warn!("telegram notification settings not saved for chat {chat_id}: {error}");
            Err(t!("telegram.mini_settings_err_save").to_string())
        }
    }
}

/// Every paired chat's notifications as stored, with their revisions, for the settings window
/// (the station's answer to the terminal included).
///
/// Returns:
///     `None` when the bot runs no notifications store.
pub fn notify_rows(
    state: &crate::TelegramState,
    telegram: &moon_core::config::TelegramConfig,
) -> Option<std::collections::BTreeMap<i64, moon_core::station_api::ChatNotifyRow>> {
    let store = state
        .service
        .as_ref()
        .and_then(moon_core::telegram::TelegramService::notify_store)?;
    let guard = store
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    Some(
        telegram
            .authorized_chat_ids
            .iter()
            .map(|&chat| {
                let row = guard.file.chats.get(&chat);
                (
                    chat,
                    moon_core::station_api::ChatNotifyRow {
                        settings: row.map(|r| r.settings.clone()).unwrap_or_default(),
                        revision: row.map_or(0, |r| r.revision),
                    },
                )
            })
            .collect(),
    )
}

/// The cores a trade rule of `chat` may name: the chat's grant — for the owner every core, so the
/// rule's own list stands; for a viewer the assigned ones.
fn rule_visible(
    telegram: &moon_core::config::TelegramConfig,
    chat: i64,
    settings: &NotifySettings,
) -> Result<Vec<u64>, String> {
    use moon_core::config::telegram_access::TelegramReportAccess;
    match telegram.report_access(chat) {
        None => Err(t!("telegram.refusal").to_string()),
        Some(TelegramReportAccess::Viewer(ids)) => Ok(ids),
        Some(TelegramReportAccess::Owner) => Ok(match &settings.trades.cores {
            CoreScope::Only(ids) => ids.clone(),
            CoreScope::All => Vec::new(),
        }),
    }
}

/// Whether [`save_notify_rows`] would take every row as it is now: each chat paired, its stored
/// revision the one given, its rules valid. Nothing is written — a caller with other changes
/// checks before making them, so a refused row cannot leave half a change behind.
pub fn check_notify_rows(
    state: &crate::TelegramState,
    telegram: &moon_core::config::TelegramConfig,
    rows: &std::collections::BTreeMap<i64, moon_core::station_api::ChatNotifyRow>,
) -> Result<(), String> {
    let store = state
        .service
        .as_ref()
        .and_then(moon_core::telegram::TelegramService::notify_store)
        .ok_or_else(|| t!("telegram.mini_settings_err_save").to_string())?;
    let guard = store
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    check_rows(&guard.file, telegram, rows)
}

/// [`check_notify_rows`] on a notifications document.
pub(super) fn check_rows(
    file: &NotifyFile,
    telegram: &moon_core::config::TelegramConfig,
    rows: &std::collections::BTreeMap<i64, moon_core::station_api::ChatNotifyRow>,
) -> Result<(), String> {
    for (&chat, row) in rows {
        let visible = rule_visible(telegram, chat, &row.settings)?;
        let stored = file.chats.get(&chat).map_or(0, |stored| stored.revision);
        if stored != row.revision {
            return Err(save_fault_text(SaveFault::Stale));
        }
        prepare_settings(row.settings.clone(), &visible).map_err(save_fault_text)?;
    }
    Ok(())
}

/// Save chats' notifications from the settings window, each through the Mini App's own checks
/// and ledger edges ([`store_settings`]) and only while its stored revision is the one given.
///
/// A chat must be paired. The cores a trade rule may name are the chat's grant: every core for
/// the owner, the assigned ones for a viewer.
///
/// Returns:
///     Why a chat was refused or not saved, in words for the settings window; chats before it
///     in id order are saved.
pub fn save_notify_rows(
    state: &crate::TelegramState,
    telegram: &moon_core::config::TelegramConfig,
    rows: &std::collections::BTreeMap<i64, moon_core::station_api::ChatNotifyRow>,
    zone: Tz,
) -> Result<(), String> {
    let store = state
        .service
        .as_ref()
        .and_then(moon_core::telegram::TelegramService::notify_store)
        .ok_or_else(|| t!("telegram.mini_settings_err_save").to_string())?;
    let now = i64::try_from(now_unix_secs()).unwrap_or(i64::MAX);
    let mut guard = store
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    for (&chat, row) in rows {
        let visible = rule_visible(telegram, chat, &row.settings)?;
        match store_settings(
            &mut guard,
            chat,
            row.settings.clone(),
            &visible,
            now,
            zone,
            row.revision,
        ) {
            SaveResult::Saved => {}
            SaveResult::Refused(fault) => return Err(save_fault_text(fault)),
            SaveResult::Failed(error) => {
                log::warn!("telegram notification settings not saved for chat {chat}: {error}");
                return Err(t!("telegram.mini_settings_err_save").to_string());
            }
        }
    }
    Ok(())
}

/// Localized text for a refusal. The HTTP body carries this string, not the key.
///
/// Args:
///     fault: Why the save was refused.
///
/// Returns:
///     The current locale's sentence.
pub(super) fn save_fault_text(fault: SaveFault) -> String {
    match fault {
        SaveFault::Cores => t!("telegram.mini_settings_err_cores").to_string(),
        SaveFault::Invalid => t!("telegram.mini_settings_err_invalid").to_string(),
        SaveFault::Stale => t!("telegram.mini_settings_err_stale").to_string(),
    }
}

/// Access, store, visible cores, report zone, and the chat row both notify routes share.
struct NotifyView {
    /// Store handle. The lock is not held.
    store: Arc<Mutex<NotifyStore>>,
    /// Cores this chat may name, in Mini App order.
    cores: Vec<(u64, String, String)>,
    /// Host report zone.
    zone: Tz,
    /// Settings stored for this chat, or the all-off default when the chat is absent.
    settings: NotifySettings,
    /// Stored revision. `0` when the chat is absent.
    revision: u64,
}

/// Load the shared notify context and a snapshot of this chat's stored row.
///
/// The row is copied under the lock, and the lock is dropped before this returns. A save locks
/// again so its revision check sees the row it is about to write.
///
/// Args:
///     host: Process that owns the bot.
///     chat_id: Signed chat.
///
/// Returns:
///     The shared context.
///
/// Errors:
///     [`MiniAppApiError::Rejected`] when the Mini App is off or the chat is not paired.
///     [`MiniAppApiError::ReadFailed`] when the bot has no notifications store.
fn notify_view(host: &dyn TgHost, chat_id: i64) -> Result<NotifyView, MiniAppApiError> {
    let access = super::mini_access(host, chat_id).ok_or(MiniAppApiError::Rejected)?;
    let store = host
        .state()
        .service
        .as_ref()
        .and_then(moon_core::telegram::TelegramService::notify_store)
        .ok_or(MiniAppApiError::ReadFailed)?;
    let cores = super::visible_cores(host, &access);
    let zone = host.report_zone();
    let (settings, revision) = {
        let guard = store
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        match guard.file.chats.get(&chat_id) {
            Some(row) => (row.settings.clone(), row.revision),
            None => (NotifySettings::default(), 0),
        }
    };
    Ok(NotifyView {
        store,
        cores,
        zone,
        settings,
        revision,
    })
}

/// Read this chat's settings. A missing chat is the all-off default and revision `0`.
///
/// Args:
///     host: Process that owns the bot.
///     chat_id: Signed chat.
///
/// Returns:
///     The document, or `Rejected` when the Mini App is off or the chat is not paired.
///     `ReadFailed` when the bot has no notifications store.
///
/// Errors:
///     [`MiniAppApiError::Rejected`] or [`MiniAppApiError::ReadFailed`].
pub(super) fn mini_notify(host: &dyn TgHost, chat_id: i64) -> Result<NotifyDto, MiniAppApiError> {
    let view = notify_view(host, chat_id)?;
    Ok(dto_of(
        view.settings,
        &view.cores,
        view.zone.to_string(),
        view.revision,
        None,
        None,
    ))
}

/// Save this chat's settings. The owner and a viewer both may save.
///
/// A refusal or a failed write is still `Ok`: the document's `error` carries the localized
/// sentence, and `settings` plus `revision` are the ones on disk. `ReadFailed` is only the
/// missing store.
///
/// Args:
///     host: Process that owns the bot.
///     chat_id: Signed chat.
///     settings: Rules the page submitted.
///     revision: Revision the page loaded. It must equal the stored revision.
///
/// Returns:
///     The document after the attempt. A successful save reports the incremented revision.
///
/// Errors:
///     [`MiniAppApiError::Rejected`] when the Mini App is off or the chat is not paired.
///     [`MiniAppApiError::ReadFailed`] when the bot has no notifications store.
pub(super) fn mini_notify_save(
    host: &dyn TgHost,
    chat_id: i64,
    mut settings: NotifySettings,
    revision: u64,
) -> Result<NotifyDto, MiniAppApiError> {
    let view = notify_view(host, chat_id)?;
    let visible: Vec<u64> = view.cores.iter().map(|(id, _, _)| *id).collect();
    let now = i64::try_from(now_unix_secs()).unwrap_or(i64::MAX);
    let mut guard = view
        .store
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    keep_stored_bot_fields(&guard.file, chat_id, &mut settings);
    let outcome = store_settings(
        &mut guard, chat_id, settings, &visible, now, view.zone, revision,
    );
    let (current, stored_revision) = match guard.file.chats.get(&chat_id) {
        Some(row) => (row.settings.clone(), row.revision),
        None => (NotifySettings::default(), 0),
    };
    drop(guard);
    let zone = view.zone.to_string();
    match outcome {
        SaveResult::Saved => Ok(dto_of(
            current,
            &view.cores,
            zone,
            stored_revision,
            None,
            None,
        )),
        SaveResult::Refused(fault) => {
            let code = fault_code(&fault).to_string();
            Ok(dto_of(
                current,
                &view.cores,
                zone,
                stored_revision,
                Some(save_fault_text(fault)),
                Some(code),
            ))
        }
        SaveResult::Failed(error) => {
            log::warn!("telegram notification settings not saved for chat {chat_id}: {error}");
            Ok(dto_of(
                current,
                &view.cores,
                zone,
                stored_revision,
                Some(t!("telegram.mini_settings_err_save").to_string()),
                Some("save".to_string()),
            ))
        }
    }
}

/// The page knows neither automatic reports, the dollar follow-up of trade cards, nor the cores'
/// own events: what it submits keeps the chat's stored ones.
pub(super) fn keep_stored_bot_fields(file: &NotifyFile, chat: i64, settings: &mut NotifySettings) {
    let stored = file
        .chats
        .get(&chat)
        .map(|row| &row.settings)
        .cloned()
        .unwrap_or_default();
    settings.reports = stored.reports;
    settings.trades.usd_followup = stored.trades.usd_followup;
    settings.events = stored.events;
}

/// Machine kind stored in [`NotifyDto::fault`].
///
/// Args:
///     fault: Why the save was refused.
///
/// Returns:
///     `stale`, `cores`, or `invalid`.
fn fault_code(fault: &SaveFault) -> &'static str {
    match fault {
        SaveFault::Stale => "stale",
        SaveFault::Cores => "cores",
        SaveFault::Invalid => "invalid",
    }
}

/// Build the wire document. `error` and `fault` are `None` when the call succeeded.
///
/// Args:
///     settings: Rules to show. On a refusal these are the stored rules, not the draft.
///     cores: Cores this chat may name.
///     zone: IANA name of the host report zone.
///     revision: Stored revision after the attempt. `0` when the chat has no row.
///     error: Localized refusal, or `None` when the call succeeded.
///     fault: Machine kind (`stale`, `cores`, `invalid`, `save`), or `None` on success.
///
/// Returns:
///     The document the page renders.
fn dto_of(
    settings: NotifySettings,
    cores: &[(u64, String, String)],
    zone: String,
    revision: u64,
    error: Option<String>,
    fault: Option<String>,
) -> NotifyDto {
    NotifyDto {
        settings,
        cores: cores
            .iter()
            .map(|(id, name, exchange)| NotifyCoreDto {
                id: *id,
                name: name.clone(),
                exchange: exchange.clone(),
            })
            .collect(),
        zone,
        revision,
        error,
        fault,
    }
}
