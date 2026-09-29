//! What the Telegram code needs from the process that runs it.
//!
//! The terminal implements this over its `Backend` on the UI thread; the station over its own
//! session and `station.toml`. Everything is called from ONE owner thread — the one that calls
//! [`crate::tick`] — so no method needs to be thread-safe; the only work that leaves that thread
//! is a [`Job`], and its result comes back through [`Finish`] on the same owner thread.

use chrono_tz::Tz;
use moon_core::config::AppConfig;
use moon_core::session::{CoreId, SessionManager};

use crate::TelegramState;

/// A result applied back on the owner thread, with the host as it is by then.
pub type Finish = Box<dyn FnOnce(&mut dyn TgHost) + Send>;

/// A blocking read run off the owner thread; its result comes back as a [`Finish`].
pub type Job = Box<dyn FnOnce() -> Finish + Send>;

/// Which process runs the bot: what a chat is told the bot depends on differs between them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HostKind {
    /// The desktop terminal: the bot answers while it runs.
    Terminal,
    /// The station: the bot answers around the clock.
    Station,
}

/// The host of the bot and the Mini App.
pub trait TgHost {
    /// Which process this is.
    fn kind(&self) -> HostKind;

    /// Saved configuration: the cores (`servers`), their order (`core_sort`) and `telegram`.
    fn config(&self) -> &AppConfig;

    /// The live core sessions.
    fn session(&self) -> &SessionManager;

    /// The live core sessions, for the Mini App's owner commands.
    fn session_mut(&mut self) -> &mut SessionManager;

    /// The bot's process-only state.
    fn state(&self) -> &TelegramState;

    /// The bot's process-only state, for changes.
    fn state_mut(&mut self) -> &mut TelegramState;

    /// Zone every report and Mini App time is shown in.
    fn report_zone(&self) -> Tz;

    /// Persist `chat_id` as paired and adopt the saved configuration.
    ///
    /// Returns:
    ///     `false` when the save failed; the configuration is then unchanged.
    fn save_paired_chat(&mut self, chat_id: i64) -> bool;

    /// Persist an empty pairing — no chats, no owner, no grants — and adopt it.
    ///
    /// Returns:
    ///     `false` when the save failed; the configuration is then unchanged.
    fn save_cleared_pairing(&mut self) -> bool;

    /// Whether Panic Sell is armed for `(core, market)`, a fresh local toggle included.
    fn is_panic_armed(&self, core: CoreId, market: &str) -> bool;

    /// Toggle Panic Sell for a market.
    ///
    /// Returns:
    ///     Whether the command was accepted, not the resulting armed state.
    fn toggle_panic_sell(&mut self, core: CoreId, market: String) -> bool;

    /// Queue a core for the host's reconnect path; the queue is drained by the host.
    fn request_reconnect(&mut self, core: CoreId);

    /// Run `job` off the owner thread and apply its [`Finish`] back on it.
    ///
    /// Every `Finish` must be applied while this state lives: it clears the busy flag its read
    /// set, and a dropped one leaves that read `Busy` for good. A host may drop it only together
    /// with the state itself.
    fn spawn(&mut self, job: Job);

    /// Tell the host its Telegram state changed, so whatever shows it redraws.
    fn repaint(&mut self);
}
