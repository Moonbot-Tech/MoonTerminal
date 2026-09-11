//! Blocking Bot API polling with application-owned localization and persistence.
use super::{Response, SharedAuthorization, Work};
use crate::config::Secret;
use crate::telegram::{
    TelegramStatus,
    api::{BotApi, ReplyMarkup},
    commands::{ParsedCommand, parse_reply_button, parse_update},
    reply::segment_pages,
};
use std::sync::{
    Arc, Mutex, Weak,
    mpsc::{self, SyncSender},
};
use std::time::{Duration, Instant};

/// Pause after a non-retryable `getMe` Telegram error so an unchanged invalid token does not
/// retry every 250 ms. Shutdown and config replacement drop `alive` and interrupt this wait.
const INVALID_CREDENTIAL_RETRY: Duration = Duration::from_secs(30);
/// Poll with private-chat pairing and Mini App guards; persistence publication owns admission.
pub(super) fn run(
    token: Secret,
    alive: Weak<()>,
    auth: SharedAuthorization,
    labels: Arc<Mutex<std::collections::BTreeMap<String, String>>>,
    menu: super::menu::SharedMenu,
    mut menu_sync: super::menu::MenuSync,
    tx: SyncSender<Work>,
) {
    let mut api = BotApi::new(token);
    api.set_liveness(alive.clone());
    let status_tx = tx.clone();
    api.set_error_observer(move |error| {
        let status = match error {
            crate::telegram::api::ApiError::Telegram {
                retry_after_secs: Some(seconds),
                ..
            } => TelegramStatus::RateLimited {
                retry_after_secs: *seconds,
            },
            _ => TelegramStatus::Unavailable,
        };
        let _ = status_tx.try_send(Work::Status(status));
    });
    let _ = tx.try_send(Work::Status(TelegramStatus::Starting));
    let mut username = None;
    let mut history = super::history::History::default();
    let mut history_path = None;
    while alive.upgrade().is_some() {
        if username.is_none() {
            match api.get_me() {
                Ok(me) => {
                    let path = crate::config::paths::telegram_chat_history(me.id);
                    history = super::history::History::load(&path);
                    history_path = Some(path);
                    username = me.username;
                }
                Err(error) => {
                    let invalid_credential = matches!(
                        error,
                        crate::telegram::api::ApiError::Telegram {
                            retry_after_secs: None,
                            ..
                        }
                    );
                    if invalid_credential {
                        publish_status(&tx, &error);
                        wait_while_alive(&alive, INVALID_CREDENTIAL_RETRY);
                    } else {
                        publish_error(&tx, error);
                    }
                    continue;
                }
            }
        }
        let menu_intent = menu.lock().map(|intent| intent.clone());
        if let Ok(intent) = menu_intent {
            if let Err(error) = menu_sync.sync(&intent, Instant::now(), |chat, button| {
                api.set_chat_menu_button(chat, button)
            }) {
                log::warn!("telegram menu update failed: {error}");
                publish_status(&tx, &error);
            }
        }
        let updates = match api.get_updates() {
            Ok(updates) => updates,
            Err(error) => {
                publish_error(&tx, error);
                continue;
            }
        };
        // A retry attempt does not prove recovery; retain the last failure until polling succeeds.
        let count = auth.lock().map(|a| a.chat_count()).unwrap_or(0);
        let _ = tx.try_send(Work::Status(if count == 0 {
            TelegramStatus::Unpaired
        } else {
            TelegramStatus::Paired { chat_count: count }
        }));
        for update in updates {
            if alive.upgrade().is_none() {
                return;
            }
            if let Some(callback) = &update.callback_query {
                if let Err(error) = api.answer_callback(&callback.id) {
                    publish_status(&tx, &error);
                }
            }
            let Some(mut inbound) = parse_update(&update, username.as_deref()) else {
                api.acknowledge_update(update.update_id);
                continue;
            };
            let mut reply_button = false;
            if inbound.command == ParsedCommand::Unknown {
                if let Some(text) = update
                    .message
                    .as_ref()
                    .and_then(|message| message.text.as_deref())
                {
                    if let Ok(labels) = labels.lock() {
                        inbound.command = parse_reply_button(text, &labels);
                        reply_button = inbound.command != ParsedCommand::Unknown;
                    }
                }
            }
            let chat_id = inbound.chat_id;
            // Match the same source as parse_update; absent chat metadata fails closed.
            let private_chat = update
                .message
                .as_ref()
                .or_else(|| {
                    update
                        .callback_query
                        .as_ref()
                        .and_then(|callback| callback.message.as_ref())
                })
                .map(|message| &message.chat)
                .is_some_and(|chat| chat.id == chat_id && chat.kind == "private");
            let (reply, rx) = mpsc::sync_channel(1);
            let is_start = matches!(inbound.command, ParsedCommand::Start);
            let is_report = matches!(
                inbound.command,
                ParsedCommand::Report(_) | ParsedCommand::Start
            );
            let work = {
                let Ok(mut ledger) = auth.lock() else {
                    return;
                };
                match inbound.command {
                    _ if !private_chat => Work::Command {
                        chat_id,
                        command: ParsedCommand::Pair {
                            code: String::new(),
                        },
                        reply,
                    },
                    ParsedCommand::Pair { ref code }
                        if ledger.pair(chat_id, code, Instant::now()).is_ok() =>
                    {
                        ledger.revoke_chat(chat_id);
                        Work::Pair { chat_id, reply }
                    }
                    _ if !ledger.is_authorized(chat_id) => Work::Command {
                        chat_id,
                        command: ParsedCommand::Pair {
                            code: String::new(),
                        },
                        reply,
                    },
                    command => Work::Command {
                        chat_id,
                        command,
                        reply,
                    },
                }
            };
            if tx.try_send(work).is_err() {
                break;
            }
            api.acknowledge_update(update.update_id);
            let Some(result) = super::response(rx, &alive, is_report) else {
                if is_report {
                    report_failure(&mut api, &tx, &labels, chat_id);
                }
                continue;
            };
            // A report may outlive a pairing revocation while its database read completes.
            if matches!(result, Response::Rich { .. })
                && !auth
                    .lock()
                    .is_ok_and(|ledger| ledger.is_authorized(chat_id))
            {
                continue;
            }
            match result {
                Response::Rich {
                    html,
                    keyboard,
                    navigation,
                } => {
                    // Keyboard owners are permanent and never enter answer cleanup tracking.
                    if is_start || !history.navigation.contains_key(&chat_id) {
                        match api.send_message(chat_id, &navigation.0, Some(&navigation.1)) {
                            Ok(sent) => {
                                history.navigation.insert(chat_id, sent.message_id);
                                if let Some(path) = &history_path {
                                    if let Err(error) = history.save(path) {
                                        log::warn!(
                                            "telegram navigation persistence failed: {error}"
                                        );
                                    }
                                }
                            }
                            Err(error) => publish_error(&tx, error),
                        }
                    }
                    let message = update
                        .callback_query
                        .as_ref()
                        .and_then(|callback| callback.message.as_ref())
                        .map(|message| message.message_id);
                    match api.rich_message(chat_id, message, &html, &keyboard) {
                        Ok(delivered) if message.is_none() => {
                            let previous = history.answers.insert(
                                chat_id,
                                super::history::Answer {
                                    id: delivered.message_id,
                                    sent_at: delivered.date,
                                },
                            );
                            let saved = history_path.as_ref().is_some_and(|path| {
                                match history.save(path) {
                                    Ok(()) => true,
                                    Err(error) => {
                                        log::warn!("telegram answer persistence failed: {error}");
                                        false
                                    }
                                }
                            });
                            if saved && reply_button {
                                if let Some(previous) = previous.filter(|answer| {
                                    answer.deletable(crate::util::time::now_unix_ms_i64() / 1000)
                                }) {
                                    if let Err(error) = api.delete_message(chat_id, previous.id) {
                                        if !crate::telegram::api::is_unavailable_delete(
                                            "deleteMessage",
                                            &error,
                                        ) {
                                            log::warn!(
                                                "telegram previous answer cleanup failed: {error}"
                                            );
                                        }
                                    }
                                }
                            }
                        }
                        Ok(_) => {}
                        Err(error) => {
                            if !crate::telegram::api::is_unchanged_edit("editMessageText", &error) {
                                publish_error(&tx, error);
                                report_failure(&mut api, &tx, &labels, chat_id);
                            }
                        }
                    }
                }
                Response::Text { text, keyboard } | Response::PairSaved { text, keyboard, .. } => {
                    let keyboard = keyboard.as_ref().filter(|_| private_chat);
                    send_text(&mut api, &tx, chat_id, &text, keyboard);
                }
            }
        }
    }
}
/// Best-effort localized feedback when a report times out or Telegram rejects its markup.
fn report_failure(
    api: &mut BotApi,
    tx: &SyncSender<Work>,
    labels: &Mutex<std::collections::BTreeMap<String, String>>,
    chat: i64,
) {
    let text = labels
        .lock()
        .ok()
        .and_then(|labels| labels.get("report_delivery_failed").cloned());
    if let Some(text) = text {
        send_text(api, tx, chat, &text, None);
    }
}
/// Deliver localized pages and expose redacted transport failures.
fn send_text(
    api: &mut BotApi,
    tx: &SyncSender<Work>,
    chat: i64,
    text: &str,
    keyboard: Option<&ReplyMarkup>,
) {
    for page in segment_pages(text) {
        if let Err(error) = api.send_message(chat, &page, keyboard) {
            publish_error(tx, error);
            break;
        }
    }
}
/// Publish typed failure and avoid hot retries for invalid credentials.
fn publish_error(tx: &SyncSender<Work>, error: crate::telegram::api::ApiError) {
    publish_status(tx, &error);
    std::thread::sleep(Duration::from_millis(250));
}

/// Map a redacted API failure onto Telegram health without sleeping.
fn publish_status(tx: &SyncSender<Work>, error: &crate::telegram::api::ApiError) {
    let status = match error {
        crate::telegram::api::ApiError::Telegram {
            retry_after_secs: Some(retry_after_secs),
            ..
        } => TelegramStatus::RateLimited {
            retry_after_secs: *retry_after_secs,
        },
        _ => TelegramStatus::Unavailable,
    };
    let _ = tx.try_send(Work::Status(status));
}

/// Sleep up to `total`, returning as soon as the service owner is dropped.
fn wait_while_alive(alive: &Weak<()>, total: Duration) {
    let start = Instant::now();
    while start.elapsed() < total {
        if alive.upgrade().is_none() {
            return;
        }
        std::thread::sleep((total - start.elapsed().min(total)).min(Duration::from_millis(100)));
    }
}
