//! Bot API serialization, long polling, and redacted errors.
//!
//! Blocking calls over the workspace `ureq` client. The token is a [`Secret`] and is used only to
//! build the request path; errors, retries, and `Debug` never carry the token or a Bot API URL.
//! There is no webhook path and no async runtime.

use std::time::Duration;

use serde::{Deserialize, Serialize, de::DeserializeOwned};

use crate::config::Secret;

mod photo;
pub use photo::CAPTION_UTF16_LIMIT;

/// How one request's body goes on the wire: JSON for every method but `sendPhoto`, whose upload
/// is multipart. Retries call it again, so it builds the body from borrowed parts each time.
type SendBody<'a> = &'a dyn Fn(
    ureq::RequestBuilder<ureq::typestate::WithBody>,
) -> Result<ureq::http::Response<ureq::Body>, ureq::Error>;

/// How long a non-polling Bot API call may take, matching the crowd REST client.
///
/// `getMe` / `sendMessage` are short JSON posts; fifteen seconds is the
/// same bound as `crowd/feed/rest.rs` and is enough for a single HTTPS round-trip without letting
/// a stalled socket pin the worker thread.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
/// TCP connect bound shared by both agents.
///
/// Five seconds is long enough for a slow path and short enough that a blackholed connect does
/// not consume the full request budget before the caller can back off.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
/// `getUpdates` `timeout` argument, in seconds.
///
/// Telegram long-polls until an update arrives or this many seconds elapse. Twenty-five seconds
/// is under Telegram's 50-second maximum and leaves headroom under [`LONG_POLL_TIMEOUT`].
const LONG_POLL_SECONDS: u64 = 25;
/// Wall-clock budget for one long-poll HTTP call.
///
/// Must exceed [`LONG_POLL_SECONDS`]: the server holds the socket for the poll, then writes the
/// JSON. Fifty seconds covers the 25 s hold plus TLS, write, and a slow body without waiting a
/// full minute on a dead connection.
const LONG_POLL_TIMEOUT: Duration = Duration::from_secs(50);
/// Largest Bot API JSON body this client will decode.
///
/// A long-poll batch of text messages is small; a megabyte is a hard ceiling against a runaway
/// payload, not a measured size.
const MAX_BODY_BYTES: u64 = 1024 * 1024;
/// First backoff after a retryable failure with no `retry_after`.
const BACKOFF_INITIAL: Duration = Duration::from_secs(1);
/// Cap for exponential backoff. Telegram's own `retry_after` may exceed this and is honored
/// verbatim; this cap only applies to locally chosen waits.
const BACKOFF_CAP: Duration = Duration::from_secs(32);
/// Attempts per call, including the first. Four retries is 1+2+4+8 s of local backoff and is
/// enough to ride a short 5xx window without turning one user command into a long stall.
const MAX_ATTEMPTS: u32 = 5;

/// Blocking Telegram Bot API client.
///
/// Owns the bot token as a [`Secret`]. Deliberately does not implement `Debug`: the token would
/// otherwise be one `:?` away from a log line even if `Secret` itself is masked.
pub struct BotApi {
    /// Redacted failure observer used to publish rate limiting before retry sleeps.
    error_observer: Option<Box<dyn Fn(&ApiError) + Send>>,
    /// Service cancellation interrupts backoff without violating Telegram deadlines.
    liveness: Option<std::sync::Weak<()>>,
    token: Secret,
    request_agent: ureq::Agent,
    long_poll_agent: ureq::Agent,
    /// Next `getUpdates` offset. Monotonic: advanced only after the corresponding `Work` is
    /// accepted (or an update is discarded as unparseable), always to `update_id+1` so Telegram
    /// can redeliver an unaccepted update.
    offset: i64,
    retry: RetryState,
}

/// Capped exponential retry bookkeeping, overwritten by Telegram `retry_after` when present.
#[derive(Clone, Copy, Debug)]
struct RetryState {
    failures: u32,
    /// Wait to honor before the next attempt, including the next public method call.
    pending: Option<Duration>,
}

/// Classified Bot API failure. Display and debug forms never include a URL or the token.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ApiError {
    /// The call exceeded its bounded timeout.
    Timeout,
    /// TLS, DNS, or connect failed. No host or path is retained.
    Transport,
    /// Telegram returned `ok=false`, or an HTTP error with a parsed description.
    Telegram {
        /// Telegram `description` with any token-shaped substring stripped.
        description: String,
        /// `parameters.retry_after` when Telegram asked the client to wait, in seconds.
        retry_after_secs: Option<u32>,
    },
    /// The body was not a Telegram envelope this client can decode.
    Protocol,
    /// HTTP 409: the token is polled somewhere else — another `getUpdates` took over, or a webhook
    /// is set. One token serves one poller; this one cannot win by retrying.
    Conflict,
}

impl std::fmt::Display for ApiError {
    /// Format the classified failure without exposing the request URL or bot token.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ApiError::Timeout => f.write_str("telegram bot api timeout"),
            ApiError::Transport => f.write_str("telegram bot api transport error"),
            ApiError::Telegram {
                description,
                retry_after_secs,
            } => match retry_after_secs {
                Some(secs) => write!(f, "telegram bot api: {description} (retry after {secs}s)"),
                None => write!(f, "telegram bot api: {description}"),
            },
            ApiError::Protocol => f.write_str("telegram bot api protocol error"),
            ApiError::Conflict => f.write_str("telegram bot api: the bot is polled elsewhere"),
        }
    }
}

impl std::error::Error for ApiError {}

/// User object returned by `getMe` and nested in messages.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct User {
    pub id: i64,
    #[serde(default)]
    pub is_bot: bool,
    #[serde(default)]
    pub first_name: String,
    #[serde(default)]
    pub username: Option<String>,
}

/// Chat identity on an incoming message.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct Chat {
    pub id: i64,
    #[serde(default, rename = "type")]
    pub kind: String,
}

/// Minimal incoming or echoed message.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct Message {
    #[serde(default)]
    pub message_id: i64,
    #[serde(default)]
    pub date: i64,
    #[serde(default)]
    pub chat: Chat,
    #[serde(default)]
    pub from: Option<User>,
    #[serde(default)]
    pub text: Option<String>,
}

/// A button press carries both its sender and the originating message for authorization.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct CallbackQuery {
    pub id: String,
    pub from: User,
    pub message: Option<Message>,
    pub data: Option<String>,
}

/// One long-poll update, including report navigation callbacks.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct Update {
    pub update_id: i64,
    #[serde(default)]
    pub message: Option<Message>,
    #[serde(default)]
    pub callback_query: Option<CallbackQuery>,
}

/// Telegram Mini App button target. The URL is the current tunnel, set per message.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct WebAppInfo {
    pub url: String,
}

/// A private chat's native menu either launches the current app or shows bot commands.
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum MenuButton {
    Commands,
    WebApp { text: String, web_app: WebAppInfo },
}

/// Always scope menu changes to one paired private chat, never the bot-wide default.
#[derive(Serialize)]
struct SetChatMenuButtonReq<'a> {
    chat_id: i64,
    menu_button: &'a MenuButton,
}

/// One inline-keyboard Mini App button.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct InlineKeyboardButton {
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub web_app: Option<WebAppInfo>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub callback_data: Option<String>,
}

/// Reply markup accepted by `sendMessage`.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct InlineKeyboardMarkup {
    pub inline_keyboard: Vec<Vec<InlineKeyboardButton>>,
}

/// Persistent text navigation; buttons send their labels as ordinary chat messages.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct ReplyKeyboardMarkup {
    pub keyboard: Vec<Vec<KeyboardButton>>,
    pub resize_keyboard: bool,
    pub is_persistent: bool,
}

/// A text-only reply button with optional Telegram-native visual emphasis.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct KeyboardButton {
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub style: Option<String>,
}

/// Telegram accepts one keyboard kind per message, without an enum discriminator.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(untagged)]
pub enum ReplyMarkup {
    Inline(InlineKeyboardMarkup),
    Reply(ReplyKeyboardMarkup),
}

/// Optional retry metadata carried by a failed Telegram response envelope.
#[derive(Deserialize)]
struct ResponseParameters {
    #[serde(default)]
    retry_after: Option<u32>,
}

/// Generic Telegram Bot API response envelope.
#[derive(Deserialize)]
struct Envelope<T> {
    ok: bool,
    /// Absent on `ok=false`. `Option` is optional in serde without `default`, so this does not
    /// impose `T: Default` on the envelope.
    result: Option<T>,
    description: Option<String>,
    parameters: Option<ResponseParameters>,
}

/// Serialized `getUpdates` request body.
#[derive(Serialize)]
struct GetUpdatesReq<'a> {
    offset: i64,
    timeout: u64,
    allowed_updates: &'a [&'a str],
}

/// `link_preview_options` object. Notifications disable previews so a URL in the card is not expanded.
#[derive(Clone, Copy, Debug, Serialize)]
struct LinkPreviewOptions {
    is_disabled: bool,
}

/// Serialized `sendMessage` request body.
#[derive(Serialize)]
struct SendMessageReq<'a> {
    chat_id: i64,
    text: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    reply_markup: Option<&'a ReplyMarkup>,
    #[serde(skip_serializing_if = "Option::is_none")]
    parse_mode: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    link_preview_options: Option<LinkPreviewOptions>,
}

/// Plain `sendMessage` body. `parse_mode` and link previews stay absent, so existing callers
/// keep the same wire body.
fn text_message_request<'a>(
    chat_id: i64,
    text: &'a str,
    reply_markup: Option<&'a ReplyMarkup>,
) -> SendMessageReq<'a> {
    SendMessageReq {
        chat_id,
        text,
        reply_markup,
        parse_mode: None,
        link_preview_options: None,
    }
}

/// HTML `sendMessage` body with link previews disabled.
fn send_html_request<'a>(chat_id: i64, html: &'a str) -> SendMessageReq<'a> {
    SendMessageReq {
        chat_id,
        text: html,
        reply_markup: None,
        parse_mode: Some("HTML"),
        link_preview_options: Some(LinkPreviewOptions { is_disabled: true }),
    }
}

impl RetryState {
    /// Start with no prior failures or deferred retry deadline.
    fn new() -> Self {
        Self {
            failures: 0,
            pending: None,
        }
    }

    /// Clear retry history after a successful API call.
    fn reset(&mut self) {
        self.failures = 0;
        self.pending = None;
    }

    /// Consume the deferred retry deadline before the next public API call.
    fn take_pending(&mut self) -> Option<Duration> {
        self.pending.take()
    }

    /// Next wait after a retryable failure.
    ///
    /// Telegram `retry_after` is used as-is when present. Otherwise the wait doubles from
    /// [`BACKOFF_INITIAL`] up to [`BACKOFF_CAP`].
    fn wait_after_failure(&mut self, retry_after_secs: Option<u32>) -> Duration {
        let exp = BACKOFF_INITIAL
            .saturating_mul(1u32 << self.failures.min(5))
            .min(BACKOFF_CAP);
        self.failures = self.failures.saturating_add(1);
        match retry_after_secs {
            Some(secs) => Duration::from_secs(u64::from(secs)),
            None => exp,
        }
    }
}

impl BotApi {
    /// Build a client that owns `token` and two timeout-bounded HTTPS agents.
    ///
    /// Args:
    ///     token: Bot API token. Empty tokens are accepted here; the service constructor is
    ///         the off switch and must not call this with an empty credential.
    ///
    /// Returns:
    ///     A client with offset 0 and a fresh retry state.
    pub fn new(token: Secret) -> Self {
        Self {
            error_observer: None,
            liveness: None,
            token,
            request_agent: build_agent(REQUEST_TIMEOUT),
            long_poll_agent: build_agent(LONG_POLL_TIMEOUT),
            offset: 0,
            retry: RetryState::new(),
        }
    }

    /// Current monotonic `getUpdates` offset (the next update_id Telegram should send).
    pub fn offset(&self) -> i64 {
        self.offset
    }

    /// Observe service shutdown between bounded network calls and during retry waits.
    pub fn set_liveness(&mut self, liveness: std::sync::Weak<()>) {
        self.liveness = Some(liveness);
    }

    /// Publish retry state immediately without moving network operations onto the UI thread.
    pub fn set_error_observer(&mut self, observer: impl Fn(&ApiError) + Send + 'static) {
        self.error_observer = Some(Box::new(observer));
    }

    /// Whether the optional service owner still permits a network operation.
    fn running(&self) -> bool {
        self.liveness
            .as_ref()
            .is_none_or(|alive| alive.upgrade().is_some())
    }

    /// `getMe` — identity of the bot, including the username used to accept command suffixes.
    ///
    /// Returns:
    ///     The bot user, or a classified error with no URL or token.
    pub fn get_me(&mut self) -> Result<User, ApiError> {
        self.post("getMe", &EmptyBody {}, false)
    }

    /// Publish the bot's command list (`setMyCommands`), which the chat's menu button shows when
    /// it is not the Mini App. One quiet attempt like [`Self::tidy_message`]: a failure is the
    /// caller's to log and retry, never the bot's health; a rate limit it meets is kept for the
    /// next request.
    ///
    /// Args:
    ///     commands: `(command, description)` pairs; the command without its slash.
    ///
    /// Returns:
    ///     `Ok(None)` when skipped for a pending rate limit or shutdown; Telegram's answer
    ///     otherwise.
    pub fn set_my_commands(
        &mut self,
        commands: &[(String, String)],
    ) -> Result<Option<bool>, ApiError> {
        if !self.running() || self.retry.pending.is_some() {
            return Ok(None);
        }
        let commands: Vec<_> = commands
            .iter()
            .map(|(command, description)| {
                serde_json::json!({"command": command, "description": description})
            })
            .collect();
        let body = serde_json::json!({"commands": commands});
        self.post_once("setMyCommands", false, &|request| request.send_json(&body))
            .map(Some)
            .map_err(|failure| {
                if let Some(secs) = retry_after_of(&failure.error) {
                    self.retry.pending = Some(Duration::from_secs(u64::from(secs)));
                }
                failure.error
            })
    }

    /// Replace a private chat's menu using the same redacted, cancellable transport as replies.
    pub fn set_chat_menu_button(
        &mut self,
        chat_id: i64,
        menu_button: &MenuButton,
    ) -> Result<(), ApiError> {
        let accepted: bool = self.post(
            "setChatMenuButton",
            &SetChatMenuButtonReq {
                chat_id,
                menu_button,
            },
            false,
        )?;
        if accepted {
            Ok(())
        } else {
            Err(ApiError::Protocol)
        }
    }

    /// Long-poll `getUpdates` without acknowledging the batch.
    ///
    /// An empty result is success (the poll timed out with no updates) and does not move the
    /// offset. A decode or transport failure leaves the offset unchanged so Telegram can
    /// redeliver. Call [`BotApi::acknowledge_update`] after the corresponding bounded `Work` is
    /// accepted, or after discarding an unparseable update.
    ///
    /// Returns:
    ///     Decoded updates in arrival order, possibly empty.
    pub fn get_updates(&mut self) -> Result<Vec<Update>, ApiError> {
        let req = GetUpdatesReq {
            offset: self.offset,
            timeout: LONG_POLL_SECONDS,
            allowed_updates: &["message", "callback_query"],
        };
        self.post("getUpdates", &req, true)
    }

    /// Advance the monotonic offset past `update_id` so Telegram will not redeliver it.
    ///
    /// Args:
    ///     update_id: The accepted or intentionally discarded update.
    pub fn acknowledge_update(&mut self, update_id: i64) {
        let next = update_id.saturating_add(1);
        if next > self.offset {
            self.offset = next;
        }
    }

    /// Send a text message with an optional inline launcher or persistent reply keyboard.
    ///
    /// Args:
    ///     chat_id: Destination chat.
    ///     text: Already-composed page; the caller owns localization and 4096-unit splitting.
    ///     reply_markup: Optional Telegram keyboard, serialized without a wrapper.
    ///
    /// Returns:
    ///     The echoed message on success.
    pub fn send_message(
        &mut self,
        chat_id: i64,
        text: &str,
        reply_markup: Option<&ReplyMarkup>,
    ) -> Result<Message, ApiError> {
        self.post(
            "sendMessage",
            &text_message_request(chat_id, text, reply_markup),
            false,
        )
    }

    /// Send one notification as Telegram HTML, with link previews disabled.
    ///
    /// Args:
    ///     chat_id: Destination chat.
    ///     html: Telegram HTML subset. The caller bounds the text to 4096 UTF-16 units.
    ///
    /// Returns:
    ///     The echoed message on success.
    ///
    /// Errors:
    ///     The same classified failures as [`BotApi::send_message`]. `post` already waits out
    ///     `retry_after` before returning a retryable error.
    pub fn send_html(&mut self, chat_id: i64, html: &str) -> Result<Message, ApiError> {
        self.post("sendMessage", &send_html_request(chat_id, html), false)
    }

    /// Replace the text of a message [`BotApi::send_html`] sent, keeping its form.
    ///
    /// Args:
    ///     chat_id: Chat the message is in.
    ///     message: The message to edit.
    ///     html: The new Telegram HTML, bounded like [`BotApi::send_html`]'s.
    ///
    /// Returns:
    ///     The edited message on success.
    ///
    /// Errors:
    ///     The same classified failures as [`BotApi::send_html`]. An edit that changes nothing is
    ///     a `Telegram` error `post` keeps out of the service health.
    pub fn edit_html(
        &mut self,
        chat_id: i64,
        message: i64,
        html: &str,
    ) -> Result<Message, ApiError> {
        let body = serde_json::json!({
            "chat_id": chat_id,
            "message_id": message,
            "text": html,
            "parse_mode": "HTML",
            "link_preview_options": {"is_disabled": true},
        });
        self.post("editMessageText", &body, false)
    }

    /// Send editable reports with their inline keyboard in the initial request.
    pub fn rich_message(
        &mut self,
        chat: i64,
        message: Option<i64>,
        html: &str,
        keyboard: &ReplyMarkup,
    ) -> Result<Message, ApiError> {
        let (method, body) = rich_request(chat, message, html, keyboard);
        self.post(method, &body, false)
    }

    /// Delete a message the chat no longer needs: one attempt with the ordinary request timeout,
    /// never retried and never reported to the error observer, so tidying a chat cannot change
    /// the bot's health. Skipped while a rate limit is pending. A rate limit it meets itself is
    /// kept for the next real request, which would otherwise walk straight into it.
    ///
    /// Returns:
    ///     `Ok(false)` when skipped; Telegram's answer otherwise.
    pub fn tidy_message(&mut self, chat: i64, message: i64) -> Result<bool, ApiError> {
        if !self.running() || self.retry.pending.is_some() {
            return Ok(false);
        }
        let body = serde_json::json!({"chat_id": chat, "message_id": message});
        self.post_once("deleteMessage", false, &|request| request.send_json(&body))
            .map_err(|failure| {
                if let Some(secs) = retry_after_of(&failure.error) {
                    self.retry.pending = Some(Duration::from_secs(u64::from(secs)));
                }
                failure.error
            })
    }

    /// Dismiss Telegram's callback spinner before waiting for report computation.
    pub fn answer_callback(&mut self, id: &str) -> Result<bool, ApiError> {
        self.post(
            "answerCallbackQuery",
            &serde_json::json!({"callback_query_id": id}),
            false,
        )
    }

    /// Honor retry deadlines while allowing service shutdown to interrupt waits between calls.
    fn post<T, B>(&mut self, method: &str, body: &B, long_poll: bool) -> Result<T, ApiError>
    where
        T: DeserializeOwned,
        B: Serialize,
    {
        self.post_with(method, long_poll, &|request| request.send_json(body))
    }

    /// [`Self::post`] for a body `send` puts on the wire.
    fn post_with<T>(
        &mut self,
        method: &str,
        long_poll: bool,
        send: SendBody<'_>,
    ) -> Result<T, ApiError>
    where
        T: DeserializeOwned,
    {
        let mut last_error = ApiError::Transport;
        for attempt in 0..MAX_ATTEMPTS {
            if !self.running() {
                return Err(ApiError::Transport);
            }
            if let Some(wait) = self.retry.take_pending() {
                let start = std::time::Instant::now();
                while start.elapsed() < wait {
                    if !self.running() {
                        return Err(ApiError::Transport);
                    }
                    std::thread::sleep(
                        (wait - start.elapsed().min(wait)).min(Duration::from_millis(100)),
                    );
                }
            }
            let result = self.post_once(method, long_poll, send);
            if let (Err(failure), Some(observer)) = (&result, &self.error_observer) {
                // A chat out of reach says nothing about the bot's own health.
                if !is_unchanged_edit(method, &failure.error)
                    && !is_unavailable_delete(method, &failure.error)
                    && !is_unreachable_chat(&failure.error)
                {
                    observer(&failure.error);
                }
            }
            match result {
                Ok(value) => {
                    self.retry.reset();
                    return Ok(value);
                }
                Err(failure) if is_attempt_retryable(&failure) && attempt + 1 < MAX_ATTEMPTS => {
                    let wait = self
                        .retry
                        .wait_after_failure(retry_after_of(&failure.error));
                    self.retry.pending = Some(wait);
                    last_error = failure.error;
                }
                Err(failure) => {
                    if is_attempt_retryable(&failure) {
                        let wait = self
                            .retry
                            .wait_after_failure(retry_after_of(&failure.error));
                        self.retry.pending = Some(wait);
                    }
                    return Err(failure.error);
                }
            }
        }
        Err(last_error)
    }

    /// Send one bounded request without sleeping or changing retry state.
    fn post_once<T>(
        &self,
        method: &str,
        long_poll: bool,
        send: SendBody<'_>,
    ) -> Result<T, AttemptError>
    where
        T: DeserializeOwned,
    {
        let url = format!(
            "https://api.telegram.org/bot{}/{}",
            self.token.expose(),
            method
        );
        let agent = if long_poll {
            &self.long_poll_agent
        } else {
            &self.request_agent
        };
        let response = match send(agent.post(&url)) {
            Ok(response) => response,
            Err(err) => {
                drop(url);
                return Err(AttemptError {
                    error: classify_ureq(&err),
                    http_retry: false,
                });
            }
        };
        drop(url);
        let status = response.status().as_u16();
        let http_retry = is_http_retryable_status(status);
        let envelope: Result<Envelope<T>, _> = response
            .into_body()
            .with_config()
            .limit(MAX_BODY_BYTES)
            .read_json();
        match envelope {
            Ok(envelope) if envelope.ok => envelope.result.ok_or(AttemptError {
                error: ApiError::Protocol,
                http_retry,
            }),
            Ok(envelope) => {
                let description = redact_secret(
                    envelope.description.as_deref().unwrap_or("ok=false"),
                    self.token.expose(),
                );
                let retry_after_secs = envelope.parameters.and_then(|p| p.retry_after);
                Err(AttemptError {
                    error: refusal(status, description, retry_after_secs),
                    http_retry,
                })
            }
            Err(_) if http_retry => Err(AttemptError {
                error: ApiError::Transport,
                http_retry: true,
            }),
            Err(_) => Err(AttemptError {
                error: ApiError::Protocol,
                http_retry: false,
            }),
        }
    }
}

/// Classified HTTP attempt. `http_retry` is true for 429 and 5xx even when the body decoded
/// as a typed Telegram envelope, so those statuses back off without collapsing other API
/// errors into transport failures.
struct AttemptError {
    error: ApiError,
    http_retry: bool,
}

/// Empty JSON request body for Bot API methods with no parameters.
#[derive(Serialize)]
struct EmptyBody {}

/// Build an HTTPS-only ureq agent with the supplied whole-request timeout.
fn build_agent(global: Duration) -> ureq::Agent {
    let config = ureq::Agent::config_builder()
        .timeout_global(Some(global))
        .timeout_connect(Some(CONNECT_TIMEOUT))
        .https_only(true)
        .http_status_as_error(false)
        .build();
    ureq::Agent::new_with_config(config)
}

/// Map ureq transport failures to the client's redacted error classification.
fn classify_ureq(err: &ureq::Error) -> ApiError {
    if is_timeout_error(err) {
        ApiError::Timeout
    } else {
        ApiError::Transport
    }
}

/// Search an error source chain for the I/O timeout that ureq does not expose directly.
fn is_timeout_error(err: &ureq::Error) -> bool {
    let mut cur: Option<&dyn std::error::Error> = Some(err);
    while let Some(e) = cur {
        if let Some(io) = e.downcast_ref::<std::io::Error>() {
            if io.kind() == std::io::ErrorKind::TimedOut {
                return true;
            }
        }
        cur = e.source();
    }
    false
}

/// The status Telegram answers `getUpdates` with while another poller or a webhook holds the
/// token: "terminated by other getUpdates request" or "can't use getUpdates method while webhook
/// is active".
const CONFLICT_STATUS: u16 = 409;

/// Classify a decoded `ok=false` envelope by its HTTP status.
///
/// Args:
///     status: HTTP status of the answer.
///     description: Telegram `description`, already redacted.
///     retry_after_secs: `parameters.retry_after`, when present.
///
/// Returns:
///     [`ApiError::Conflict`] for 409, otherwise [`ApiError::Telegram`].
fn refusal(status: u16, description: String, retry_after_secs: Option<u32>) -> ApiError {
    if status == CONFLICT_STATUS {
        return ApiError::Conflict;
    }
    ApiError::Telegram {
        description,
        retry_after_secs,
    }
}

/// HTTP 429 and 5xx retry regardless of whether the body was a Telegram envelope.
fn is_http_retryable_status(status: u16) -> bool {
    status == 429 || (500..600).contains(&status)
}

/// Return whether an error class can be retried without an HTTP status override.
fn is_retryable(err: &ApiError) -> bool {
    match err {
        ApiError::Timeout | ApiError::Transport => true,
        ApiError::Telegram {
            retry_after_secs, ..
        } => retry_after_secs.is_some(),
        ApiError::Protocol | ApiError::Conflict => false,
    }
}

/// Retry on transport/timeout/`retry_after`, and on HTTP 429/5xx typed envelopes.
fn is_attempt_retryable(attempt: &AttemptError) -> bool {
    attempt.http_retry || is_retryable(&attempt.error)
}

/// Extract Telegram's requested retry delay when the failure includes one.
fn retry_after_of(err: &ApiError) -> Option<u32> {
    match err {
        ApiError::Telegram {
            retry_after_secs, ..
        } => *retry_after_secs,
        _ => None,
    }
}

/// Strip the live token from a Telegram description so it can be stored on [`ApiError`].
///
/// Args:
///     text: Telegram `description` or a fallback label.
///     token: Live token bytes; replaced with `***` if they appear.
///
/// Returns:
///     A description that does not contain the token.
fn redact_secret(text: &str, token: &str) -> String {
    if token.is_empty() || !text.contains(token) {
        text.to_string()
    } else {
        text.replace(token, "***")
    }
}

/// Keep removal of a reply keyboard out of every message that will later be edited.
fn rich_request(
    chat: i64,
    message: Option<i64>,
    html: &str,
    keyboard: &ReplyMarkup,
) -> (&'static str, serde_json::Value) {
    let mut body = serde_json::json!({"chat_id":chat,"rich_message":{"html":html,"skip_entity_detection":true},"reply_markup":keyboard});
    let method = if let Some(id) = message {
        body["message_id"] = id.into();
        "editMessageText"
    } else {
        "sendRichMessage"
    };
    (method, body)
}

/// Telegram reports an idempotent edit as an error; it must not change service health.
pub fn is_unchanged_edit(method: &str, error: &ApiError) -> bool {
    matches!(method, "editMessageText" | "editMessageReplyMarkup")
        && matches!(error, ApiError::Telegram { description, retry_after_secs: None }
        if description.contains("message is not modified"))
}

/// An expired or already removed cleanup target is not a bot connectivity failure.
pub fn is_unavailable_delete(method: &str, error: &ApiError) -> bool {
    method == "deleteMessage"
        && matches!(error, ApiError::Telegram { description, retry_after_secs: None }
            if description == "Bad Request: message to delete not found"
                || description == "Bad Request: message can't be deleted")
}

/// A 400 whose description will not change on retry: bad HTML, an over-long body, or empty text.
///
/// Rate limits, 5xx and timeouts are not payload errors. Telegram appends a byte offset after
/// "can't parse entities", so the prefix is enough. Empty text matches both "text must be
/// non-empty" and the Bot API string "message text is empty".
pub fn is_permanent_payload_error(error: &ApiError) -> bool {
    matches!(error, ApiError::Telegram { description, retry_after_secs: None }
        if description.starts_with("Bad Request: can't parse entities")
            || description.contains("message is too long")
            || description.contains("text must be non-empty")
            || description.contains("message text is empty"))
}

/// A Bad Request 400 with no `retry_after`. Sending the same body again will not succeed.
///
/// Timeouts, transport errors, 5xx, and 429 stay retryable. Forbidden stays on
/// [`is_unreachable_chat`]. [`is_permanent_payload_error`] stays the narrow HTML, length, and
/// empty-text check. The sender drops on this wider predicate.
///
/// Args:
///     error: Failure from one Bot API call.
///
/// Returns:
///     `true` when `error` is a Telegram error whose description starts with `Bad Request:`
///     and `retry_after_secs` is `None`.
pub fn is_permanent_bad_request(error: &ApiError) -> bool {
    matches!(error, ApiError::Telegram { description, retry_after_secs: None }
        if description.starts_with("Bad Request:"))
}

/// Telegram refused a call because this one chat is out of the bot's reach — its user is gone,
/// deactivated or blocked the bot. The bot itself is fine; retrying changes nothing until the user
/// acts.
pub fn is_unreachable_chat(error: &ApiError) -> bool {
    matches!(error, ApiError::Telegram { description, retry_after_secs: None }
        if description.starts_with("Forbidden:")
            || description == "Bad Request: user not found"
            || description == "Bad Request: chat not found")
}

impl InlineKeyboardButton {
    /// A bounded, application-owned report navigation action.
    pub fn callback(text: impl Into<String>, data: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            web_app: None,
            callback_data: Some(data.into()),
        }
    }
    /// Mini App button. The URL is the current public tunnel, not a BotFather menu URL.
    pub fn web_app(text: impl Into<String>, url: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            web_app: Some(WebAppInfo { url: url.into() }),
            callback_data: None,
        }
    }
}

impl InlineKeyboardMarkup {
    /// One-column keyboard from the given buttons.
    pub fn from_rows(rows: Vec<Vec<InlineKeyboardButton>>) -> Self {
        Self {
            inline_keyboard: rows,
        }
    }
}

#[cfg(test)]
mod tests;
