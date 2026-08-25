//! The time axis a replicated report row actually lives on, and the one seam that leaves it.
//!
//! # Why this exists
//!
//! Three time axes run through the terminal, and until this module only two of them were named:
//!
//! - **CORE-LOCAL** — `orders_rep.buydate` / `sellsetdate` / `closedate`, [`crate::feed::CoreLogLine::time_ms`],
//!   MoonBot's own trade-log filenames, and the `WorkingTime` schedule strings the tuner writes
//!   back. Authority: the MoonBot machine's wall clock. `db::rep::apply_upsert` stores every
//!   replicated field verbatim, so whatever second the core sent is exactly what lands in the
//!   table, with no zone recorded anywhere beside it.
//! - **TRUE UTC** — candles and trades (MoonProto normalizes those on the wire),
//!   [`crate::util::time::now_unix_ms`], [`crate::feed::CoreLogLine::recv_ms`], the spot-rate
//!   minutes behind `db::valuation`, and `strat_db`'s version boundaries. Authority: this machine.
//! - **DISPLAY** — anything rendered through [`crate::util::display_time`] or the `mt_*` SQL
//!   scalars in the user's selected IANA zone.
//!
//! Treating a CORE-LOCAL second as TRUE UTC is the bug this module closes. It is not only a
//! display bug: `db::valuation` keys a genuinely-UTC rate series by `closedate`, and
//! `strat_db::stats` joins `buydate` against boundaries stamped by this machine's clock.
//!
//! # Why the correction happens on READ, never at INGEST
//!
//! The replica stays byte-faithful. Four reasons, in force order:
//!
//! 1. `db::valuation`'s `trade_values` cache is keyed in part by `closedate` and joined on it, so
//!    rewriting the column in place desynchronizes the whole cache.
//! 2. The valuation worker walks a descending `(closedate, core_uid, row_id)` reconciliation
//!    cursor whose total monotonicity is load-bearing; an in-place rewrite makes rows read as
//!    already-visited or never-visited.
//! 3. `db::rep`'s upsert is a partial column write reconciled against the core's own alive map. A
//!    corrected value carries no marker, so a later live upsert cannot tell what it overwrites.
//! 4. A better offset estimate must fix all history retroactively. On read that is free; at
//!    ingest it needs a migration the append-only replica schema has nowhere to record.
//!
//! # Offsets, not zones
//!
//! What can be measured from the wire is an OFFSET, never an IANA zone: one offset matches many
//! zones with different daylight-saving rules, so naming a zone from it would be a guess. The
//! offset is therefore stored per core as an append-only list of segments — an adopted change
//! opens a new segment rather than rewriting history — and the earliest segment extends backward
//! without bound so pre-observation rows are still corrected.
//!
//! A segment is selected by comparing the value being converted against the segment's start. The
//! start is a TRUE-UTC instant while a `closedate` is CORE-LOCAL, so a row falling within the
//! offset's own width of a segment boundary may pick the neighbouring segment. Accepted
//! deliberately: segment boundaries are adoption events, which sit hours-to-months apart, and the
//! alternative — resolving the segment with the offset the segment itself defines — is circular.
//!
//! # What this type must never be applied to
//!
//! Some consumers of these columns are ALREADY correct and a blanket correction breaks them:
//!
//! - Values used as IDENTITY or ORDERING — the valuation reconciliation cursor, the
//!   `trade_values` join, the tuner's threshold tie-break — must keep reading the raw column.
//! - The `closedate > 0` open/closed partition is a flag test, not an instant.
//! - `closedate - buydate` durations are a difference on one axis and cancel the offset already.
//! - MoonBot trade-log filename lookups are core-local on BOTH sides.
//! - The tuner's `WorkingTime` output is read back by MoonBot in the core's own local time, so its
//!   schedule axis stays core-local permanently.

use std::collections::HashMap;

use chrono_tz::Tz;

/// Widest offset from UTC any real time zone reaches, in seconds.
///
/// The extremes are UTC-12:00 (Baker Island) and UTC+14:00 (Line Islands). A measurement outside
/// this band is a broken clock rather than a zone, and is refused instead of being adopted.
pub const MIN_OFFSET_SECS: i32 = -12 * 3_600;
/// Widest positive offset from UTC any real time zone reaches, in seconds.
pub const MAX_OFFSET_SECS: i32 = 14 * 3_600;

/// One adopted offset and the instant it began to apply.
///
/// `from_utc` is the TRUE-UTC instant of the observation that adopted `offset_secs`. The earliest
/// segment of a core applies backward without bound regardless of its own `from_utc`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct OffsetSegment {
    /// True-UTC instant from which this offset applies, in seconds.
    pub from_utc: i64,
    /// Seconds east of UTC on the core's clock: `core_clock - true_utc`.
    pub offset_secs: i32,
}

/// The axis every read of a replicated report timestamp passes through.
///
/// Holds one append-only segment list per core uid plus the zone those corrected instants are
/// finally displayed in. A core with no segments converts as the identity, which is what an
/// honest UTC core needs and the only assumption that cannot make such a core worse.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ReportAxis {
    segments: HashMap<u64, Vec<OffsetSegment>>,
    zone: Tz,
}

impl ReportAxis {
    /// Build the axis that leaves every replicated timestamp exactly as the core wrote it.
    ///
    /// Conversion is the identity and the display zone is UTC, so rendering a core-local second
    /// through it reproduces the core's own wall clock — which is what MoonBot's report prints.
    /// This is the axis the terminal uses until a core's real offset has been measured.
    ///
    /// Returns:
    ///     An axis holding no offsets, displaying in UTC.
    pub fn identity_core_local() -> Self {
        Self {
            segments: HashMap::new(),
            zone: Tz::UTC,
        }
    }

    /// Build an axis from measured per-core offsets and the user's selected display zone.
    ///
    /// Segment lists are sorted and clamped here so every later lookup is a plain scan; a segment
    /// whose offset falls outside [`MIN_OFFSET_SECS`]..=[`MAX_OFFSET_SECS`] is dropped rather than
    /// trusted, because such a value is a broken clock and correcting by it would move a row
    /// further from the truth than leaving it alone.
    ///
    /// Args:
    ///     measured: Per-core segment lists, in any order.
    ///     zone: Display zone corrected instants are finally rendered in.
    ///
    /// Returns:
    ///     An axis that converts each core's rows by its own measured offset.
    pub fn from_measured(measured: HashMap<u64, Vec<OffsetSegment>>, zone: Tz) -> Self {
        let segments = measured
            .into_iter()
            .filter_map(|(core_uid, mut list)| {
                list.retain(|s| (MIN_OFFSET_SECS..=MAX_OFFSET_SECS).contains(&s.offset_secs));
                list.sort_by_key(|s| s.from_utc);
                (!list.is_empty()).then_some((core_uid, list))
            })
            .collect();
        Self { segments, zone }
    }

    /// Load every measured offset from the replica and pair it with a display zone.
    ///
    /// FAILS CLOSED, and that is the whole reason this returns a [`ReadResult`] rather than an
    /// axis: on a skewed core the identity axis is the WRONG-MONEY axis, so a measurement that
    /// cannot be read must stop the read rather than quietly become "no measurement". An ABSENT
    /// table is a different fact and is not a failure — a fresh install genuinely has nothing
    /// measured — and [`crate::db::rep::core_offset::load_all`] is what keeps the two apart.
    ///
    /// Args:
    ///     conn: Open report reader or pinned snapshot.
    ///     zone: Display zone the corrected instants are finally rendered in.
    ///
    /// Returns:
    ///     The axis in force, or a classified read failure.
    pub fn load(conn: &rusqlite::Connection, zone: Tz) -> crate::db::ReadResult<Self> {
        Ok(Self::from_measured(
            crate::db::rep::core_offset::load_all(conn)?,
            zone,
        ))
    }

    /// Return the display zone corrected instants are rendered in.
    ///
    /// Returns:
    ///     The selected IANA zone, or UTC on the identity axis.
    pub fn zone(&self) -> Tz {
        self.zone
    }

    /// Return the offset applying to one core at one instant.
    ///
    /// Args:
    ///     core_uid: Stable uid of the core that produced the row.
    ///     at: Instant to resolve, in seconds.
    ///
    /// Returns:
    ///     Seconds east of UTC, or `None` when this core has no measured offset at all. `None` is
    ///     deliberately distinguishable from `Some(0)`: a diagnosis surface must be able to say
    ///     "never measured" rather than claiming the core runs on UTC.
    pub fn offset_secs(&self, core_uid: u64, at: i64) -> Option<i32> {
        self.segment_at(core_uid, at).map(|s| s.offset_secs)
    }

    /// Convert one stored core-local timestamp to true UTC.
    ///
    /// Args:
    ///     secs: Value as stored in the replica, in seconds on the core's own clock.
    ///     core_uid: Stable uid of the core that produced the row.
    ///
    /// Returns:
    ///     The same instant in true UTC seconds, or `secs` unchanged when this core has no
    ///     measured offset.
    pub fn to_utc(&self, secs: i64, core_uid: u64) -> i64 {
        match self.offset_secs(core_uid, secs) {
            Some(offset) => secs - i64::from(offset),
            None => secs,
        }
    }

    /// Convert one true-UTC instant into the core-local value a stored column would hold.
    ///
    /// This is the direction a WINDOW BOUND takes. Converting the bound rather than the column
    /// keeps `closedate` bare on the left of a comparison, which is what leaves the replica's
    /// indexes usable; wrapping the column in a function instead turns the report's period filter
    /// into a full table scan.
    ///
    /// Args:
    ///     secs: True-UTC instant, in seconds.
    ///     core_uid: Stable uid of the core whose rows the bound will be compared against.
    ///
    /// Returns:
    ///     The value to compare against the raw stored column, or `secs` unchanged when this core
    ///     has no measured offset.
    pub fn from_utc(&self, secs: i64, core_uid: u64) -> i64 {
        match self.offset_secs(core_uid, secs) {
            Some(offset) => secs + i64::from(offset),
            None => secs,
        }
    }

    /// Return the whole segment applying to one core at one instant, not just its offset.
    ///
    /// [`offset_secs`](Self::offset_secs) answers "by how much", which is all a CONVERSION needs.
    /// A diagnosis surface needs the other half — WHEN this offset was adopted — and inventing
    /// that timestamp at the point of display is how a value measured last week comes to claim it
    /// was measured just now.
    ///
    /// Args:
    ///     core_uid: Stable uid of the core.
    ///     at: Instant to resolve, in seconds.
    ///
    /// Returns:
    ///     The segment in force, or `None` when this core has no measured offset at all.
    pub fn segment_at(&self, core_uid: u64, at: i64) -> Option<OffsetSegment> {
        let list = self.segments.get(&core_uid)?;
        let idx = match list.binary_search_by_key(&at, |s| s.from_utc) {
            Ok(hit) => hit,
            // The earliest segment applies backward without bound, so a value before every
            // boundary still resolves rather than falling through to `None`.
            Err(0) => 0,
            Err(next) => next - 1,
        };
        list.get(idx).copied()
    }

    /// Convert one true-UTC bound into the core-local value to compare a stored column against,
    /// using an offset that is ALREADY resolved.
    ///
    /// The counterpart to [`from_utc`](Self::from_utc) for a caller that has grouped its cores:
    /// the group IS the offset, so looking it up again per bound would only re-derive what the
    /// grouping already decided, and would do it against the bound's own instant rather than the
    /// instant the grouping used — two answers where the predicate needs one.
    ///
    /// Args:
    ///     secs: True-UTC instant, in seconds.
    ///     offset_secs: Seconds east of UTC on the clock of every core in the group.
    ///
    /// Returns:
    ///     The value to compare against the raw stored column.
    pub fn shift_bound(secs: i64, offset_secs: i32) -> i64 {
        secs.saturating_add(i64::from(offset_secs))
    }

    /// Group cores by the offset applying to them at one instant.
    ///
    /// A window predicate is built one branch per group, each branch naming its cores and the
    /// bounds already converted for them, so every branch still leads with `core_uid, closedate`
    /// and stays index-eligible. A fleet on one offset — the common case — collapses to a single
    /// branch identical in shape to the uncorrected query.
    ///
    /// Args:
    ///     cores: Core uids in scope for this read.
    ///     at: Instant whose offsets decide the grouping, in seconds.
    ///
    /// Returns:
    ///     One entry per distinct offset, each carrying its cores in the order given. Cores with
    ///     no measured offset group under `0`, matching [`to_utc`](Self::to_utc)'s identity.
    /// Group every core that HAS a measured offset by the offset applying to it at one instant.
    ///
    /// This is the unbounded-scope counterpart to [`groups`](Self::groups). A read over "all
    /// cores" cannot name its cores, so the predicate it needs is one branch per measured offset
    /// naming those cores explicitly, plus a final catch-all for everything else — which converts
    /// as the identity, exactly as [`to_utc`](Self::to_utc) does for a core with no segments.
    /// [`measured_cores`](Self::measured_cores) supplies that catch-all's exclusion list.
    ///
    /// The result is ordered by offset, and each core list ascending, so the SQL a caller builds
    /// from it is stable across runs rather than reshuffling with the map's iteration order.
    ///
    /// Args:
    ///     at: Instant whose offsets decide the grouping, in seconds.
    ///
    /// Returns:
    ///     One entry per distinct measured offset. Empty when nothing has been measured, which is
    ///     the honest signal that the uncorrected single-branch predicate is still correct.
    pub fn measured_groups(&self, at: i64) -> Vec<(i32, Vec<u64>)> {
        let mut by_offset: HashMap<i32, Vec<u64>> = HashMap::new();
        for (&core_uid, _) in self.segments.iter() {
            if let Some(offset) = self.offset_secs(core_uid, at) {
                by_offset.entry(offset).or_default().push(core_uid);
            }
        }
        let mut grouped: Vec<(i32, Vec<u64>)> = by_offset
            .into_iter()
            .map(|(offset, mut cores)| {
                cores.sort_unstable();
                (offset, cores)
            })
            .collect();
        grouped.sort_unstable_by_key(|(offset, _)| *offset);
        grouped
    }

    /// Every core uid this axis holds a measurement for, ascending.
    ///
    /// The exclusion list for an unbounded read's catch-all branch: a core absent from it has no
    /// measured offset and therefore converts as the identity.
    ///
    /// Returns:
    ///     Measured core uids in ascending order.
    pub fn measured_cores(&self) -> Vec<u64> {
        let mut cores: Vec<u64> = self.segments.keys().copied().collect();
        cores.sort_unstable();
        cores
    }

    pub fn groups(&self, cores: &[u64], at: i64) -> Vec<(i32, Vec<u64>)> {
        let mut order: Vec<i32> = Vec::new();
        let mut by_offset: HashMap<i32, Vec<u64>> = HashMap::new();
        for &core_uid in cores {
            let offset = self.offset_secs(core_uid, at).unwrap_or(0);
            let bucket = by_offset.entry(offset).or_default();
            if bucket.is_empty() {
                order.push(offset);
            }
            bucket.push(core_uid);
        }
        order
            .into_iter()
            .filter_map(|offset| by_offset.remove(&offset).map(|cores| (offset, cores)))
            .collect()
    }
}

#[cfg(test)]
mod tests;
