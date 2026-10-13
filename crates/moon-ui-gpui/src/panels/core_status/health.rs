//! Health-strip counts for the Core Status panel.
//!
//! The four chip filters and the headline counts share [`row_matches`], so a chip and the
//! verdict cannot disagree about which cores need attention. Alarm starts are a separate
//! 24-hour window over episode timestamps; they do not change the tone.

use moon_core::feed::ConnStatus;

use super::model::CoreStatusRow;
use super::presentation::{LoadLevel, status_level};

/// Whole day, in milliseconds. The alarm window is open on the left: a start exactly this far
/// before `now` is already outside it.
const DAY_MS: i64 = 24 * 60 * 60 * 1000;

/// One health-strip chip.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum HealthFilter {
    /// Failed, dropped, or still carrying a fault from an attempt that has not reached Ready.
    Offline,
    /// Sustained above-baseline client-to-core ping.
    Ping,
    /// Reported build is behind the newest build in the fleet.
    Version,
    /// API key inside the warning horizon or the visual notice horizon.
    ApiExpiry,
}

/// Chip filters, in strip order. [`verdict`] counts a core once however many of these it hits.
const FILTERS: [HealthFilter; 4] = [
    HealthFilter::Offline,
    HealthFilter::Ping,
    HealthFilter::Version,
    HealthFilter::ApiExpiry,
];

/// Whether one core belongs to a health chip.
///
/// Offline is the alarm colour from [`status_level`] — [`LoadLevel::Critical`], which is
/// `Failed` and `Disconnected` — or a retained fault on a core that is not [`ConnStatus::Ready`].
/// The store keeps that fault across the backoff retry and clears it only at Ready, so a Ready
/// core with a stale fault is not offline. Ping reads `ping_warn`. Version reads
/// `version_behind`. API expiry reads `api_warn` or `api_notice`.
///
/// Args:
///     row: One visible core snapshot.
///     filter: The chip being counted or toggled.
///
/// Returns:
///     Whether the core is a member of that chip.
pub(super) fn row_matches(row: &CoreStatusRow, filter: HealthFilter) -> bool {
    match filter {
        HealthFilter::Offline => {
            status_level(&row.status) == LoadLevel::Critical
                || (row.fault.is_some() && !matches!(row.status, ConnStatus::Ready))
        }
        HealthFilter::Ping => row.ping_warn,
        HealthFilter::Version => row.version_behind.is_some(),
        HealthFilter::ApiExpiry => row.api_warn || row.api_notice,
    }
}

/// Headline counts for the health strip.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(super) struct HealthCounts {
    /// Cores matching [`HealthFilter::Offline`].
    pub(super) offline_failed: usize,
    /// Cores matching [`HealthFilter::Ping`].
    pub(super) ping_high: usize,
    /// Cores matching [`HealthFilter::Version`].
    pub(super) version_behind: usize,
    /// Cores matching [`HealthFilter::ApiExpiry`].
    pub(super) api_expiring: usize,
    /// Episode starts inside `(now - 24h, now]`.
    pub(super) alarms_24h: usize,
}

/// Count chip members and the alarm starts that fall inside the last 24 hours.
///
/// Each chip count is the number of rows for which [`row_matches`] is true. An episode start
/// counts when it is strictly after `now_ms - 24h` and at or before `now_ms`.
///
/// Args:
///     rows: Visible core snapshots.
///     episode_starts_ms: Unix-millisecond start of each alarm episode, in any order.
///     now_ms: Current Unix milliseconds.
///
/// Returns:
///     The five counts. A chip with no members is `0`, not omitted.
pub(super) fn health_counts(
    rows: &[CoreStatusRow],
    episode_starts_ms: &[i64],
    now_ms: i64,
) -> HealthCounts {
    let mut counts = HealthCounts::default();
    for row in rows {
        if row_matches(row, HealthFilter::Offline) {
            counts.offline_failed += 1;
        }
        if row_matches(row, HealthFilter::Ping) {
            counts.ping_high += 1;
        }
        if row_matches(row, HealthFilter::Version) {
            counts.version_behind += 1;
        }
        if row_matches(row, HealthFilter::ApiExpiry) {
            counts.api_expiring += 1;
        }
    }
    let oldest_excluded = now_ms - DAY_MS;
    counts.alarms_24h = episode_starts_ms
        .iter()
        .filter(|&&start| start > oldest_excluded && start <= now_ms)
        .count();
    counts
}

/// Colour of the health-strip verdict.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Tone {
    /// At least one core is offline or failed.
    Bad,
    /// Nobody is offline, but at least one core matches a chip.
    Warn,
    /// No chip matches.
    Ok,
}

/// The sentence at the left of the health strip.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Verdict {
    /// [`Tone::Bad`] when any core is offline, else [`Tone::Warn`] when any core needs
    /// attention, else [`Tone::Ok`].
    pub(super) tone: Tone,
    /// Distinct cores matching at least one chip.
    pub(super) attention: usize,
    /// Visible cores, including ones that need nothing.
    pub(super) total: usize,
    /// Cores whose status is [`ConnStatus::Ready`].
    pub(super) online: usize,
    /// Server count supplied by the caller. This function does not derive it from the rows.
    pub(super) servers: usize,
}

/// Summarise the strip from the rows and the chip counts.
///
/// Attention walks the rows through [`row_matches`], so a core on two chips counts once, and
/// also counts every core that is not Ready. Tone is Bad when anyone is offline,
/// Ok only when every core is online and nothing needs attention, and Warn otherwise.
/// `alarms_24h` does not affect the tone. Online is Ready only.
///
/// Args:
///     rows: Visible core snapshots.
///     counts: Chip counts for these rows. Offline tone trusts this, not a second walk.
///     servers: How many servers the panel is showing.
///
/// Returns:
///     The verdict line.
pub(super) fn verdict(rows: &[CoreStatusRow], counts: &HealthCounts, servers: usize) -> Verdict {
    let total = rows.len();
    let online = rows
        .iter()
        .filter(|row| matches!(row.status, ConnStatus::Ready))
        .count();
    // A connecting core matches no chip, so a core counts when it is not Ready OR hits a chip.
    // One walk over the union: a max of the two tallies would undercount a fleet holding one
    // connecting core and one slow Ready core.
    let attention = rows
        .iter()
        .filter(|row| {
            !matches!(row.status, ConnStatus::Ready)
                || FILTERS.iter().any(|filter| row_matches(row, *filter))
        })
        .count();
    let tone = if counts.offline_failed > 0 {
        Tone::Bad
    } else if online == total && attention == 0 {
        Tone::Ok
    } else {
        Tone::Warn
    };
    Verdict {
        tone,
        attention,
        total,
        online,
        servers,
    }
}

/// Counts printed on the five view segments.
///
/// Every figure comes from the same rows and chip counts as the strip, so a segment and the
/// chip that names the same thing cannot disagree.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(super) struct SegmentCounts {
    /// Visible cores.
    pub(super) cores: usize,
    /// Distinct servers among the visible cores.
    pub(super) servers: usize,
    /// Distinct cores needing attention, the same figure the verdict names.
    pub(super) problems: usize,
    /// Alarm starts inside the last 24 hours, the same figure as the alarms chip.
    pub(super) alarms: usize,
    /// Cores behind the fleet build, the same figure as the version chip.
    pub(super) updates: usize,
}

/// Derive the segment counts from the strip's own figures.
///
/// Args:
///     verdict: Verdict for the visible rows. Its `total` and `attention` become the Cores and
///         Problems counts.
///     counts: Chip counts for the same rows.
///     servers: Server groups the panel shows, one per distinct server key.
///
/// Returns:
///     The five counts. A healthy fleet reads `problems == 0`.
pub(super) fn segment_counts(
    verdict: &Verdict,
    counts: &HealthCounts,
    servers: usize,
) -> SegmentCounts {
    SegmentCounts {
        cores: verdict.total,
        servers,
        problems: verdict.attention,
        alarms: counts.alarms_24h,
        updates: counts.version_behind,
    }
}

#[cfg(test)]
mod tests;
