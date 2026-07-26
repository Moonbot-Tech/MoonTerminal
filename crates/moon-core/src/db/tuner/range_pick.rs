//! Picking the most profitable contiguous range out of quantile-sliced samples.
//!
//! Both tuner searches ask the same question and used to answer it with their own copy of an
//! O(edges²) double loop: the single-field suggestion in [`super::best_range`] over sample
//! positions, and the all-fields threshold search over per-bin prefix sums. The two differ only
//! in what they slice, so the decision itself lives here once.

#[cfg(test)]
mod tests;

/// Best contiguous slice `[i, j)` by profit, subject to the two rejections both callers share.
///
/// `profit_at[k]` and `count_at[k]` are the cumulative profit and cumulative trade count at
/// quantile edge `k`. `count_at` must be non-decreasing in `k`, which is what makes the linear
/// scan below valid; profit may move in either direction. A candidate must keep at least `min_n`
/// trades, must not cover all `tot_c` of them (that is functionally no filter), and must beat
/// `floor`.
///
/// `floor` is supplied rather than derived from a baseline here: both callers happen to build it
/// the same way, as the no-filter profit plus [`super::super::metrics::improvement_margin`], but
/// taking the finished threshold keeps `f64::NEG_INFINITY` — "accept any slice" — expressible,
/// which is what the exhaustive-scan oracle uses to compare the two searches over the whole
/// candidate space rather than only over the improving part of it.
///
/// Runs in O(edges) rather than over all O(edges²) pairs: because `count_at` is non-decreasing,
/// the lower edges admissible for a given upper edge `j` form a prefix, and that prefix only
/// grows as `j` grows. The best lower edge for each `j` is therefore the running minimum of
/// `profit_at` over that prefix.
///
/// Ties resolve exactly as the pairwise scan did — the lexicographically smallest `(i, j)` among
/// equal profits — because the pairwise scan visited `i` then `j` ascending and kept the first
/// strict maximum. That is not cosmetic: on data with repeated profits several slices score
/// identically, and picking a different one hands the user different thresholds.
pub(super) fn best_pair(
    profit_at: &[f64],
    count_at: &[usize],
    min_n: usize,
    tot_c: usize,
    floor: f64,
) -> Option<(usize, usize)> {
    let last = profit_at.len() - 1;
    // A slice keeping nothing is not a filter, and admitting it would let the lower edge run
    // past the upper one. Both callers already pass at least 1; this makes the kernel safe on
    // its own terms rather than by their good manners.
    let min_n = min_n.max(1);
    // A slice can equal the whole sample only when the edges already account for every trade.
    // Where they do not — the threshold search parks out-of-range trades in a bucket no slice
    // can reach — no `(i, j)` can total `tot_c` and this rejection cannot trigger at all.
    let coverable = count_at[last] == tot_c;
    let mut best: Option<(usize, usize)> = None;
    let mut best_profit = floor;
    // Running minima of `profit_at[i]` over the admissible prefix: `any` over all of it, and
    // `dropping` restricted to lower edges that already exclude at least one trade. The second
    // serves the `j` values where a lower edge at zero would cover the whole sample.
    let mut admitted = 0usize;
    let mut min_any: Option<(f64, usize)> = None;
    let mut min_dropping: Option<(f64, usize)> = None;
    for j in 1..=last {
        if count_at[j] < min_n {
            continue;
        }
        let limit = count_at[j] - min_n;
        while admitted <= last && count_at[admitted] <= limit {
            let value = profit_at[admitted];
            if min_any.is_none_or(|(m, _)| value < m) {
                min_any = Some((value, admitted));
            }
            if count_at[admitted] > 0 && min_dropping.is_none_or(|(m, _)| value < m) {
                min_dropping = Some((value, admitted));
            }
            admitted += 1;
        }
        // Reaching every trade from an empty lower prefix is the no-op filter the pairwise scan
        // rejected by exact count; fall back to a lower edge that drops something.
        let covers_everything = coverable && count_at[j] == tot_c;
        let picked = if covers_everything {
            min_dropping
        } else {
            min_any
        };
        let Some((min_value, min_index)) = picked else {
            continue;
        };
        let profit = profit_at[j] - min_value;
        let pair = (min_index, j);
        if profit > best_profit || (profit == best_profit && best.is_some_and(|b| pair < b)) {
            best_profit = profit;
            best = Some(pair);
        }
    }
    best
}
