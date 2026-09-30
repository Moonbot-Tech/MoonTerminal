//! What the Telegram code needs from the process that runs it.
//!
//! The terminal implements this over its `Backend` on the UI thread; the station over its own
//! session and `station.toml`. Everything is called from ONE owner thread — the one that calls
//! [`crate::tick`] — so no method needs to be thread-safe; the only work that leaves that thread
//! is a [`Job`], and its result comes back through [`Finish`] on the same owner thread.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::SyncSender;

use chrono_tz::Tz;
use moon_core::config::AppConfig;
use moon_core::session::{CoreId, SessionManager};
use moon_core::telegram::runtime::Response;

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

/// The revision of everything a Mini App report or trades read reads besides its request, the
/// model Analytics refreshes on: a commit of the report replica or of the valuation cache moves it,
/// and so does any change of whether readers attach the valuation cache or of the process state
/// report money is built from (`moon_core::db::read_state_revision`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ReportRevision {
    reports: u64,
    valuation: u64,
    attach: u64,
    state: u64,
}

impl ReportRevision {
    /// The revision now.
    ///
    /// Args:
    ///     reports: The report writer's generation (`ReportsHandle::generation`).
    ///     valuation: The valuation worker's generation (`ValuationHandle::generation`), absent
    ///         where no worker runs and so nothing writes the cache.
    ///
    /// Returns:
    ///     `None` while readers do not attach the valuation cache, or while a read now might not
    ///     read what an earlier one did (`moon_core::db::read_state_revision`): a read then
    ///     differs from a later one with no generation to say so.
    pub fn current(reports: &AtomicU64, valuation: Option<&AtomicU64>) -> Option<Self> {
        let attach = moon_core::db::valuation::attach_epoch()?;
        let state = moon_core::db::read_state_revision()?;
        Some(Self {
            reports: reports.load(Ordering::Acquire),
            valuation: valuation.map_or(0, |generation| generation.load(Ordering::Acquire)),
            attach,
            state,
        })
    }

    /// A revision of given parts, for tests.
    #[cfg(test)]
    pub(crate) fn from_parts(reports: u64, valuation: u64, attach: u64, state: u64) -> Self {
        Self {
            reports,
            valuation,
            attach,
            state,
        }
    }
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

    /// The revision of the report data, taken before a Mini App read so a later request can tell
    /// whether reading again would answer anything new.
    ///
    /// Returns:
    ///     `None` from a host that cannot name it, whose finished reads then answer only for the
    ///     plain cache TTL.
    fn report_revision(&self) -> Option<ReportRevision> {
        None
    }

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

    /// Answer the station's status through `reply` (the chat's "Status"): the station reads it on
    /// its loop and looks for a newer release off it ([`crate::station_status_reply`]).
    ///
    /// Returns:
    ///     `false` from a host with no station behind its bot — the terminal —, which then
    ///     answers it as a request it cannot run.
    fn station_status(&mut self, reply: SyncSender<Response>) -> bool {
        let _ = reply;
        false
    }

    /// Ask the station's updater, which runs as root beside it, to install the latest release.
    /// The request names no version: the updater finds the release and checks it itself.
    ///
    /// Returns:
    ///     `None` from a host with no station — the terminal; `Err` with why the request could
    ///     not be filed.
    fn request_station_update(&mut self) -> Option<Result<(), String>> {
        None
    }
}
