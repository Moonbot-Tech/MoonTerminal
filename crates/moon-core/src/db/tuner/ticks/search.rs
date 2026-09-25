//! The search of the "Entry/Exit" axis: coordinate descent with restarts over the discrete
//! grids the caller hands it ([`SearchParams::grids`], `params::range`), scoring a point by REPLAYING every covered deal under it — the shape
//! of `threshold_search`, with the SQL mask replaced by [`simulate`]. Restart 0 starts from the
//! strategy itself; the others from the strategy moved a few steps on a few fields, each walking
//! the fields in an order of its own. A pass that moves no single field then tries PAIRS of the
//! Entry group's number fields, one a step down and another a step up, so a corridor's distance
//! can move between the base fields and the modifiers ([`descend`]).
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
//! matrix prints. A deal the variant never fills is not a trade and drops out of `n`; the caller
//! prints "by N of M" beside the column so a variant that wins by trading less is visible as
//! such. A point that buys a deal and does not close it inside its tape, or leaves a strategy
//! with nothing standing to close a trade, is refused outright ([`closing`], the developer,
//! 2026-09-24): dropping the deal would reward the loss it carries past the tape. A switch the
//! point turns on brings the values it needs ([`deps`]).
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

use super::mshot::{CorridorStep, EntryMethod, MshotEntry, MshotParams};
use super::params::range::Grids;
use super::params::{
    ParamGroup, ParamKind, StrategyValues, TICK_PARAMS, TickParam, exit_params, mshot_params,
};
use super::settings::ModelSettings;
use super::{Deal, Deltas, EntryParams, ExitParams, Outcome, entry_model_for, simulate};
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
    /// on — the axis passes the variant's edits for every search, so a search runs on from what
    /// the earlier ones found (the searched fields among them, which the point then overrides).
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
    /// Whether a point whose entry corridor comes nearer the price than a trade's own, at any
    /// moment of that trade's entry order, is out of the search
    /// ([`MshotParams::never_closer_than`]). Read only while the Entry group is searched: a
    /// search of the exit alone moves no corridor.
    pub keep_corridor: bool,
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
    /// No point the search visited closed every deal it bought inside the tape with something
    /// standing to close each trade — a stop, or a trailing without a take profit ([`closing`]).
    Unclosed,
}

/// How a search went — what shows whether its restarts and passes changed anything.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SearchStats {
    /// Restarts that ran to the end (a stop leaves the rest out).
    pub restarts: usize,
    /// The restart the answer came from: 0 starts from the strategy itself.
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
    /// How many of the deals held back the answer bought and left open inside the tape — none
    /// may be among the deals it was fitted on; the holdout is only scored, so it says them.
    pub holdout_open: usize,
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
            for index in 0..grids.arity(field) {
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
            for &down in pairs {
                for &up in pairs {
                    // Each pair is a replay of the sample: a stop is noticed between two of
                    // them, not after a whole row.
                    if handle.is_cancelled() {
                        handle.note_abandoned();
                        return None;
                    }
                    if down.key == up.key {
                        continue;
                    }
                    let (Some(d), Some(u)) = (
                        grid_index(grids, down, &point, start),
                        grid_index(grids, up, &point, start),
                    ) else {
                        continue;
                    };
                    if d == 0 || u + 1 >= grids.arity(up) {
                        continue;
                    }
                    let (was_down, was_up) =
                        (point.get(down.key).cloned(), point.get(up.key).cloned());
                    point.insert(down.key, grids.spell(down, d - 1));
                    point.insert(up.key, grids.spell(up, u + 1));
                    let trial = evaluate(&point);
                    if better_score(&trial, &score, min_n) {
                        score = trial;
                        improved = true;
                    } else {
                        restore(&mut point, down.key, was_down);
                        restore(&mut point, up.key, was_up);
                    }
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
                if next_random(state) % 2 == 0 {
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

    /// Whether `value` of `key` is something at least one base, under `held`, does not hold.
    fn moves(&self, held: &HashMap<String, String>, key: &str, value: &str) -> bool {
        self.owns
            .iter()
            .any(|own| held.get(key).or_else(|| own.get(key)).map(String::as_str) != Some(value))
    }
}

/// Every deal's result under one point, in order — `(money, spent)`, `None` where the point
/// makes no trade of the deal — and whether it bought the deal and left it open.
///
/// Args:
///     deals: The deals.
///     of_deal: Each deal's index into `params` ([`Bases::of_deal`]), as long as `deals`.
///     params: The point's parameters per base ([`Bases::params`]).
fn results(
    deals: &[PreparedDeal],
    of_deal: &[usize],
    params: &[(EntryParams, ExitParams)],
) -> Vec<(Option<(f64, f64)>, bool)> {
    deals
        .par_iter()
        .zip(of_deal.par_iter())
        .map(|(d, &base)| {
            let (entry, exit) = &params[base];
            let outcome = simulate(&d.deal, &d.ticks, entry, exit, d.entry_line.as_deref());
            let result = outcome
                .profit_money(&d.deal)
                .map(|money| (money, d.deal.spent));
            (result, outcome.left_open())
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
    for (money, size) in results.into_iter().filter_map(|(result, _)| result) {
        tally.push(money);
        spent += size;
    }
    (tally, spent)
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
///     the caller asked for — or none that keeps the corridor.
pub fn suggest(
    deals: &[PreparedDeal],
    params: &SearchParams<'_>,
    handle: &SearchHandle,
) -> Result<SearchResult, SearchMiss> {
    let fields = varied(params);
    // The strategies of the WHOLE sample stay the bases throughout, a strategy whose every deal
    // leaves the sample below among them: where each number field starts, what completes a point
    // (`deps`) and the parameters built per base are then the same for the filter and for every
    // point scored after it, restart 0's included.
    let whole = Bases::of(deals);
    let (start, deps) = deps::dependents_of(params, &whole.owns);
    // A deal the strategies as they stand leave open is out of the sample (`closing`).
    let (kept, of_kept, left_open) = closing::closable_at_base(deals, &whole, params, &deps);
    let deals = kept.as_slice();
    if deals.is_empty() || fields.is_empty() {
        return Err(SearchMiss::Nothing);
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
    let bases = Bases {
        owns: whole.owns,
        of_deal: of_kept,
    };
    let train_of = &bases.of_deal[..train_n];
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
    let evaluate = |point: &Point| -> Option<Tally> {
        let per_base = per_base_at(point);
        // A point that inverts the corridor's two fields is never proposed, whatever the
        // switch: the searched fields are gridded one by one, and nothing else ties them. Only
        // a search of the Entry group can produce one — an Exit search leaves the strategy's
        // own fields alone, whatever they hold.
        if (params.vary_entry && inverts(&start_ordered, &per_base))
            || guard
                .as_ref()
                .is_some_and(|g| !g.holds(&bases.of_deal, &per_base))
        {
            cornered.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            return None;
        }
        // Counted here, past the refusals: what the stats call a scored point is a replay.
        evaluations.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        // A trade must be closed while it lasts: something that can close it stands on every
        // strategy, and none of the deals it bought is left open (`closing`).
        let closed = closing::protected(&per_base)
            .then(|| closing::closed_tally(train, train_of, &per_base))
            .flatten();
        if closed.is_none() {
            unclosed.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
        closed
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
    let runs: Vec<Run> = install(|| {
        (0..restarts)
            .into_par_iter()
            .map(|restart| {
                if handle.is_cancelled() {
                    handle.note_abandoned();
                    return None;
                }
                // Restart 0 starts from the base itself, in grid order. The others start from
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
                let walked = descend(
                    point,
                    params.grids,
                    &order,
                    &pairs,
                    &coupling,
                    &start,
                    &evaluate,
                    min_n,
                    max_passes,
                    handle,
                )?;
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
    // leaves a field at its base by not holding it, a random restart by holding the base's
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
    let stats = SearchStats {
        restarts: restarts_done,
        best_restart: best.restart,
        passes: best.passes,
        converged: best.converged,
        distinct,
        evaluations: evaluations.load(std::sync::atomic::Ordering::Relaxed),
        refused,
        left_open: left_open.len(),
    };
    let (point, score) = (best.point, best.score);
    // The strategies as they stand against the answer, on the slice both were fitted on: a best
    // below its own base is a search that could not reach the base, and the log says so.
    let base_score = evaluate(&Point::new());
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
        "[x] ticks search: base (n, profit) {:?} against best {:?} over {} training deal(s), {} left out; base refused by (inverts, deals nearer than their own corridor, guarded) {:?}; points refused by the corridor {}, by a deal left open {}",
        brief(&base_score),
        brief(&score),
        train_n,
        left_open.len(),
        base_why,
        cornered.load(std::sync::atomic::Ordering::Relaxed),
        unclosed.load(std::sync::atomic::Ordering::Relaxed)
    );
    // The answer as it was scored — every field it switched on at a value — less what is in
    // effect on no strategy.
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
        let load =
            |c: &std::sync::atomic::AtomicUsize| c.load(std::sync::atomic::Ordering::Relaxed);
        return Err(if load(&unclosed) > load(&cornered) {
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
    let (holdout, holdout_open) = if train_n < deals.len() {
        let per_base = bases.params(params.held, params.defaults, &point, params.kind, model);
        let (tally, open) =
            closing::tally_counting_open(&deals[train_n..], &bases.of_deal[train_n..], &per_base);
        (Some(tally), open)
    } else {
        (None, 0)
    };
    Ok(SearchResult {
        values,
        train: train_tally,
        holdout,
        holdout_open,
        seed,
        stats,
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

mod closing;
pub use self::closing::unguarded_strategies;
mod coupled;
mod deps;
pub(in crate::db::tuner::ticks) use self::deps::strategy_values;

#[cfg(test)]
pub(in crate::db::tuner::ticks) mod test_grids;
#[cfg(test)]
mod tests;
