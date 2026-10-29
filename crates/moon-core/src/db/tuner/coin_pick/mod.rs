//! Train-only coin-list selection with an untouched chronological holdout.
//! The noise floor is 5% of absolute Fact profit or the manual-filtered holdout's
//! sample deviation times the square root of trades removed from Manual, whichever is larger.

use std::collections::{BTreeMap, HashSet};
use std::ops::RangeInclusive;

use crate::db::ReadResult;
use crate::db::analytics::Query;
use crate::db::metrics::Tally;
use crate::db::read_fail::read_fail_on;
use crate::symbol::{CoinMask, coin_match_key};

/// The same pick for every strategy of the list at once.
mod batch;
mod whitelist;
pub use batch::{
    BatchProgress, KeyedTrade, ManualLists, PickSettings, coin_picks_from_rows,
    suggest_coins_by_strategy,
};

/// Supported share of fresh trades reserved for validation.
pub const HOLDOUT_PCT_RANGE: RangeInclusive<u32> = 10..=50;
/// Default fresh-trade share.
pub const DEFAULT_HOLDOUT_PCT: u32 = 30;
/// Default minimum observations per candidate.
pub const DEFAULT_MIN_TRADES: u32 = 5;
/// Minimum validation sample size.
pub const MIN_HOLDOUT_TRADES: usize = 20;
/// Minimum validation observations removed by a suggestion.
pub const MIN_HOLDOUT_TOUCHED: usize = 5;
/// Minimum absolute standardized mean for the legacy gated candidates.
pub const T_GATE: f64 = 1.0;
/// Maximum candidates in either suggested list.
pub const MAX_PICK: usize = 50;
/// Absolute Fact-profit share used as the minimum meaningful gain.
pub const NOISE_ABS_SHARE: f64 = 0.05;

/// Settings and existing lists; blacklist takes precedence over whitelist.
#[derive(Debug, Clone)]
pub struct CoinPickParams {
    pub min_trades: u32,
    pub holdout_pct: u32,
    pub black: bool,
    pub white: bool,
    pub manual_black: HashSet<String>,
    pub manual_white: HashSet<String>,
}

/// One closed trade in the selected query's monetary units.
#[derive(Debug, Clone)]
pub struct CoinTrade {
    pub token: String,
    pub close: i64,
    pub profit: f64,
}

/// Candidate tagged with its full-period observations, not its fitting score.
#[derive(Debug, Clone)]
pub struct CoinSuggestion {
    pub token: String,
    pub n: i64,
    pub profit: f64,
}

/// Why fresh trades do not support adopting the suggestion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NoiseReason {
    SmallGain,
    HoldoutWorse,
    FewHoldout,
}

/// Assessment of incremental profit against the manual lists.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PickVerdict {
    Useful,
    Noise(NoiseReason),
    NothingFound,
    TooFewTrades,
}

/// Train-selected lists and three evaluations on the same fresh time interval.
#[derive(Debug, Clone)]
pub struct CoinPick {
    pub black: Vec<CoinSuggestion>,
    pub white: Vec<CoinSuggestion>,
    pub train_n: usize,
    pub holdout_n: usize,
    pub fact_holdout: Tally,
    pub manual_holdout: Tally,
    pub pick_holdout: Tally,
    pub verdict: PickVerdict,
}

/// Running sample moments avoid subtracting nearly equal squared sums.
#[derive(Default)]
struct Moments {
    n: usize,
    sum: f64,
    mean: f64,
    m2: f64,
}

impl Moments {
    /// Accumulate a profit using Welford's recurrence.
    fn push(&mut self, profit: f64) {
        self.n += 1;
        self.sum += profit;
        let delta = profit - self.mean;
        self.mean += delta / self.n as f64;
        self.m2 += delta * (profit - self.mean);
    }

    /// Sample standard deviation; a singleton has no observed dispersion.
    fn sd(&self) -> f64 {
        if self.n < 2 {
            0.0
        } else {
            (self.m2.max(0.0) / (self.n - 1) as f64).sqrt()
        }
    }

    /// Constant nonzero samples pass the directional gate once the count guard passes.
    fn score(&self) -> f64 {
        let sd = self.sd();
        if sd == 0.0 {
            self.mean.signum() * T_GATE
        } else {
            self.mean / (sd / (self.n as f64).sqrt())
        }
    }
}

/// Apply whitelist restriction before blacklist exclusion.
fn allowed(token: &str, black: &CoinMask, white: &CoinMask) -> bool {
    (white.is_empty() || white.matches(token)) && !black.matches(token)
}

/// Aggregate rows in their chronological order for correct drawdown.
fn tally(rows: &[CoinTrade], keep: impl Fn(&CoinTrade) -> bool) -> Tally {
    let mut out = Tally::default();
    for row in rows.iter().filter(|r| keep(r)) {
        out.push(row.profit);
    }
    out
}

/// Accumulate fitting moments only for trades admitted by the manual lists.
fn score_rows(
    rows: &[CoinTrade],
    manual: impl Fn(&CoinTrade) -> bool,
) -> BTreeMap<String, Moments> {
    let mut scores: BTreeMap<String, Moments> = BTreeMap::new();
    for row in rows.iter().filter(|r| manual(r)) {
        scores
            .entry(row.token.clone())
            .or_default()
            .push(row.profit);
    }
    scores
}

/// Keep the existing count, sign, t-score and ranking rules for either direction.
fn gated_candidates(
    scores: &BTreeMap<String, Moments>,
    p: &CoinPickParams,
    black: bool,
    cap: usize,
) -> HashSet<String> {
    let mut picked: Vec<_> = scores
        .iter()
        .filter(|(_, s)| {
            s.n >= p.min_trades.max(1) as usize
                && if black {
                    s.sum < 0.0 && s.score() <= -T_GATE
                } else {
                    s.sum > 0.0 && s.score() >= T_GATE
                }
        })
        .collect();
    picked.sort_by(|(a, x), (b, y)| {
        let order = if black {
            x.sum.total_cmp(&y.sum)
        } else {
            y.sum.total_cmp(&x.sum)
        };
        order.then_with(|| a.cmp(b))
    });
    picked
        .into_iter()
        .take(cap)
        .map(|(token, _)| token.clone())
        .collect()
}

/// Fit only on older rows; preserve existing lists before trying a fixed top-20 whitelist.
pub fn coin_pick_from_rows(rows: &[CoinTrade], p: &CoinPickParams) -> CoinPick {
    fit_with_cap(rows, p, MAX_PICK)
}

/// Use the production fit with an explicit cap for paired diagnostic comparisons.
fn fit_with_cap(rows: &[CoinTrade], p: &CoinPickParams, cap: usize) -> CoinPick {
    let mut rows = rows.to_vec();
    for row in &mut rows {
        row.token = coin_match_key(&row.token);
    }
    rows.sort_by(|a, b| {
        a.close
            .cmp(&b.close)
            .then_with(|| a.profit.total_cmp(&b.profit))
            .then_with(|| a.token.cmp(&b.token))
    });
    let closes: Vec<_> = rows.iter().map(|r| r.close).collect();
    let pct = p
        .holdout_pct
        .clamp(*HOLDOUT_PCT_RANGE.start(), *HOLDOUT_PCT_RANGE.end());
    let train_n = super::threshold_search::train_split(&closes, 1.0 - f64::from(pct) / 100.0);
    let (train, holdout) = rows.split_at(train_n);
    let manual_black = CoinMask::new(&p.manual_black);
    let manual_white = CoinMask::new(&p.manual_white);
    let manual = |r: &CoinTrade| allowed(&r.token, &manual_black, &manual_white);
    let fact_holdout = tally(holdout, |_| true);
    let manual_holdout = tally(holdout, manual);
    let mut out = CoinPick {
        black: Vec::new(),
        white: Vec::new(),
        train_n,
        holdout_n: holdout.len(),
        fact_holdout,
        pick_holdout: manual_holdout.clone(),
        manual_holdout,
        verdict: PickVerdict::TooFewTrades,
    };
    if train_n == 0 || holdout.len() < MIN_HOLDOUT_TRADES {
        return out;
    }
    let scores = score_rows(train, manual);
    let mut full: BTreeMap<String, Moments> = BTreeMap::new();
    for row in &rows {
        full.entry(row.token.clone()).or_default().push(row.profit);
    }
    let black = if p.black {
        gated_candidates(&scores, p, true, cap)
    } else {
        HashSet::new()
    };
    let white = if p.white {
        whitelist::fit(train, &scores, p, &black, cap)
    } else {
        HashSet::new()
    };
    let after_black = |r: &CoinTrade| manual(r) && !black.contains(&r.token);
    let pick = |r: &CoinTrade| after_black(r) && (white.is_empty() || white.contains(&r.token));
    out.pick_holdout = tally(holdout, pick);
    let tags = |tokens: &HashSet<String>, descending: bool| {
        let mut tags: Vec<_> = tokens
            .iter()
            .map(|token| CoinSuggestion {
                token: token.clone(),
                n: full[token].n as i64,
                profit: full[token].sum,
            })
            .collect();
        tags.sort_by(|a, b| {
            let order = if descending {
                scores[&b.token].sum.total_cmp(&scores[&a.token].sum)
            } else {
                scores[&a.token].sum.total_cmp(&scores[&b.token].sum)
            };
            order.then_with(|| a.token.cmp(&b.token))
        });
        tags
    };
    out.black = tags(&black, false);
    out.white = tags(&white, true);
    let touched = holdout.iter().filter(|r| manual(r) && !pick(r)).count();
    let mut moments = Moments::default();
    for row in holdout.iter().filter(|r| manual(r)) {
        moments.push(row.profit);
    }
    let gain = out.pick_holdout.profit - out.manual_holdout.profit;
    let floor = (NOISE_ABS_SHARE * out.fact_holdout.profit.abs())
        .max(moments.sd() * (touched as f64).sqrt());
    out.verdict = if black.is_empty() && white.is_empty() {
        PickVerdict::NothingFound
    } else if touched < MIN_HOLDOUT_TOUCHED {
        PickVerdict::Noise(NoiseReason::FewHoldout)
    } else if gain <= 0.0 {
        PickVerdict::Noise(NoiseReason::HoldoutWorse)
    } else if gain > floor {
        PickVerdict::Useful
    } else {
        PickVerdict::Noise(NoiseReason::SmallGain)
    };
    out
}

/// Materialize query trades in the tuner's pinned valuation snapshot, then fit in memory.
/// Run on a background executor because this scans the query period.
pub fn suggest_coins(q: &Query, p: &CoinPickParams) -> ReadResult<CoinPick> {
    let rows = super::read_tuner_rows(q, |conn, q, src| {
        let ctx = "tuner: suggest_coins";
        let sql = format!("SELECT o.coin, COALESCE(o.closedate,0), COALESCE(o.pnl,0) FROM {src}");
        let mut stmt = conn.prepare(&sql).map_err(|e| read_fail_on(conn, ctx, e))?;
        let mapped = stmt
            .query_map(rusqlite::params![q.from, q.to], |r| {
                Ok(CoinTrade {
                    // Raw report name: `coin_pick_from_rows` owns the normalization.
                    token: r.get(0)?,
                    close: r.get(1)?,
                    profit: r.get(2)?,
                })
            })
            .map_err(|e| read_fail_on(conn, ctx, e))?;
        mapped
            .map(|r| r.map_err(|e| read_fail_on(conn, ctx, e)))
            .collect::<ReadResult<Vec<_>>>()
    })?;
    Ok(coin_pick_from_rows(&rows, p))
}

#[cfg(test)]
mod tests;
