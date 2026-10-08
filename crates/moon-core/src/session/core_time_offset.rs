//! Per-core wall-clock offset estimator, fed (core-clock, receipt) pairs taken from the core's Ping.
//!
//! The live source is MoonProto's per-client `MoonClient::server_time_delta_ms`, refreshed on every
//! received Ping: `Ping.InitialTime` — the core's local wall clock, timezone included — minus this
//! machine's OS UTC clock at receipt. `feed::live::ping_clock` samples it every few seconds and
//! hands it here as the pair (`recv_ms + delta`, `recv_ms`). Until 2026-10 the pairs came from the
//! core's log lines (`CoreLogLine::time_ms`, `recv_ms`); the Ping needs no log subscription, so a
//! station can switch the log stream off, and it reaches a core whose log stays quiet.
//!
//! # The principle
//!
//! The core-clock half is the core's own wall clock at the moment it sent the Ping; `recv_ms` is
//! this machine's clock at the moment it was received. Their difference is `offset - lag`, where
//! `offset` is the core's true clock offset from this machine and `lag` is network and queueing
//! delay -- always non-negative, since a Ping cannot arrive before it was sent. Every sample is
//! therefore at or below the true offset, never above it, so the MAXIMUM sample in a window is the
//! estimate least polluted by lag. That is the whole principle behind [`OffsetEstimator`].
//!
//! Sign convention matches [`crate::db::report_axis::OffsetSegment::offset_secs`]: seconds EAST of
//! UTC on the core's clock, i.e. `core_clock - true_utc`.
//!
//! # Whole seconds, not zones
//!
//! The adopted offset is the core's clock as it is, rounded to the second: a UTC+3 core whose
//! clock runs forty seconds fast is `+03:00:40`, and its report rows are corrected by exactly
//! that. Until 2026-10 the value was rounded to a quarter hour, which turned such a core into a
//! clean `+03:00` and left every one of its rows forty seconds off. What is dropped is only the
//! fraction of a second, and that fraction is the network's, not the clock's: the delay of the
//! samples behind the maximum ran 15–80 ms in the 2026-10-08 shadow run.
//!
//! One exception: a value at most [`ZONE_SNAP_SECS`] from a whole zone (a multiple of
//! [`ZONE_STEP_SECS`]) IS that zone. The snap is kept under half of [`DEADBAND_SECS`] on purpose:
//! an estimate wavering by a second across the snap edge then moves by at most three seconds and
//! stays inside the deadband, where a snap as wide as the deadband would let a core about 4.5 s
//! off a zone flip between the zone and `+5 s` and store a segment each time. The delay always pulls a sample
//! down, so an honest UTC core behind a slow link would otherwise read `-1 s`, lose its identity
//! axis and start a report branch of its own; and two cores of one fleet, both NTP-synced, would
//! differ by a second each. Snapped, they share the zone — one SQL branch (`report_axis::groups`)
//! and one Core Status value — while a clock genuinely seconds off stays exactly that.
//!
//! # The deadband, and why it is not a zone rule
//!
//! An adoption opens a new segment in `core_time_offset`, and every new segment makes the
//! valuation worker re-value that core's whole history. So a new offset is adopted only when it
//! differs from the one in force by at least [`DEADBAND_SECS`]: a late Ping, a reconnect or a
//! second rounding the other way changes nothing, while a corrected clock or a timezone change
//! does. The offset in force starts as the one the database already holds
//! ([`OffsetEstimator::seed`]), so a fresh connection measures against history rather than
//! against nothing. This is unrelated to [`crate::session::clock_skew`]'s 45-minute deadband,
//! which exists so that an honest UTC core sees no correction at all; here a UTC+1 core and a
//! clock forty seconds fast are both ordinary values to adopt.
//!
//! # Cost
//!
//! [`OffsetEstimator::observe`] runs on the feed thread for every Ping sample of every connected
//! core — one per `ping_clock::SAMPLE_EVERY` at most, so a full window holds about sixty — and
//! it re-reads the window each time: a sort and, for the agreement rule, at most one pass per
//! retained sample. Sixty samples make that a few thousand comparisons every five seconds per
//! core, which is why it is not kept incrementally.
//!
//! # What rules 1 and 4 still defend against
//!
//! Both were written for the log source, where a reconnect REPLAYS old lines under one fresh
//! receipt time. The Ping has no replay, and the rules stay because they still bound what one
//! connection moment can decide. Around a (re)connect the delay of the first Pings is at its
//! largest — hundreds of milliseconds in the 2026-10-08 shadow run against tens in steady state.
//! [`OffsetEstimator::note_ready`]'s quarantine keeps that stretch out of the window, and the
//! distinct-arrival spread requires the agreeing samples to have actually arrived
//! [`MIN_SPREAD_MS`] apart, so an adoption always rests on Pings spread over real time rather than
//! on one burst. (The Ping MoonProto keeps returning through a reconnect never gets here twice:
//! `ping_clock` feeds a reading only while Ready and only when it differs from the last one.)

use std::collections::VecDeque;

use crate::db::report_axis::{MAX_OFFSET_SECS, MIN_OFFSET_SECS};

/// Minimum agreeing samples before an offset is adopted.
pub const MIN_SAMPLES: usize = 3;
/// Minimum real time the agreeing samples must span, in milliseconds.
pub const MIN_SPREAD_MS: i64 = 20_000;
/// Samples received within this long of a connection becoming Ready are discarded.
pub const QUARANTINE_MS: i64 = 15_000;
/// How far below the window's best sample another may sit and still agree with it, in
/// milliseconds. Two seconds holds every Ping of one steady connection — their delays differ by
/// tens of milliseconds, hundreds at a connect — and stays under [`DEADBAND_SECS`], so two
/// offsets that would open different segments never agree.
pub const AGREE_MS: i64 = 2_000;
/// Smallest change from the offset in force that is adopted, in seconds.
pub const DEADBAND_SECS: i32 = 5;
/// The grid real zones sit on, in seconds: every zone offset in use is a multiple of a quarter
/// hour (UTC+5:45, UTC+8:45, UTC+12:45 included).
pub const ZONE_STEP_SECS: i32 = 900;
/// How far from a whole zone a value is still that zone, in seconds; under half of
/// [`DEADBAND_SECS`] (see the module docs).
pub const ZONE_SNAP_SECS: i32 = 2;
/// Rolling window of retained samples, in milliseconds.
pub const WINDOW_MS: i64 = 300_000;

/// Where an adopted offset came from.
///
/// [`Self::None`] is the DEFAULT because a core nothing has been measured on is the state every
/// core starts in, and it must stay distinguishable from a measured zero: a diagnosis surface that
/// cannot tell "never measured" from "runs on UTC" turns a silent estimator failure into a
/// confident wrong claim.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum OffsetSource {
    /// The core's Ping clock: the live source.
    Ping,
    /// The core's log lines: the live source until 2026-10, still the label of the segments it
    /// stored, which a restart seeds from.
    Log,
    Replica,
    Skew,
    #[default]
    None,
}

impl OffsetSource {
    /// The label stored in `core_time_offset.source`.
    ///
    /// Returns:
    ///     The stable lowercase name [`Self::from_label`] reads back.
    pub fn label(self) -> &'static str {
        match self {
            OffsetSource::Ping => "ping",
            OffsetSource::Log => "log",
            OffsetSource::Replica => "replica",
            OffsetSource::Skew => "skew",
            OffsetSource::None => "none",
        }
    }

    /// Read a stored label back.
    ///
    /// Args:
    ///     label: A `core_time_offset.source` value.
    ///
    /// Returns:
    ///     The source it names, or [`Self::None`] for a label this build does not know — the
    ///     segment's offset still applies, only its provenance is unknown.
    pub fn from_label(label: &str) -> Self {
        match label {
            "ping" => OffsetSource::Ping,
            "log" => OffsetSource::Log,
            "replica" => OffsetSource::Replica,
            "skew" => OffsetSource::Skew,
            _ => OffsetSource::None,
        }
    }
}

/// One retained (core-clock, receipt) sample, kept for rolling-window eviction and agreement.
#[derive(Clone, Copy, Debug)]
struct Sample {
    /// `core_time_ms - recv_ms`: at most the true offset in milliseconds, since lag is never
    /// negative.
    raw_ms: i64,
    /// Local receipt time, used for window eviction and the distinct-arrival spread check.
    recv_ms: i64,
}

/// Per-core offset estimator, fed one observed (core-time, receipt-time) pair at a time.
#[derive(Clone, Debug, Default)]
pub struct OffsetEstimator {
    /// Retained samples within [`WINDOW_MS`] of the newest one.
    window: VecDeque<Sample>,
    /// Receipt time the connection last entered Ready, gating the quarantine in `observe`.
    ready_at: Option<i64>,
    /// The offset in force: the last one adopted, or the stored one [`Self::seed`] handed in;
    /// `None` when neither exists.
    adopted: Option<i32>,
    /// The estimate the last [`Self::observe`] reached, before the deadband: `None` when that
    /// sample left no qualifying value (quarantined, too few arrivals, out of the zone band).
    best: Option<i32>,
}

impl OffsetEstimator {
    /// Build an estimator with no retained samples or adopted offset.
    ///
    /// Returns:
    ///     A fresh per-core offset estimator.
    pub fn new() -> Self {
        Self::default()
    }

    /// Called when the connection enters Ready; starts the quarantine window.
    ///
    /// Args:
    ///     now_ms: Local receipt time at which the connection became ready, in milliseconds.
    ///
    /// Returns:
    ///     Nothing; samples received before the quarantine expires are ignored.
    pub fn note_ready(&mut self, now_ms: i64) {
        self.ready_at = Some(now_ms);
    }

    /// Start from the offset already in force for this core.
    ///
    /// Args:
    ///     stored: The newest stored segment's offset in seconds, `None` for a core never
    ///         measured. A later adoption must differ from it by [`DEADBAND_SECS`].
    pub fn seed(&mut self, stored: Option<i32>) {
        self.adopted = stored;
    }

    /// Feed one observed pair. Returns Some(offset_secs) ONLY when this sample ADOPTS a value
    /// at least [`DEADBAND_SECS`] away from the offset in force (or the first one, when nothing
    /// is in force); None every other time, including when the same offset is re-confirmed.
    ///
    /// Args:
    ///     core_time_ms: The core's wall clock at sending, in milliseconds.
    ///     recv_ms: Local receipt timestamp for the same sample, in milliseconds.
    ///
    /// Returns:
    ///     A newly adopted offset in seconds, or `None` when the window has no changed candidate.
    pub fn observe(&mut self, core_time_ms: i64, recv_ms: i64) -> Option<i32> {
        self.best = None;
        if let Some(ready_at) = self.ready_at {
            if recv_ms - ready_at < QUARANTINE_MS {
                return None;
            }
        }

        let raw_ms = core_time_ms - recv_ms;
        self.window.push_back(Sample { raw_ms, recv_ms });
        let cutoff = recv_ms - WINDOW_MS;
        while matches!(self.window.front(), Some(s) if s.recv_ms < cutoff) {
            self.window.pop_front();
        }

        // The best estimate is the HIGHEST sample that enough others agree with: lag can only
        // pull a sample below the true offset, so the highest surviving value is the one least
        // polluted by it (see the module docs). A sample agrees with a candidate when it sits at
        // most AGREE_MS below it; the candidate qualifies when its agreeing samples arrived at
        // MIN_SAMPLES distinct instants spanning MIN_SPREAD_MS (rule 4). Walking candidates from
        // the top, the first that qualifies is the answer — a lone high outlier is skipped
        // rather than blocking every value beneath it.
        let mut by_raw: Vec<Sample> = self.window.iter().copied().collect();
        by_raw.sort_unstable_by_key(|s| std::cmp::Reverse(s.raw_ms));
        let best_raw = by_raw.iter().find_map(|top| {
            let mut arrivals: Vec<i64> = by_raw
                .iter()
                .filter(|s| s.raw_ms <= top.raw_ms && top.raw_ms - s.raw_ms <= AGREE_MS)
                .map(|s| s.recv_ms)
                .collect();
            arrivals.sort_unstable();
            arrivals.dedup();
            let spread = arrivals.last()? - arrivals.first()?;
            (arrivals.len() >= MIN_SAMPLES && spread >= MIN_SPREAD_MS).then_some(top.raw_ms)
        })?;

        // Whole seconds, half away from zero.
        let candidate = (best_raw as f64 / 1_000.0).round() as i64;
        if !(i64::from(MIN_OFFSET_SECS)..=i64::from(MAX_OFFSET_SECS)).contains(&candidate) {
            return None;
        }
        // Safe: just confirmed `candidate` sits within MIN_OFFSET_SECS..=MAX_OFFSET_SECS, an i32
        // range.
        let candidate = candidate as i32;
        // Within the deadband of a whole zone, the zone (see the module docs).
        let zone =
            (f64::from(candidate) / f64::from(ZONE_STEP_SECS)).round() as i32 * ZONE_STEP_SECS;
        let candidate = if (candidate - zone).abs() <= ZONE_SNAP_SECS {
            zone
        } else {
            candidate
        };
        self.best = Some(candidate);
        if self
            .adopted
            .is_some_and(|in_force| (candidate - in_force).abs() < DEADBAND_SECS)
        {
            return None;
        }
        self.adopted = Some(candidate);
        Some(candidate)
    }

    /// The offset in force, or None when nothing has ever been adopted.
    ///
    /// Returns:
    ///     The adopted offset in seconds, or `None` before the first adoption.
    pub fn adopted(&self) -> Option<i32> {
        self.adopted
    }

    /// The estimate the last sample reached, whether or not it was adopted.
    ///
    /// Returns:
    ///     The qualifying offset in seconds after snapping, or `None` when the last sample left
    ///     none — so a caller can tell "the in-force offset was just re-measured" from silence.
    pub fn best(&self) -> Option<i32> {
        self.best
    }

    /// How many samples currently sit in the window.
    ///
    /// Returns:
    ///     Count of retained samples.
    pub fn samples(&self) -> u32 {
        self.window.len() as u32
    }
}

#[cfg(test)]
mod tests;
