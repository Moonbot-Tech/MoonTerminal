//! Telegram bot token, pairing boundary, polling thread, and Mini App pipeline — UI-agnostic.
//!
//! Blocking I/O and protocol/security/domain code live here. The GPUI `Backend` talks to this
//! crate only through the typed bounded channels owned by [`runtime::TelegramService`]. An empty
//! bot token is the hard off switch: [`TelegramService::start`] returns `None` before any
//! channel, thread, path, listener, or helper process is created.
//!
//! Transport requests flow toward Backend; localized responses return on per-request channels.

pub mod api;
pub mod auth;
pub mod cloudflared;
pub mod commands;
pub mod init_data;
pub mod reply;
pub mod runtime;
pub mod tunnel;
pub mod web;

/// Snapshot of Telegram runtime health for Settings and typed replies.
///
/// Variants carry no secret and no Bot API URL. `Unavailable` represents a
/// configured token whose transport is currently unavailable.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TelegramStatus {
    /// No token is configured; no worker exists.
    Disabled,
    /// Token is present and the blocking transport is coming up.
    Starting,
    /// Shutdown has been requested and background workers are being joined.
    Stopping,
    /// Worker is running and no chat is paired yet.
    Unpaired,
    /// At least one chat is authorized.
    Paired {
        /// Number of authorized chat ids.
        chat_count: usize,
    },
    /// Telegram asked the client to wait before the next call.
    RateLimited {
        /// Seconds to wait before retrying.
        retry_after_secs: u32,
    },
    /// Helper or transport is down; bot commands may still be available later.
    Unavailable,
    /// Worker was stopped after a present-to-empty token transition.
    Stopped,
}

pub use runtime::TelegramService;
