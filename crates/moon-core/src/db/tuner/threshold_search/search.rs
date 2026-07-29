//! Database-free core of the threshold search: random-restart coordinate descent over
//! quantile bins.
//!
//! A naive suggestion optimizes each field independently, so the combined result is not optimal
//! because fields interact. Here, the state assigns every field a range or no filter; each step
//! reoptimizes one field by testing pairs of QUANTILE edges (quantiles are actual data values;
//! the edge count is configurable) while holding the others fixed. Passes continue until
//! convergence or the 16-pass cap. The descent is greedy and can become trapped in local optima,
//! so it uses multiple RESTARTS: the first starts empty, while the others use random
//! initialization and shuffled field order; the best result wins.
//!
//! This module owns no SQL and no I/O. It exists apart from the scan so the optimizer can be
//! benchmarked, unit-tested against a brute-force reference, and parallelized — none of which is
//! possible while the search is welded to a `rusqlite` cursor.
//!
//! Restarts are independent, so they run across a worker pool. Reproducibility survives that
//! because each restart derives its own random stream from `(seed, restart index)` and the merge
//! is ordered by index rather than by completion, so the answer does not depend on scheduling.
//!
//! The sample may be split chronologically: the descent sees only the leading TRAIN rows, and
//! the rows after them are held back so the ranges can be measured on trades that had no say in
//! choosing them. The split reaches all the way into the quantile edges, which are derived from
//! the train rows alone — edges drawn over the whole column would place every threshold with
//! knowledge of the future the holdout is supposed to test.

use std::ops::Range;
use std::sync::{Arc, OnceLock};

use rayon::prelude::*;

use super::super::{range_pick, round_pair_outward};
use super::handle::SearchHandle;
use crate::db::metrics::{improvement_margin, Tally};

#[cfg(test)]
mod tests;

/// Sentinel bin for a value below the first quantile edge. Quantile edges currently include the
/// observed minimum, making this a defensive, unreachable case for derived bins.
///
/// It has to sit OUTSIDE the range of real bin indices, which is what ties this type to
/// `EDGES_MAX`: the highest index a depth of `ne` produces is `ne - 1`, so the sentinel is only
/// distinguishable while `EDGES_MAX` stays below the type's maximum. A `u8` sentinel of 255 and a
/// depth of 256 would collide, and the top bin's trades would read as "below the range" — silently
/// excluded from every filter rather than loudly wrong.
const BELOW: u16 = u16::MAX;

/// Passes one restart may take before it is cut off, converged or not.
const MAX_PASSES: usize = 16;

/// Logical CPUs this process may run on.
///
/// The one place the machine's size is read. Both the worker pool and the hard availability bar
/// use this value, so neither can classify the machine differently from the other.
///
/// It can UNDER-report: on Windows `available_parallelism` honours job-object and affinity
/// limits, so a capable machine may answer with less than it has. Everything reading this must
/// therefore degrade gracefully rather than refuse to work.
///
/// Answered ONCE per process, deliberately. The OS figure can move at runtime when a quota or an
/// affinity mask changes, and this number decides two things that are themselves fixed on first
/// use: the worker pool's thread count, and which settings the UI offers. Re-reading it would let
/// a mid-session change put those out of step — a switch disappearing while the run button still
/// carries its label, or heavy settings appearing in front of a pool sized for a smaller machine.
/// It is also read from render, where an OS query per frame is not free.
pub(super) fn logical_cores() -> usize {
    static CORES: OnceLock<usize> = OnceLock::new();
    *CORES.get_or_init(|| {
        std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(1)
    })
}

/// Threads the search may use: every logical CPU but one, and never fewer than one.
pub(super) fn worker_cores() -> usize {
    logical_cores().saturating_sub(1).max(1)
}

/// Logical cores a machine must have before the tuner offers its most expensive settings.
pub const HEAVY_SEARCH_MIN_CORES: usize = 16;

/// Whether this machine is offered the tuner's expensive settings at all: automatic field-set
/// composition, the finest quantile depth, and the higher restart ceiling.
///
/// These settings either multiply the work behind one click or admit substantially more of it.
/// Below this hard bar they are not merely scaled to the reported core count: composition is not
/// run, depth 256 is not offered, and the restart count uses its fixed lower ceiling.
///
/// The single availability predicate, so the controls and the search can never disagree about
/// what a click does. [`logical_cores`] can UNDER-report — on Windows it honours job-object and
/// affinity limits — so a capable machine confined to a few cores is not offered them. That is
/// the accepted cost of a hard bar: the process genuinely may not use more than it is told it
/// has, so a machine reporting half the bar would run these at half the bar's speed.
pub fn heavy_search_supported() -> bool {
    logical_cores() >= HEAVY_SEARCH_MIN_CORES
}

/// Worker pool for the restart fan-out, or `None` when one could not be built.
///
/// A pool of its own rather than rayon's global one, sized to leave a core free: the search is
/// started from a window that must keep repainting and answering the Stop button while it runs,
/// and saturating every core makes that window stutter for the whole run.
///
/// `None` means the fan-out falls back to rayon's GLOBAL pool, which is sized to every logical
/// CPU — the search still finishes and still answers a stop, but the free core is gone with it.
/// Building a pool only fails when the process can no longer spawn threads, so this is a
/// degraded path, not a supported mode.
fn pool() -> Option<&'static rayon::ThreadPool> {
    static POOL: OnceLock<Option<rayon::ThreadPool>> = OnceLock::new();
    POOL.get_or_init(|| {
        rayon::ThreadPoolBuilder::new()
            .num_threads(worker_cores())
            .thread_name(|i| format!("moon-tuner-{i}"))
            .build()
            .inspect_err(|e| {
                log::warn!("tuner: threshold search falls back to rayon's global pool — {e}")
            })
            .ok()
    })
    .as_ref()
}

/// Random stream seed of one restart, mixed from the base seed and the restart index.
///
/// Deriving per restart rather than drawing from one shared generator is what makes the parallel
/// fan-out reproducible: a restart's random starts depend on the seed and its own index alone,
/// never on how many draws the restarts before it happened to take.
///
/// The low bit is forced on because xorshift64* is degenerate on a zero state and the mix can
/// legitimately produce zero.
pub(super) fn restart_seed(base: u64, restart: usize) -> u64 {
    // splitmix64, the standard companion mixer for seeding a weaker generator.
    let mut z = (base ^ restart as u64).wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    (z ^ (z >> 31)) | 1
}

/// Merge two candidate restarts, keeping the one a single-threaded run would have kept.
///
/// The sequential loop replaced its best only on a STRICT improvement, so among equal profits the
/// winner is the LOWEST restart index. Stating that rule explicitly is what lets restarts finish
/// in any order and still agree with a sequential run bit for bit.
fn keep_better(
    a: Option<(usize, Outcome)>,
    b: Option<(usize, Outcome)>,
) -> Option<(usize, Outcome)> {
    match (a, b) {
        (None, other) | (other, None) => other,
        (Some(a), Some(b)) => {
            let b_wins = b.1.profit > a.1.profit || (b.1.profit == a.1.profit && b.0 < a.0);
            Some(if b_wins { b } else { a })
        }
    }
}

/// Simple PRNG (xorshift64*) that avoids a rand dependency.
///
/// The seed is supplied by the caller so the stream is reproducible for a given seed.
struct Rng(u64);

impl Rng {
    /// Draw the next 64-bit value, advancing the state.
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    /// Draw a value in `0..n`, treating `n == 0` as `n == 1`.
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
}

/// One restart's selection plus the profit it achieves, which is what restarts compete on.
///
/// The trade count is deliberately absent: it is fully determined by `sel` and the sample, and
/// the figures actually shown come from [`Search::tally`], which scores the FINAL bounds rather
/// than the bins these indices address.
pub(super) struct Outcome {
    /// Per-field quantile edge pair `(i, j)`, or `None` for "no filter on this field".
    pub(super) sel: Vec<Option<(usize, usize)>>,
    /// Total profit of the trades passing every selected range.
    pub(super) profit: f64,
}

/// Scratch buffers reused across restarts.
///
/// One restart allocates `nf * n` booleans plus several per-trade vectors. At the ceiling that
/// is thousands of allocations of megabytes each, all identical in shape, so they are allocated
/// once and reinitialized instead.
struct Workspace {
    /// Per-field pass mask, flattened to `fi * n + t` for locality.
    pass: Vec<bool>,
    /// Number of fields each trade currently fails.
    fail: Vec<u16>,
    /// Per-bin profit and count, plus their prefix sums. Sized `ne + 1`: bins then BELOW.
    bin_profit: Vec<f64>,
    bin_count: Vec<usize>,
    pre_profit: Vec<f64>,
    pre_count: Vec<usize>,
    /// Field visiting order for the current pass.
    order: Vec<usize>,
}

impl Workspace {
    /// Allocate every buffer once for a given problem shape.
    fn new(nf: usize, n: usize, ne: usize) -> Self {
        Self {
            pass: vec![true; nf * n],
            fail: vec![0; n],
            bin_profit: vec![0.0; ne + 1],
            bin_count: vec![0; ne + 1],
            pre_profit: vec![0.0; ne + 1],
            pre_count: vec![0; ne + 1],
            order: Vec::with_capacity(nf),
        }
    }
}

/// The scanned sample itself, owned once and shared by every search built over it.
///
/// Separate from [`Search`] because a search is defined by the WINDOW it fits on, and several
/// windows over one scan are a normal thing to want: each derives its own quantile edges and
/// bins from its own leading rows while reading these same columns. Copying the columns per
/// window would be megabytes apiece and would give the sample more than one owner — the shape
/// in which a window ends up scored against the wrong column.
pub(super) struct Columns {
    /// Per-trade profit, COALESCEd to zero, in chronological order.
    pub(super) profits: Vec<f64>,
    /// Per-field effective values, NULL already folded to `0.0`, same order. Retained so a final
    /// bound set can be scored on rows no descent saw — bins answer "which quantile", not "does
    /// this trade pass this range".
    pub(super) vals: Vec<Vec<f64>>,
}

/// Prepared sample: the shared columns plus the quantile bins derived from them. Built once per
/// search and shared by every restart.
///
/// Every column runs the FULL chronologically ordered sample, while the descent addresses only
/// its first `n` rows. That is the whole split: one prepared sample, a train window the search
/// may fit on, and a tail it may only be measured against afterwards.
pub(super) struct Search {
    /// The scanned sample, shared with every other search built over the same scan.
    cols: Arc<Columns>,
    /// Per-field quantile edges, `ne + 1` per field, derived from the TRAIN rows only.
    edges: Vec<Vec<f64>>,
    /// Per-field quantile bin index (or `BELOW`) for each TRAIN trade — the descent's own view,
    /// and the only column here that stops at the train window.
    bins: Vec<Vec<u16>>,
    /// Failure counts contributed by fields the caller fixed, per trade.
    base_fail: Vec<u16>,
    /// Whether a field may be searched (`false` for a caller-fixed field).
    free: Vec<bool>,
    /// Whether a field occupies a Delta2/Delta3 strategy slot.
    is_slot: Vec<bool>,
    /// Slots already consumed by caller-fixed slot fields.
    locked_slots: usize,
    /// Quantile edge count.
    ne: usize,
    /// Minimum trades a candidate range must retain.
    min_n: usize,
    /// Trades the descent may fit on: the leading rows of the sample.
    n: usize,
}

impl Search {
    /// Prepare a sample for searching.
    ///
    /// Args:
    ///     cols: The scanned sample, shared with every other window built over the same scan.
    ///     locked: Per field: `None` to search it, `Some(range)` to hold it fixed, where
    ///         `(None, None)` means the field is excluded entirely.
    ///     is_slot: Per field, whether it occupies a Delta2/Delta3 strategy slot.
    ///     min_n: Minimum trades a candidate range must retain; at least 1.
    ///     ne: Quantile edge count, already clamped by the caller.
    ///     train_n: Leading trades the descent may fit on; the rest are held back. Pass the
    ///         full length for no split.
    ///
    /// Returns:
    ///     A prepared search, or `None` when the TRAIN part holds fewer than `min_n` trades —
    ///     the holdout cannot make up for a train window too small to fit anything on.
    pub(super) fn new(
        cols: Arc<Columns>,
        locked: &[Option<(Option<f64>, Option<f64>)>],
        is_slot: Vec<bool>,
        min_n: usize,
        ne: usize,
        train_n: usize,
    ) -> Option<Self> {
        let total = cols.profits.len();
        let nf = cols.vals.len();
        let min_n = min_n.max(1);
        let n = train_n.min(total);
        if n < min_n {
            return None;
        }
        // Quantile edges and each trade's bin for every field, both derived from the TRAIN rows
        // alone — the edges because thresholds placed with knowledge of the held-back period
        // would make the holdout meaningless, and the bins because nothing outside the descent
        // reads them: a bound set is scored against the raw values in `tally`, not against bins.
        let mut edges: Vec<Vec<f64>> = Vec::with_capacity(nf);
        let mut bins: Vec<Vec<u16>> = Vec::with_capacity(nf);
        for col in &cols.vals {
            let mut sorted = col[..n].to_vec();
            sorted.sort_by(|a, b| a.total_cmp(b));
            let e: Vec<f64> = (0..=ne).map(|k| sorted[k * (n - 1) / ne]).collect();
            let b = col[..n]
                .iter()
                .map(|v| {
                    if *v < e[0] {
                        BELOW
                    } else {
                        // The last edge is inclusive, so clamp it into the top bin.
                        let mut lo = 0usize;
                        let mut hi = ne;
                        while lo + 1 < hi {
                            let m = (lo + hi) / 2;
                            if *v >= e[m] {
                                lo = m
                            } else {
                                hi = m
                            }
                        }
                        lo.min(ne - 1) as u16
                    }
                })
                .collect();
            edges.push(e);
            bins.push(b);
        }
        // Searchable fields; fixed masks remain embedded in the baseline failure counts.
        let free: Vec<bool> = (0..nf)
            .map(|fi| locked.get(fi).is_none_or(|l| l.is_none()))
            .collect();
        // Held-back rows carry these counts too: the fixed filters apply to the whole period,
        // and scoring a bound set on the tail has to honour them exactly as the descent did.
        let mut base_fail: Vec<u16> = vec![0; total];
        for (fi, col) in cols.vals.iter().enumerate() {
            let Some(Some((lo, hi))) = locked.get(fi) else {
                continue;
            };
            if lo.is_none() && hi.is_none() {
                continue;
            }
            for (t, slot) in base_fail.iter_mut().enumerate() {
                let v = col[t];
                let ok = lo.is_none_or(|l| v >= l) && hi.is_none_or(|h| v <= h);
                if !ok {
                    *slot += 1;
                }
            }
        }
        // `min_n` counts the trades a suggestion must RETAIN, so it has to be measured against
        // what the caller's own fixed filters leave behind, not against the raw train length. A
        // scope whose fixed bounds already cut it to five trades cannot honestly answer a request
        // for twenty, and returning the unfiltered baseline over those five is how a figure with
        // no statistical content acquires a confident label.
        if base_fail[..n].iter().filter(|f| **f == 0).count() < min_n {
            return None;
        }
        // Slots occupied by fixed fields reduce the available Delta2/Delta3 pair limit.
        let locked_slots: usize = (0..nf)
            .filter(|fi| {
                is_slot[*fi]
                    && matches!(locked.get(*fi), Some(Some((lo, hi))) if lo.is_some() || hi.is_some())
            })
            .count();
        Some(Self {
            cols,
            edges,
            bins,
            base_fail,
            free,
            is_slot,
            locked_slots,
            ne,
            min_n,
            n,
        })
    }

    /// Turn a restart's bin selection into the value ranges that will actually be applied.
    ///
    /// The one place that rule lives. It used to sit in the caller while the tests kept a copy
    /// of it, which meant the very disagreement this function exists to prevent — between the
    /// range the descent scored and the range that gets applied — could be reintroduced in the
    /// caller without a single test noticing.
    ///
    /// Args:
    ///     sel: Per-field bin pair, as a restart left it.
    ///     round: Whether to widen the bounds outward to three significant digits.
    ///
    /// Returns:
    ///     Field index with its inclusive `(from, to)`, omitting fields left unfiltered and
    ///     pairs that span the whole observed range, which filter nothing.
    pub(super) fn applied_ranges(
        &self,
        sel: &[Option<(usize, usize)>],
        round: bool,
    ) -> Vec<(usize, f64, f64)> {
        sel.iter()
            .enumerate()
            .filter_map(|(fi, s)| {
                let (i, j) = (*s).filter(|(i, j)| !(*i == 0 && *j == self.ne))?;
                let (mut from, mut to) = self.bound_values(fi, i, j);
                if round {
                    let e = &self.edges[fi];
                    (from, to) = round_pair_outward(from, to, e[0], e[self.ne]);
                }
                Some((fi, from, to))
            })
            .collect()
    }

    /// Trades the descent fitted on: the sample's leading rows.
    pub(super) fn train_n(&self) -> usize {
        self.n
    }

    /// The inclusive value range that selects exactly the train trades a bin pair selects.
    ///
    /// The optimizer scores a HALF-OPEN bin pair `i..j`, while a range is APPLIED — by the KPI's
    /// SQL, by the strategy it is saved into, and by [`Self::tally`] — as an inclusive
    /// `from..=to` on the value. Handing back `(edges[i], edges[j])` therefore returns a filter
    /// that also passes every trade sitting exactly on `edges[j]`, which the optimizer had
    /// excluded: with a loss parked on that edge, the returned range can score WORSE than the
    /// baseline it was chosen for beating.
    ///
    /// Taking the observed minimum and maximum of the selected train values closes that gap
    /// exactly: a trade's bin rises monotonically with its value, so every value between those
    /// two extremes belongs to the same bin span, and no value outside them does.
    ///
    /// Rounding, when the caller asks for it, then widens the pair again and can readmit an
    /// adjacent trade — but that is a decision the user made, and its cost is measured: the
    /// reported figures are tallied from the bounds AFTER rounding, so a range that rounding
    /// made worse shows up as a worse number rather than as a silent discrepancy.
    ///
    /// Args:
    ///     fi: Field the pair belongs to.
    ///     i: First selected bin.
    ///     j: One past the last selected bin.
    ///
    /// Returns:
    ///     The lowest and highest train value the pair covers.
    pub(super) fn bound_values(&self, fi: usize, i: usize, j: usize) -> (f64, f64) {
        let (mut lo, mut hi) = (f64::INFINITY, f64::NEG_INFINITY);
        for t in 0..self.n {
            let b = self.bins[fi][t];
            if b != BELOW && (i..j).contains(&(b as usize)) {
                lo = lo.min(self.cols.vals[fi][t]);
                hi = hi.max(self.cols.vals[fi][t]);
            }
        }
        if lo > hi {
            // No train trade lands in the pair. Unreachable for a SELECTED pair, which had to
            // retain at least `min_n`; falling back to the edges keeps a reversed range from
            // ever reaching a caller.
            (self.edges[fi][i], self.edges[fi][j])
        } else {
            (lo, hi)
        }
    }

    /// Score a final bound set over a slice of the sample.
    ///
    /// The test is the INCLUSIVE `from..=to` on the field's own value — the same predicate the
    /// variant KPI issues as SQL — rather than bin membership, so the figures shown next to a
    /// suggestion describe the filter the user is about to apply, rounding included. The
    /// caller-fixed filters are honoured through the baseline failure counts.
    ///
    /// Args:
    ///     applied: Field index with the inclusive bounds finally chosen for it.
    ///     rows: Sample rows to aggregate, in their stored chronological order.
    ///
    /// Returns:
    ///     Totals, win split and drawdown of the trades passing every bound.
    pub(super) fn tally(&self, applied: &[(usize, f64, f64)], rows: Range<usize>) -> Tally {
        let mut out = Tally::default();
        for t in rows {
            let passes = self.base_fail[t] == 0
                && applied.iter().all(|(fi, from, to)| {
                    let v = self.cols.vals[*fi][t];
                    v >= *from && v <= *to
                });
            if passes {
                out.push(self.cols.profits[t]);
            }
        }
        out
    }

    /// Number of fields in the sample.
    fn field_count(&self) -> usize {
        self.bins.len()
    }

    /// Fields this search is allowed to optimize at all, i.e. the ones the caller left unfixed.
    ///
    /// The candidate universe for anything choosing a SUBSET of the fields: a field the caller
    /// pinned or excluded is not a candidate, and asking for it cannot make it one.
    pub(super) fn free_mask(&self) -> &[bool] {
        &self.free
    }

    /// Per field, whether it occupies a Delta2/Delta3 strategy slot.
    ///
    /// Borrowed rather than copied by anything that needs it: a second vector would have to agree
    /// with this one for a caller's slot arithmetic to match the descent's own.
    pub(super) fn slot_flags(&self) -> &[bool] {
        &self.is_slot
    }

    /// Delta2/Delta3 slots already consumed by caller-fixed fields, which is how many of the two
    /// the search itself may still hand out.
    pub(super) fn locked_slots(&self) -> usize {
        self.locked_slots
    }

    /// Run `restarts` restarts over every searchable field. See [`Self::run_masked`].
    pub(super) fn run(&self, restarts: usize, seed: u64, handle: &SearchHandle) -> Option<Outcome> {
        self.run_masked(&self.free, restarts, seed, handle)
    }

    /// Run `restarts` restarts across the worker pool, over the fields `allow` admits, and return
    /// the best outcome found.
    ///
    /// `allow` is INTERSECTED with the searchable fields rather than replacing them: a caller
    /// admitting a field it also pinned would otherwise have that field optimized on top of its
    /// own fixed filter, which is neither what it asked for nor a state the rest of the search
    /// accounts for.
    ///
    /// Restart 0 is greedy from an empty state and therefore deterministic; every other restart
    /// draws its random initialization and per-pass field order from its OWN stream, derived by
    /// [`restart_seed`] from `seed` and the restart index. That stream depends on the SEARCHABLE
    /// fields, never on `allow` — see [`Self::one_restart`] — so two masks over the same search
    /// give their shared fields the same random starts and are therefore comparable, and a mask
    /// admitting everything reproduces a plain run bit for bit.
    ///
    /// Cancellation through `handle` is honoured between restarts and between fields inside one.
    /// An abandoned restart contributes nothing, so the answer is the best of those that
    /// COMPLETED — a stopped search still returns what it had found. `handle.completed()` is
    /// cumulative across every call sharing that handle, while `handle.abandoned()` says whether
    /// any requested unit was cut short.
    ///
    /// Every parallel job builds a [`Workspace`] of its own — a worker may run several jobs, and
    /// so hold several over a run. The buffers are per-restart scratch, and sharing one across
    /// threads is exactly what makes the fan-out unsound.
    pub(super) fn run_masked(
        &self,
        allow: &[bool],
        restarts: usize,
        seed: u64,
        handle: &SearchHandle,
    ) -> Option<Outcome> {
        let (nf, n, ne) = (self.field_count(), self.n, self.ne);
        let allow: Vec<bool> = (0..nf).map(|fi| self.free[fi] && allow[fi]).collect();
        // A workspace is allocated per rayon JOB, not per thread, and over a large report it is
        // close to a megabyte. Floor the job size at the pool's share of the restarts: rayon may
        // still split further, so this bounds the count near the worker count rather than capping
        // it exactly, but it keeps a SHORT run from splitting down to one restart per job.
        // Composition turns one long call into hundreds of short ones, so a floor tied to the
        // restart count alone stops protecting exactly where it is needed most.
        let chunk = (restarts / worker_cores()).max(1);
        let search = || {
            (0..restarts)
                .into_par_iter()
                .with_min_len(chunk)
                .map_init(
                    || Workspace::new(nf, n, ne),
                    |w, restart| {
                        if handle.is_cancelled() {
                            handle.note_abandoned();
                            return None;
                        }
                        let mut rng = Rng(restart_seed(seed, restart));
                        let Some(outcome) = self.one_restart(&allow, restart, &mut rng, w, handle)
                        else {
                            handle.note_abandoned();
                            return None;
                        };
                        handle.record_restart();
                        Some((restart, outcome))
                    },
                )
                .reduce(|| None, keep_better)
                .map(|(_, outcome)| outcome)
        };
        match pool() {
            Some(pool) => pool.install(search),
            None => search(),
        }
    }

    /// Run one restart of coordinate descent to convergence or the pass cap, over the fields
    /// `allow` admits.
    ///
    /// `allow` is expected to be already intersected with [`Self::free_mask`] by the caller.
    ///
    /// Every random draw is taken for every SEARCHABLE field, admitted or not, and only the
    /// ASSIGNMENT is withheld. That is what makes the stream a property of the search rather than
    /// of the mask: two masks over the same search hand their shared fields identical starts, so
    /// the outcomes are comparable, and a mask admitting everything reproduces a plain run bit for
    /// bit — which is what keeps a seed the user pinned earlier replaying to the same answer.
    ///
    /// Returns `None` when cancellation cut the restart short: its state is mid-descent and not
    /// comparable with a converged one, so it is dropped rather than scored.
    fn one_restart(
        &self,
        allow: &[bool],
        restart: usize,
        rng: &mut Rng,
        w: &mut Workspace,
        handle: &SearchHandle,
    ) -> Option<Outcome> {
        let nf = self.field_count();
        let (n, ne) = (self.n, self.ne);
        let mut sel: Vec<Option<(usize, usize)>> = vec![None; nf];
        if restart > 0 {
            // About 30% of fields start with a random range, with at most two slots.
            let mut slots_used = 0usize;
            for (fi, s) in sel.iter_mut().enumerate() {
                if !self.free[fi] {
                    continue;
                }
                if rng.below(100) < 30 {
                    if self.is_slot[fi] {
                        if slots_used >= 2 {
                            continue;
                        }
                        slots_used += 1;
                    }
                    let i = rng.below(ne);
                    let j = i + 1 + rng.below(ne - i);
                    if allow[fi] {
                        *s = Some((i, j));
                    }
                }
            }
        }
        // Masks from initialization, layered over fixed filters. `base_fail` spans the whole
        // sample while the workspace spans the train window, so only its head is copied.
        w.pass.fill(true);
        w.fail.copy_from_slice(&self.base_fail[..n]);
        for (fi, chosen) in sel.iter().enumerate() {
            if let Some((i, j)) = *chosen {
                let base = fi * n;
                for t in 0..n {
                    let b = self.bins[fi][t];
                    let ok = b != BELOW && (i..j).contains(&(b as usize));
                    if !ok {
                        w.fail[t] += 1;
                    }
                    w.pass[base + t] = ok;
                }
            }
        }
        // Ordered over every FREE field, not just the allowed ones, for the same reason the
        // initialization above draws for all of them: the shuffle below consumes one draw per
        // element, so a shorter vector would spend a mask-dependent number of draws and shift
        // every later value. Two runs of one seed differing by a single candidate field would
        // then visit the SHARED fields in different orders and descend to different local optima
        // — and composition would be reading that difference as the candidate's contribution.
        w.order.clear();
        w.order.extend((0..nf).filter(|fi| self.free[*fi]));
        for _pass_no in 0..MAX_PASSES {
            if restart > 0 {
                // Fisher-Yates gives each pass its own field order.
                for k in (1..w.order.len()).rev() {
                    w.order.swap(k, rng.below(k + 1));
                }
            }
            let mut changed = false;
            for oi in 0..w.order.len() {
                // Between fields, not only between passes: one pass over a large report is
                // seconds of work, and a Stop button that takes seconds to answer reads as hung.
                if handle.is_cancelled() {
                    return None;
                }
                let fi = w.order[oi];
                // Its draws were spent above; only the VISIT is withheld.
                if !allow[fi] {
                    continue;
                }
                let best = self.best_for_field(fi, &sel, w);
                if best != sel[fi] {
                    self.apply_field(fi, best, w);
                    sel[fi] = best;
                    changed = true;
                }
            }
            if !changed {
                break;
            }
        }
        self.drop_redundant(allow, &mut sel, w);
        // Score this restart's result over the train window it was fitted on.
        let mut profit = 0.0f64;
        for t in 0..n {
            if w.fail[t] == 0 {
                profit += self.cols.profits[t];
            }
        }
        Some(Outcome { sel, profit })
    }

    /// Install a new range for one field, updating its mask, the failure counts, and the
    /// candidate list.
    fn apply_field(&self, fi: usize, best: Option<(usize, usize)>, w: &mut Workspace) {
        let base = fi * self.n;
        for t in 0..self.n {
            let ok = match best {
                None => true,
                Some((i, j)) => {
                    let b = self.bins[fi][t];
                    b != BELOW && (i..j).contains(&(b as usize))
                }
            };
            if ok != w.pass[base + t] {
                if ok {
                    w.fail[t] -= 1;
                } else {
                    w.fail[t] += 1;
                }
                w.pass[base + t] = ok;
            }
        }
    }

    /// Best edge pair for one field, holding every other field fixed.
    ///
    /// Returns `None` when no range beats leaving the field unfiltered.
    fn best_for_field(
        &self,
        fi: usize,
        sel: &[Option<(usize, usize)>],
        w: &mut Workspace,
    ) -> Option<(usize, usize)> {
        let ne = self.ne;
        let base = fi * self.n;
        // Only two Delta2/Delta3 slots exist. If two other slots already have ranges, this field
        // may only choose no filter — decided BEFORE the per-trade scan below, since nothing that
        // scan computes can change the answer. `FIELDS` carries six slot fields for two slots, so
        // once both are taken this saves a full pass over the train window on every visit to each
        // of the other four.
        let slot_full = self.is_slot[fi]
            && self.locked_slots
                + sel
                    .iter()
                    .enumerate()
                    .filter(|(o, s)| *o != fi && self.is_slot[*o] && s.is_some())
                    .count()
                >= 2;
        if slot_full {
            return None;
        }
        // Sum bins among trades that pass every other field.
        w.bin_profit.fill(0.0);
        w.bin_count.fill(0);
        let (mut tot_p, mut tot_c) = (0.0f64, 0usize);
        for t in 0..self.n {
            let others_ok = w.fail[t] == u16::from(!w.pass[base + t]);
            if !others_ok {
                continue;
            }
            tot_p += self.cols.profits[t];
            tot_c += 1;
            let b = self.bins[fi][t];
            let idx = if b == BELOW { ne } else { b as usize };
            w.bin_profit[idx] += self.cols.profits[t];
            w.bin_count[idx] += 1;
        }
        // Prefix sums over the real bins; the BELOW bucket at index `ne` is deliberately left
        // out, because no contiguous edge range can include it.
        w.pre_profit[0] = 0.0;
        w.pre_count[0] = 0;
        for k in 0..ne {
            w.pre_profit[k + 1] = w.pre_profit[k] + w.bin_profit[k];
            w.pre_count[k + 1] = w.pre_count[k] + w.bin_count[k];
        }
        // The no-filter candidate is the baseline. A range must beat it beyond floating-point
        // noise because bin sums and `tot_p` accumulate in different orders; otherwise an
        // equivalent trade set can win by about 1e-12 and leave a no-op filter.
        let floor = tot_p + improvement_margin(tot_p);
        range_pick::best_pair(
            &w.pre_profit[..=ne],
            &w.pre_count[..=ne],
            self.min_n,
            tot_c,
            floor,
        )
    }

    /// Drop ranges that are jointly redundant.
    ///
    /// Descent may hit the pass limit before removing a field whose rejected trades are already
    /// rejected by OTHER filters. A field uniquely rejects row `t` only when `!pass && fail == 1`.
    /// Such a range is a no-op: removing it changes neither profit nor count. Repeat until stable.
    fn drop_redundant(
        &self,
        allow: &[bool],
        sel: &mut [Option<(usize, usize)>],
        w: &mut Workspace,
    ) {
        loop {
            let mut removed = false;
            for (fi, chosen) in sel.iter_mut().enumerate() {
                if !allow[fi] || chosen.is_none() {
                    continue;
                }
                let base = fi * self.n;
                // Only a trade this field uniquely rejects can refute redundancy.
                let redundant = (0..self.n).all(|t| w.pass[base + t] || w.fail[t] > 1);
                if redundant {
                    self.apply_field(fi, None, w);
                    *chosen = None;
                    removed = true;
                }
            }
            if !removed {
                break;
            }
        }
    }
}
