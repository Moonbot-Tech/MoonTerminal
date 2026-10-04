//! Blocking Bot API polling with application-owned localization and persistence.
use super::{Response, SharedAuthorization, Work};
use crate::config::Secret;
use crate::telegram::{
    TelegramStatus,
    api::{BotApi, ReplyMarkup},
    commands::{ParsedCommand, parse_reply_button, parse_update},
    menu_action::{MenuAction, SettingsAction},
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
/// Pause after a 409: another poller holds the token, and every `getUpdates` of ours would only
/// terminate its poll for a moment. Long enough not to fight it, short enough to take over soon
/// after it stops.
const CONFLICT_RETRY: Duration = Duration::from_secs(10);
/// Poll with private-chat pairing and Mini App guards; persistence publication owns admission.
/// Refresh persisted navigation when the host's current role-filtered keyboard changes.
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
        let _ = status_tx.try_send(Work::Status(status_of(error)));
    });
    let _ = tx.try_send(Work::Status(TelegramStatus::Starting));
    let mut username = None;
    let mut history = super::history::History::default();
    let mut commands = Commands::default();
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
        commands.sync(&mut api, &labels, Instant::now());
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
            Err(crate::telegram::api::ApiError::Conflict) => {
                publish_status(&tx, &crate::telegram::api::ApiError::Conflict);
                wait_while_alive(&alive, CONFLICT_RETRY);
                continue;
            }
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
                    // Neither a command nor a button: an answer the owner thread may be waiting
                    // for, or an unknown command if it is not.
                    let text = text.trim();
                    if inbound.command == ParsedCommand::Unknown
                        && !text.is_empty()
                        && !text.starts_with('/')
                    {
                        inbound.command = ParsedCommand::Text(
                            text.chars()
                                .take(crate::telegram::commands::TEXT_KEEP)
                                .collect(),
                        );
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
            // A preset from the custom-period screen answers with its report.
            let is_report = matches!(
                inbound.command,
                ParsedCommand::Report(_)
                    | ParsedCommand::Start
                    | ParsedCommand::Menu(MenuAction::Preset(_))
            );
            // The station's status looks for a newer release before it answers.
            let slow = is_report
                || matches!(
                    inbound.command,
                    ParsedCommand::StationStatus
                        | ParsedCommand::Menu(MenuAction::Settings(SettingsAction::StationStatus))
                );
            // The message under which the pressed button sits; a typed command has none.
            let pressed = update
                .callback_query
                .as_ref()
                .and_then(|callback| callback.message.as_ref())
                .map(|message| message.message_id);
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
                        message: None,
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
                        message: None,
                        reply,
                    },
                    command => Work::Command {
                        chat_id,
                        command,
                        message: pressed,
                        reply,
                    },
                }
            };
            if tx.try_send(work).is_err() {
                break;
            }
            api.acknowledge_update(update.update_id);
            let Some(result) = super::response(rx, &alive, slow) else {
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
            let answered = match result {
                Response::Rich {
                    html,
                    keyboard,
                    navigation,
                } => {
                    // A keyboard owner stays until a new one replaces it; it never enters answer
                    // cleanup tracking.
                    if history.needs_navigation(chat_id, &navigation.1, is_start) {
                        match api.send_message(chat_id, &navigation.0, Some(&navigation.1)) {
                            Ok(sent) => {
                                let previous = history.replace_navigation(
                                    chat_id,
                                    sent.message_id,
                                    navigation.1,
                                );
                                if let Some(path) = &history_path {
                                    if let Err(error) = history.save(path) {
                                        log::warn!(
                                            "telegram navigation persistence failed: {error}"
                                        );
                                    }
                                }
                                // The new message owns the keyboard now; the old one only
                                // repeats the same hint above it.
                                if let Some(previous) = previous {
                                    tidy(&mut api, chat_id, previous, "keyboard");
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
                                    tidy(&mut api, chat_id, previous.id, "previous answer");
                                }
                            }
                            true
                        }
                        Ok(_) => true,
                        Err(error) => {
                            if crate::telegram::api::is_unchanged_edit("editMessageText", &error) {
                                true
                            } else {
                                publish_error(&tx, error);
                                report_failure(&mut api, &tx, &labels, chat_id);
                                false
                            }
                        }
                    }
                }
                Response::Text { text, keyboard } | Response::PairSaved { text, keyboard, .. } => {
                    let keyboard = keyboard.as_ref().filter(|_| private_chat);
                    send_text(&mut api, &tx, chat_id, &text, keyboard)
                }
            };
            // An answered press has done its job: the button's text would only pile up in the
            // chat. One left unanswered stays, beside whatever said why.
            if let Some(pressed) = pressed_button(&update, answered && reply_button && private_chat)
            {
                tidy(&mut api, chat_id, pressed, "button press");
            }
        }
    }
}

/// The command list Telegram holds for the bot, kept in step with the host's labels: published
/// when it differs from what was last published (the first poll, a language switch), retried
/// after [`COMMANDS_RETRY`] when publishing failed. Without a list Telegram hides the chat's menu
/// button whenever it is not the Mini App's.
#[derive(Default)]
struct Commands {
    published: Option<Vec<(String, String)>>,
    retry_at: Option<Instant>,
}

/// How long a failed publication waits before the next attempt.
const COMMANDS_RETRY: Duration = Duration::from_secs(60);

impl Commands {
    /// Publish the list the labels describe now, when it is not the one published.
    fn sync(
        &mut self,
        api: &mut BotApi,
        labels: &std::sync::Mutex<std::collections::BTreeMap<String, String>>,
        now: Instant,
    ) {
        let wanted = labels
            .lock()
            .map(|labels| bot_commands(&labels))
            .unwrap_or_default();
        if wanted.is_empty()
            || self.published.as_ref() == Some(&wanted)
            || self.retry_at.is_some_and(|at| now < at)
        {
            return;
        }
        match api.set_my_commands(&wanted) {
            Ok(Some(true)) => {
                self.published = Some(wanted);
                self.retry_at = None;
            }
            // Skipped for a pending rate limit: the next pass tries again.
            Ok(None) => {}
            Ok(Some(false)) => {
                log::info!("telegram command list not accepted");
                self.retry_at = Some(now + COMMANDS_RETRY);
            }
            Err(error) => {
                log::info!("telegram command list not published: {error}");
                self.retry_at = Some(now + COMMANDS_RETRY);
            }
        }
    }
}

/// The command list from the host's labels, in menu order.
fn bot_commands(labels: &std::collections::BTreeMap<String, String>) -> Vec<(String, String)> {
    COMMANDS
        .iter()
        .filter_map(|name| {
            labels
                .get(&format!("command_{name}"))
                .filter(|text| !text.trim().is_empty())
                .map(|text| ((*name).to_owned(), text.chars().take(256).collect()))
        })
        .collect()
}

/// The commands every paired chat may use, in menu order. The owner's Settings and Status are on
/// the owner's own keyboard; the Mini App has its own menu button while it runs.
const COMMANDS: [&str; 7] = [
    "today",
    "yesterday",
    "month",
    "lastmonth",
    "daily",
    "hour",
    "help",
];

/// The message of a reply-keyboard press to remove once answered: a private chat's own message,
/// young enough for Telegram to delete (48 hours).
fn pressed_button(update: &crate::telegram::api::Update, reply_button: bool) -> Option<i64> {
    let message = update.message.as_ref().filter(|_| reply_button)?;
    let now = crate::util::time::now_unix_ms_i64() / 1000;
    super::history::Answer {
        id: message.message_id,
        sent_at: message.date,
    }
    .deletable(now)
    .then_some(message.message_id)
}

/// Delete a message the chat no longer needs ([`BotApi::tidy_message`]); one already gone or too
/// old is not worth a line, and any other failure is only logged.
fn tidy(api: &mut BotApi, chat: i64, message: i64, what: &str) {
    if let Err(error) = api.tidy_message(chat, message) {
        if !crate::telegram::api::is_unavailable_delete("deleteMessage", &error) {
            log::info!("telegram {what} cleanup skipped: {error}");
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
) -> bool {
    for page in segment_pages(text) {
        if let Err(error) = api.send_message(chat, &page, keyboard) {
            publish_error(tx, error);
            return false;
        }
    }
    true
}
/// Publish typed failure and avoid hot retries for invalid credentials.
fn publish_error(tx: &SyncSender<Work>, error: crate::telegram::api::ApiError) {
    publish_status(tx, &error);
    std::thread::sleep(Duration::from_millis(250));
}

/// Map a redacted API failure onto Telegram health without sleeping.
///
/// A chat out of the bot's reach (blocked, deleted) is that chat's state, not the bot's: it is
/// logged and leaves the status alone.
fn publish_status(tx: &SyncSender<Work>, error: &crate::telegram::api::ApiError) {
    if crate::telegram::api::is_unreachable_chat(error) {
        log::info!("telegram chat unreachable: {error}");
        return;
    }
    let _ = tx.try_send(Work::Status(status_of(error)));
}

/// Telegram health for a redacted API failure.
fn status_of(error: &crate::telegram::api::ApiError) -> TelegramStatus {
    match error {
        crate::telegram::api::ApiError::Telegram {
            retry_after_secs: Some(retry_after_secs),
            ..
        } => TelegramStatus::RateLimited {
            retry_after_secs: *retry_after_secs,
        },
        crate::telegram::api::ApiError::Conflict => TelegramStatus::Conflict,
        _ => TelegramStatus::Unavailable,
    }
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

#[cfg(test)]
mod tests;
