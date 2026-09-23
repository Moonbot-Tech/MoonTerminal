//! The search of the "Entry/Exit" axis: coordinate descent with random restarts over the
//! discrete grids of [`TICK_PARAMS`], scoring a point by REPLAYING every covered deal under
//! it — the same shape as `threshold_search`, with the SQL mask replaced by [`simulate`].
//!
//! A point is a set of strategy values in the strategy's own spelling, laid over the values
//! each deal's OWN strategy holds now ([`PreparedDeal::own`]) — a field the point leaves alone
//! runs every deal at its strategy's value, never at a default because the selected strategies
//! disagree on it; the model's parameter structs are built from that overlay through
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
//! A MoonShot variant's entry is replayed the way the caller's model settings pick
//! ([`super::mshot::EntryMethod`]): the corridor model from the order's creation, or the fact's
//! order shifted at the spike. The trade's own settings take the fact's fill either way, and a
//! field the picked method does not read is not searched
//! ([`super::mshot::EntryMethod::reads`]).
//!
//! Chronological order is kept on purpose: the train/holdout cut and the drawdown read the
//! SEQUENCE, and the deals arrive sorted by close from `read_deals`.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use rayon::prelude::*;

use super::mshot::{CorridorStep, EntryMethod, MshotEntry};
use super::params::{
    ParamGroup, ParamKind, StrategyValues, TICK_PARAMS, exit_params, mshot_params,
};
use super::settings::ModelSettings;
use super::{Deal, EntryParams, ExitParams, Outcome, entry_model_for, simulate};
use crate::db::metrics::Tally;
use crate::db::tuner::threshold_search::search::{install, restart_seed};
use crate::db::tuner::threshold_search::{SearchHandle, train_split};
use crate::feed::types::Tick;

/// Passes of coordinate descent one restart may take before it is called converged, when the
/// caller does not say ([`SearchParams::max_passes`]).
pub const DEFAULT_MAX_PASSES: usize = 16;

/// One deal with its tape, ready to be replayed as often as the search asks.
#[derive(Clone)]
pub struct PreparedDeal {
    pub deal: Deal,
    /// The window's prints, ascending; shared, never copied per evaluation.
    pub ticks: Arc<[Tick]>,
    /// The archived points of the entry line, when the archive holds it; shared like the
    /// tape.
    pub entry_line: Option<Arc<[(i64, f64)]>>,
    /// How far past the close the HELD COVERAGE of this deal's window reaches, in
    /// milliseconds — the caller's word from the tile store, not the last print's stamp: a
    /// quiet market prints nothing for seconds, and a tail measured by its last print would
    /// read as shorter than what is actually held. What [`common_horizon_ms`] takes the
    /// sample's horizon from; a covered row holds at least the model's tail
    /// (`required_spans`), so it is never under `TAIL_MS` there.
    pub trail_ms: i64,
    /// The values the deal's own strategy holds now, in strategy spelling — the base every
    /// variant and every point of a search is laid over on THIS deal. Shared by the deals of
    /// one strategy; empty when the strategy could not be read, and then every field reads as
    /// default.
    pub own: Arc<HashMap<String, String>>,
}

/// Cut every deal's tape at the same distance past its close — the exit horizon the whole
/// sample is judged on.
///
/// The tapes of a sample were captured under different margins (the setting moves; a close
/// filed under 5 min sits beside one filed under 30 s), and a variant judged on each deal's OWN
/// tape end is judged unevenly: on the long tape it gets minutes to reach its take, on the
/// short one seconds, and a variant that outlives the tape drops out of the tally
/// (`OpenAtWindowEnd`) — the long tapes then flatter every slow exit. One horizon for all,
/// the shortest trail among them, is the only fair comparison the sample allows; a deal a
/// variant has not closed by then still drops out, but now every deal drops out at the same
/// distance. The decision of 2026-09-20.
///
/// Args:
///     deals: The prepared sample; a deal whose tape already ends at or before the horizon is
///         left untouched.
///     horizon_ms: The horizon past each close, from [`common_horizon_ms`].
pub fn clip_to_horizon(deals: &mut [PreparedDeal], horizon_ms: i64) {
    for deal in deals.iter_mut() {
        let end_ms = deal.deal.close_ms.saturating_add(horizon_ms.max(0));
        let keep = deal.ticks.partition_point(|t| (t.time_ms as i64) <= end_ms);
        if keep < deal.ticks.len() {
            deal.ticks = Arc::from(&deal.ticks[..keep]);
        }
    }
}

/// The exit horizon a sample allows: the shortest held trail past the close among its deals
/// ([`PreparedDeal::trail_ms`]), or `None` for an empty sample. See [`clip_to_horizon`].
pub fn common_horizon_ms(deals: &[PreparedDeal]) -> Option<i64> {
    deals.iter().map(|d| d.trail_ms.max(0)).min()
}

/// What one search varies and how.
pub struct SearchParams<'a> {
    /// Values held over every deal's own base ([`PreparedDeal::own`]) before the point is laid
    /// on — the variant's edits when one field is searched in it (the searched field among
    /// them, which the point then overrides); empty otherwise.
    pub held: &'a HashMap<String, String>,
    /// Schema defaults for the keys a deal's base leaves out.
    pub defaults: &'a HashMap<String, f64>,
    /// The strategy kind of the sample, for the fields the grid offers.
    pub kind: &'a str,
    /// Whether the Entry group is searched (only when the kind has an entry model).
    pub vary_entry: bool,
    /// Whether the Exit group is searched.
    pub vary_exit: bool,
    /// Field keys held at each deal's base value.
    pub locked: &'a HashSet<String>,
    /// Restart count, at least 1.
    pub restarts: usize,
    /// Minimum trades a point must keep, or one tenth of the fitted sample.
    pub min_n: Option<i64>,
    /// Base seed of the restarts; `None` draws one from the clock.
    pub seed: Option<u64>,
    /// Share of the period, oldest first, the search may fit on.
    pub train_frac: f64,
    /// Passes of coordinate descent per restart, at least 1.
    pub max_passes: usize,
    /// The model's own settings, the entry method among them.
    pub model: ModelSettings,
}

/// What the search found.
#[derive(Clone, Debug)]
pub struct SearchResult {
    /// The winning values, in strategy spelling — only the fields that moved off the base of
    /// at least one deal.
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

/// The model parameters a point comes to on a deal whose strategy holds `own`, with `held` laid
/// over it first.
fn params_of(
    own: &HashMap<String, String>,
    held: &HashMap<String, String>,
    defaults: &HashMap<String, f64>,
    point: &Point,
    kind: &str,
    model: ModelSettings,
) -> (EntryParams, ExitParams) {
    let mut values = own.clone();
    for (key, value) in held {
        values.insert(key.clone(), value.clone());
    }
    for (key, value) in point {
        values.insert((*key).to_string(), value.clone());
    }
    let sv = StrategyValues {
        values: &values,
        defaults,
    };
    let entry = if entry_model_for(kind) {
        EntryParams::MoonShot(mshot_params(&sv, model))
    } else {
        EntryParams::Fact
    };
    (entry, exit_params(&sv, model))
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
        // The same gate the grid applies: a field this kind's model does not read would be
        // varied for nothing, land in a variant column the grid cannot show, and be written by
        // Save all the same.
        .filter(|f| !f.not_kinds.contains(&p.kind))
        // A field the entry method does not read moves nothing either.
        .filter(|f| p.model.entry_method.reads(f.key))
        .filter(|f| !p.locked.contains(f.key))
        .collect()
}

/// The distinct strategy bases of a sample ([`PreparedDeal::own`]) and which one each deal runs
/// over: a point's parameters are built once per strategy, not once per deal.
struct Bases<'a> {
    owns: Vec<&'a HashMap<String, String>>,
    /// Each deal's index into `owns`, in the deals' order.
    of_deal: Vec<usize>,
}

impl<'a> Bases<'a> {
    fn of(deals: &'a [PreparedDeal]) -> Self {
        let mut owns: Vec<&'a HashMap<String, String>> = Vec::new();
        let of_deal = deals
            .iter()
            .map(|d| {
                let own = d.own.as_ref();
                match owns.iter().position(|o| std::ptr::eq(*o, own) || *o == own) {
                    Some(index) => index,
                    None => {
                        owns.push(own);
                        owns.len() - 1
                    }
                }
            })
            .collect();
        Self { owns, of_deal }
    }

    /// A point's parameters on every base, in the bases' order.
    fn params(
        &self,
        held: &HashMap<String, String>,
        defaults: &HashMap<String, f64>,
        point: &Point,
        kind: &str,
        model: ModelSettings,
    ) -> Vec<(EntryParams, ExitParams)> {
        self.owns
            .iter()
            .map(|own| params_of(own, held, defaults, point, kind, model))
            .collect()
    }

    /// Whether `value` of `key` is something at least one base, under `held`, does not hold.
    fn moves(&self, held: &HashMap<String, String>, key: &str, value: &str) -> bool {
        self.owns
            .iter()
            .any(|own| held.get(key).or_else(|| own.get(key)).map(String::as_str) != Some(value))
    }
}

/// Every deal's result under one point, in order — `(money, spent)`, `None` where the point
/// makes no trade of the deal.
///
/// Args:
///     deals: The deals.
///     of_deal: Each deal's index into `params` ([`Bases::of_deal`]), as long as `deals`.
///     params: The point's parameters per base ([`Bases::params`]).
fn results(
    deals: &[PreparedDeal],
    of_deal: &[usize],
    params: &[(EntryParams, ExitParams)],
) -> Vec<Option<(f64, f64)>> {
    deals
        .par_iter()
        .zip(of_deal.par_iter())
        .map(|(d, &base)| {
            let (entry, exit) = &params[base];
            simulate(&d.deal, &d.ticks, entry, exit, d.entry_line.as_deref())
                .profit_money(&d.deal)
                .map(|money| (money, d.deal.spent))
        })
        .collect()
}

/// The tally of a point over `deals`, in order, and the spend of the deals it traded; arguments
/// as for [`results`].
fn tally_and_spent(
    deals: &[PreparedDeal],
    of_deal: &[usize],
    params: &[(EntryParams, ExitParams)],
) -> (Tally, f64) {
    // The replay of every deal is independent; the tally is folded in order afterwards.
    let results = results(deals, of_deal, params);
    let mut tally = Tally::default();
    let mut spent = 0.0;
    for (money, size) in results.into_iter().flatten() {
        tally.push(money);
        spent += size;
    }
    (tally, spent)
}

/// The tally of a point over `deals`, in order; arguments as for [`results`].
fn tally(deals: &[PreparedDeal], of_deal: &[usize], params: &[(EntryParams, ExitParams)]) -> Tally {
    tally_and_spent(deals, of_deal, params).0
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

/// How many deals of a sample, oldest first, the search fits on under `train_frac` — the slice
/// its `min_n` floor is held on; the rest is the holdout. Taken on the close stamps alone, so a
/// caller can ask before it has the tapes at hand.
///
/// Args:
///     closes: The sample's close stamps, chronological.
///     train_frac: [`SearchParams::train_frac`].
pub fn train_len(closes: &[i64], train_frac: f64) -> usize {
    train_split(closes, train_frac)
}

/// Run the search.
///
/// Args:
///     deals: The covered deals, chronological by close.
///     params: What to vary and how hard to look.
///     handle: Stop and progress; a fresh one per run.
///
/// Returns:
///     The best point found, or `None` when the sample is empty, nothing is varied, the run
///     was stopped before its first restart finished, or no point it visited keeps `min_n`
///     trades — the richest point under the floor is not what the caller asked for.
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
    let train_n = train_len(&closes, params.train_frac);
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
    let max_passes = params.max_passes.max(1);
    let model = params.model.sanitized();
    let bases = Bases::of(deals);
    let train_of = &bases.of_deal[..train_n];
    let evaluate = |point: &Point| -> Tally {
        let per_base = bases.params(params.held, params.defaults, point, params.kind, model);
        tally(train, train_of, &per_base)
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
                for _ in 0..max_passes {
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
    // `better` ranks a point under the floor below any above it, so a best under it means no
    // point held the floor at all.
    if train_tally.n < min_n {
        return None;
    }
    // A field every deal's base already spells so is not a change; only what moved is
    // reported — a value one strategy holds and another does not is a change for the other.
    let mut values: Vec<(String, String)> = point
        .iter()
        .filter(|(key, value)| bases.moves(params.held, key, value))
        .map(|(key, value)| ((*key).to_string(), value.clone()))
        .collect();
    values.sort();
    let holdout = (train_n < deals.len()).then(|| {
        let per_base = bases.params(params.held, params.defaults, &point, params.kind, model);
        tally(&deals[train_n..], &bases.of_deal[train_n..], &per_base)
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
///     deals: The covered deals, chronological; each is run over its own strategy's values
///         ([`PreparedDeal::own`]).
///     defaults: Schema defaults.
///     kind: The strategy kind.
///     values: The variant's changes over each deal's base, in strategy spelling.
///     model: The model's own settings, the entry method among them.
pub fn variant_tally(
    deals: &[PreparedDeal],
    defaults: &HashMap<String, f64>,
    kind: &str,
    values: &[(String, String)],
    model: ModelSettings,
) -> (Tally, f64) {
    let bases = Bases::of(deals);
    let per_base = bases.params(
        &HashMap::new(),
        defaults,
        &point_of(values),
        kind,
        model.sanitized(),
    );
    install(|| tally_and_spent(deals, &bases.of_deal, &per_base))
}

/// One variant on one deal, as the tuner's trade pane draws it.
#[derive(Clone, Debug, PartialEq)]
pub struct VariantPicture {
    /// Where the entry filled and the exit closed.
    pub outcome: Outcome,
    /// The order's corridor as the model walked it, placement by placement — a MoonShot variant
    /// replayed by the corridor model only; empty for a shift and for a kind without an entry
    /// model, which walk no corridor of their own.
    pub corridor: Vec<CorridorStep>,
}

/// One variant replayed on ONE deal — what the tuner's trade pane draws beside the fact: where
/// the variant's entry filled and where its exit closed, by the same parameters and the same
/// replay [`variant_tally`] scores the column with, and the corridor its order walked.
///
/// Args:
///     deal: The deal with its tape, cut at the sample's horizon as the column's are.
///     defaults, kind, values, model: As for [`variant_tally`].
///
/// Returns:
///     The modelled outcome and corridor.
pub fn variant_picture(
    deal: &PreparedDeal,
    defaults: &HashMap<String, f64>,
    kind: &str,
    values: &[(String, String)],
    model: ModelSettings,
) -> VariantPicture {
    let (entry, exit) = params_of(
        &deal.own,
        &HashMap::new(),
        defaults,
        &point_of(values),
        kind,
        model.sanitized(),
    );
    let line = deal.entry_line.as_deref();
    let outcome = simulate(&deal.deal, &deal.ticks, &entry, &exit, line);
    // A variant that keeps the trade's own entry fills where the report says (`simulate`), and
    // the fact's own line is already on the chart: a modelled path beside it would end somewhere
    // else than the fill it is drawn with.
    let own = |params: &super::MshotParams| {
        matches!(
            deal.deal.own_entry.as_ref(),
            Some(EntryParams::MoonShot(own)) if own.same_strategy(params)
        )
    };
    let corridor = match &entry {
        EntryParams::MoonShot(params)
            if params.model.entry_method == EntryMethod::Model && !own(params) =>
        {
            MshotEntry::new(params)
                .corridor(&deal.deal, &deal.ticks, line)
                .1
        }
        _ => Vec::new(),
    };
    VariantPicture { outcome, corridor }
}

/// Each deal's `(report_uid, (money, per cent))` under one variant, in the deals' order; `None`
/// where the variant makes no trade of the deal.
pub type DealResults = Vec<(i64, Option<(f64, f64)>)>;

/// Every deal's result under one variant — `(money, per cent)`, money in the sample's unit —
/// what the deal table's plan column shows. `None` where the variant makes no trade of the deal
/// (no fill, or still open where the tape ends): the same rule that leaves the deal out of
/// [`variant_tally`].
///
/// Args:
///     deals, defaults, kind, values, model: As for [`variant_tally`].
///
/// Returns:
///     The tally, the spent sum, and each deal's result ([`DealResults`]).
pub fn variant_tally_by_deal(
    deals: &[PreparedDeal],
    defaults: &HashMap<String, f64>,
    kind: &str,
    values: &[(String, String)],
    model: ModelSettings,
) -> (Tally, f64, DealResults) {
    let bases = Bases::of(deals);
    let per_base = bases.params(
        &HashMap::new(),
        defaults,
        &point_of(values),
        kind,
        model.sanitized(),
    );
    install(|| {
        let money: DealResults = deals
            .par_iter()
            .zip(bases.of_deal.par_iter())
            .map(|(d, &base)| {
                let (entry, exit) = &per_base[base];
                let outcome = simulate(&d.deal, &d.ticks, entry, exit, d.entry_line.as_deref());
                let result = outcome.profit_money(&d.deal).zip(outcome.profit_pct);
                (d.deal.report_uid, result)
            })
            .collect();
        let mut tally = Tally::default();
        let mut spent = 0.0;
        for (deal, (_, value)) in deals.iter().zip(&money) {
            if let Some((value, _)) = value {
                tally.push(*value);
                spent += deal.deal.spent;
            }
        }
        (tally, spent, money)
    })
}

/// A variant's changes as a point — the one reading [`variant_tally`],
/// [`variant_tally_by_deal`] and [`variant_picture`] take, so a column, its per-deal share and a
/// picture cannot differ. A key the grid does not know is dropped.
fn point_of(values: &[(String, String)]) -> Point {
    let mut point = Point::new();
    for (key, value) in values {
        if let Some(field) = TICK_PARAMS.iter().find(|f| f.key == key) {
            point.insert(field.key, value.clone());
        }
    }
    point
}

#[cfg(test)]
mod tests;
