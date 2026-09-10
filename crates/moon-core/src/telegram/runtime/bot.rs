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
    while alive.upgrade().is_some() {
        if username.is_none() {
            match api.get_me() {
                Ok(me) => username = me.username,
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
            let Some(mut inbound) = parse_update(&update, username.as_deref()) else {
                api.acknowledge_update(update.update_id);
                continue;
            };
            if inbound.command == ParsedCommand::Unknown {
                if let Some(text) = update
                    .message
                    .as_ref()
                    .and_then(|message| message.text.as_deref())
                {
                    if let Ok(labels) = labels.lock() {
                        inbound.command = parse_reply_button(text, &labels);
                    }
                }
            }
            let chat_id = inbound.chat_id;
            // Match the same source as parse_update; absent chat metadata fails closed.
            let private_chat = update
                .message
                .as_ref()
                .map(|message| &message.chat)
                .is_some_and(|chat| chat.id == chat_id && chat.kind == "private");
            let (reply, rx) = mpsc::sync_channel(1);
            let work = {
                let Ok(mut ledger) = auth.lock() else {
                    return;
                };
                match inbound.command {
                    ParsedCommand::Pair { .. }
                    | ParsedCommand::MiniApp
                    | ParsedCommand::Start
                    | ParsedCommand::Help
                        if !private_chat =>
                    {
                        Work::Command {
                            chat_id,
                            command: ParsedCommand::Pair {
                                code: String::new(),
                            },
                            reply,
                        }
                    }
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
            let Some(result) = super::response(rx, &alive) else {
                continue;
            };
            let (text, keyboard) = match result {
                Response::Text { text, keyboard } => (text, keyboard),
                Response::PairSaved { text, keyboard, .. } => (text, keyboard),
            };
            // Both persistent navigation and Mini App launchers belong to private chats.
            let keyboard = keyboard.as_ref().filter(|_| private_chat);
            send_text(&mut api, &tx, chat_id, &text, keyboard);
        }
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
