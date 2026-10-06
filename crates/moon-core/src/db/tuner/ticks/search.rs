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
//! with nothing standing to close a trade, is refused outright ([`closing`], the developer,
//! 2026-09-24): dropping the deal would reward the loss it carries past the tape. So is one
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
use crate::db::tuner::threshold_search::search::{install, restart_seed};
use crate::db::tuner::threshold_search::{SearchHandle, train_split};
use crate::feed::types::Tick;

/// types items for tick searches.
mod types;
pub use types::{
    DEFAULT_MAX_PASSES, PreparedDeal, SearchMiss, SearchParams, SearchResult, SearchStats,
    clip_to_horizon, common_horizon_ms,
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
    let score_point = |point: &Point| -> Option<Tally> {
        let per_base = per_base_at(point);
        if corridor_refuses(&per_base) {
            return None;
        }
        // Counted here, past the refusals: what the stats call a scored point is a replay.
        evaluations.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        // A trade must be closed while it lasts: something that can close it stands on every
        // strategy, and none of the deals it bought is left open (`closing`).
        let closed = closing::protected(&per_base)
            .then(|| {
                let fills = fill_cache.fills(train, train_of, &per_base);
                closing::closed_tally(train, train_of, &per_base, &fills)
            })
            .flatten();
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
    let evaluate = |point: &Point| -> Option<Tally> {
        let tally = score_point(point)?;
        if params.risk.allows(&tally, &fact_train) {
            Some(tally)
        } else {
            risky.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            None
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
                    nested::descend_nested(point, &walk, &evaluate)
                } else {
                    descend(
                        point,
                        params.grids,
                        &order,
                        &pairs,
                        &coupling,
                        &start,
                        &evaluate,
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
    let refused = runs.iter().filter(|run| run.score.is_none()).count();
    let best = runs
        .into_iter()
        .reduce(|a, b| {
            if better_score(&b.score, &a.score, min_n) {
                b
            } else {
                a
            }
        })
        .ok_or(SearchMiss::Nothing)?;
    // What the descents refused, read before the trim's own trials add to it.
    let load = |c: &std::sync::atomic::AtomicUsize| c.load(std::sync::atomic::Ordering::Relaxed);
    let (cornered_n, unclosed_n, risky_n) = (load(&cornered), load(&unclosed), load(&risky));
    let (best_restart, passes, converged) = (best.restart, best.passes, best.converged);
    let (point, score) =
        drop_passengers(best.point, best.score, &pinned, &evaluate, min_n, &|| {
            handle.is_cancelled()
        });
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
    // The answer as it was scored — every field it switched on at a value, the passengers already
    // dropped — less what is in effect on no strategy.
    let point = deps.prune(
        &deps.complete(&point, &bases.owns, params.held, params.defaults),
        &bases.owns,
        params.held,
        params.defaults,
    );
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
    // A field every deal's base already spells so is not a change; only what moved is
    // reported — a value one strategy holds and another does not is a change for the other.
    let mut values: Vec<(String, String)> = point
        .iter()
        .filter(|(key, value)| bases.moves(params.held, key, value))
        .map(|(key, value)| ((*key).to_string(), value.clone()))
        .collect();
    values.sort();
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
