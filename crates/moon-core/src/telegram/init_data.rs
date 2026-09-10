//! Telegram WebApp `initData` parsing, HMAC verification, and paired-chat authorization.
//!
//! HMAC authenticity is not MoonTerminal authorization. Callers must verify the signature
//! first, then map the signed identity onto configured paired chat ids before the Mini App
//! session check is allowed.

use std::collections::HashSet;

use hmac::{Hmac, Mac};
use serde_json::Value;
use sha2::Sha256;

type HmacSha256 = Hmac<Sha256>;

/// Maximum accepted age of a signed `auth_date`, in seconds.
pub const INIT_DATA_MAX_AGE_SECS: u64 = 3600;
/// Small clock-skew allowance for an `auth_date` slightly ahead of local Unix time.
pub const INIT_DATA_FUTURE_SKEW_SECS: u64 = 60;

/// Configurable freshness window applied to a verified `auth_date`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FreshnessBounds {
    /// Reject launches older than this many seconds.
    pub max_age_secs: u64,
    /// Reject launches this many seconds in the future.
    pub future_skew_secs: u64,
}

impl FreshnessBounds {
    /// Default Mini App launch window: one hour of age and one minute of future skew.
    pub const DEFAULT: Self = Self {
        max_age_secs: INIT_DATA_MAX_AGE_SECS,
        future_skew_secs: INIT_DATA_FUTURE_SKEW_SECS,
    };
}

impl Default for FreshnessBounds {
    /// Return the production Mini App age and future-skew limits.
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// Failure while parsing, verifying, or authorizing Telegram `initData`.
///
/// Variants never carry the bot token, the received hash, or the data-check string.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InitDataError {
    /// The query string is empty, not form-encoded, or contains an invalid percent-escape.
    Malformed,
    /// A key, including `hash`, appears more than once.
    DuplicateKey,
    /// No `hash` field was present.
    MissingHash,
    /// The hash is not 64 lowercase hex characters, or HMAC verification failed.
    InvalidHash,
    /// `auth_date` is missing.
    MissingAuthDate,
    /// `auth_date` is not a parseable Unix-seconds integer.
    InvalidAuthDate,
    /// The signed `user` object is missing.
    MissingUser,
    /// The signed `user` object is not JSON or has no integer `id`.
    InvalidUser,
    /// `auth_date` is older than the configured freshness window.
    Stale,
    /// `auth_date` is further in the future than the allowed skew.
    Future,
    /// HMAC is valid but the signed chat is not in the paired set.
    Unpaired,
}

/// Signed user facts taken from the verified `user` JSON object.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SignedUser {
    /// Telegram user id.
    pub id: i64,
    /// Display first name from the signed payload.
    pub first_name: String,
    /// Optional last name.
    pub last_name: Option<String>,
    /// Optional @username without the leading at-sign.
    pub username: Option<String>,
    /// Optional IETF language tag.
    pub language_code: Option<String>,
}

/// Signed chat facts taken from the optional verified `chat` JSON object.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SignedChat {
    /// Telegram chat id.
    pub id: i64,
    /// Telegram chat type (`private`, `group`, `supergroup`, `channel`) when present.
    pub chat_type: Option<String>,
    /// Chat title when present.
    pub title: Option<String>,
}

/// Typed facts from a HMAC-verified Telegram Mini App launch.
///
/// The hash, bot token, and raw query string are intentionally absent.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SignedInitData {
    /// Unix seconds of the WebApp launch, taken from signed `auth_date`.
    pub auth_date: u64,
    /// Signed Telegram user.
    pub user: SignedUser,
    /// Signed chat when the Mini App was opened in a group, supergroup, or channel.
    pub chat: Option<SignedChat>,
    /// Optional WebApp `query_id`.
    pub query_id: Option<String>,
}

impl SignedInitData {
    /// Chat id used for pairing: signed `chat.id` when present, otherwise the private-chat
    /// identity `user.id`.
    pub fn pairing_chat_id(&self) -> i64 {
        self.chat
            .as_ref()
            .map(|chat| chat.id)
            .unwrap_or(self.user.id)
    }
}

/// Verify Telegram WebApp `initData` with the documented two-stage HMAC-SHA256.
///
/// Duplicate keys (including duplicate `hash`) are rejected before any MAC is computed.
/// `auth_date` must be a parseable Unix timestamp inside [`FreshnessBounds::DEFAULT`].
/// The signed `user` object must be present and contain an integer `id`.
///
/// Args:
///     init_data: Raw `Telegram.WebApp.initData` query string.
///     bot_token: Bot token used as the HMAC message in the first stage.
///     now_unix: Current Unix seconds used for freshness comparison.
///
/// Returns:
///     Typed signed user and chat facts on success.
///
/// Errors:
///     Returns [`InitDataError`] without echoing the token or hash.
pub fn verify_init_data(
    init_data: &str,
    bot_token: &str,
    now_unix: u64,
) -> Result<SignedInitData, InitDataError> {
    verify_init_data_with_bounds(init_data, bot_token, now_unix, FreshnessBounds::DEFAULT)
}

/// Verify `initData` using explicit freshness bounds.
///
/// Args:
///     init_data: Raw `Telegram.WebApp.initData` query string.
///     bot_token: Bot token used as the HMAC message in the first stage.
///     now_unix: Current Unix seconds used for freshness comparison.
///     bounds: Maximum age and future-skew allowances.
///
/// Returns:
///     Typed signed user and chat facts on success.
///
/// Errors:
///     Returns [`InitDataError`] without echoing the token or hash.
pub fn verify_init_data_with_bounds(
    init_data: &str,
    bot_token: &str,
    now_unix: u64,
    bounds: FreshnessBounds,
) -> Result<SignedInitData, InitDataError> {
    let fields = decode_unique_form(init_data)?;
    let hash = fields
        .iter()
        .find(|(key, _)| key == "hash")
        .map(|(_, value)| value.as_str())
        .ok_or(InitDataError::MissingHash)?;
    let expected = decode_lowercase_hex_sha256(hash).ok_or(InitDataError::InvalidHash)?;

    let mut check_lines: Vec<(&str, &str)> = fields
        .iter()
        .filter(|(key, _)| key != "hash")
        .map(|(key, value)| (key.as_str(), value.as_str()))
        .collect();
    check_lines.sort_unstable_by(|left, right| left.0.cmp(right.0));
    let mut data_check = String::new();
    for (index, (key, value)) in check_lines.iter().enumerate() {
        if index > 0 {
            data_check.push('\n');
        }
        data_check.push_str(key);
        data_check.push('=');
        data_check.push_str(value);
    }

    let mut token_mac =
        HmacSha256::new_from_slice(b"WebAppData").map_err(|_| InitDataError::InvalidHash)?;
    token_mac.update(bot_token.as_bytes());
    let secret_key = token_mac.finalize().into_bytes();
    let mut data_mac =
        HmacSha256::new_from_slice(&secret_key).map_err(|_| InitDataError::InvalidHash)?;
    data_mac.update(data_check.as_bytes());
    data_mac
        .verify_slice(&expected)
        .map_err(|_| InitDataError::InvalidHash)?;

    let auth_date = parse_auth_date(&fields)?;
    if auth_date > now_unix.saturating_add(bounds.future_skew_secs) {
        return Err(InitDataError::Future);
    }
    if now_unix.saturating_sub(auth_date) > bounds.max_age_secs {
        return Err(InitDataError::Stale);
    }

    let user = parse_signed_user(&fields)?;
    let chat = parse_signed_chat(&fields)?;
    let query_id = fields
        .iter()
        .find(|(key, _)| key == "query_id")
        .map(|(_, value)| value.clone());
    Ok(SignedInitData {
        auth_date,
        user,
        chat,
        query_id,
    })
}

/// Authorize a HMAC-verified identity against configured paired chat ids.
///
/// Telegram authenticity is not sufficient: the signed pairing chat id must be present in
/// `authorized_chat_ids` before the Mini App session check is allowed.
///
/// Args:
///     signed: Facts returned by [`verify_init_data`].
///     authorized_chat_ids: Chat ids stored after a successful `/pair`.
///
/// Returns:
///     The paired chat id that matched.
///
/// Errors:
///     [`InitDataError::Unpaired`] when the signed chat is absent from the configured set.
pub fn authorize_paired_identity(
    signed: &SignedInitData,
    authorized_chat_ids: &[i64],
) -> Result<i64, InitDataError> {
    let chat_id = signed.pairing_chat_id();
    if authorized_chat_ids.iter().any(|id| *id == chat_id) {
        Ok(chat_id)
    } else {
        Err(InitDataError::Unpaired)
    }
}

/// Verify `initData` and authorize the signed identity in one step.
///
/// Args:
///     init_data: Raw `Telegram.WebApp.initData` query string.
///     bot_token: Bot token used for HMAC verification.
///     now_unix: Current Unix seconds.
///     authorized_chat_ids: Chat ids stored after a successful `/pair`.
///
/// Returns:
///     The verified payload and the paired chat id.
///
/// Errors:
///     HMAC, freshness, or pairing failures from [`InitDataError`].
pub fn verify_and_authorize(
    init_data: &str,
    bot_token: &str,
    now_unix: u64,
    authorized_chat_ids: &[i64],
) -> Result<(SignedInitData, i64), InitDataError> {
    let signed = verify_init_data(init_data, bot_token, now_unix)?;
    let chat_id = authorize_paired_identity(&signed, authorized_chat_ids)?;
    Ok((signed, chat_id))
}

/// Strictly form-decode `application/x-www-form-urlencoded` and reject duplicate keys.
fn decode_unique_form(input: &str) -> Result<Vec<(String, String)>, InitDataError> {
    if input.is_empty() {
        return Err(InitDataError::Malformed);
    }
    let mut seen = HashSet::new();
    let mut fields = Vec::new();
    for part in input.split('&') {
        let (raw_key, raw_value) = part.split_once('=').ok_or(InitDataError::Malformed)?;
        let key = percent_decode_form(raw_key)?;
        if key.is_empty() {
            return Err(InitDataError::Malformed);
        }
        if !seen.insert(key.clone()) {
            return Err(InitDataError::DuplicateKey);
        }
        let value = percent_decode_form(raw_value)?;
        fields.push((key, value));
    }
    Ok(fields)
}

/// Percent-decode a form component, treating `+` as space and rejecting truncated escapes.
fn percent_decode_form(input: &str) -> Result<String, InitDataError> {
    let mut bytes = Vec::with_capacity(input.len());
    let raw = input.as_bytes();
    let mut index = 0;
    while index < raw.len() {
        match raw[index] {
            b'+' => {
                bytes.push(b' ');
                index += 1;
            }
            b'%' => {
                if index + 2 >= raw.len() {
                    return Err(InitDataError::Malformed);
                }
                let hi = from_hex_digit(raw[index + 1]).ok_or(InitDataError::Malformed)?;
                let lo = from_hex_digit(raw[index + 2]).ok_or(InitDataError::Malformed)?;
                bytes.push((hi << 4) | lo);
                index += 3;
            }
            byte => {
                bytes.push(byte);
                index += 1;
            }
        }
    }
    String::from_utf8(bytes).map_err(|_| InitDataError::Malformed)
}

/// Parse one ASCII hex digit.
fn from_hex_digit(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

/// Decode a 64-character lowercase hex SHA-256 digest.
fn decode_lowercase_hex_sha256(value: &str) -> Option<[u8; 32]> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
    {
        return None;
    }
    let mut digest = [0u8; 32];
    let bytes = value.as_bytes();
    for (index, slot) in digest.iter_mut().enumerate() {
        let hi = from_hex_digit(bytes[index * 2])?;
        let lo = from_hex_digit(bytes[index * 2 + 1])?;
        *slot = (hi << 4) | lo;
    }
    Some(digest)
}

/// Require a parseable Unix-seconds `auth_date`.
fn parse_auth_date(fields: &[(String, String)]) -> Result<u64, InitDataError> {
    let value = fields
        .iter()
        .find(|(key, _)| key == "auth_date")
        .map(|(_, value)| value.as_str())
        .ok_or(InitDataError::MissingAuthDate)?;
    if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(InitDataError::InvalidAuthDate);
    }
    value.parse().map_err(|_| InitDataError::InvalidAuthDate)
}

/// Require a signed `user` JSON object with an integer `id`.
fn parse_signed_user(fields: &[(String, String)]) -> Result<SignedUser, InitDataError> {
    let raw = fields
        .iter()
        .find(|(key, _)| key == "user")
        .map(|(_, value)| value.as_str())
        .ok_or(InitDataError::MissingUser)?;
    let value: Value = serde_json::from_str(raw).map_err(|_| InitDataError::InvalidUser)?;
    let object = value.as_object().ok_or(InitDataError::InvalidUser)?;
    let id = json_i64(object.get("id")).ok_or(InitDataError::InvalidUser)?;
    let first_name = object
        .get("first_name")
        .and_then(Value::as_str)
        .ok_or(InitDataError::InvalidUser)?
        .to_owned();
    Ok(SignedUser {
        id,
        first_name,
        last_name: object
            .get("last_name")
            .and_then(Value::as_str)
            .map(str::to_owned),
        username: object
            .get("username")
            .and_then(Value::as_str)
            .map(str::to_owned),
        language_code: object
            .get("language_code")
            .and_then(Value::as_str)
            .map(str::to_owned),
    })
}

/// Parse optional signed `chat` JSON. Absent is `Ok(None)`; malformed is an error.
fn parse_signed_chat(fields: &[(String, String)]) -> Result<Option<SignedChat>, InitDataError> {
    let Some(raw) = fields
        .iter()
        .find(|(key, _)| key == "chat")
        .map(|(_, value)| value.as_str())
    else {
        return Ok(None);
    };
    let value: Value = serde_json::from_str(raw).map_err(|_| InitDataError::InvalidUser)?;
    let object = value.as_object().ok_or(InitDataError::InvalidUser)?;
    let id = json_i64(object.get("id")).ok_or(InitDataError::InvalidUser)?;
    Ok(Some(SignedChat {
        id,
        chat_type: object
            .get("type")
            .and_then(Value::as_str)
            .map(str::to_owned),
        title: object
            .get("title")
            .and_then(Value::as_str)
            .map(str::to_owned),
    }))
}

/// Read a JSON number as i64, rejecting non-integers.
fn json_i64(value: Option<&Value>) -> Option<i64> {
    match value? {
        Value::Number(number) => number.as_i64(),
        _ => None,
    }
}

#[cfg(test)]
mod tests;
