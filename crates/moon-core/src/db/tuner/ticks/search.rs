//! The search of the "Entry/Exit" axis: coordinate descent with restarts over the discrete
//! grids the caller hands it ([`SearchParams::grids`], `params::range`), scoring a point by REPLAYING every covered deal under it — the shape
//! of `threshold_search`, with the SQL mask replaced by [`simulate`]. Restart 0 starts from the
//! strategy itself, but for a field it holds off the field's grid ([`pinned`]); the others from
//! the strategy moved a few steps on a few fields, each walking
//! the fields in an order of its own. A pass that moves no single field then tries PAIRS of the
//! Entry group's number fields, one a step down and another a step up, so a corridor's distance
//! can move between the base fields and the modifiers ([`descend`]). A search of both groups
//! scores every entry point by a descent of the exit under it ([`nested`]), so an entry that pays
//! only with its own exit is reached.
//!
//! A point is a set of strategy values in the strategy's own spelling, laid over the values
//! each deal's OWN strategy holds now ([`PreparedDeal::own`]) — a field the point leaves alone
//! runs every deal at its strategy's value, never at a default because the selected strategies
//! disagree on it; the model's parameter structs are built from that overlay through
//! the same builders the grid's "now" column uses, so what the search varies is exactly what
//! Save writes. Only the fields of the groups the caller switched on are searched, minus the
//! ones it locked.
//!
//! The objective is the total result over the fitted deals, in the scope's metric and net of
//! each deal's own execution cost as the "Fact" column counts it
//! ([`super::Outcome::profit_metric`]), with at least `min_n` of them still trading, ties broken
//! by the profit factor — `metrics::Tally`, the same figures the KPI matrix prints. A deal the
//! variant never fills is not a trade and drops out of `n`; the caller prints "by N of M" beside
//! the column so a variant that wins by trading less is visible as such. A point that buys a deal and does not close it inside its tape, or leaves a strategy
//! with nothing standing to close a trade, is never the answer ([`closing`], the developer,
//! 2026-09-24): dropping the deal would reward the loss it carries past the tape. A search of the
//! Entry group still steers by such a point — by what it makes on the deals it closed — and walks
//! the exit under it to close the rest; the best one that leaves deals open is handed back beside
//! the answer, for the axis to lay over В1 with its open deals counted ([`candidate`], LinKvo,
//! 2026-10-07). A point that closes every deal is refused outright
//! whose max drawdown or win rate on the fitted deals is worse than the fact's there by more
//! than the risk limits allow ([`risk`]). A switch the point turns on brings the values it
//! needs ([`deps`]).
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

use super::exit::line::LinePoint;
use super::mshot::{CorridorStep, EntryMethod, MshotEntry, MshotParams};
use super::params::range::Grids;
use super::params::{
    ParamGroup, ParamKind, StrategyValues, TICK_PARAMS, TickParam, exit_params, mshot_params,
};
use super::settings::ModelSettings;
use super::unmodelled::same_value;
use super::{
    Deal, Deltas, EntryParams, ExitModel, ExitParams, Fill, Outcome, entry_model_for, simulate,
    simulate_from,
};
use crate::db::metrics::Tally;
use crate::db::tuner::threshold_search::search::{install, install_column, restart_seed};
use crate::db::tuner::threshold_search::{SearchHandle, train_split};
use crate::feed::types::Tick;

/// types items for tick searches.
mod types;
pub use types::{
    Candidate, DEFAULT_MAX_PASSES, PreparedDeal, SearchMiss, SearchParams, SearchResult,
    SearchStats, clip_to_horizon, common_horizon_ms,
};

/// descent items for tick searches.
mod descent;
use descent::*;

/// scoring items for tick searches.
mod scoring;
use scoring::*;
pub use scoring::{CorridorCheck, VariantScore, check_corridors, train_len};

/// tally items for tick searches.
mod tally;
use tally::*;
pub use tally::{
    DealResults, MIN_HOLDOUT, MIN_SEARCH_DEALS, VariantPicture, comparable, default_min_n,
    fact_tally, sample_floor, variant_picture, variant_tally, variant_tally_by_deal,
};

/// Run the search.
///
/// Args:
///     deals: The covered deals, chronological by close.
///     params: What to vary and how hard to look.
///     handle: Stop and progress; a fresh one per run.
///
/// Returns:
///     The best point found, or why there is none ([`SearchMiss`]): nothing to search or a
///     stop, no point that keeps `min_n` trades — the richest point under the floor is not what
///     the caller asked for — none that keeps the corridor, none that closes what it buys, or
///     none within the risk limits.
pub fn suggest(
    deals: &[PreparedDeal],
    params: &SearchParams<'_>,
    handle: &SearchHandle,
) -> Result<SearchResult, SearchMiss> {
    suggest_with_candidate(deals, params, handle).result
}

/// What a search answers, and the point beside it that leaves deals open.
#[derive(Debug)]
pub struct Suggested {
    /// The answer, or why there is none — as [`suggest`].
    pub result: Result<SearchResult, SearchMiss>,
    /// The best point a search of the Entry group met that leaves deals of the training slice
    /// open inside the tape, when it earns more on the deals it closed than the answer — or
    /// when there is no answer ([`candidate`]). Never [`Self::result`]: what its open deals make
    /// is on no record, so a caller that shows it says how many it left open.
    pub candidate: Option<Candidate>,
    /// The fields the search varied, sorted — as [`SearchResult::searched`], and there too when
    /// the search has no answer, so a candidate can be laid over them.
    pub searched: Vec<String>,
}

/// Run the search, and keep the point that leaves deals open beside its answer
/// ([`Suggested::candidate`]); arguments as for [`suggest`].
pub fn suggest_with_candidate(
    deals: &[PreparedDeal],
    params: &SearchParams<'_>,
    handle: &SearchHandle,
) -> Suggested {
    let mut candidate = None;
    let result = search(deals, params, handle, &mut candidate);
    let mut searched: Vec<String> = varied(params).iter().map(|f| f.key.to_string()).collect();
    searched.sort();
    Suggested {
        result,
        candidate,
        searched,
    }
}

/// The search itself; `candidate_out` takes the point beside the answer ([`Suggested`]).
fn search(
    deals: &[PreparedDeal],
    params: &SearchParams<'_>,
    handle: &SearchHandle,
    candidate_out: &mut Option<Candidate>,
) -> Result<SearchResult, SearchMiss> {
    let fields = varied(params);
    // A searched field starts from the strategies (or its grid step, `pinned`), never from what
    // the held edits put there (LinKvo, 2026-09-25: "a ticked field is searched anew, whatever
    // В1 holds"): its held value is set aside, and only the fields the search leaves alone are
    // held.
    let held: HashMap<String, String> = params
        .held
        .iter()
        .filter(|(key, _)| !fields.iter().any(|f| f.key == key.as_str()))
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();
    let params = &SearchParams {
        held: &held,
        ..*params
    };
    // The strategies of the WHOLE sample stay the bases throughout, a strategy whose every deal
    // leaves the sample below among them: where each number field starts, what completes a point
    // (`deps`) and the parameters built per base are then the same for the filter and for every
    // point scored after it, restart 0's included.
    let Cut {
        whole,
        start,
        deps,
        kept,
        of_kept,
        left_open,
    } = cut(deals, params);
    let deals = kept.as_slice();
    if deals.is_empty() || fields.is_empty() {
        return Err(SearchMiss::Nothing);
    }
    let closes: Vec<i64> = deals.iter().map(|d| d.deal.close_ms).collect();
    let train_n = train_len(&closes, params.train_frac);
    let train = &deals[..train_n];
    let min_n = params
        .min_n
        .unwrap_or_else(|| default_min_n(train_n))
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
    let bases = Bases {
        owns: whole.owns,
        of_deal: of_kept,
    };
    let train_of = &bases.of_deal[..train_n];
    // A searched field some strategy holds off its grid starts every restart on the grid
    // (`pinned`): the value the range leaves out is never an answer by standing still.
    let pinned = pinned::off_grid(&fields, params.grids, &start, &bases.owns, params.defaults);
    // Held over the whole sample, the holdout included: a corridor nearer the price than a
    // trade's own is out whichever side of the cut the trade sits on.
    let guard = (params.keep_corridor && params.vary_entry).then(|| CorridorGuard::of(deals));
    // Which bases start with their corridor fields in order: the search must not invert one
    // that is, and must not be held hostage by one that already is — a strategy stored inverted
    // would otherwise refuse every point of a search that never touches its corridor. The
    // write warns about that one (`check_corridors`).
    let start_ordered: Vec<bool> = bases
        .params(
            params.held,
            params.defaults,
            &Point::new(),
            params.kind,
            model,
        )
        .iter()
        .map(|(entry, _)| ordered(entry))
        .collect();
    let evaluations = std::sync::atomic::AtomicUsize::new(0);
    // Why points were refused, for the answer's reason when none is left.
    let (cornered, unclosed) = (
        std::sync::atomic::AtomicUsize::new(0),
        std::sync::atomic::AtomicUsize::new(0),
    );
    // Every number field the point switches on stands at a value (`deps`).
    let per_base_at = |point: &Point| {
        let full = deps.complete(point, &bases.owns, params.held, params.defaults);
        bases.params(params.held, params.defaults, &full, params.kind, model)
    };
    let coupling = coupled::Coupling::of(&fields, &per_base_at);
    // A point that inverts the corridor's two fields is never proposed, whatever the switch: the
    // searched fields are gridded one by one, and nothing else ties them. Only a search of the
    // Entry group can produce one — an Exit search leaves the strategy's own fields alone,
    // whatever they hold. Both rules read the entry alone, which is what lets the nested search
    // refuse an entry point before it searches the exit under it.
    let corridor_refuses = |per_base: &[(EntryParams, ExitParams)]| {
        let refused = (params.vary_entry && inverts(&start_ordered, per_base))
            || guard
                .as_ref()
                .is_some_and(|g| !g.holds(&bases.of_deal, per_base));
        if refused {
            cornered.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
        refused
    };
    // The fills of the entries scored lately: most points share their entry with the point
    // before (`fills`).
    let fill_cache = fills::FillCache::default();
    // A point's replay of the training slice, or `None` where it cannot trade at all: a corridor
    // rule refuses it, or a strategy is left with nothing standing to close a trade (`closing`).
    let replay_point = |point: &Point| -> Option<closing::Replayed> {
        let per_base = per_base_at(point);
        if corridor_refuses(&per_base) {
            return None;
        }
        // Counted here, past the refusals: what the stats call a scored point is a replay.
        evaluations.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        if !closing::protected(&per_base) {
            unclosed.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            return None;
        }
        let fills = fill_cache.fills(train, train_of, &per_base);
        Some(closing::replay(train, train_of, &per_base, &fills))
    };
    // A point the search may answer: one that closes every deal it bought (`closing`).
    let score_point = |point: &Point| -> Option<Tally> {
        let replayed = replay_point(point)?;
        let closed = replayed.closed_all().cloned();
        if closed.is_none() {
            unclosed.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
        closed
    };
    // The strategies as they stand on the training slice, which the log sets the answer
    // against.
    let base_score = score_point(&Point::new());
    // The fact on the same slice — the "Fact" column the KPI matrix compares a variant with, and
    // always there, where a replayed base may be refused — is what the risk limits hold a point
    // to (`risk`).
    let fact_train = fact_tally(train);
    let risky = std::sync::atomic::AtomicUsize::new(0);
    let within_risk = |tally: &Tally| {
        let allowed = params.risk.allows(tally, &fact_train);
        if !allowed {
            risky.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
        allowed
    };
    let evaluate = |point: &Point| -> Option<Tally> {
        let tally = score_point(point)?;
        within_risk(&tally).then_some(tally)
    };
    // The best points the restarts met: the one to answer, and the one beside it that leaves
    // deals open (`candidate`).
    let tracker = candidate::Tracker::default();
    // How a restart scores a point, offering it to the tracker on the way. An exit search walks
    // by the answer's own score; a search of the Entry group steers by what a point makes on the
    // deals it closed (`Steer::Closed`) and, under an entry point, walks the exit to close its
    // deals first (`Steer::Closing`).
    let scored_in = |restart: usize, steer: Steer, point: &Point| -> Option<Tally> {
        let replayed = replay_point(point)?;
        if replayed.open > 0 {
            unclosed.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            // An exit search answers or refuses; nothing else is made of an open point.
            if steer == Steer::Answer {
                return None;
            }
        }
        // The risk limits refuse what may be answered — a point that closes every deal, as they
        // always did. A point that leaves deals open is not refused by them: they would read a
        // curve with holes in it, and refuse the very points the exit walk passes through on its
        // way to closing them. It is held to them on the deals it closed only to be shown as the
        // candidate, uncounted, since nothing was refused.
        let within = if replayed.open == 0 {
            if !within_risk(&replayed.closed) {
                return None;
            }
            true
        } else {
            params.risk.allows(&replayed.closed, &fact_train)
        };
        if within {
            tracker.offer(point, &replayed, restart, min_n);
        }
        match steer {
            Steer::Answer => replayed.closed_all().cloned(),
            Steer::Closed => Some(replayed.closed),
            Steer::Closing => Some(candidate::closing_score(&replayed)),
        }
    };
    // The fields that move in pairs: the Entry group's numbers, where a corridor's distance is
    // shared between the base fields and the modifiers and one field alone cannot move it.
    let pairs: Vec<&'static TickParam> = if params.vary_entry {
        fields
            .iter()
            .filter(|f| f.group == ParamGroup::Entry && f.kind == ParamKind::Num)
            .copied()
            .collect()
    } else {
        Vec::new()
    };
    // Both groups searched: every entry point is scored by a search of the exit under it
    // (`nested`), each group with its own coupling — the Delta Modifiers diagonals are the exit's.
    let (entry_fields, exit_fields): (Vec<&'static TickParam>, Vec<&'static TickParam>) =
        fields.iter().partition(|f| f.group == ParamGroup::Entry);
    let nested_on = !entry_fields.is_empty() && !exit_fields.is_empty();
    let entry_coupling = coupled::Coupling::of(&entry_fields, &per_base_at);
    let exit_coupling = coupled::Coupling::of(&exit_fields, &per_base_at);
    let entry_scored = std::sync::atomic::AtomicUsize::new(0);
    let refuses_entry = |entry: &Point| corridor_refuses(&per_base_at(entry));
    let same_entry = |a: &Point, b: &Point| {
        per_base_at(a)
            .iter()
            .zip(per_base_at(b).iter())
            .all(|((a, _), (b, _))| a == b)
    };
    let runs: Vec<Run> = install(|| {
        (0..restarts)
            .into_par_iter()
            .map(|restart| {
                if handle.is_cancelled() {
                    handle.note_abandoned();
                    return None;
                }
                // Restart 0 starts from the base itself, in grid order, the fields pinned on
                // their grids aside. The others start from
                // the base moved a few steps on a few fields, and walk the fields in an order of
                // their own: a start anywhere on the grid lands far from anything a strategy
                // would run and descends into a worse valley every time (2026-09-24: 19 of 20
                // random restarts lost to restart 0 on every run).
                let mut order = fields.clone();
                let mut point = Point::new();
                if restart > 0 {
                    let mut state = restart_seed(seed, restart);
                    shuffle(&mut order, &mut state);
                    perturb(&mut point, params.grids, &order, &start, &mut state);
                }
                for (key, value) in &pinned {
                    point.entry(key).or_insert_with(|| value.clone());
                }
                let walked = if nested_on {
                    // Each group in this restart's own order.
                    let (entry, exit): (Vec<&'static TickParam>, Vec<&'static TickParam>) =
                        order.iter().partition(|f| f.group == ParamGroup::Entry);
                    let walk = nested::Nested {
                        grids: params.grids,
                        start: &start,
                        entry: &entry,
                        exit: &exit,
                        pairs: &pairs,
                        entry_coupling: &entry_coupling,
                        exit_coupling: &exit_coupling,
                        min_n,
                        max_passes,
                        handle,
                        refused: &refuses_entry,
                        searched: &entry_scored,
                        screen: params.screen_entry,
                        same_entry: &same_entry,
                    };
                    let inner = |p: &Point| scored_in(restart, Steer::Closing, p);
                    let outer = |p: &Point| scored_in(restart, Steer::Closed, p);
                    nested::descend_nested(point, &walk, &inner, &outer)
                } else {
                    // An entry search steers by the deals a point closes; an exit search by the
                    // answer's own score, as it always did.
                    let steer = if params.vary_entry && !entry_fields.is_empty() {
                        Steer::Closed
                    } else {
                        Steer::Answer
                    };
                    descend(
                        point,
                        params.grids,
                        &order,
                        &pairs,
                        &coupling,
                        &start,
                        &|p: &Point| scored_in(restart, steer, p),
                        None,
                        min_n,
                        max_passes,
                        handle,
                    )
                }?;
                handle.record_restart();
                Some(Run {
                    restart,
                    point: walked.point,
                    score: walked.score,
                    passes: walked.passes,
                    converged: walked.converged,
                })
            })
            .flatten()
            .collect()
    });
    // Among equal scores the LOWEST restart wins, so the parallel fan-out answers as a
    // sequential run would: in restart order, a later run takes the lead only by beating it.
    let mut runs = runs;
    runs.sort_by_key(|run| run.restart);
    // An end point by the parameters it comes to on every base, not by its spelling: restart 0
    // leaves an on-grid field at its base by not holding it, a random restart by holding the base's
    // value — or the schema default's, for a field the strategy leaves out — and those are one
    // end point, not two.
    let mut ends: Vec<Vec<(EntryParams, ExitParams)>> = Vec::new();
    for run in &runs {
        let full = deps.complete(&run.point, &bases.owns, params.held, params.defaults);
        let end = bases.params(params.held, params.defaults, &full, params.kind, model);
        if !ends.contains(&end) {
            ends.push(end);
        }
    }
    let distinct = ends.len();
    let restarts_done = runs.len();
    if runs.is_empty() {
        return Err(SearchMiss::Nothing);
    }
    // What the descents refused, read before the counts below and the trim add to it.
    let load = |c: &std::sync::atomic::AtomicUsize| c.load(std::sync::atomic::Ordering::Relaxed);
    let (cornered_n, unclosed_n, risky_n) = (load(&cornered), load(&unclosed), load(&risky));
    // A restart steered by the deals a point closes can end on one that leaves some open: it is
    // refused all the same, read the answer's way.
    let refused = runs
        .iter()
        .filter(|run| run.score.is_none() || (params.vary_entry && evaluate(&run.point).is_none()))
        .count();
    // The answer is the best point that closes every deal, met anywhere in any restart — a
    // descent steered by the deals closed need not stand on it at its end.
    let answer = tracker.answer();
    let (best_restart, passes, converged) = answer
        .as_ref()
        .and_then(|met| runs.iter().find(|run| run.restart == met.restart))
        .map_or((0, 0, false), |run| {
            (run.restart, run.passes, run.converged)
        });
    let (point, score) = match answer {
        Some(met) => drop_passengers(
            met.point,
            Some(met.score),
            &pinned,
            &evaluate,
            min_n,
            &|| handle.is_cancelled(),
        ),
        None => (Point::new(), None),
    };
    // Every point scored, the trim's trials included.
    let stats = SearchStats {
        restarts: restarts_done,
        best_restart,
        passes,
        converged,
        distinct,
        evaluations: evaluations.load(std::sync::atomic::Ordering::Relaxed),
        refused,
        left_open: left_open.len(),
        entry_points: entry_scored.load(std::sync::atomic::Ordering::Relaxed),
        fills_reused: fill_cache.reused(),
    };
    // The strategies as they stand against the answer, on the slice both were fitted on: a best
    // below its own base is a search that could not reach the base — or one whose typed range
    // leaves the base's value out, which no restart stands on ([`pinned`]) — and the log says so.
    let brief = |t: &Option<Tally>| {
        t.as_ref()
            .map(|t| (t.n, (t.profit * 100.0).round() / 100.0))
    };
    // Which rule refused the base, when one did: how many deals' corridors it comes nearer
    // than, whether it inverts, whether it is guarded.
    let base_why = base_score.is_none().then(|| {
        let full = deps.complete(&Point::new(), &bases.owns, params.held, params.defaults);
        let per_base = bases.params(params.held, params.defaults, &full, params.kind, model);
        let nearer = guard.as_ref().map(|g| {
            g.deals
                .iter()
                .filter(|(i, own, deltas)| {
                    !keeps_corridor(&per_base[bases.of_deal[*i]].0, own, deltas)
                })
                .count()
        });
        (
            params.vary_entry && inverts(&start_ordered, &per_base),
            nearer,
            closing::protected(&per_base),
        )
    });
    log::info!(
        target: crate::diagnostics::TICKS_AXIS_TARGET,
        "[x] ticks search: base (n, profit) {:?} against best {:?} over {} training deal(s), {} left out; base refused by (inverts, deals nearer than their own corridor, guarded) {:?}; points refused by the corridor {}, by a deal left open {}, by the risk limits {}; points scored on entry fills read before {} of {}",
        brief(&base_score),
        brief(&score),
        train_n,
        left_open.len(),
        base_why,
        cornered_n,
        unclosed_n,
        risky_n,
        stats.fills_reused,
        stats.evaluations
    );
    // A point as scored — every field it switched on at a value — less what is in effect on no
    // strategy, and the fields of it that moved: a field every deal's base already spells so is
    // not a change, and a value one strategy holds and another does not is a change for the
    // other.
    let spelled = |point: &Point| -> (Point, Vec<(String, String)>) {
        let point = deps.prune(
            &deps.complete(point, &bases.owns, params.held, params.defaults),
            &bases.owns,
            params.held,
            params.defaults,
        );
        let mut values: Vec<(String, String)> = point
            .iter()
            .filter(|(key, value)| bases.moves(params.held, key, value))
            .map(|(key, value)| ((*key).to_string(), value.clone()))
            .collect();
        values.sort();
        (point, values)
    };
    // The answer, the passengers already dropped.
    let (point, values) = spelled(&point);
    // The best point that leaves deals open, when it earns more on the deals it closed than the
    // answer does on all of them — or when there is no answer at all: handed back beside the
    // answer, or beside why there is none, never as it (`candidate`).
    *candidate_out = tracker
        .candidate()
        .filter(|met| better_score(&Some(met.score.clone()), &score, min_n))
        .map(|met| Candidate {
            values: spelled(&met.point).1,
            train: met.score,
            open: met.open,
            deals: train_n,
        });
    if let Some(c) = candidate_out.as_ref() {
        log::info!(
            target: crate::diagnostics::TICKS_AXIS_TARGET,
            "[x] ticks search: candidate (closed n, profit) {:?}, {} left open, against the answer's {:?}: {:?}",
            brief(&Some(c.train.clone())),
            c.open,
            brief(&score),
            c.values
        );
    }
    // `better_score` ranks a refused point below every other, and `better` one under the floor
    // below any above it: a best refused or under the floor means no point held either.
    let Some(train_tally) = score else {
        // The rule that refused the most points is the one to name.
        let (unclosed, cornered, risky) = (unclosed_n, cornered_n, risky_n);
        return Err(if risky > unclosed.max(cornered) {
            SearchMiss::Risk
        } else if unclosed > cornered {
            SearchMiss::Unclosed
        } else {
            SearchMiss::Corridor
        });
    };
    if train_tally.n < min_n {
        return Err(SearchMiss::Floor);
    }
    let (holdout, holdout_open, holdout_open_profit) = if train_n < deals.len() {
        let per_base = bases.params(params.held, params.defaults, &point, params.kind, model);
        let held_back = self::score(&deals[train_n..], &bases.of_deal[train_n..], &per_base);
        (Some(held_back.tally), held_back.open, held_back.open_profit)
    } else {
        (None, 0, 0.0)
    };
    let fact_holdout = holdout.is_some().then(|| fact_tally(&deals[train_n..]));
    let holdout_loses = match (&holdout, &fact_holdout) {
        (Some(ours), Some(fact)) => holdout_open > 0 || ours.profit < fact.profit,
        _ => false,
    };
    let mut searched: Vec<String> = fields.iter().map(|f| f.key.to_string()).collect();
    searched.sort();
    Ok(SearchResult {
        values,
        searched,
        train: train_tally,
        holdout,
        holdout_open,
        holdout_open_profit,
        fact_train,
        fact_holdout,
        holdout_loses,
        seed,
        stats,
    })
}

mod candidate;
use self::candidate::Steer;
mod closing;
pub use self::closing::unguarded_strategies;
mod coupled;
mod deps;
mod fills;
mod nested;
mod pinned;
mod risk;
mod screen;
pub use self::risk::{DEFAULT_WORSE_PCT, RiskLimits};
mod size;
pub(in crate::db::tuner::ticks) use self::deps::strategy_values;
pub use self::size::{SearchSize, full_replays, point_cost, search_size};

#[cfg(test)]
pub(in crate::db::tuner::ticks) mod test_grids;
#[cfg(test)]
mod tests;
