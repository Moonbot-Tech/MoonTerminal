//! The search of the "Entry/Exit" axis: coordinate descent with random restarts over the
//! discrete grids of [`TICK_PARAMS`], scoring a point by REPLAYING every covered deal under
//! it — the same shape as `threshold_search`, with the SQL mask replaced by [`simulate`].
//!
//! A point is a set of strategy values in the strategy's own spelling, laid over the values
//! the strategies hold now; the model's parameter structs are built from that overlay through
//! the same builders the grid's "now" column uses, so what the search varies is exactly what
//! Save writes. Only the fields of the groups the caller switched on are searched, minus the
//! ones it locked.
//!
//! The objective is the total money result over the fitted deals with at least `min_n` of them
//! still trading, ties broken by the profit factor — `metrics::Tally`, the same figures the KPI
//! matrix prints. A deal the variant never fills, or never closes inside its tape, is not a
//! trade and drops out of `n`; the caller prints "by N of M" beside the column so a variant
//! that wins by trading less is visible as such.
//!
//! Chronological order is kept on purpose: the train/holdout cut and the drawdown read the
//! SEQUENCE, and the deals arrive sorted by close from `read_deals`.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use rayon::prelude::*;

use super::params::{
    ParamGroup, ParamKind, StrategyValues, TICK_PARAMS, exit_params, mshot_params,
};
use super::{Deal, EntryParams, ExitParams, entry_model_for, simulate};
use crate::db::metrics::Tally;
use crate::db::tuner::threshold_search::search::{install, restart_seed};
use crate::db::tuner::threshold_search::{SearchHandle, train_split};
use crate::feed::types::Tick;

/// Passes of coordinate descent one restart may take before it is called converged.
const MAX_PASSES: usize = 16;

/// One deal with its tape, ready to be replayed as often as the search asks.
#[derive(Clone)]
pub struct PreparedDeal {
    pub deal: Deal,
    /// The window's prints, ascending; shared, never copied per evaluation.
    pub ticks: Arc<[Tick]>,
    /// The archived first point of the entry line, when the archive holds it.
    pub entry_start: Option<(i64, f64)>,
}

/// What one search varies and how.
pub struct SearchParams<'a> {
    /// The values every selected strategy holds now, in strategy spelling — the base every
    /// point is laid over. Fields the strategies disagree on are absent and read as default.
    pub base: &'a HashMap<String, String>,
    /// Schema defaults for the keys the base leaves out.
    pub defaults: &'a HashMap<String, f64>,
    /// The strategy kind of the sample, for the fields the grid offers.
    pub kind: &'a str,
    /// Whether the Entry group is searched (only when the kind has an entry model).
    pub vary_entry: bool,
    /// Whether the Exit group is searched.
    pub vary_exit: bool,
    /// Field keys held at the base value.
    pub locked: &'a HashSet<String>,
    /// Restart count, at least 1.
    pub restarts: usize,
    /// Minimum trades a point must keep, or one tenth of the fitted sample.
    pub min_n: Option<i64>,
    /// Base seed of the restarts; `None` draws one from the clock.
    pub seed: Option<u64>,
    /// Share of the period, oldest first, the search may fit on.
    pub train_frac: f64,
    /// Replacement latency of the model, milliseconds.
    pub latency_ms: f64,
}

/// What the search found.
#[derive(Clone, Debug)]
pub struct SearchResult {
    /// The winning values, in strategy spelling — only the fields that moved off the base.
    pub values: Vec<(String, String)>,
    /// What they achieve on the deals they were fitted on.
    pub train: Tally,
    /// What they achieve on the deals held back, when any were.
    pub holdout: Option<Tally>,
    /// The seed the restarts were derived from.
    pub seed: u64,
}

/// One point of the grid: the varied fields' values, in strategy spelling.
type Point = HashMap<&'static str, String>;

/// The model parameters a point comes to.
fn params_of(
    base: &HashMap<String, String>,
    defaults: &HashMap<String, f64>,
    point: &Point,
    kind: &str,
    latency_ms: f64,
) -> (EntryParams, ExitParams) {
    let mut values = base.clone();
    for (key, value) in point {
        values.insert((*key).to_string(), value.clone());
    }
    let sv = StrategyValues {
        values: &values,
        defaults,
    };
    let entry = if entry_model_for(kind) {
        EntryParams::MoonShot(mshot_params(&sv, latency_ms))
    } else {
        EntryParams::Fact
    };
    let mut exit = exit_params(&sv);
    exit.latency_ms = latency_ms;
    (entry, exit)
}

/// The spelling of one grid value in the strategy's format.
fn spell(kind: &ParamKind, index: usize) -> String {
    match kind {
        ParamKind::Num { grid } => {
            let v = grid[index];
            if v.fract() == 0.0 {
                format!("{v:.0}")
            } else {
                format!("{v}")
            }
        }
        ParamKind::Bool => (if index == 0 { "NO" } else { "YES" }).to_string(),
        ParamKind::Enum(options) => options[index].to_string(),
    }
}

/// How many values a field's grid offers.
fn arity(kind: &ParamKind) -> usize {
    match kind {
        ParamKind::Num { grid } => grid.len(),
        ParamKind::Bool => 2,
        ParamKind::Enum(options) => options.len(),
    }
}

/// The fields one search varies.
fn varied<'a>(p: &SearchParams<'a>) -> Vec<&'static super::params::TickParam> {
    TICK_PARAMS
        .iter()
        .filter(|f| match f.group {
            ParamGroup::Entry => p.vary_entry,
            ParamGroup::Exit => p.vary_exit,
        })
        .filter(|f| f.kinds.is_empty() || f.kinds.contains(&p.kind))
        .filter(|f| !p.locked.contains(f.key))
        .collect()
}

/// The tally of a point over `deals`, in order, and the spend of the deals it traded.
fn tally_and_spent(deals: &[PreparedDeal], entry: &EntryParams, exit: &ExitParams) -> (Tally, f64) {
    // The replay of every deal is independent; the tally is folded in order afterwards.
    let results: Vec<Option<(f64, f64)>> = deals
        .par_iter()
        .map(|d| {
            simulate(&d.deal, &d.ticks, entry, exit, d.entry_start)
                .profit_money(&d.deal)
                .map(|money| (money, d.deal.spent))
        })
        .collect();
    let mut tally = Tally::default();
    let mut spent = 0.0;
    for (money, size) in results.into_iter().flatten() {
        tally.push(money);
        spent += size;
    }
    (tally, spent)
}

/// The tally of a point over `deals`, in order.
fn tally(deals: &[PreparedDeal], entry: &EntryParams, exit: &ExitParams) -> Tally {
    tally_and_spent(deals, entry, exit).0
}

/// Whether `a` beats `b` under the objective, with the sample floor.
fn better(a: &Tally, b: &Tally, min_n: i64) -> bool {
    let a_ok = a.n >= min_n;
    let b_ok = b.n >= min_n;
    if a_ok != b_ok {
        return a_ok;
    }
    if a.profit != b.profit {
        return a.profit > b.profit;
    }
    a.profit_factor() > b.profit_factor()
}

/// xorshift64*, the same stream shape the threshold search draws its starts from.
fn next_random(state: &mut u64) -> u64 {
    let mut x = *state;
    x ^= x >> 12;
    x ^= x << 25;
    x ^= x >> 27;
    *state = x;
    x.wrapping_mul(0x2545_F491_4F6C_DD1D)
}

/// Run the search.
///
/// Args:
///     deals: The covered deals, chronological by close.
///     params: What to vary and how hard to look.
///     handle: Stop and progress; a fresh one per run.
///
/// Returns:
///     The best point found, or `None` when the sample is empty, nothing is varied, or the
///     run was stopped before its first restart finished.
pub fn suggest(
    deals: &[PreparedDeal],
    params: &SearchParams<'_>,
    handle: &SearchHandle,
) -> Option<SearchResult> {
    let fields = varied(params);
    if deals.is_empty() || fields.is_empty() {
        return None;
    }
    let closes: Vec<i64> = deals.iter().map(|d| d.deal.close_ms).collect();
    let train_n = train_split(&closes, params.train_frac);
    let train = &deals[..train_n];
    let min_n = params
        .min_n
        .unwrap_or_else(|| (train_n as i64 / 10).max(1))
        .max(1);
    let seed = params.seed.unwrap_or_else(|| {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(1)
            | 1
    });
    let restarts = params.restarts.max(1);
    let evaluate = |point: &Point| -> Tally {
        let (entry, exit) = params_of(
            params.base,
            params.defaults,
            point,
            params.kind,
            params.latency_ms,
        );
        tally(train, &entry, &exit)
    };
    let best = install(|| {
        (0..restarts)
            .into_par_iter()
            .map(|restart| {
                if handle.is_cancelled() {
                    handle.note_abandoned();
                    return None;
                }
                // Restart 0 starts from the base itself; the others from a random grid point
                // per varied field, so the descent is not trapped in the base's own valley.
                let mut point = Point::new();
                if restart > 0 {
                    let mut state = restart_seed(seed, restart);
                    for field in &fields {
                        let index = (next_random(&mut state) % arity(&field.kind) as u64) as usize;
                        point.insert(field.key, spell(&field.kind, index));
                    }
                }
                let mut score = evaluate(&point);
                for _ in 0..MAX_PASSES {
                    let mut improved = false;
                    for field in &fields {
                        if handle.is_cancelled() {
                            handle.note_abandoned();
                            return None;
                        }
                        let mut current = point.get(field.key).cloned();
                        for index in 0..arity(&field.kind) {
                            let candidate = spell(&field.kind, index);
                            if current.as_deref() == Some(candidate.as_str()) {
                                continue;
                            }
                            point.insert(field.key, candidate.clone());
                            let trial = evaluate(&point);
                            if better(&trial, &score, min_n) {
                                score = trial;
                                improved = true;
                                // The accepted value is what a rejected later candidate
                                // restores to.
                                current = Some(candidate);
                            } else {
                                match &current {
                                    Some(c) => {
                                        point.insert(field.key, c.clone());
                                    }
                                    None => {
                                        point.remove(field.key);
                                    }
                                }
                            }
                        }
                        // A field that moved may enable a better value of one visited before,
                        // hence the passes; within one pass every field is visited once.
                    }
                    if !improved {
                        break;
                    }
                }
                handle.record_restart();
                Some((restart, point, score))
            })
            .flatten()
            // Among equal scores the LOWEST restart wins, so the parallel fan-out answers as a
            // sequential run would.
            .reduce_with(|a, b| {
                if better(&b.2, &a.2, min_n) || (!better(&a.2, &b.2, min_n) && b.0 < a.0) {
                    b
                } else {
                    a
                }
            })
    })?;
    let (_, point, train_tally) = best;
    // The base's own spelling of a field is not a change; only what moved is reported.
    let mut values: Vec<(String, String)> = point
        .iter()
        .filter(|(key, value)| params.base.get(**key) != Some(*value))
        .map(|(key, value)| ((*key).to_string(), value.clone()))
        .collect();
    values.sort();
    let holdout = (train_n < deals.len()).then(|| {
        let (entry, exit) = params_of(
            params.base,
            params.defaults,
            &point,
            params.kind,
            params.latency_ms,
        );
        tally(&deals[train_n..], &entry, &exit)
    });
    Some(SearchResult {
        values,
        train: train_tally,
        holdout,
        seed,
    })
}

/// The KPI of one explicit set of values over `deals` — a variant column — and the spend of
/// the deals it traded.
///
/// Args:
///     deals: The covered deals, chronological.
///     base: The strategies' current values.
///     defaults: Schema defaults.
///     kind: The strategy kind.
///     values: The variant's changes over the base, in strategy spelling.
///     latency_ms: Replacement latency of the model.
pub fn variant_tally(
    deals: &[PreparedDeal],
    base: &HashMap<String, String>,
    defaults: &HashMap<String, f64>,
    kind: &str,
    values: &[(String, String)],
    latency_ms: f64,
) -> (Tally, f64) {
    let mut point = Point::new();
    for (key, value) in values {
        if let Some(field) = TICK_PARAMS.iter().find(|f| f.key == key) {
            point.insert(field.key, value.clone());
        }
    }
    let (entry, exit) = params_of(base, defaults, &point, kind, latency_ms);
    install(|| tally_and_spent(deals, &entry, &exit))
}

#[cfg(test)]
mod tests;
