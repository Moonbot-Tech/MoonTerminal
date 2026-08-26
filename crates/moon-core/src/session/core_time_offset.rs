//! Per-core wall-clock offset estimator, measured from [`crate::feed::types::CoreLogLine`] pairs
//! (`time_ms`, `recv_ms`) received off the wire.
//!
//! # The principle
//!
//! `time_ms` is the core's own wall clock at the moment it wrote the line; `recv_ms` is this
//! machine's clock at the moment the line was received. Their difference is `offset - lag`, where
//! `offset` is the core's true clock offset from this machine and `lag` is network and queueing
//! delay -- always non-negative, since a line cannot arrive before it was written. Every sample is
//! therefore at or below the true offset, never above it, so the MAXIMUM sample in a window is the
//! estimate least polluted by lag. That is the whole principle behind [`OffsetEstimator`].
//!
//! Sign convention matches [`crate::db::report_axis::OffsetSegment::offset_secs`]: seconds EAST of
//! UTC on the core's clock, i.e. `core_clock - true_utc`.
//!
//! # Cost, and the part of it left standing
//!
//! [`OffsetEstimator::observe`] runs on the feed thread for EVERY log line of every connected
//! core, and it re-summarizes the whole retained window each time. The per-bucket maximum is
//! accumulated in the same single pass that buckets the samples, so the cost is one walk of the
//! window rather than one walk per bucket -- but it is still a walk per line rather than
//! incremental bookkeeping maintained on push and evict. Making it incremental is a real
//! improvement at 200 cores and is deliberately NOT done here: it rewrites the adoption rule's
//! own data structure, and that rule is the thing under proof. Left as a follow-up on purpose.
//!
//! # Why there is no deadband
//!
//! [`crate::session::clock_skew`] carries a 45-minute deadband because it only ever corrects a
//! genuinely large skew and an honest UTC core must see no correction at all. This estimator has
//! none: a UTC+1 core is a real and ordinary case, and rounding it away would leave every UTC+1
//! user's report wrong by exactly one hour.
//!
//! # The replay attack rules 1 and 4 defend against
//!
//! On reconnect the core REPLAYS old log lines. Every replayed line receives the SAME fresh
//! `recv_ms` while keeping its OLD `time_ms`, so three replayed lines out of one historical
//! 15-minute bucket agree with each other perfectly and would satisfy a naive three-sample rule.
//! Taking the maximum over the window alone does not defend against this -- two separate checks
//! do: [`OffsetEstimator::note_ready`]'s quarantine keeps the replay burst that follows a
//! reconnect out of the window entirely, and the distinct-arrival spread below requires the
//! agreeing samples to have actually arrived [`MIN_SPREAD_MS`] apart in real time, which a single
//! replay burst cannot satisfy even once the quarantine has passed. Do not read the spread check
//! as redundant belt-and-braces on top of the bucket agreement -- it is the one that catches a
//! burst the quarantine already let through.

use std::collections::{HashMap, VecDeque};

use crate::db::report_axis::{MAX_OFFSET_SECS, MIN_OFFSET_SECS};

/// Minimum agreeing samples before an offset is adopted.
pub const MIN_SAMPLES: usize = 3;
/// Minimum real time the agreeing samples must span, in milliseconds.
pub const MIN_SPREAD_MS: i64 = 20_000;
/// Samples received within this long of a connection becoming Ready are discarded.
pub const QUARANTINE_MS: i64 = 15_000;
/// Rounding granularity of an adopted offset, in seconds.
pub const BUCKET_SECS: i64 = 900;
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
    Log,
    Replica,
    Skew,
    #[default]
    None,
}

/// One retained (core-clock, receipt) sample, kept for rolling-window eviction and bucketing.
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
    /// Currently adopted offset, or `None` before the first adoption.
    adopted: Option<i32>,
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

    /// Feed one observed pair. Returns Some(offset_secs) ONLY when this sample ADOPTS a value
    /// different from the currently adopted one; None every other time, including when the
    /// same offset is merely re-confirmed.
    ///
    /// Args:
    ///     core_time_ms: Timestamp the core put on the log line, in milliseconds.
    ///     recv_ms: Local receipt timestamp for the same line, in milliseconds.
    ///
    /// Returns:
    ///     A newly adopted offset in seconds, or `None` when the window has no changed candidate.
    pub fn observe(&mut self, core_time_ms: i64, recv_ms: i64) -> Option<i32> {
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

        // Group every retained sample by its rounded bucket, so agreement (rule 3) can be
        // measured, then keep only the buckets whose distinct arrivals clear rule 4.
        //
        // Each bucket's own MAXIMUM raw value is accumulated in this SAME pass rather than
        // rescanned per qualifying bucket afterwards. The result is identical by construction --
        // it is the maximum over exactly the same samples -- but it costs one walk of the window
        // instead of one walk per bucket, and this runs on the feed thread for every log line of
        // every connected core.
        let mut by_bucket: HashMap<i64, (Vec<i64>, i64)> = HashMap::new();
        for sample in &self.window {
            let entry = by_bucket
                .entry(bucket_key(sample.raw_ms))
                .or_insert_with(|| (Vec::new(), i64::MIN));
            entry.0.push(sample.recv_ms);
            entry.1 = entry.1.max(sample.raw_ms);
        }

        // Among every qualifying bucket, the winning raw sample is the algebraic maximum: lag
        // can only ever pull a sample below the true offset, so the highest surviving value is
        // the one least polluted by it (see the module docs).
        let mut best_raw: Option<i64> = None;
        for (recv_list, bucket_max_raw) in by_bucket.values_mut() {
            recv_list.sort_unstable();
            recv_list.dedup();
            if recv_list.len() < MIN_SAMPLES {
                continue;
            }
            let spread = recv_list[recv_list.len() - 1] - recv_list[0];
            if spread < MIN_SPREAD_MS {
                continue;
            }
            let bucket_max_raw = *bucket_max_raw;
            best_raw = Some(best_raw.map_or(bucket_max_raw, |cur| cur.max(bucket_max_raw)));
        }

        let candidate = bucket_key(best_raw?) * BUCKET_SECS;
        if !(i64::from(MIN_OFFSET_SECS)..=i64::from(MAX_OFFSET_SECS)).contains(&candidate) {
            return None;
        }
        // Safe: just confirmed `candidate` sits within MIN_OFFSET_SECS..=MAX_OFFSET_SECS, an i32
        // range.
        let candidate = candidate as i32;
        if self.adopted == Some(candidate) {
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

    /// How many samples currently sit in the window.
    ///
    /// Returns:
    ///     Count of retained samples.
    pub fn samples(&self) -> u32 {
        self.window.len() as u32
    }

    /// Discard the window WITHOUT discarding the adopted value.
    ///
    /// Returns:
    ///     Nothing; the currently adopted offset remains in force.
    pub fn clear_window(&mut self) {
        self.window.clear();
    }
}

/// Round a raw millisecond difference to the nearest [`BUCKET_SECS`] index.
///
/// Args:
///     raw_ms: `core_time_ms - recv_ms`, in milliseconds.
///
/// Returns:
///     Bucket index; the bucket's own value in seconds is `index * BUCKET_SECS`.
fn bucket_key(raw_ms: i64) -> i64 {
    (raw_ms as f64 / (BUCKET_SECS * 1000) as f64).round() as i64
}

#[cfg(test)]
mod tests;
