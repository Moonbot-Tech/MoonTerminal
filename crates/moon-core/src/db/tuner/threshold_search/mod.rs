//! Automatic threshold tuning for the "By filter" axis: which report fields to use and which
//! ranges on them maximize period profit.
//!
//! This module owns the database side — one scan of the tuner source into column-oriented
//! memory — and hands the result to the DB-free threshold optimizer in [`search`] and field-set
//! selector in [`compose`]. Semantics match tuner variants: NULL fields equal 0 (COALESCE), and
//! bounds are inclusive. The database is scanned once and all later work stays in memory, so call
//! this ONLY from a background executor.
//!
//! The sample is ordered chronologically here, which is what makes a train/holdout split and a
//! drawdown figure mean anything at all: both read the sequence, not the set.

mod compose;
mod handle;
mod search;

#[cfg(test)]
mod tests;

pub use compose::{composition_budget, ComposeBudget};
pub use handle::SearchHandle;
pub use search::{heavy_search_supported, HEAVY_SEARCH_MIN_CORES};

use std::cmp::Ordering;
use std::sync::Arc;

use rusqlite::Connection;

use super::{FieldClass, FIELDS};
use crate::db::analytics::Query;
use crate::db::metrics::Tally;
use crate::db::read_fail::read_fail;
use crate::db::ReadResult;

/// Minimum accepted restart count. Shared with the UI so displayed and executed counts agree.
pub const RESTARTS_MIN: usize = 1;

/// Maximum accepted restart count on a machine that clears [`HEAVY_SEARCH_MIN_CORES`].
///
/// Restarts run across a worker pool, report their progress, and stop on request, so the ceiling
/// is no longer "what a user can be asked to wait through blind" — it is a guard against a typo
/// becoming an unbounded run. Work is strictly linear in the count: one restart is `FIELDS.len()`
/// fields × up to 16 passes × (`n` trades + the edge search), measured at ~5 ms of single-core
/// time per restart over a 26k-trade report at depth 64. Spread across the pool of a machine at
/// or above the bar that is on the order of a few seconds — visibly progressing and interruptible,
/// which is what the ceiling is for: it guards against a typo, not against the wait.
pub const RESTARTS_MAX: usize = 100_000;

/// Maximum accepted restart count below that bar.
///
/// This is the fixed lower tier of the same hard machine bar that gates composition and the
/// finest quantile depth, not a budget scaled continuously with the detected core count.
/// Restarts have sharply diminishing returns — they explore starts, and the best of many is
/// quickly the best of most — so the number that changes here is the worst case, not the answer.
pub const RESTARTS_MAX_LIGHT: usize = 10_000;

/// The restart ceiling on THIS machine.
///
/// Read this, never [`RESTARTS_MAX`] directly, anywhere a count is clamped or a limit is shown:
/// the box would otherwise accept and display a number the search then quietly rewrote.
pub fn restarts_max() -> usize {
    restarts_max_for(heavy_search_supported())
}

/// [`restarts_max`] with the machine class supplied, so both branches can be checked in one test
/// run — a given machine only ever exercises one of them.
fn restarts_max_for(heavy: bool) -> usize {
    if heavy {
        RESTARTS_MAX
    } else {
        RESTARTS_MAX_LIGHT
    }
}

/// Minimum quantile-edge count accepted by the search and exposed to the UI.
pub const EDGES_MIN: usize = 4;

/// Maximum quantile-edge count on a machine that clears [`HEAVY_SEARCH_MIN_CORES`]; keeps bin
/// indices clear of the search's `BELOW` sentinel.
///
/// The bin index is a `u16` and the sentinel is its maximum, so a depth up to 65 535 stays
/// distinguishable. The ceiling is set far below that by cost, not by the encoding: the edge
/// search is linear in the depth while the per-trade scan is not, so finer slicing buys
/// increasingly little and slices the sample into groups too small to mean anything.
pub const EDGES_MAX: usize = 512;

/// Maximum quantile-edge count below that bar — two choices coarser, since each finer depth
/// doubles the edges every pass of every restart walks.
pub const EDGES_MAX_LIGHT: usize = 128;

/// The quantile-depth ceiling on THIS machine.
///
/// Enforced here and not only in the dropdown, for the same reason as [`restarts_max`]: a depth
/// saved on a bigger machine reaches this function through `layout.toml` whatever the UI offers,
/// and the bar is meant to be a property of the search, not of one control.
pub fn edges_max() -> usize {
    edges_max_for(heavy_search_supported())
}

/// [`edges_max`] with the machine class supplied, so both branches can be checked in one test run
/// — a given machine only ever exercises one of them. The same shape as [`restarts_max_for`], on
/// purpose: one question, one idiom.
fn edges_max_for(heavy: bool) -> usize {
    if heavy {
        EDGES_MAX
    } else {
        EDGES_MAX_LIGHT
    }
}

/// Final range for one field.
#[derive(Clone, Debug)]
pub struct FieldRange {
    /// Report field the range constrains.
    pub field: &'static str,
    /// Inclusive lower bound.
    pub from: f64,
    /// Inclusive upper bound.
    pub to: f64,
}

/// The field set composition chose, reported through the ranges that were actually applied.
///
/// Keyed to the applied ranges rather than to composition's own mask on purpose: those are two
/// definitions of "selected" that a reduced-budget ranking followed by a full-budget refit can
/// legitimately disagree on, and a field badged as chosen beside an empty threshold box is the
/// shape of that disagreement reaching the user.
#[derive(Clone, Debug)]
pub struct ComposedSet {
    /// Which of the three independently evaluated search paths won.
    pub decision: ComposeDecision,
    /// Chosen fields that the final refit gave a range, each with the number of folds that also
    /// gave it one — how reproducible its contribution is across the period.
    pub fields: Vec<(&'static str, u8)>,
    /// Folds those counts are out of.
    pub folds: u8,
    /// Fields composition chose that the final refit then left unfiltered. Normally zero; a
    /// standing non-zero count means the ranking budget is too small to agree with the refit.
    pub dropped_at_refit: usize,
}

/// Final path selected by composition's reserved gate folds.
///
/// These are equal candidate answers, not a primary answer plus error recovery: composition
/// explicitly scores all three and reports the strongest supported one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ComposeDecision {
    /// A strict non-empty subset of the admitted fields ranked strongest across the gate folds.
    ReducedSet,
    /// Joint search across every field admitted by the checkboxes was strongest.
    AllAllowedFields,
    /// Applying no additional report-field threshold was strongest.
    NoAdditionalFilters,
}

/// Why a requested composition did not run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ComposeSkip {
    /// The fitting region could not be cut into at least four walk-forward folds — too few
    /// trades, or too few distinct close timestamps to cut between. Composition needs at least
    /// two folds to select a candidate and two untouched folds to compare the three paths.
    TooFewFolds,
    /// No strategy was selected, so the sample is the whole replica rather than one strategy on
    /// the cores it runs on. Thresholds are never fitted across that mixture, and a set composed
    /// from it would describe the pooling instead of a strategy.
    NoStrategyScope,
}

/// Search result: the ranges plus what they achieve, split by whether the search was allowed to
/// see the trades being measured.
#[derive(Clone, Debug)]
pub struct SearchResult {
    /// Inclusive field ranges selected by the best completed restart.
    pub fields: Vec<FieldRange>,
    /// What the ranges achieve on the trades they were FITTED on. Flattering by construction —
    /// the search maximized exactly this number.
    pub train: Tally,
    /// What they achieve on the trades held back from the search. `None` when nothing was held
    /// back, i.e. the whole period was fitted on. This is the only figure here that was not
    /// chosen for, so it is the one that says whether the ranges found a pattern or the noise.
    pub holdout: Option<Tally>,
    /// Base seed the restarts were derived from, so a result can be reproduced exactly.
    ///
    /// For a single all-fields search, the caller-owned [`SearchHandle`] carries the
    /// completed-restart count
    /// instead of duplicating it here. Under composition that counter spans all ranking runs and
    /// folds, so it remains progress rather than metadata about the final refit.
    pub seed: u64,
    /// The field set composition chose, or `None` when composition was not asked for or could
    /// not run.
    pub composed: Option<ComposedSet>,
    /// Why composition did not run, when it WAS asked for. The ranges above then come directly
    /// from the all-fields path, so the caller has an answer and a reason rather than a silence.
    pub compose_skipped: Option<ComposeSkip>,
}

/// Inputs of one threshold search, beyond the report scope itself.
pub struct SearchParams<'a> {
    /// Restart count, clamped to `RESTARTS_MIN..=`[`restarts_max`] — the ceiling is lower on a
    /// machine below the heavy-search bar.
    pub restarts: usize,
    /// Minimum trades a suggested combination must retain, or `None` for one tenth of the trades
    /// the search actually fits on.
    ///
    /// The automatic value is resolved HERE rather than by the caller because it is a share of
    /// the TRAIN window, and only this module knows how big that ended up: the requested share is
    /// snapped to a change of `closedate`, and a period that closed within one timestamp is not
    /// split at all. A caller working it out from the period length would be answering about a
    /// different sample.
    pub min_n: Option<i64>,
    /// Per field: `None` to search it, a fixed range to hold it, `(None, None)` to exclude it.
    pub locked: &'a [Option<(Option<f64>, Option<f64>)>],
    /// Quantile resolution per field, clamped to `EDGES_MIN..=`[`edges_max`] for this machine.
    pub edges: usize,
    /// Whether to round the resulting bounds outwards.
    pub round: bool,
    /// Base seed of the random restarts; `None` draws a fresh one from the clock.
    pub seed: Option<u64>,
    /// Share of the period, oldest trades first, the search may fit on. `1.0` fits on
    /// everything; anything lower holds the remaining tail back as a holdout. Clamped so a
    /// split always leaves at least one trade on each side.
    pub train_frac: f64,
    /// Whether the FIELD SET is chosen automatically as well as the thresholds.
    ///
    /// With this off, every field `locked` leaves free is searched and the descent keeps
    /// whichever ones improve the fit — the right tool when the user has already decided which
    /// fields belong. With it on, the set is composed out-of-sample first (see [`compose`]) and
    /// only the composed fields are refitted.
    ///
    /// Honoured only on a machine that clears [`HEAVY_SEARCH_MIN_CORES`]; below that it is
    /// ignored and the all-fields search runs, because the feature is not offered there at all.
    pub compose: bool,
}

/// Maximize combined profit with random-restart coordinate descent while retaining at least
/// `params.min_n` trades, or — where that is `None` — one tenth of the trades fitted on.
///
/// Each attempt runs at most 16 passes. At most two Delta2/3 slot fields may carry ranges because
/// the strategy format cannot store more.
///
/// `handle` stops the search and carries its progress; build a fresh one per call. A stopped
/// search returns the best complete answer it reached. For an all-fields search, `completed()` is
/// the number of completed restarts behind that answer; during composition it is an internal
/// work counter spanning candidate rankings, folds, and the final refit.
///
/// With `params.train_frac` below 1 the search only ever sees the oldest share of the period and
/// the rest becomes the result's holdout, which is what separates a range that found something
/// from one that memorized the sample it was handed.
///
/// `Ok(None)` means the sample is too small, or the search was stopped before it had an answer;
/// `NotReady` means the replica or required schema is absent, while `Failed` means opening or
/// scanning the replica failed.
pub fn suggest(
    q: &Query,
    params: SearchParams<'_>,
    handle: &SearchHandle,
) -> ReadResult<Option<SearchResult>> {
    let ne = params.edges.clamp(EDGES_MIN, edges_max());
    // Stopped mid-scan: nothing was searched, so there is nothing to report.
    let Some((profits, vals, closes)) = scan(q, handle)? else {
        return Ok(None);
    };
    let is_slot: Vec<bool> = FIELDS
        .iter()
        .map(|s| s.class == FieldClass::DeltaSlot)
        .collect();
    let total = profits.len();
    let requested_train = train_split(&closes, params.train_frac);
    // One tenth of what the descent will actually see, which is why this waits for the snapped
    // split rather than being handed down from the period length.
    let min_n = params.min_n.unwrap_or((requested_train / 10) as i64).max(1) as usize;
    // One owner for the scan, so a search is defined by the window it fits on rather than by a
    // copy of the sample.
    let cols = Arc::new(search::Columns { profits, vals });
    // Not enough sample — a legitimate "no suggestion", not a failure.
    let Some(search) = search::Search::new(
        cols.clone(),
        params.locked,
        is_slot.clone(),
        min_n,
        ne,
        requested_train,
    ) else {
        return Ok(None);
    };
    let restarts = params.restarts.clamp(RESTARTS_MIN, restarts_max());
    // Without a chosen seed, take one from the clock so repeated searches explore different
    // starts. Restart 0 is greedy from empty and stays deterministic either way.
    let seed = params.seed.unwrap_or_else(|| {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0)
    });
    // Read back from the search rather than reusing `requested_train`: everything below must
    // describe the window it actually fitted on, not the one it was asked for.
    let train_n = search.train_n();
    // Compose the field set first, when asked. It runs entirely inside `0..train_n`, so the
    // holdout tallied at the end stays a period nothing here optimized against.
    //
    // A machine below the bar has no budget, and composition simply does not happen — no skip
    // reason either. The switch does not exist there, so a request can only arrive from a
    // settings file carried over from a larger machine, and a note explaining a feature this
    // build never shows would be a worse answer than directly running the all-fields path.
    let composed = params
        .compose
        .then(composition_budget)
        .flatten()
        .map(|budget| {
            // The scope rule, enforced where composition lives rather than trusted to the UI.
            // An empty strategy list means the whole replica — dozens of cores and many
            // strategies trading different situations — and across that mixture no report field
            // relates to profit at all. A search over it still returns a confident-looking set,
            // chosen from correlations that belong to the pooling rather than to any strategy.
            if q.strategies.is_empty() {
                return Err(ComposeSkip::NoStrategyScope);
            }
            compose_set(
                &cols, &params, &is_slot, &closes, min_n, ne, train_n, seed, budget, handle,
            )
        });
    let (allow, skipped) = match &composed {
        // Stopped before it had a set. Refitting on whatever it had reached would answer with a
        // set nobody finished choosing, so this is a stop like any other.
        Some(Ok(None)) => return Ok(None),
        Some(Ok(Some(out))) => (Some(out.chosen.clone()), None),
        Some(Err(why)) => (None, Some(*why)),
        None => (None, None),
    };
    // Composition's per-step progress belongs to composition. Leaving the last stage published
    // through the refit would freeze the caption on a field the search is no longer weighing.
    handle.clear_stage();
    // The final refit always runs at the FULL restart count the user asked for: composition's
    // reduced budget exists to ORDER candidates, never to decide the thresholds that get applied.
    let run = match &allow {
        Some(mask) => search.run_masked(mask, restarts, seed, handle),
        None => search.run(restarts, seed, handle),
    };
    // No restart completed: too small a sample was already handled above, so this is a stop.
    let Some(outcome) = run else {
        return Ok(None);
    };
    // The ranges in the form the scorer needs them, derived by the search itself so the figures
    // reported below describe exactly the filter the user is about to apply.
    let applied = search.applied_ranges(&outcome.sel, params.round);
    let fields = applied
        .iter()
        .map(|(fi, from, to)| FieldRange {
            field: FIELDS[*fi].col,
            from: *from,
            to: *to,
        })
        .collect();
    Ok(Some(SearchResult {
        fields,
        train: search.tally(&applied, 0..train_n),
        holdout: (train_n < total).then(|| search.tally(&applied, train_n..total)),
        seed,
        composed: match composed {
            Some(Ok(Some(out))) => Some(composed_set(&out, &applied)),
            _ => None,
        },
        compose_skipped: skipped,
    }))
}

/// Report a composition through the ranges the final refit actually applied.
///
/// A chosen field the refit left unfiltered is counted, not listed: the user is shown the set
/// their thresholds came from, and a badge beside an empty threshold box would describe a
/// different set than the grid does.
fn composed_set(out: &compose::ComposeOutcome, applied: &[(usize, f64, f64)]) -> ComposedSet {
    let fields: Vec<(&'static str, u8)> = applied
        .iter()
        .filter(|(fi, _, _)| out.chosen[*fi])
        .map(|(fi, _, _)| (FIELDS[*fi].col, out.support[*fi]))
        .collect();
    ComposedSet {
        decision: out.decision,
        dropped_at_refit: out.chosen.iter().filter(|c| **c).count() - fields.len(),
        fields,
        folds: out.folds,
    }
}

/// Cut the fitting region into walk-forward folds and compose the field set on them.
///
/// Every fold is a search over the SAME scanned columns with its own shorter fit prefix, so each
/// derives its own quantile edges from its own rows — edges taken from the whole fitting region
/// would place a fold's thresholds using the very stretch that fold exists to be measured on.
///
/// `Err` means composition was asked for and could not honestly run; the caller then executes
/// the all-fields path directly and reports the reason. `Ok(None)` means it was stopped.
///
/// Args:
///     cols: One shared columnar scan of the selected report rows.
///     params: User search settings and fixed field masks.
///     is_slot: Fields that consume a Delta2/Delta3 slot.
///     closes: Chronological close timestamps used to cut folds.
///     min_n: Minimum retained trades in the full fitting region.
///     ne: Quantile edge count.
///     train_n: Rows available to fitting and composition.
///     seed: Base restart seed.
///     budget: Machine-specific fold, depth, adaptive-beam, seed-group, and ranking limits.
///     handle: Cancellation and progress channel.
///
/// Returns:
///     A completed composed set, an honest skip reason, or `None` when cancelled.
#[allow(clippy::too_many_arguments)]
fn compose_set(
    cols: &Arc<search::Columns>,
    params: &SearchParams<'_>,
    is_slot: &[bool],
    closes: &[i64],
    min_n: usize,
    ne: usize,
    train_n: usize,
    seed: u64,
    budget: ComposeBudget,
    handle: &SearchHandle,
) -> Result<Option<compose::ComposeOutcome>, ComposeSkip> {
    let folds = build_folds(
        cols,
        params.locked,
        is_slot,
        closes,
        min_n,
        ne,
        train_n,
        budget.folds,
    );
    // At least two folds select the subset candidate (three under the standard budget); the final
    // two stay out of that ranking and compare the subset, all-fields, and no-filter paths.
    if folds.len() < 4 {
        return Err(ComposeSkip::TooFewFolds);
    }
    let p = compose::ComposeParams {
        ranking_restarts: budget
            .ranking_restarts(params.restarts.clamp(RESTARTS_MIN, restarts_max())),
        gate_restarts: params.restarts.clamp(RESTARTS_MIN, restarts_max()),
        seed,
        round: params.round,
        max_fields: budget.max_fields,
        beam_width_min: budget.beam_width_min,
        beam_width_max: budget.beam_width_max,
        seed_groups: budget.seed_groups,
    };
    Ok(compose::compose(&folds, &p, handle))
}

/// Build the walk-forward folds composition is scored on.
///
/// Separated from [`compose_set`] so the fold's OWN `fit_end` — the argument that decides whether
/// a fold's thresholds were placed knowing the rows that fold exists to be measured on — sits on a
/// path a test can drive without rebuilding it. A test that assembles its own folds proves only
/// that [`search::Search`] honours the window it was handed, which was never in doubt.
///
/// Folds that cannot be built are dropped, so the caller counts what survived rather than what
/// was asked for.
#[allow(clippy::too_many_arguments)]
fn build_folds(
    cols: &Arc<search::Columns>,
    locked: &[Option<(Option<f64>, Option<f64>)>],
    is_slot: &[bool],
    closes: &[i64],
    min_n: usize,
    ne: usize,
    train_n: usize,
    k: usize,
) -> Vec<compose::Fold> {
    fold_cuts(closes, train_n, k)
        .into_iter()
        .filter_map(|(fit_end, validate_end)| {
            // `min_n` counts trades a suggestion must RETAIN, so on a fold fitting on part of the
            // region it scales with that part — held at its absolute value it would ask a half-
            // sized fold for the selectivity of a full one and refuse to build at all.
            let fold_min_n = (min_n * fit_end / train_n.max(1)).max(1);
            // `fit_end`, never `train_n`: the fold's quantile edges must come from its own rows.
            let search = search::Search::new(
                cols.clone(),
                locked,
                is_slot.to_vec(),
                fold_min_n,
                ne,
                fit_end,
            )?;
            Some(compose::Fold {
                search,
                validate: fit_end..validate_end,
            })
        })
        .collect()
}

/// Leading trades the search may fit on for a requested train share.
///
/// A split always leaves at least one trade on each side: a holdout of zero trades would report
/// metrics over nothing, and a train window of zero has nothing to fit. A share that is not a
/// real number below 1 — including the default 1.0 — means no split at all.
///
/// The boundary is then SNAPPED to a change of `closedate`, and that is not a detail. Trades
/// sharing a timestamp are ordered by profit (the only total order available — see
/// [`chronological_order`]), so a boundary falling inside such a group would divide it by the
/// very quantity being predicted: the losses to one side, the wins to the other. The holdout
/// would then report a number that was decided by the sort, not by the ranges. Snapping moves
/// the whole group to one side, where its internal order cannot be observed across the split.
///
/// A sample whose trades ALL share one timestamp has no boundary to snap to, and no later period
/// to hold back — so it is not split at all.
fn train_split(closes: &[i64], frac: f64) -> usize {
    let n = closes.len();
    if n < 2 || !frac.is_finite() || frac >= 1.0 {
        return n;
    }
    let target = ((n as f64 * frac.max(0.0)).round() as usize).clamp(1, n - 1);
    // No boundary anywhere means every trade closed at the same instant: nothing to hold back,
    // so the sample is not split at all.
    snap_to_close_change(closes, target, n - 1).unwrap_or(n)
}

/// The nearest index to `target`, within `1..=limit`, where `closedate` changes — i.e. a cut that
/// falls BETWEEN two groups of equally-timed trades rather than through one.
///
/// Searches forward first, then backward, so a cut lands as close to what was asked for as the
/// data allows. Returns `None` when the whole span up to `limit` shares one timestamp and there
/// is no such boundary at all.
///
/// The `None` is the point of this signature. Its two callers want opposite things from that
/// case — [`train_split`] declines to split, while [`fold_cuts`] drops the fold — and an
/// unsplittable span answered with a silent fallback would give one of them a cut it explicitly
/// must not make.
fn snap_to_close_change(closes: &[i64], target: usize, limit: usize) -> Option<usize> {
    debug_assert!(target >= 1, "callers clamp; index `k - 1` below needs it");
    let changes = |k: &usize| closes[*k] != closes[*k - 1];
    (target..=limit)
        .find(changes)
        .or_else(|| (1..target).rev().find(changes))
}

/// Chronological fold boundaries inside the fitting region, as `(fit_end, validate_end)` pairs:
/// a fold fits on `0..fit_end` and is measured on `fit_end..validate_end`.
///
/// The boundaries are ANCHORED walk-forward — every fold fits from the very first trade, on a
/// growing prefix — cut at `train_n * (k + j) / (2k)`. So the first fold fits on the leading half
/// and the validation windows tile the trailing half without overlapping: every fold is measured
/// on trades strictly later than the ones it saw, which is the only arrangement under which a
/// score means "would have held up", and no trade is measured twice.
///
/// `train_n` itself is the LAST endpoint and is never snapped: it is the boundary the caller
/// already chose, and moving it would either drop the final group of trades or reach past the
/// fitting region into the held-back tail — the one thing composition may never see.
///
/// Folds whose window would be empty are dropped rather than returned degenerate, which happens
/// when two boundaries snap onto the same index or a span shares a single timestamp. The result
/// can therefore be SHORTER than `k`, or empty.
fn fold_cuts(closes: &[i64], train_n: usize, k: usize) -> Vec<(usize, usize)> {
    if k == 0 || train_n < 2 {
        return Vec::new();
    }
    let limit = train_n - 1;
    // Endpoint `j == k` is `train_n` exactly; the interior ones snap to a timestamp change.
    let mut cuts: Vec<usize> = (0..k)
        .filter_map(|j| {
            let target = (train_n as u128 * (k + j) as u128 / (2 * k) as u128) as usize;
            snap_to_close_change(closes, target.clamp(1, limit), limit)
        })
        .collect();
    cuts.push(train_n);
    cuts.dedup();
    cuts.windows(2)
        .map(|w| (w[0], w[1]))
        .filter(|(fit_end, validate_end)| *fit_end > 0 && validate_end > fit_end)
        .collect()
}

/// The column-oriented sample the optimizer runs on: per-trade profits, one value column per
/// entry in `FIELDS`, and the close timestamps the split is cut on. Rows are ordered oldest
/// first — see [`chronological_order`].
type Sample = (Vec<f64>, Vec<Vec<f64>>, Vec<i64>);

/// Rows read between two cancellation checks during the scan.
///
/// The check is one relaxed atomic load, so it could run per row; batching keeps it out of the
/// inner loop's way while still answering a stop within a fraction of a scan. Now that the search
/// itself is measured in milliseconds, this scan is the longer half of a suggestion, and a Stop
/// that only takes effect after it would be the slow one.
const SCAN_CANCEL_EVERY: usize = 4096;

/// Scan profit and effective field values (COALESCE 0) into column-oriented memory, oldest
/// trade first.
///
/// Args:
///     q: Report scope and period.
///     handle: Cancellation of the search this scan feeds.
///
/// Returns:
///     Per-trade profits and one value column per entry in `FIELDS`, `None` when the search was
///     stopped mid-scan, or a classified read failure. Every value feeds the optimizer, so an
///     unreadable cell aborts the calculation; NULL deliberately maps to `0.0`, a read error
///     does not.
fn scan(q: &Query, handle: &SearchHandle) -> ReadResult<Option<Sample>> {
    super::read_tuner_rows(q, |conn, q, src| scan_on(conn, q, src, handle))
}

/// Materialize one threshold-search sample inside the quote-preflight snapshot.
///
/// Args:
///     conn: Pinned report snapshot.
///     q: Floored tuner query used by the unified source.
///     src: Unified report source built from the same snapshot.
///     handle: Cancellation state sampled during the scan.
///
/// Returns:
///     The complete sample, `None` when cancelled, or a classified read failure.
fn scan_on(
    conn: &Connection,
    q: &Query,
    src: &str,
    handle: &SearchHandle,
) -> ReadResult<Option<Sample>> {
    const CTX: &str = "tuner: threshold_search";
    let nf = FIELDS.len();
    let cols = FIELDS
        .iter()
        .map(|s| format!("o.\"{}\"", s.col))
        .collect::<Vec<_>>()
        .join(", ");
    // Ordering is imposed in memory rather than by the query: no single column of this UNION is
    // unique, so no SQL `ORDER BY` can be total, and a partial one only hides that.
    let sql = format!("SELECT {cols}, COALESCE(o.pnl,0), COALESCE(o.closedate,0) FROM {src}");
    let mut profits: Vec<f64> = Vec::new();
    let mut closes: Vec<i64> = Vec::new();
    let mut vals: Vec<Vec<f64>> = vec![Vec::new(); nf];
    let mut stmt = conn.prepare(&sql).map_err(|e| read_fail(CTX, e))?;
    let mut rows = stmt
        .query(rusqlite::params![q.from, q.to])
        .map_err(|e| read_fail(CTX, e))?;
    while let Some(r) = rows.next().map_err(|e| read_fail(CTX, e))? {
        if profits.len().is_multiple_of(SCAN_CANCEL_EVERY) && handle.is_cancelled() {
            // The scan is a unit of work like any other, and dropping it mid-way is exactly what
            // `abandoned` exists to report — otherwise a stop during the scan reads as "the
            // period simply has no suggestion".
            handle.note_abandoned();
            return Ok(None);
        }
        profits.push(r.get(nf).map_err(|e| read_fail(CTX, e))?);
        closes.push(r.get(nf + 1).map_err(|e| read_fail(CTX, e))?);
        for (fi, col) in vals.iter_mut().enumerate() {
            let v = r.get::<_, Option<f64>>(fi).map_err(|e| read_fail(CTX, e))?;
            col.push(v.filter(|v| v.is_finite()).unwrap_or(0.0));
        }
    }
    let order = chronological_order(&closes, &profits, &vals);
    let sorted_vals = vals.iter().map(|col| gather(col, &order)).collect();
    let sorted_closes = order.iter().map(|t| closes[*t]).collect();
    Ok(Some((gather(&profits, &order), sorted_vals, sorted_closes)))
}

/// Row indices in chronological order, tie-broken to a TOTAL order on the row's content.
///
/// Two things read the sequence rather than the set — the train/holdout cut and the drawdown —
/// so a run whose answer depends on how SQLite happened to return equally timed rows would be
/// silently irreproducible. The unified report source projects no unique key, so the tie-break
/// falls back to the row's own values: profit first, then every field column. Rows that still
/// compare equal are interchangeable BECAUSE they are equal in everything the search reads, so
/// their relative order cannot be observed. The sort is a STABLE one so that even those rows
/// come out in a stated order rather than an arbitrary one.
fn chronological_order(closes: &[i64], profits: &[f64], vals: &[Vec<f64>]) -> Vec<usize> {
    let mut order: Vec<usize> = (0..closes.len()).collect();
    order.sort_by(|a, b| {
        closes[*a]
            .cmp(&closes[*b])
            .then_with(|| profits[*a].total_cmp(&profits[*b]))
            .then_with(|| {
                vals.iter()
                    .map(|col| col[*a].total_cmp(&col[*b]))
                    .find(|o| o.is_ne())
                    .unwrap_or(Ordering::Equal)
            })
    });
    order
}

/// Rearrange one column into the given row order.
fn gather(col: &[f64], order: &[usize]) -> Vec<f64> {
    order.iter().map(|t| col[*t]).collect()
}
