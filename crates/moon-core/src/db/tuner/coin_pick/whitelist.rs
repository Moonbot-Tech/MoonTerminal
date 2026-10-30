//! Preserve existing coin picks and fill empty results with one pre-registered top-20 fit.

use super::{CoinMask, CoinPickParams, CoinTrade, Moments, allowed, gated_candidates, tally};
use std::collections::{BTreeMap, HashSet};

/// Fixed before any validation: no fresh-deal outcome chooses the list size.
pub(super) const TOP_N: usize = 20;
/// A fallback must represent several coins rather than a single noisy winner.
const MIN_COINS: usize = 3;

/// Fit today's t-gated family with its unchanged train-profit eligibility check.
pub(super) fn legacy(
    train: &[CoinTrade],
    scores: &BTreeMap<String, Moments>,
    p: &CoinPickParams,
    black: &HashSet<String>,
    cap: usize,
) -> HashSet<String> {
    let mut white = gated_candidates(scores, p, false, cap);
    let mb = CoinMask::new(&p.manual_black);
    let mw = CoinMask::new(&p.manual_white);
    let after_black = |r: &CoinTrade| allowed(&r.token, &mb, &mw) && !black.contains(&r.token);
    if !white.is_empty()
        && tally(train, |r| after_black(r) && white.contains(&r.token)).profit
            <= tally(train, after_black).profit
    {
        white.clear();
    }
    white
}

/// Rank eligible positive TRAIN sums without a t gate or a train-profit clearing check.
pub(super) fn fallback(
    scores: &BTreeMap<String, Moments>,
    min_trades: u32,
    cap: usize,
) -> HashSet<String> {
    let mut positive: Vec<_> = scores
        .iter()
        .filter(|(_, s)| s.n >= min_trades.max(1) as usize && s.sum > 0.0)
        .collect();
    positive.sort_by(|(a, x), (b, y)| y.sum.total_cmp(&x.sum).then_with(|| a.cmp(b)));
    let tokens: HashSet<_> = positive
        .into_iter()
        .take(TOP_N.min(cap))
        .map(|(t, _)| t.clone())
        .collect();
    if tokens.len() >= MIN_COINS {
        tokens
    } else {
        HashSet::new()
    }
}

/// Preserve every existing pick; try the fixed fallback only when both legacy lists are empty.
pub(super) fn fit(
    train: &[CoinTrade],
    scores: &BTreeMap<String, Moments>,
    p: &CoinPickParams,
    black: &HashSet<String>,
    cap: usize,
) -> HashSet<String> {
    let white = legacy(train, scores, p, black, cap);
    if white.is_empty() && black.is_empty() {
        fallback(scores, p.min_trades, cap)
    } else {
        white
    }
}
