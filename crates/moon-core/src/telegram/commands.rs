//! Command parsing for the Bot API surface.
//!
//! Pairing, navigation, bounded report periods, and report callbacks share typed commands.
//! Command suffixes are accepted only for the configured bot username from `getMe`.

use super::api::{Message, Update};
use super::report::{Period, ReportRequest};

/// Parsed inbound command, localization-free.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ParsedCommand {
    /// Read a page of closed real trades from the terminal's local report replica.
    Report(ReportRequest),
    /// Show the paired chat's welcome message and available next action.
    Start,
    /// Explain navigation without requiring the user to remember `/miniapp`.
    Help,
    MiniApp,
    Pair {
        code: String,
    },
    /// Not a known command. Unpaired chats must still see no hint of a terminal behind the bot.
    Unknown,
    /// Known command with an unusable argument (`/pair` with no code).
    InvalidArgument,
}

/// Chat-scoped parse result.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Inbound {
    pub chat_id: i64,
    pub command: ParsedCommand,
}

/// Parse a Bot API update into at most one inbound command.
///
/// Args:
///     update: Decoded message or private sender-matched report callback.
///     bot_username: Username from `getMe`, without a leading `@`. When `None`, any `@suffix` is
///         rejected so a command aimed at another bot cannot run here.
///
/// Returns:
///     `None` when no text exists or callback identity cannot be established.
pub fn parse_update(update: &Update, bot_username: Option<&str>) -> Option<Inbound> {
    if let Some(callback) = &update.callback_query {
        let message = callback.message.as_ref()?;
        if message.chat.kind != "private" || message.chat.id != callback.from.id {
            return None;
        }
        return Some(Inbound {
            chat_id: message.chat.id,
            command: callback
                .data
                .as_deref()
                .and_then(ReportRequest::parse_callback)
                .map(ParsedCommand::Report)
                .unwrap_or(ParsedCommand::Unknown),
        });
    }
    update
        .message
        .as_ref()
        .and_then(|message| parse_message(message, bot_username))
}

/// Parse a text message.
///
/// Args:
///     message: Incoming message.
///     bot_username: Configured bot username, if `getMe` has succeeded.
///
/// Returns:
///     `None` when the message has no text.
pub fn parse_message(message: &Message, bot_username: Option<&str>) -> Option<Inbound> {
    let text = message.text.as_deref()?;
    Some(Inbound {
        chat_id: message.chat.id,
        command: parse_text(text, bot_username),
    })
}

/// Parse `/command@bot args` text.
///
/// Args:
///     text: Raw message text.
///     bot_username: Accepted suffix, case-insensitive, without `@`.
///
/// Returns:
///     A typed command; anything else is [`ParsedCommand::Unknown`].
pub fn parse_text(text: &str, bot_username: Option<&str>) -> ParsedCommand {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return ParsedCommand::Unknown;
    }
    let Some(rest) = trimmed.strip_prefix('/') else {
        return ParsedCommand::Unknown;
    };
    let (head, args) = split_head_args(rest);
    if head.is_empty() {
        return ParsedCommand::Unknown;
    }
    let (name, suffix) = split_name_suffix(head);
    if let Some(suffix) = suffix {
        match bot_username {
            Some(expected) if suffix.eq_ignore_ascii_case(expected) => {}
            _ => return ParsedCommand::Unknown,
        }
    }
    let name = name.to_ascii_lowercase();
    match name.as_str() {
        "today" | "day" | "report" if args.is_empty() => {
            ParsedCommand::Report(ReportRequest::new(Period::Today, false))
        }
        "hour" => require_no_args(
            args,
            ParsedCommand::Report(ReportRequest::new(Period::Hour, false)),
        ),
        "yesterday" => require_no_args(
            args,
            ParsedCommand::Report(ReportRequest::new(Period::Yesterday, false)),
        ),
        "month" => require_no_args(
            args,
            ParsedCommand::Report(ReportRequest::new(Period::Month, false)),
        ),
        "lastmonth" => require_no_args(
            args,
            ParsedCommand::Report(ReportRequest::new(Period::LastMonth, false)),
        ),
        "daily" if args.is_empty() => {
            ParsedCommand::Report(ReportRequest::new(Period::Month, true))
        }
        "report" | "daily" => {
            let dates: Vec<_> = args.split_whitespace().collect();
            if dates.len() != 2 {
                return ParsedCommand::InvalidArgument;
            }
            match ReportRequest::dates(dates[0], dates[1]) {
                Some(mut request) => {
                    request.daily = name == "daily";
                    request.by_exchange = !request.daily;
                    ParsedCommand::Report(request)
                }
                None => ParsedCommand::InvalidArgument,
            }
        }
        "start" => require_no_args(args, ParsedCommand::Start),
        "help" => require_no_args(args, ParsedCommand::Help),
        "miniapp" => require_no_args(args, ParsedCommand::MiniApp),
        "pair" => parse_pair(args),
        _ => ParsedCommand::Unknown,
    }
}

/// Resolve exact application-localized reply labels in every supported language.
///
/// This is parsing only: the runtime still checks private-chat identity and authorization.
/// Slash commands never become button clicks, even if a supplied label resembles a command.
pub fn parse_reply_button(
    text: &str,
    labels: &std::collections::BTreeMap<String, String>,
) -> ParsedCommand {
    let text = text.trim();
    if text.is_empty() || text.starts_with('/') {
        return ParsedCommand::Unknown;
    }
    for locale in ["ru", "en", "es"] {
        for (name, command) in [
            ("miniapp", ParsedCommand::MiniApp),
            ("help", ParsedCommand::Help),
            ("home", ParsedCommand::Start),
            (
                "today",
                ParsedCommand::Report(ReportRequest::new(Period::Today, false)),
            ),
            (
                "yesterday",
                ParsedCommand::Report(ReportRequest::new(Period::Yesterday, false)),
            ),
            (
                "month",
                ParsedCommand::Report(ReportRequest::new(Period::Month, false)),
            ),
            (
                "lastmonth",
                ParsedCommand::Report(ReportRequest::new(Period::LastMonth, false)),
            ),
            (
                "daily",
                ParsedCommand::Report(ReportRequest::new(Period::Month, true)),
            ),
        ] {
            if [
                format!("button_{name}_{locale}"),
                format!("button_{name}_emoji_{locale}"),
                format!("button_{name}_legacy_emoji_{locale}"),
            ]
            .iter()
            .any(|key| labels.get(key).is_some_and(|label| label == text))
            {
                return command;
            }
        }
    }
    ParsedCommand::Unknown
}

#[cfg(test)]
mod tests;

/// Split a command remainder into its first token and trimmed trailing arguments.
fn split_head_args(rest: &str) -> (&str, &str) {
    match rest.split_once(char::is_whitespace) {
        Some((head, args)) => (head, args.trim()),
        None => (rest, ""),
    }
}

/// Split an optional Telegram bot-username suffix from a command head.
fn split_name_suffix(head: &str) -> (&str, Option<&str>) {
    match head.split_once('@') {
        Some((name, suffix)) => (name, Some(suffix)),
        None => (head, None),
    }
}

/// Preserve `command` only when no trailing arguments were supplied.
fn require_no_args(args: &str, command: ParsedCommand) -> ParsedCommand {
    if args.is_empty() {
        command
    } else {
        ParsedCommand::InvalidArgument
    }
}

/// Parse the required single pairing code argument.
fn parse_pair(args: &str) -> ParsedCommand {
    if args.is_empty() || args.split_whitespace().nth(1).is_some() {
        return ParsedCommand::InvalidArgument;
    }
    ParsedCommand::Pair {
        code: args.to_string(),
    }
}
