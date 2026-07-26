//! Automatic threshold tuning for the "By filter" axis: which range on which report field
//! maximizes period profit, searched over all fields jointly.
//!
//! This module owns the database side — one scan of the tuner source into column-oriented
//! memory — and hands the result to the DB-free optimizer in [`search`]. Semantics match tuner
//! variants: NULL fields equal 0 (COALESCE), and bounds are inclusive. The database is scanned
//! once and all later work stays in memory, so call this ONLY from a background executor.
//!
//! The sample is ordered chronologically here, which is what makes a train/holdout split and a
//! drawdown figure mean anything at all: both read the sequence, not the set.

mod handle;
mod search;

#[cfg(test)]
mod tests;

pub use handle::SearchHandle;

use std::cmp::Ordering;

use super::{FieldClass, FIELDS};
use crate::db::analytics::Query;
use crate::db::metrics::Tally;
use crate::db::read_fail::read_fail;
use crate::db::ReadResult;

/// Minimum accepted restart count. Shared with the UI so displayed and executed counts agree.
pub const RESTARTS_MIN: usize = 1;

/// Maximum accepted restart count.
///
/// Restarts run across a worker pool, report their progress, and stop on request, so the ceiling
/// is no longer "what a user can be asked to wait through blind" — it is a guard against a typo
/// becoming an unbounded run. Work is strictly linear in the count: one restart is `FIELDS.len()`
/// fields × up to 16 passes × (`n` trades + the edge search), measured at ~5 ms of single-core
/// time per restart over a 26k-trade report at depth 64. The ceiling is therefore on the order of
/// ten seconds spread across the pool — long, visibly progressing, and interruptible.
pub const RESTARTS_MAX: usize = 20_000;

/// Minimum quantile-edge count accepted by the search and exposed to the UI.
pub const EDGES_MIN: usize = 4;

/// Maximum quantile-edge count; keeps bin indices clear of the search's `BELOW` sentinel.
///
/// The bin index is a `u16` and the sentinel is its maximum, so a depth up to 65 535 stays
/// distinguishable. The ceiling is set far below that by cost, not by the encoding: the edge
/// search is linear in the depth while the per-trade scan is not, so finer slicing buys
/// increasingly little and slices the sample into groups too small to mean anything.
pub const EDGES_MAX: usize = 256;

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
    /// How many restarts stand behind the result is deliberately NOT here: the caller supplies
    /// the [`SearchHandle`] and can read `completed()` off it, and carrying a second copy in the
    /// result gave one number two owners that a late stop could put out of step.
    pub seed: u64,
}

/// Inputs of one threshold search, beyond the report scope itself.
pub struct SearchParams<'a> {
    /// Restart count, clamped to `RESTARTS_MIN..=RESTARTS_MAX`.
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
    /// Quantile resolution per field, clamped to `EDGES_MIN..=EDGES_MAX`.
    pub edges: usize,
    /// Whether to round the resulting bounds outwards.
    pub round: bool,
    /// Base seed of the random restarts; `None` draws a fresh one from the clock.
    pub seed: Option<u64>,
    /// Share of the period, oldest trades first, the search may fit on. `1.0` fits on
    /// everything; anything lower holds the remaining tail back as a holdout. Clamped so a
    /// split always leaves at least one trade on each side.
    pub train_frac: f64,
}

/// Maximize combined profit with random-restart coordinate descent while retaining at least
/// `params.min_n` trades, or — where that is `None` — one tenth of the trades fitted on.
///
/// Each attempt runs at most 16 passes. At most two Delta2/3 slot fields may carry ranges because
/// the strategy format cannot store more.
///
/// `handle` stops the search and carries its progress; build a fresh one per call. A stopped
/// search returns the best of the restarts that completed, and the same `handle` reports how many
/// those were through `completed()`.
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
    let ne = params.edges.clamp(EDGES_MIN, EDGES_MAX);
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
    // Not enough sample — a legitimate "no suggestion", not a failure.
    let Some(search) = search::Search::new(
        profits,
        vals,
        params.locked,
        is_slot,
        min_n,
        ne,
        requested_train,
    ) else {
        return Ok(None);
    };
    let restarts = params.restarts.clamp(RESTARTS_MIN, RESTARTS_MAX);
    // Without a chosen seed, take one from the clock so repeated searches explore different
    // starts. Restart 0 is greedy from empty and stays deterministic either way.
    let seed = params.seed.unwrap_or_else(|| {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0)
    });
    // No restart completed: too small a sample was already handled above, so this is a stop.
    let Some(outcome) = search.run(restarts, seed, handle) else {
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
    // Read back from the search rather than reusing `requested_train`: the tallies below must
    // describe the window it actually fitted on, not the one it was asked for.
    let train_n = search.train_n();
    Ok(Some(SearchResult {
        fields,
        train: search.tally(&applied, 0..train_n),
        holdout: (train_n < total).then(|| search.tally(&applied, train_n..total)),
        seed,
    }))
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
    let changes = |k: &usize| closes[*k] != closes[*k - 1];
    (target..n)
        .find(changes)
        .or_else(|| (1..=target).rev().find(changes))
        .unwrap_or(n)
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
    const CTX: &str = "tuner: threshold_search";
    let (conn, q, src) = super::open_tuner_source(q)?;
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
