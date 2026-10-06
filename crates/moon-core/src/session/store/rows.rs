//! Retained row models and per-core capacity limits.

use super::*;

/// Maximum number of recent detects retained in memory for each core.
pub(super) const MAX_DETECTS: usize = 2000;

/// Telegram events retained per core for the bot, which drains them every few seconds.
pub(super) const MAX_TG_EVENTS: usize = 256;

/// Entries remembered per core against announcing one trade twice.
pub(super) const MAX_TG_OPENED: usize = 512;

/// One [`CoreTgEvent`] as the bot reads it: stamped and numbered on arrival.
#[derive(Debug, Clone, PartialEq)]
pub struct TgEventRow {
    /// Per-core number, rising; the bot keeps its own cursor on it.
    pub seq: u64,
    /// When it happened, true UTC milliseconds: the entry stamp of an opened trade, the arrival of
    /// a detect.
    pub at_utc_ms: i64,
    pub event: CoreTgEvent,
}

/// Maximum number of recent server-log lines retained per core for live viewing and search.
/// Older history remains in `logs/<date>_<core>.log` files.
pub(super) const MAX_LOG: usize = 5000;

/// Maximum number of undelivered Engine action toasts queued while no window is active.
/// The active window's shell consumes the queue.
pub(super) const MAX_ENGINE_ACTIONS: usize = 64;
/// One filed answer about a report row's archived traces; see `CoreData::report_traces`.
#[derive(Debug, Clone, PartialEq)]
pub struct ReportTracesEntry {
    /// `CoreData::report_traces_rev` at the moment this answer was filed.
    pub rev: u64,
    pub outcome: crate::feed::ReportTracesOutcome,
}

/// Cap on retained archived-trace answers per core.
///
/// The map is a mailbox: the UI's trace resolver takes what it is waiting for on every feed drain,
/// and the feed sends at most eight requests unanswered at once (`live::trace_backfill`), so far
/// fewer than this can land between two drains — an answer can only be evicted long after it was
/// read. The cap exists so a long session's backfill answers, which nothing in the UI is waiting
/// for, cannot grow this map without bound; an entry is a few dozen bytes plus the lines, which a
/// drawing surface keeps alive by its own `Arc` regardless.
pub(super) const MAX_REPORT_TRACES: usize = 256;

pub type CoreId = u64;

/// The store's best available trust classification for a core's USD balance figures.
///
/// The classification lives here, next to the inputs it reads, because the raw numbers
/// alone cannot be rendered honestly: missing pricing can produce a finite zero or partial sum,
/// and a retained snapshot survives a reconnect. Every consumer of `assets.global` must agree
/// about that, so they all go through [`CoreData::balance_state`] instead of re-deriving the rule
/// from `status`/`assets_rev`/`usd_rate_known` on their own.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BalanceState {
    /// A snapshot exists, the connection is ready, no stale marker remains, and the USD
    /// valuation is valid. See [`CoreData::assets_stale`] for the freshness limit.
    Live,
    /// The connection is not ready, or it became ready but still awaits a fresh snapshot.
    /// Retained figures may be shown only with an explicit stale marker.
    Stale,
    /// No snapshot has arrived: the balance is UNKNOWN, not zero.
    Awaiting,
    /// A snapshot exists but its free/total USD valuation is incomplete or non-finite.
    /// The figures must render as unavailable rather than as a zero or partial balance.
    Unpriced,
}

impl BalanceState {
    /// Whether there is a usable number to render and to sum.
    pub fn has_value(self) -> bool {
        matches!(self, BalanceState::Live | BalanceState::Stale)
    }

    /// Whether the store classifies the number as current enough to show without a stale marker.
    ///
    /// This is the companion to [`Self::has_value`]: one asks whether there is a figure, the
    /// other whether the available freshness signals classify it as live. The known limit on
    /// [`CoreData::assets_stale`] still applies.
    pub fn is_current(self) -> bool {
        matches!(self, BalanceState::Live)
    }

    /// Stable small integer for hashing this state into a render signature.
    ///
    /// Exists so consumers do not invent their own numbering: the exhaustive match keeps a new
    /// variant a compile error here rather than a silently unhashed state somewhere downstream.
    pub fn code(self) -> u64 {
        match self {
            BalanceState::Live => 1,
            BalanceState::Stale => 2,
            BalanceState::Awaiting => 3,
            BalanceState::Unpriced => 4,
        }
    }
}
