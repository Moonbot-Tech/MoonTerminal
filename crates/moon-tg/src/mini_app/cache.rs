//! When a finished Mini App report or trades read may answer again without reading.
//!
//! A finished read answers the same chat and grant for [`REPORT_CACHE_TTL`], as it always has. After
//! that it answers again only while every input the read was computed from is unchanged — the
//! data's revision ([`ReportRevision`]), the period's window, the zone, the language and the cores'
//! names, exchanges and order — because a read of the same inputs returns the same answer; the
//! answer is then built again from the rows as read, as a new read would build it. A host that
//! cannot name the revision ([`TgHost::report_revision`] is `None`) keeps the plain TTL.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use chrono::NaiveDate;
use chrono_tz::Tz;
use moon_core::config::telegram_access::TelegramReportAccess;
use moon_core::db::CoreNames;
use moon_core::session::core_order::CoreOrder;
use moon_core::telegram::web::dto::{ReportDto, ReportPeriodDto, TradesDto};
use moon_core::venue::CoreVenue;

#[cfg(doc)]
use crate::TgHost;
use crate::host::ReportRevision;
use crate::report::{MiniReport, MiniTrade};

/// How long a finished read answers the same chat without its inputs being compared.
pub(super) const REPORT_CACHE_TTL: Duration = Duration::from_secs(15);

/// The rows a report window selects, as a key two reads can be compared on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Window {
    /// A window with both ends fixed: Yesterday, Last month.
    Fixed { from: i64, to: i64 },
    /// A window ending now (Today, Month) on the local day `day`. Its end moves on every request,
    /// but a later end selects no other row while none lies past the earlier one — which is why
    /// a read that found one is never filed under this key.
    ToNow { from: i64, day: NaiveDate },
}

impl Window {
    /// The key of the window `[from, to]` requested at `now`.
    ///
    /// Args:
    ///     from: Inclusive window start, UTC seconds.
    ///     to: Inclusive window end, UTC seconds.
    ///     now: The instant the bounds were resolved at.
    ///     zone: The report zone the day is read in.
    ///
    /// Returns:
    ///     `ToNow` for a window that ends at `now`, `Fixed` otherwise; `None` when `to` has no
    ///     civil date, where no read succeeds either.
    pub(super) fn of(from: i64, to: i64, now: i64, zone: Tz) -> Option<Self> {
        if to != now {
            return Some(Self::Fixed { from, to });
        }
        let day = moon_core::util::display_time::date(to, zone)?;
        Some(Self::ToNow { from, day })
    }

    /// Whether this window ends at the request's instant.
    pub(super) fn ends_now(self) -> bool {
        matches!(self, Self::ToNow { .. })
    }

    /// Whether a read of this window may be filed under it.
    ///
    /// Args:
    ///     rows_after_to: Whether the read found a closed row past its end; `None` when it did
    ///         not find out.
    pub(super) fn holds(self, rows_after_to: Option<bool>) -> bool {
        match self {
            Self::Fixed { .. } => true,
            Self::ToNow { .. } => rows_after_to == Some(false),
        }
    }
}

/// Everything a report read is computed from besides its chat, period and grant.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ReportInputs {
    pub(super) revision: ReportRevision,
    pub(super) window: Window,
    pub(super) zone: Tz,
    /// The process locale the exchange sections are captioned in.
    pub(super) locale: String,
    pub(super) names: CoreNames,
    pub(super) venues: HashMap<u64, CoreVenue>,
    pub(super) order: CoreOrder,
}

/// Everything a trades read is computed from besides its chat and grant.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct TradesInputs {
    pub(super) revision: ReportRevision,
    pub(super) zone: Tz,
    pub(super) names: CoreNames,
}

/// The last finished Mini App report.
pub(crate) struct CachedReport {
    pub(super) chat: i64,
    pub(super) period: ReportPeriodDto,
    pub(super) access: TelegramReportAccess,
    pub(super) at: Instant,
    /// What it was read from, or `None` when it may only answer within the TTL.
    pub(super) inputs: Option<ReportInputs>,
    /// The report as read, built into an answer again past the TTL with the window's current end.
    pub(super) report: MiniReport,
    /// The answer as built when the read finished, served within the TTL.
    pub(super) dto: ReportDto,
}

/// The last finished Mini App trades read.
pub(crate) struct CachedTrades {
    pub(super) chat: i64,
    pub(super) access: TelegramReportAccess,
    pub(super) at: Instant,
    /// What it was read from, or `None` when it may only answer within the TTL.
    pub(super) inputs: Option<TradesInputs>,
    /// The rows as read. The page's own fields — strategy names, exchanges, "today" — come from
    /// live state and are built again for an answer past the TTL, as a new read would build them.
    pub(super) trades: Vec<MiniTrade>,
    /// The answer as built when the read finished, served within the TTL.
    pub(super) dto: TradesDto,
}

/// What to do with a cached read for one request.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Reuse {
    /// Answer from the cache as it was built: within the TTL.
    Serve,
    /// Answer from the rows as read, built again: past the TTL with every input unchanged.
    Rebuild,
    /// Forget the cache — it belongs to another grant — and read.
    Drop,
    /// Read, keeping the cache until the read replaces it.
    Read,
}

/// Decide whether a cached read answers this request.
///
/// Args:
///     same_request: Whether the cache is this chat's, for this period.
///     age: How long ago the cached read finished.
///     same_grant: Whether the cache was admitted under the chat's current grant.
///     stored: The inputs the cached read was computed from.
///     current: The inputs a read now would be computed from.
///
/// Returns:
///     `Drop` under another grant; `Serve` within [`REPORT_CACHE_TTL`]; `Rebuild` later while both
///     inputs are known and equal; `Read` otherwise.
pub(super) fn reuse<I: PartialEq>(
    same_request: bool,
    age: Duration,
    same_grant: bool,
    stored: Option<&I>,
    current: Option<&I>,
) -> Reuse {
    if !same_request {
        return Reuse::Read;
    }
    if !same_grant {
        return Reuse::Drop;
    }
    if age < REPORT_CACHE_TTL {
        return Reuse::Serve;
    }
    match (stored, current) {
        (Some(stored), Some(current)) if stored == current => Reuse::Rebuild,
        _ => Reuse::Read,
    }
}

#[cfg(test)]
mod tests;
