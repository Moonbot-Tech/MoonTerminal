//! Command parsing for the Bot API surface.
//!
//! The parser recognizes `/pair <code>` and `/miniapp` only. Command suffixes
//! (`/miniapp@botname`) are accepted only for the configured bot username from `getMe`.

use super::api::{Message, Update};

/// Parsed inbound command, localization-free.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ParsedCommand {
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
///     update: Decoded `message`.
///     bot_username: Username from `getMe`, without a leading `@`. When `None`, any `@suffix` is
///         rejected so a command aimed at another bot cannot run here.
///
/// Returns:
///     `None` when the update has no text.
pub fn parse_update(update: &Update, bot_username: Option<&str>) -> Option<Inbound> {
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
        "miniapp" => require_no_args(args, ParsedCommand::MiniApp),
        "pair" => parse_pair(args),
        _ => ParsedCommand::Unknown,
    }
}

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
