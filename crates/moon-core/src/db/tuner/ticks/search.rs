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
    /// on — the axis passes the variant's edits for every search, so the fields it leaves alone
    /// run at what the earlier searches found. The fields it varies are not held: their values
    /// here are set aside, and each starts from the strategies, or from its grid step where a
    /// strategy holds it off the grid (`pinned`; [`SearchResult::searched`]).
    /// Empty searches from the strategies as they stand.
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
    /// Each number field's candidate values (`params::range::resolve`); a number field without
    /// one has nothing to try and is not varied.
    pub grids: &'a Grids,
    /// Restart count, at least 1.
    pub restarts: usize,
    /// Minimum trades a point must keep, or half the train deals.
    pub min_n: Option<i64>,
    /// Base seed of the restarts; `None` draws one from the clock.
    pub seed: Option<u64>,
    /// Share of the period, oldest first, the search may fit on.
    pub train_frac: f64,
    /// Passes of coordinate descent per restart, at least 1.
    pub max_passes: usize,
    /// The model's own settings, the entry method among them.
    pub model: ModelSettings,
    /// Whether a point whose entry corridor comes nearer the price than a trade's own, at any
    /// moment of that trade's entry order, is out of the search
    /// ([`MshotParams::never_closer_than`]). Read only while the Entry group is searched: a
    /// search of the exit alone moves no corridor.
    pub keep_corridor: bool,
    /// How much riskier than the fact an answer may be, on the deals it is fitted on: a point
    /// past a limit is refused ([`risk`]).
    pub risk: RiskLimits,
    /// Whether a search of both groups screens the entry moves of each step under the exit found
    /// so far and runs a whole exit descent under the best few only ([`screen`]). Read only when
    /// both groups are searched.
    pub screen_entry: bool,
}

/// Why a search came back with nothing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SearchMiss {
    /// Nothing to search — no deals, no field to vary — or the run was stopped before its
    /// first restart finished.
    Nothing,
    /// No point the search visited kept `min_n` trades.
    Floor,
    /// No point the search visited kept a corridor it may propose: `MShotPriceMin` below
    /// `MShotPrice` where the strategy had them so ([`MshotParams::is_ordered`]), and, under
    /// [`SearchParams::keep_corridor`], every trade's corridor at least as far from the price as
    /// the trade's own.
    Corridor,
    /// No point the search visited kept its max drawdown and win rate within the risk limits
    /// of the fact ([`SearchParams::risk`]).
    Risk,
    /// No point the search visited closed every deal it bought inside the tape with something
    /// standing to close each trade — a stop, or a trailing without a take profit ([`closing`]).
    Unclosed,
    /// The scored set holds fewer than [`MIN_SEARCH_DEALS`] deals: whatever a search fits on it
    /// is noise, so none is run ([`sample_floor`]).
    TooFew {
        /// Deals the set holds.
        n: usize,
    },
}

/// How a search went — what shows whether its restarts and passes changed anything.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SearchStats {
    /// Restarts that ran to the end (a stop leaves the rest out).
    pub restarts: usize,
    /// The restart the answer came from: 0 starts from the strategy itself, but for the fields
    /// pinned on their grids ([`pinned`]).
    pub best_restart: usize,
    /// Passes of coordinate descent the winning restart took.
    pub passes: usize,
    /// Whether the winning restart stopped because a pass changed nothing — `false` means it
    /// was still improving when the pass limit cut it.
    pub converged: bool,
    /// Distinct end points among the restarts: 1 means every restart ended at the same point.
    pub distinct: usize,
    /// Points scored over the whole run, each a replay of the training slice.
    pub evaluations: usize,
    /// Restarts that ended on a point the corridor rules or the closing rule (`closing`) refuse
    /// — none of their moves reached an allowed one.
    pub refused: usize,
    /// Deals the strategies as they stand leave open inside the tape, taken out of the sample
    /// before the search (`closing::closable_at_base`).
    pub left_open: usize,
    /// Entry points scored by a whole search of the exit under them — a search of both groups
    /// ([`nested`]); zero for a search of one.
    pub entry_points: usize,
    /// Points scored on the entry fills of a point scored before ([`fills`]) — the exit replayed
    /// alone.
    pub fills_reused: usize,
}

/// What the search found.
#[derive(Clone, Debug)]
pub struct SearchResult {
    /// The winning values, in strategy spelling — only the fields that moved off the base of
    /// at least one deal.
    pub values: Vec<(String, String)>,
    /// The fields the search varied, sorted: each one's answer is in `values`, or it is at the
    /// strategies' own value where that value is on the field's grid ([`pinned`]) — never at what
    /// the held edits had it at, which the search set aside.
    pub searched: Vec<String>,
    /// What they achieve on the deals they were fitted on.
    pub train: Tally,
    /// What they achieve on the deals held back, when any were.
    pub holdout: Option<Tally>,
    /// How many of the deals held back the answer bought and left open inside the tape — none
    /// may be among the deals it was fitted on; the holdout is only scored, so it says them.
    pub holdout_open: usize,
    /// What the `holdout_open` deals would make closed at the last print of their tapes
    /// ([`VariantScore::open_profit`]) — an estimate beside `holdout`, never part of it.
    pub holdout_open_profit: f64,
    /// The fact over the deals the search was fitted on ([`fact_tally`]).
    pub fact_train: Tally,
    /// The fact over the deals held back, when any were — the slice `holdout` is scored on.
    pub fact_holdout: Option<Tally>,
    /// Whether the answer loses to the fact on the holdout: it leaves a held-back deal open, or
    /// its holdout profit is below the fact's there.
    pub holdout_loses: bool,
    /// The seed the restarts were derived from.
    pub seed: u64,
    /// How the run went.
    pub stats: SearchStats,
}

/// Where one descent stopped.
struct Walked {
    point: Point,
    score: Option<Tally>,
    passes: usize,
    converged: bool,
}

/// One restart's descent from `point`: every pass visits each field in `order` over its whole
/// grid, keeping any value that beats the score; a pass that moves no single field then tries
/// the pairs — one field of `pairs` a step down, another a step up — so a distance shared
/// between two fields can move from one to the other, which no single move reaches when each
/// alone makes the score worse or breaks a corridor rule. The Delta Modifiers section is a
/// product ([`coupled`]): a field of it that moves nothing at the point is not scanned, and the
/// same stalled pass walks each (coefficient, term) pair stuck at zero along a diagonal of their
/// grids. The walk ends when a pass moves nothing, or at `max_passes`.
///
/// With a `screen`, a field's other values and the pair moves are each first scored by it, and
/// only the [`screen::KEEP`] best of a field, or of the pair moves, are scored by `evaluate`
/// ([`screen`]).
///
/// Returns:
///     Where it stopped, or `None` when the run was stopped.
#[allow(clippy::too_many_arguments)]
fn descend(
    mut point: Point,
    grids: &Grids,
    order: &[&'static TickParam],
    pairs: &[&'static TickParam],
    coupling: &coupled::Coupling<'_>,
    start: &HashMap<&'static str, usize>,
    evaluate: &(dyn Fn(&Point) -> Option<Tally> + Sync),
    screen: Option<&screen::Screen<'_>>,
    min_n: i64,
    max_passes: usize,
    handle: &SearchHandle,
) -> Option<Walked> {
    let mut score = evaluate(&point);
    let (mut passes, mut converged) = (0, false);
    for _ in 0..max_passes {
        passes += 1;
        let mut improved = false;
        for field in order {
            if handle.is_cancelled() {
                handle.note_abandoned();
                return None;
            }
            if coupling.inert(field, &point) {
                continue;
            }
            let mut current = point.get(field.key).cloned();
            let indices: Vec<usize> = match screen {
                None => (0..grids.arity(field)).collect(),
                Some(quick) => {
                    // Nearest the point first: of moves the screen cannot tell apart, the
                    // nearest are kept (`screen::best`).
                    let mut nearest: Vec<usize> = (0..grids.arity(field)).collect();
                    if let Some(at) = grid_index(grids, field, &point, start) {
                        nearest.sort_by_key(|&index| index.abs_diff(at));
                    }
                    let mut scored = Vec::new();
                    for index in nearest {
                        let candidate = grids.spell(field, index);
                        if current.as_deref() == Some(candidate.as_str()) {
                            continue;
                        }
                        if handle.is_cancelled() {
                            handle.note_abandoned();
                            return None;
                        }
                        point.insert(field.key, candidate);
                        scored.push((index, quick(&point)));
                        restore(&mut point, field.key, current.clone());
                    }
                    screen::best(scored, screen::KEEP, min_n)
                }
            };
            for index in indices {
                let candidate = grids.spell(field, index);
                if current.as_deref() == Some(candidate.as_str()) {
                    continue;
                }
                point.insert(field.key, candidate.clone());
                let trial = evaluate(&point);
                if better_score(&trial, &score, min_n) {
                    score = trial;
                    improved = true;
                    // The accepted value is what a rejected later candidate restores to.
                    current = Some(candidate);
                } else {
                    restore(&mut point, field.key, current.clone());
                }
            }
            // A field that moved may enable a better value of one visited before, hence the
            // passes; within one pass every field is visited once.
        }
        if !improved {
            let mut moves: Vec<(&'static TickParam, &'static TickParam)> = pairs
                .iter()
                .flat_map(|&down| pairs.iter().map(move |&up| (down, up)))
                .filter(|(down, up)| down.key != up.key)
                .collect();
            if let Some(quick) = screen {
                let mut scored = Vec::new();
                for (down, up) in moves {
                    if handle.is_cancelled() {
                        handle.note_abandoned();
                        return None;
                    }
                    if let Some(moved) = pair_moved(&point, grids, (down, up), start) {
                        scored.push(((down, up), quick(&moved)));
                    }
                }
                moves = screen::best(scored, screen::KEEP, min_n);
            }
            for (down, up) in moves {
                // Each pair is a replay of the sample: a stop is noticed between two of them,
                // not after a whole row.
                if handle.is_cancelled() {
                    handle.note_abandoned();
                    return None;
                }
                // Read where the point stands now: a pair kept earlier in this pass moved it.
                let Some(moved) = pair_moved(&point, grids, (down, up), start) else {
                    continue;
                };
                let trial = evaluate(&moved);
                if better_score(&trial, &score, min_n) {
                    score = trial;
                    improved = true;
                    point = moved;
                }
            }
            for (coefficient, term) in coupling.stuck(&point) {
                // A path walked earlier in this pass may have freed the pair.
                if !coupling.is_stuck(coefficient, term, &point) {
                    continue;
                }
                for path in coupled::Coupling::diagonals(grids, coefficient, term) {
                    improved |= coupled::walk_path(
                        &mut point,
                        (coefficient, term),
                        &path,
                        evaluate,
                        &mut score,
                        min_n,
                        handle,
                    )?;
                }
            }
        }
        if !improved {
            converged = true;
            break;
        }
    }
    Some(Walked {
        point,
        score,
        passes,
        converged,
    })
}

/// `point` with `down` a grid step down and `up` a step up, or `None` where either has no step
/// that way, or is not a number on its grid.
fn pair_moved(
    point: &Point,
    grids: &Grids,
    (down, up): (&'static TickParam, &'static TickParam),
    start: &HashMap<&'static str, usize>,
) -> Option<Point> {
    let d = grid_index(grids, down, point, start)?;
    let u = grid_index(grids, up, point, start)?;
    if d == 0 || u + 1 >= grids.arity(up) {
        return None;
    }
    let mut moved = point.clone();
    moved.insert(down.key, grids.spell(down, d - 1));
    moved.insert(up.key, grids.spell(up, u + 1));
    Some(moved)
}

/// Put a field back to what the point held: a value, or none (the base's).
fn restore(point: &mut Point, key: &'static str, was: Option<String>) {
    match was {
        Some(value) => {
            point.insert(key, value);
        }
        None => {
            point.remove(key);
        }
    }
}

/// Where a number field stands on its grid: the step the point holds, else the base's
/// (`start`). `None` for a field that is not a number, or a base with no value to snap.
fn grid_index(
    grids: &Grids,
    field: &TickParam,
    point: &Point,
    start: &HashMap<&'static str, usize>,
) -> Option<usize> {
    if field.kind != ParamKind::Num {
        return None;
    }
    match point.get(field.key) {
        Some(value) => (0..grids.arity(field)).find(|&i| grids.spell(field, i) == *value),
        None => start.get(field.key).copied(),
    }
}

/// The grid step nearest `value`.
fn nearest_step(grid: &[f64], value: f64) -> usize {
    grid.iter()
        .enumerate()
        .min_by(|(_, a), (_, b)| (*a - value).abs().total_cmp(&(*b - value).abs()))
        .map_or(0, |(i, _)| i)
}

/// Fisher–Yates over the restart's own stream.
fn shuffle<T>(items: &mut [T], state: &mut u64) {
    for i in (1..items.len()).rev() {
        let j = (next_random(state) % (i as u64 + 1)) as usize;
        items.swap(i, j);
    }
}

/// Move the base a little: one to three fields of `order`, a number field one to three grid
/// steps either way from where it stands (`start`), any other field to a value of its own.
fn perturb(
    point: &mut Point,
    grids: &Grids,
    order: &[&'static TickParam],
    start: &HashMap<&'static str, usize>,
    state: &mut u64,
) {
    if order.is_empty() {
        return;
    }
    let moves = 1 + (next_random(state) % 3) as usize;
    for _ in 0..moves {
        let field = order[(next_random(state) % order.len() as u64) as usize];
        let n = grids.arity(field);
        // A field with nothing to try is not varied (`varied`); guarded all the same, as the
        // modulo and the `n - 1` below would not survive it.
        if n == 0 {
            continue;
        }
        let index = match (&field.kind, start.get(field.key)) {
            (ParamKind::Num, Some(&at)) => {
                let step = 1 + (next_random(state) % 3) as usize;
                if next_random(state).is_multiple_of(2) {
                    at.saturating_sub(step)
                } else {
                    (at + step).min(n - 1)
                }
            }
            _ => (next_random(state) % n as u64) as usize,
        };
        point.insert(field.key, grids.spell(field, index));
    }
}

/// One restart's end: where the descent stopped and how it got there.
struct Run {
    restart: usize,
    point: Point,
    score: Option<Tally>,
    passes: usize,
    converged: bool,
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

/// The fields one search varies: those it offers ([`deps::offered`]) less the locked and the
/// number fields with nothing to try — no grid, as for a field nothing is known of.
fn varied<'a>(p: &SearchParams<'a>) -> Vec<&'static super::params::TickParam> {
    deps::offered(p)
        .into_iter()
        .filter(|f| !p.locked.contains(f.key))
        .filter(|f| p.grids.arity(f) > 0)
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

    /// Whether `value` of `key` is something at least one base, under `held`, does not hold — as
    /// a value, not as text: the search spells its points itself (`1`), a strategy as the core
    /// wrote it (`1.0`), and the two are one value (PriceDownTimer on HookTest01, 2026-09-26,
    /// landed in В1 as a change of `1.0` to `1`). A base leaving the field out — or holding it
    /// blank, which `same_value` would read as a boolean `false` — moves on any value: a value the
    /// search completed for a switch it turned on (`deps`) is written with it even where it
    /// equals the schema's default.
    fn moves(&self, held: &HashMap<String, String>, key: &str, value: &str) -> bool {
        self.owns.iter().any(|own| {
            held.get(key)
                .or_else(|| own.get(key))
                .filter(|base| !base.trim().is_empty())
                .is_none_or(|base| !same_value(base, value))
        })
    }
}

/// Every deal's result under one point, in order — `(result, spent)`, the result in the scope's
/// metric as the "Fact" column holds it ([`super::Outcome::profit_metric`]), `None` where the
/// point makes no trade of the deal — whether it bought the deal and left it open, and what
/// such a deal would make at the tape's end ([`super::Outcome::open_metric_at_tape_end`]).
///
/// Args:
///     deals: The deals.
///     of_deal: Each deal's index into `params` ([`Bases::of_deal`]), as long as `deals`.
///     params: The point's parameters per base ([`Bases::params`]).
///     fills: Each deal's entry fill under `params`, when read already ([`fills::FillCache`]);
///         `None` replays the entry too.
fn results(
    deals: &[PreparedDeal],
    of_deal: &[usize],
    params: &[(EntryParams, ExitParams)],
    fills: Option<&[Option<Fill>]>,
) -> Vec<(Option<(f64, f64)>, bool, Option<f64>)> {
    deals
        .par_iter()
        .enumerate()
        .zip(of_deal.par_iter())
        .map(|((i, d), &base)| {
            let (entry, exit) = &params[base];
            let outcome = match fills {
                Some(fills) => simulate_from(&d.deal, &d.ticks, fills[i], exit),
                None => simulate(&d.deal, &d.ticks, entry, exit, d.entry_line.as_deref()),
            };
            let result = outcome
                .profit_metric(&d.deal)
                .map(|value| (value, d.deal.spent));
            let at_tape_end = outcome.open_metric_at_tape_end(&d.deal, &d.ticks);
            (result, outcome.left_open(), at_tape_end)
        })
        .collect()
}

/// One variant's score over a set of deals: the tally of the deals it traded, their spend, and
/// the deals that fell out of the tally — so a variant never reads better than the fact because
/// deals silently dropped out.
#[derive(Clone, Debug, Default)]
pub struct VariantScore {
    /// The results of the deals the variant traded and closed, in order.
    pub tally: Tally,
    /// Sum of the entry sizes of the deals in `tally`.
    pub spent: f64,
    /// Deals the variant bought and left open inside the tape — no result on record.
    pub open: usize,
    /// What the `open` deals would make closed at the last print of their tapes, summed in the
    /// tally's metric ([`super::Outcome::open_metric_at_tape_end`]). An estimate shown beside
    /// the tally, never part of it; NaN once an open deal has no estimate (no print, no price,
    /// nothing spent under the percent metric), so the sum never claims deals it did not value.
    pub open_profit: f64,
    /// Deals the variant has no result for: not bought (its entry did not fill on the tape), or
    /// bought and closed with no result in the scope's metric (nothing spent under the percent
    /// metric).
    pub untraded: usize,
}

impl VariantScore {
    /// Count one deal's replay: its `(metric, spent)` when it made a trade, whether the
    /// position was left open, and what an open one makes at the tape's end.
    fn push(&mut self, result: Option<(f64, f64)>, left_open: bool, at_tape_end: Option<f64>) {
        match result {
            Some((value, size)) => {
                self.tally.push(value);
                self.spent += size;
            }
            None if left_open => {
                self.open += 1;
                self.open_profit += at_tape_end.unwrap_or(f64::NAN);
            }
            None => self.untraded += 1,
        }
    }
}

/// The score of a point over `deals`, in order; arguments as for [`results`].
fn score(
    deals: &[PreparedDeal],
    of_deal: &[usize],
    params: &[(EntryParams, ExitParams)],
) -> VariantScore {
    // The replay of every deal is independent; the score is folded in order afterwards.
    let mut score = VariantScore::default();
    for (result, left_open, at_tape_end) in results(deals, of_deal, params, None) {
        score.push(result, left_open, at_tape_end);
    }
    score
}

/// Each MoonShot deal's own corridor and the deltas its entry order lived through — what a
/// point's corridor is held against under [`SearchParams::keep_corridor`]. Read once per search:
/// the deltas along a track are the same for every point.
struct CorridorGuard {
    /// `(deal index, own corridor, deltas)`; a deal without a MoonShot entry of its own holds
    /// no corridor to keep.
    deals: Vec<(usize, MshotParams, Vec<Deltas>)>,
}

impl CorridorGuard {
    fn of(deals: &[PreparedDeal]) -> Self {
        Self {
            deals: deals
                .iter()
                .enumerate()
                .filter_map(|(i, d)| match &d.deal.own_entry {
                    Some(EntryParams::MoonShot(own)) => {
                        Some((i, own.clone(), d.deal.entry_deltas()))
                    }
                    _ => None,
                })
                .collect(),
        }
    }

    /// Whether a point's parameters keep every deal's corridor.
    ///
    /// Args:
    ///     of_deal: Each deal's index into `params` ([`Bases::of_deal`]).
    ///     params: The point's parameters per base ([`Bases::params`]).
    fn holds(&self, of_deal: &[usize], params: &[(EntryParams, ExitParams)]) -> bool {
        self.deals
            .iter()
            .all(|(i, own, deltas)| keeps_corridor(&params[of_deal[*i]].0, own, deltas))
    }
}

/// Whether an entry keeps a trade's own corridor ([`MshotParams::never_closer_than`]); an entry
/// of the fact's has none to move. The near bound is held only where the entry method reads it.
fn keeps_corridor(entry: &EntryParams, own: &MshotParams, deltas: &[Deltas]) -> bool {
    match entry {
        EntryParams::MoonShot(variant) => {
            let near_too = variant.model.entry_method != EntryMethod::Shift;
            variant.never_closer_than(own, deltas, near_too)
        }
        EntryParams::Fact => true,
    }
}

/// Whether an entry's corridor fields are in order ([`MshotParams::is_ordered`]); an entry of the
/// fact's has none.
fn ordered(entry: &EntryParams) -> bool {
    match entry {
        EntryParams::MoonShot(params) => params.is_ordered(),
        EntryParams::Fact => true,
    }
}

/// Whether a point's parameters invert the corridor fields of a base that started in order
/// (`start_ordered`, one flag per base, in the bases' order).
fn inverts(start_ordered: &[bool], params: &[(EntryParams, ExitParams)]) -> bool {
    start_ordered
        .iter()
        .zip(params)
        .any(|(was, (entry, _))| *was && !ordered(entry))
}

/// What [`check_corridors`] finds of one variant over a sample.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CorridorCheck {
    /// Deals whose corridor the variant brings nearer the price than their own.
    pub nearer: usize,
    /// Deals whose corridor fields the variant inverts (`MShotPriceMin ≥ MShotPrice`).
    pub inverted: usize,
    /// Deals with a MoonShot corridor of their own to hold.
    pub checked: usize,
}

/// The corridor rules the search keeps, asked of a variant the search did not make — one typed
/// into a column, before it is written: on how many deals it comes nearer the price than their
/// own corridor ([`SearchParams::keep_corridor`]), and on how many it inverts the corridor's two
/// fields ([`MshotParams::is_ordered`]).
///
/// Args:
///     deals: Each deal with its strategy's current values ([`PreparedDeal::own`]).
///     defaults, values, model: As for [`variant_tally`]; the kind is each deal's own.
pub fn check_corridors<'a>(
    deals: impl IntoIterator<Item = (&'a Deal, &'a HashMap<String, String>)>,
    defaults: &HashMap<String, f64>,
    values: &[(String, String)],
    model: ModelSettings,
) -> CorridorCheck {
    let point = point_of(values);
    let model = model.sanitized();
    let held = HashMap::new();
    let mut out = CorridorCheck::default();
    for (deal, own_base) in deals {
        let Some(EntryParams::MoonShot(own)) = &deal.own_entry else {
            continue;
        };
        let (entry, _) = params_of(own_base, &held, defaults, &point, &deal.kind, model);
        out.checked += 1;
        if !keeps_corridor(&entry, own, &deal.entry_deltas()) {
            out.nearer += 1;
        }
        if !ordered(&entry) {
            out.inverted += 1;
        }
    }
    out
}

/// Whether `a` beats `b` where a point may be out of the search: `None` is a point the corridor
/// guard refused, below every point it let through.
fn better_score(a: &Option<Tally>, b: &Option<Tally>, min_n: i64) -> bool {
    match (a, b) {
        (Some(a), Some(b)) => better(a, b, min_n),
        (Some(_), None) => true,
        (None, _) => false,
    }
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

/// `point` less every field the answer does not need: each in turn, in name order, is put back to
/// what the strategies hold, and stays out when the point scores no worse without it.
///
/// A restart starts from the strategies moved a few steps on a few fields, and descent never
/// moves a field whose every step scores the same: such a field rides along to the answer at the
/// value the start drew, and Save would write it. A field the dependency rules see as in effect
/// can still move nothing — 2026-10-01, `SellLevelDelayNext` 1 answered on strategies whose
/// SellLevel was off — so the answer is held to the score, not to the rules alone.
///
/// A field held off its grid ([`pinned`]) is never put back: the strategy's value is one its
/// range leaves out, and the answer comes from the range. Nor is one that trades a different
/// number of deals for the same result: it changes which deals trade, and that is not nothing.
///
/// Args:
///     point: The best point.
///     score: Its score; a refused point (`None`) is returned as it is.
///     pinned: The fields the search holds on their grids.
///     evaluate: The search's scoring, refusals included.
///     min_n: The sample floor.
///     cancelled: Whether the search was stopped; a stopped search answers its point untrimmed.
fn drop_passengers(
    point: Point,
    score: Option<Tally>,
    pinned: &Point,
    evaluate: &dyn Fn(&Point) -> Option<Tally>,
    min_n: i64,
    cancelled: &dyn Fn() -> bool,
) -> (Point, Option<Tally>) {
    if score.is_none() {
        return (point, score);
    }
    let mut keys: Vec<&'static str> = point
        .keys()
        .copied()
        .filter(|key| !pinned.contains_key(key))
        .collect();
    keys.sort_unstable();
    let (mut point, mut score) = (point, score);
    for key in keys {
        if cancelled() {
            break;
        }
        let mut without = point.clone();
        without.remove(key);
        let trial = evaluate(&without);
        let same_trades = matches!((&score, &trial), (Some(a), Some(b)) if a.n == b.n);
        let drop = better_score(&trial, &score, min_n)
            || (same_trades && !better_score(&score, &trial, min_n));
        if drop {
            point = without;
            score = trial;
        }
    }
    (point, score)
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

/// The fact of `deals`, in order: each deal's reported result — the same per-deal value
/// [`super::fact_stats`] folds into the "Fact" column, so the two cannot drift. Every deal
/// counts, as there.
pub fn fact_tally(deals: &[PreparedDeal]) -> Tally {
    super::stats::fact_tally_of(deals.iter().map(|d| &d.deal)).0
}

/// The deals a variant and the fact are compared over: `deals` less those the strategies as
/// they stand leave open inside the tape — the cut the search makes before it fits
/// ([`closing::closable_at_base`]), so the search's fact and the "Fact" column over the variant
/// columns are one set. A kept deal a variant leaves open stays out of its tally; its tape-end
/// estimate is counted beside it ([`VariantScore::open_profit`]).
///
/// Args:
///     deals: The covered deals, chronological.
///     params: What the base is completed with — `held`, `defaults`, `kind`, `model`, and the
///         grids and groups the dependents are read from.
///
/// Returns:
///     The kept deals, in order, and how many the base leaves open.
pub fn comparable(deals: &[PreparedDeal], params: &SearchParams<'_>) -> (Vec<PreparedDeal>, usize) {
    let cut = install(|| cut(deals, params));
    (cut.kept, cut.left_open.len())
}

/// The sample's bases, their completion, and the sample less the deals the strategies as they
/// stand leave open inside the tape — the one cut both the search and the variant columns
/// ([`comparable`]) make.
struct Cut<'a> {
    /// The strategies of the whole sample, the dropped deals' among them.
    whole: Bases<'a>,
    /// Where each number field starts on its grid ([`deps::dependents_of`]).
    start: HashMap<&'static str, usize>,
    /// What completes a point over `whole`.
    deps: deps::Dependents,
    /// The kept deals, in order.
    kept: Vec<PreparedDeal>,
    /// Each kept deal's index into `whole.owns`.
    of_kept: Vec<usize>,
    /// The dropped deals' ids.
    left_open: Vec<i64>,
}

/// Make the [`Cut`] of `deals` under `params` (`closing::closable_at_base`).
fn cut<'a>(deals: &'a [PreparedDeal], params: &SearchParams<'_>) -> Cut<'a> {
    let whole = Bases::of(deals);
    let (start, deps) = deps::dependents_of(params, &whole.owns);
    let (kept, of_kept, left_open) = closing::closable_at_base(deals, &whole, params, &deps);
    Cut {
        whole,
        start,
        deps,
        kept,
        of_kept,
        left_open,
    }
}

/// The trade floor a search holds when the caller sets none: half the deals it fits on.
pub fn default_min_n(train_n: usize) -> i64 {
    (train_n as i64 / 2).max(1)
}

/// The fewest held-back deals that make an out-of-sample check: a holdout under this is no
/// check at all, and the answer reads as fitted and judged on the same deals.
pub const MIN_HOLDOUT: i64 = 5;

/// The fewest deals a search of the Entry/Exit axis is fitted on: under it a point is fitted
/// on noise. 20 keeps, at the 70 % training share, at least 6 held-back deals — above
/// [`MIN_HOLDOUT`], so every answer the search gives can be checked out of sample.
pub const MIN_SEARCH_DEALS: usize = 20;

/// Whether a scored set of `n` deals is large enough to search ([`MIN_SEARCH_DEALS`]) — the
/// one rule both the search buttons and a hand-typed variant's column read.
///
/// Returns:
///     `Err(SearchMiss::TooFew)` under the floor.
pub fn sample_floor(n: usize) -> Result<(), SearchMiss> {
    if n < MIN_SEARCH_DEALS {
        Err(SearchMiss::TooFew { n })
    } else {
        Ok(())
    }
}

/// The score of one explicit set of values over `deals` — a variant column: its tally, the
/// spend of the deals it traded, and the deals it left open or never traded.
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
) -> VariantScore {
    let bases = Bases::of(deals);
    let per_base = bases.params(
        &HashMap::new(),
        defaults,
        &point_of(values),
        kind,
        model.sanitized(),
    );
    install(|| score(deals, &bases.of_deal, &per_base))
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
    /// The sell order's path as the exit model walked it from the fill — every level the line
    /// stood at, placement first; empty when the entry never filled.
    pub sell_line: Vec<LinePoint>,
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
    // The same walk `simulate` took its exit from — `ExitModel::exit` is this walk's `exit` —
    // run again for its levels: one deal, once per pane refresh.
    let sell_line = outcome.fill.map_or_else(Vec::new, |fill| {
        ExitModel::new(&exit)
            .walk(&deal.deal, &deal.ticks, fill)
            .points
    });
    VariantPicture {
        outcome,
        corridor,
        sell_line,
    }
}

/// Each deal's `(report_uid, (money, per cent))` under one variant, in the deals' order; `None`
/// where the variant makes no trade of the deal.
pub type DealResults = Vec<(i64, Option<(f64, f64)>)>;

/// Every deal's result under one variant — `(money, per cent)`: money in the deal's own money
/// (the sample's unit, except under the percent metric, where each row keeps its quote and only
/// the per cent is read) and per cent of the deal's spend, both net of the fact's cost
/// ([`super::Outcome::profit_money`]) —
/// what the deal table's plan column shows. `None` where the variant makes no trade of the deal
/// (no fill, or still open where the tape ends): the same rule that leaves the deal out of
/// [`variant_tally`].
///
/// Args:
///     deals, defaults, kind, values, model: As for [`variant_tally`].
///
/// Returns:
///     The score ([`VariantScore`]) and each deal's result ([`DealResults`]).
pub fn variant_tally_by_deal(
    deals: &[PreparedDeal],
    defaults: &HashMap<String, f64>,
    kind: &str,
    values: &[(String, String)],
    model: ModelSettings,
) -> (VariantScore, DealResults) {
    let bases = Bases::of(deals);
    let per_base = bases.params(
        &HashMap::new(),
        defaults,
        &point_of(values),
        kind,
        model.sanitized(),
    );
    install(|| {
        let scored: Vec<_> = deals
            .par_iter()
            .zip(bases.of_deal.par_iter())
            .map(|(d, &base)| {
                let (entry, exit) = &per_base[base];
                let outcome = simulate(&d.deal, &d.ticks, entry, exit, d.entry_line.as_deref());
                // A deal that spent nothing has no per cent. The percent projection admits only
                // `spentbtc > 0`, so the zero is never a percent-metric cell or tally value.
                let result = outcome.profit_money(&d.deal).map(|money| {
                    let on_spent = outcome.profit_on_spent(&d.deal).unwrap_or(0.0);
                    (money, on_spent)
                });
                let metric = outcome
                    .profit_metric(&d.deal)
                    .map(|value| (value, d.deal.spent));
                let at_tape_end = outcome.open_metric_at_tape_end(&d.deal, &d.ticks);
                (
                    d.deal.report_uid,
                    result,
                    metric,
                    outcome.left_open(),
                    at_tape_end,
                )
            })
            .collect();
        let mut score = VariantScore::default();
        for (_, _, metric, left_open, at_tape_end) in &scored {
            score.push(*metric, *left_open, *at_tape_end);
        }
        let money = scored
            .into_iter()
            .map(|(uid, result, _, _, _)| (uid, result))
            .collect();
        (score, money)
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
