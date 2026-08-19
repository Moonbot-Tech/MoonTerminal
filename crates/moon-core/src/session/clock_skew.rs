//! Per-core estimate of a core's wall-clock skew from the LOCAL clock, used to correct order
//! record and trace times before they ever reach the retained line store or the chart.
//!
//! `OrderRecord::adjust_time` exists in moonproto but is never called (`state/orders.rs:288` calls
//! it only on the LEGACY `ServerTimeDelta` path, which corrects trace points, not record times),
//! and that delta is unreachable from here anyway: its accessor is `#[cfg(test)] pub(crate)`. So a
//! core whose clock is not UTC ships every order's `create_time`, `close_time`, and trace start raw,
//! and the chart draws them there. This module recovers the skew from the wire data itself and
//! corrects it before anything downstream sees a raw time.
//!
//! # Sign convention
//!
//! `skew_ms = core_clock - true_UTC` (a core running two hours ahead of UTC carries
//! `skew_ms = +7_200_000`). `correct` always computes `corrected = raw - skew_ms`, the same
//! direction moonproto's own (unreachable) `ServerTimeDelta` correction uses.
//!
//! Both signals below measure the core's clock against the LOCAL machine's clock, not against true
//! UTC — same as moonproto's own `ServerTimeDelta`. A wrong local clock therefore mis-corrects
//! every core; that is a separate bug with a separate fix.
//!
//! # Two sample classes over one stream
//!
//! - **LOOSE (lower bound)**: `trace.points[0].time_ms - trace.points[1].time_ms` for any trace
//!   with at least two points — its unknown term is the core's tick cadence between two trace
//!   points, not an order's age. This equals `skew_ms - elapsed`, and `elapsed >= 0`, so every
//!   LOOSE sample is a lower bound on the true skew — it can only ever be at or below it, never
//!   above. Because of that, the algebraic MAXIMUM of several LOOSE samples is the tightest
//!   (closest to the truth) estimate, regardless of the skew's own sign: a fresher sample
//!   (`elapsed` near zero) sits closer to the truth than a stale one, which sits further below it.
//!   Adopting a LOOSE candidate therefore may only ever RAISE the magnitude of the currently
//!   adopted correction, never lower it — a looser (smaller-magnitude) bound is simply less
//!   informative, not evidence the skew shrank. `row.create_time_ms - now_ms` deliberately does
//!   NOT feed this class: unlike a trace's second point, nothing bounds an order's age, so a
//!   same-age cohort of old orders at (re)connect would cluster in one bucket and read as a false
//!   skew on an honest core. See TIGHT below for the one place that formula is trusted.
//! - **TIGHT (point estimate)**: `row.create_time_ms - now_ms`, but only for a uid the order-line
//!   store has never seen, and only once the estimator itself has been observing continuously for
//!   `WARMUP_MS`. A freshly created order has `elapsed` near zero, so this is a near-exact point
//!   estimate rather than a bound, and — unlike LOOSE — it is trusted to move the adopted skew in
//!   EITHER direction, including revoking it back toward zero. This is what makes un-learning a
//!   DST change or a core swap possible: a quiet core that produces no new orders simply keeps its
//!   last estimate, deliberately.
//!
//! The warmup gate exists because the first batch after a (re)connect is full of orders that are
//! new only TO US, not new in age: their `elapsed` is whatever their real lifetime is, often much
//! more than zero, so treating them as point estimates would read a stale order's large `elapsed`
//! as if it were the skew itself.
//!
//! # Voting
//!
//! Samples are bucketed to the nearest `BUCKET_MS` and voted per call to [`CoreClockSkew::observe`]
//! (bucket votes do not persist between calls — only the adopted [`CoreClockSkew::skew_ms`] and the
//! warmup clock do). A bucket's votes are counted by DISTINCT uid, not by sample: one order can
//! contribute a buy-trace sample and a sell-trace sample to the same bucket, and counting both
//! would let a single order cross `MIN_VOTES` alone. A bucket must collect at least `MIN_VOTES`
//! agreeing UIDS before it is trusted, so one wire glitch, one aged order, or one order's two
//! traces in isolation can never move the estimate — the whole design is built on requiring at
//! least two independent orders to agree. Adoption prefers the highest-magnitude qualifying TIGHT
//! bucket; only when none qualifies does it fall back to the qualifying LOOSE bucket with the
//! greatest algebraic value, subject to the raise-only rule above. A winning candidate whose
//! magnitude sits below `MIN_ABS_SKEW_MS` corrects to `0.0` — deliberately including a TIGHT
//! winner, which is exactly how a TIGHT bucket at zero un-learns a stale nonzero estimate.
//!
//! # Correction generation
//!
//! [`CoreClockSkew::generation`] increments every time [`CoreClockSkew::observe`] adopts a changed
//! estimate, and again on [`CoreClockSkew::reset`]. `session::order_lines::OrderLineStore` compares
//! it against each retained order's own marker to decide whether that order's `create_ms` and
//! `entry_fill_ms` were derived under the currently adopted correction or a stale one, and
//! RE-DERIVES them from the current (already corrected) row when they were not — see that module
//! for why re-deriving, not delta-shifting, is the correct repair for those two fields.
//!
//! # What this does not correct
//!
//! `feed/live/mod.rs` writes each `CoreLogLine.time_ms` to the on-disk log file on the FEED thread,
//! before anything reaches `CoreData`. Correcting only the in-memory copy shown in the Log panel
//! would make it disagree with the file it is supposed to mirror — worse than today's consistent
//! (uncorrected) shift — so that field is deliberately left alone here.
//!
//! # Risk if moonproto starts calling `adjust_time`
//!
//! If a future moonproto release starts calling `OrderRecord::adjust_time` itself, this estimator
//! would apply a second correction on top of an already-corrected wire time. It self-heals within
//! two samples once the new offset (likely zero) is observed, but `make update-moonproto` should
//! re-run `grep -rn adjust_time` against the vendored checkout to catch this before it ships.

use std::collections::{BTreeMap, HashSet};

use crate::feed::OrderRow;

/// Width of one skew bucket. Wide enough that ordinary sample noise from `elapsed` does not
/// straddle a bucket boundary, narrow enough that a real hour-scale skew lands solidly inside one.
const BUCKET_MS: f64 = 15.0 * 60_000.0;

/// Deadband: a winning bucket below this magnitude corrects to `0.0`. Sits below the smallest real
/// timezone offset (1h) and above credible NTP drift, so an honest UTC core keeps exactly today's
/// behaviour.
const MIN_ABS_SKEW_MS: f64 = 45.0 * 60_000.0;

/// Plausibility window. A sample outside this range cannot be a real clock skew — it is a wire
/// glitch or a genuinely ancient order — and is dropped before voting, so it can never poison a
/// bucket even if paired with a matching glitch.
const MIN_SKEW_MS: f64 = -12.0 * 3_600_000.0;
const MAX_SKEW_MS: f64 = 14.0 * 3_600_000.0;

/// Minimum agreeing samples a bucket needs before it is trusted. One sample alone could be a
/// one-off wire glitch or a single stale order; two independent orders agreeing on the same
/// 15-minute bucket are not.
const MIN_VOTES: u32 = 2;

/// How long the estimator must have been observing continuously before a never-seen uid is trusted
/// as a TIGHT sample. See the module docs for why the first post-connect batch cannot be trusted
/// this way.
const WARMUP_MS: f64 = 60_000.0;

/// Which of the two sample classes documented on the module voted for a bucket.
#[derive(Clone, Copy, PartialEq, Eq)]
enum SampleClass {
    Tight,
    Loose,
}

impl SampleClass {
    fn label(self) -> &'static str {
        match self {
            SampleClass::Tight => "tight",
            SampleClass::Loose => "loose",
        }
    }
}

/// Per-bucket agreement tally for one `observe` call, keyed by DISTINCT uid so that one order's
/// buy and sell traces cannot cross `MIN_VOTES` by themselves.
///
/// No cap on the number of tallied buckets is needed: `record` already drops any sample outside
/// `MIN_SKEW_MS..=MAX_SKEW_MS` before it reaches here, and that window holds at most 105 distinct
/// `BUCKET_MS`-wide buckets.
#[derive(Clone, Default)]
struct BucketVotes {
    tight: HashSet<u64>,
    loose: HashSet<u64>,
}

/// Per-core clock-skew estimator and corrector. Pure and clock-injected: nothing here calls
/// `now_unix_ms` itself, so it is deterministic under test and agnostic to which core owns it.
#[derive(Default)]
pub struct CoreClockSkew {
    /// Currently adopted correction; `0.0` means no correction is applied.
    skew_ms: f64,
    /// Wall-clock time of this instance's first `observe` call since construction or `reset`, used
    /// to gate the TIGHT class behind `WARMUP_MS`.
    first_seen_ms: Option<f64>,
    /// Bumped whenever `observe` adopts a changed estimate, and again on `reset`. See the module
    /// docs' "Correction generation" section.
    generation: u64,
}

impl CoreClockSkew {
    /// Observes one fresh order-row batch and returns the change in the adopted skew, if any.
    ///
    /// Must be called with the RAW (uncorrected) batch, before [`CoreClockSkew::correct`] runs on
    /// it — the sample formulas assume `row.create_time_ms` is still off the wire.
    ///
    /// Args:
    ///     rows: Raw combined order-row batch for this core.
    ///     known: Whether the order-line store has already retained this uid from an earlier
    ///         batch, used to gate the TIGHT class to genuinely new orders.
    ///     now_ms: Local wall clock at the moment this batch was ingested.
    ///
    /// Returns:
    ///     `Some(old_skew_ms - new_skew_ms)` when adoption changed the estimate, so the caller can
    ///     shift every already-retained wire time by exactly the amount the fresh correction now
    ///     applies; `None` when nothing changed.
    pub fn observe(
        &mut self,
        rows: &[OrderRow],
        known: impl Fn(u64) -> bool,
        now_ms: f64,
    ) -> Option<f64> {
        let first_seen_ms = *self.first_seen_ms.get_or_insert(now_ms);
        let warmed_up = now_ms - first_seen_ms >= WARMUP_MS;

        let mut buckets: BTreeMap<i64, BucketVotes> = BTreeMap::new();
        for r in rows {
            // `create_time_ms - now_ms` is trusted only as a TIGHT point estimate (new uid,
            // post-warmup) — never as LOOSE evidence, since nothing bounds an order's age and a
            // same-age cohort of old orders would otherwise cluster into a false skew. See the
            // module docs.
            if r.create_time_ms > 1.0 && warmed_up && !known(r.uid) {
                let sample = r.create_time_ms - now_ms;
                record(&mut buckets, sample, SampleClass::Tight, r.uid);
            }
            for (trace, anchored_by_buy) in
                [(r.buy_trace.as_ref(), true), (r.sell_trace.as_ref(), false)]
                    .into_iter()
                    .filter_map(|(t, buy)| t.map(|t| (t, buy)))
            {
                // Same anchor test as `correct`: once a line has been shrunk from the front, its
                // first point is a corrected tick and the difference below measures nothing but
                // the gap between two ticks.
                let raw_create = if anchored_by_buy {
                    r.create_time_ms
                } else {
                    r.sell_create_time_ms
                };
                if trace.points.len() >= 2
                    && raw_create > 1.0
                    && trace.points[0].time_ms == raw_create
                {
                    let sample = trace.points[0].time_ms - trace.points[1].time_ms;
                    record(&mut buckets, sample, SampleClass::Loose, r.uid);
                }
            }
        }

        let old_skew = self.skew_ms;
        let winner = if let Some((bucket, votes)) = best_bucket(&buckets, SampleClass::Tight) {
            let candidate = deadband(bucket as f64 * BUCKET_MS);
            (candidate != old_skew).then_some((bucket, candidate, votes, SampleClass::Tight))
        } else if let Some((bucket, votes)) = best_bucket(&buckets, SampleClass::Loose) {
            let candidate = deadband(bucket as f64 * BUCKET_MS);
            (candidate.abs() > old_skew.abs()).then_some((
                bucket,
                candidate,
                votes,
                SampleClass::Loose,
            ))
        } else {
            None
        };

        let (bucket, new_skew, votes, class) = winner?;
        self.skew_ms = new_skew;
        self.generation = self.generation.wrapping_add(1);
        self.diag_adoption(rows, now_ms, bucket, new_skew, votes, class);
        Some(old_skew - new_skew)
    }

    /// Currently adopted skew in milliseconds; `0.0` before adoption or while the winning candidate
    /// sits below the deadband.
    pub fn skew_ms(&self) -> f64 {
        self.skew_ms
    }

    /// Correction generation, bumped whenever an adoption changes the estimate or `reset` runs. See
    /// the module docs' "Correction generation" section.
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// Subtracts the adopted skew from every wire time in `rows`, in place. A no-op while nothing
    /// has been adopted, which keeps an honest UTC core's batch byte-identical.
    ///
    /// Corrects `create_time_ms`, `sell_create_time_ms`, and `entry_fill_time_ms` — each only when
    /// present (`> 1.0`) — plus `buy_trace.points[0]` and `sell_trace.points[0]` ONLY. The rest of
    /// each trace, `tmp_point`, and `stop_time_ms` are already corrected by the core itself (see
    /// `feed/live/convert.rs::order_trace`), so touching them again would double-correct exactly the
    /// values that need no help.
    pub fn correct(&self, rows: &mut [OrderRow]) {
        if self.skew_ms == 0.0 {
            return;
        }
        for r in rows.iter_mut() {
            // Captured BEFORE the scalars move, since the trace anchor is identified by matching
            // the leg's raw create time.
            let create_raw = r.create_time_ms;
            let sell_create_raw = r.sell_create_time_ms;
            if r.create_time_ms > 1.0 {
                r.create_time_ms -= self.skew_ms;
            }
            if r.sell_create_time_ms > 1.0 {
                r.sell_create_time_ms -= self.skew_ms;
            }
            if r.entry_fill_time_ms > 1.0 {
                r.entry_fill_time_ms -= self.skew_ms;
            }
            // A trace's FIRST point is raw only while it is still the ANCHOR moonproto planted at
            // open — the leg's own `create_time`. A long-lived line is shrunk from the FRONT once
            // it outgrows its cap, and after that the first point is an ordinary tick that already
            // went through `adjust_time`. Correcting it then would push that line one whole skew
            // left of the candles, so the anchor is IDENTIFIED rather than assumed: it is the point
            // whose time still equals the leg's raw create time.
            let anchors = [
                (r.buy_trace.as_mut(), create_raw),
                (r.sell_trace.as_mut(), sell_create_raw),
            ];
            for (trace, raw_create) in anchors {
                let Some(trace) = trace else {
                    continue;
                };
                if raw_create <= 1.0 {
                    continue;
                }
                if let Some(p0) = trace.points.first_mut() {
                    if p0.time_ms == raw_create {
                        p0.time_ms -= self.skew_ms;
                    }
                }
            }
        }
    }

    /// Clears all state for a replacement feed, which may be a different MoonBot on a different
    /// clock than the connection it replaces.
    ///
    /// Bumps `generation` too, so every retained line the previous connection shifted re-derives
    /// its `create_ms` / `entry_fill_ms` from the next batch that carries it — as raw, uncorrected
    /// time, matching the reset estimate, rather than keeping the old connection's shift applied
    /// to a new one that has not been observed at all.
    pub fn reset(&mut self) {
        self.skew_ms = 0.0;
        self.first_seen_ms = None;
        self.generation = self.generation.wrapping_add(1);
    }

    /// Logs one adoption to the `channels.orders` diagnostic, gated on the winning row's own
    /// market.
    ///
    /// `CoreData` does not know its own `CoreId`, so `order_diag::follows`'s CORE-scoped form
    /// (`CORE/MARKET`) cannot be evaluated from in here — only `CoreStore`'s caller has the id, and
    /// reaching it would mean threading a diagnostic concern up through `session::store` and
    /// `session::lifecycle`. Instead this calls `follows` with an empty core string: a plain-market
    /// selector or `channels.orders = 1` still narrows and fires correctly, and only a `CORE/MARKET`
    /// selector silently fails to match — an accepted, documented degradation of this one line.
    fn diag_adoption(
        &self,
        rows: &[OrderRow],
        now_ms: f64,
        bucket: i64,
        skew_ms: f64,
        votes: u32,
        class: SampleClass,
    ) {
        let Some(market) = market_for_bucket(rows, now_ms, bucket) else {
            return;
        };
        if !crate::order_diag::follows("", market) {
            return;
        }
        crate::order_diag::line(&format!(
            "clock_skew market={market} adopted skew_ms={skew_ms} bucket_ms={bucket_ms} votes={votes} class={class}",
            bucket_ms = bucket as f64 * BUCKET_MS,
            class = class.label(),
        ));
    }
}

/// Records one sample into its rounded bucket, keyed by the sample's own uid so a bucket's vote
/// count reflects distinct orders, dropping the sample when implausible.
fn record(buckets: &mut BTreeMap<i64, BucketVotes>, sample: f64, class: SampleClass, uid: u64) {
    if !(MIN_SKEW_MS..=MAX_SKEW_MS).contains(&sample) {
        return;
    }
    let bucket = (sample / BUCKET_MS).round() as i64;
    let entry = buckets.entry(bucket).or_default();
    match class {
        SampleClass::Tight => entry.tight.insert(uid),
        SampleClass::Loose => entry.loose.insert(uid),
    };
}

/// Winning bucket for one class, or `None` when no bucket reaches `MIN_VOTES` distinct uids.
///
/// TIGHT prefers the highest-MAGNITUDE qualifying bucket: a point estimate carries no lower-bound
/// guarantee, so among several agreeing clusters the one furthest from zero is the strongest signal
/// rather than measurement noise. LOOSE prefers the greatest ALGEBRAIC value: every LOOSE sample is
/// `skew - elapsed` with `elapsed >= 0`, so the largest value among qualifying buckets is the
/// tightest lower bound on the true skew regardless of its sign.
fn best_bucket(buckets: &BTreeMap<i64, BucketVotes>, class: SampleClass) -> Option<(i64, u32)> {
    let mut best: Option<(i64, u32)> = None;
    for (&bucket, votes) in buckets {
        let n = match class {
            SampleClass::Tight => votes.tight.len() as u32,
            SampleClass::Loose => votes.loose.len() as u32,
        };
        if n < MIN_VOTES {
            continue;
        }
        let better = match best {
            None => true,
            Some((best_bucket, _)) => match class {
                SampleClass::Tight => {
                    (bucket as f64 * BUCKET_MS).abs() > (best_bucket as f64 * BUCKET_MS).abs()
                }
                SampleClass::Loose => bucket > best_bucket,
            },
        };
        if better {
            best = Some((bucket, n));
        }
    }
    best
}

/// A winning candidate below the deadband corrects to exactly `0.0` — including a TIGHT winner,
/// which is how a bucket at (or near) zero un-learns a stale nonzero estimate.
fn deadband(candidate_ms: f64) -> f64 {
    if candidate_ms.abs() >= MIN_ABS_SKEW_MS {
        candidate_ms
    } else {
        0.0
    }
}

/// First row in this batch whose own sample rounds to `target_bucket`, for the diagnostic line.
///
/// Recomputes the same formulas `observe` used to build the buckets it is searching, over the same
/// `rows` slice, so the winning bucket is always found: only rows in THIS call ever vote in it.
fn market_for_bucket(rows: &[OrderRow], now_ms: f64, target_bucket: i64) -> Option<&str> {
    for r in rows {
        if r.create_time_ms > 1.0 {
            let sample = r.create_time_ms - now_ms;
            if (sample / BUCKET_MS).round() as i64 == target_bucket {
                return Some(r.market.as_str());
            }
        }
        for trace in [r.buy_trace.as_ref(), r.sell_trace.as_ref()]
            .into_iter()
            .flatten()
        {
            if trace.points.len() >= 2 {
                let sample = trace.points[0].time_ms - trace.points[1].time_ms;
                if (sample / BUCKET_MS).round() as i64 == target_bucket {
                    return Some(r.market.as_str());
                }
            }
        }
    }
    None
}

#[cfg(test)]
mod tests;
