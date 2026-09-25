//! The candidate values of the search's number fields. They are not a ladder kept by hand per
//! field any more: they come from what the live strategies hold of each field (the developer's
//! call, 2026-09-25 — a table of per-field rules is one more place a new field has to be
//! remembered in, and one that goes stale as the strategies move).
//!
//! A field's automatic range runs from the 5th to the 95th percentile of the values the live
//! strategies of the scope's kinds set ([`Population`]) — each strategy once however many cores
//! carry it, only where the field is in effect (`StopLoss` stays in the dump of a strategy with
//! `UseStopLoss = NO` and says nothing there: +50 on this machine, 2026-09-25), only where it
//! differs from the schema default (the core leaves a field at its default out of the dump, so
//! a tail cut over every strategy would cut a field 5 % of them set down to its default) —
//! widened to take the default and the selected strategies' own values in ([`field_span`]).
//! It is cut into about `steps` equal steps, the step rounded UP to 1, 2, 2.5 or 5 times a
//! power of ten and never finer than the finest digit the values carry ([`FieldSpan::auto`]);
//! the selected strategies' own values join the grid exactly, so restart 0 stands on the
//! strategy and "leave it" is an answer the search can give.
//!
//! The user may type any of from, to and step over the automatic ones ([`TickRange`]); a slot
//! left empty stays automatic ([`resolve`]). Every value is rounded to the field's precision and
//! the grid deduplicated, so a step finer than the field holds yields each value once: a step
//! of 0.3 on a whole-number field gives 1, 2, 3, not 0, 0, 1, 1, 1, 2 — a repeated value is a
//! point the search would replay the sample for again, and a restart's shift of a few steps
//! that stands still.

use std::collections::HashMap;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use super::{ParamKind, TICK_PARAMS, TickParam, parse_num};
use crate::db::tuner::LiveStrategy;
use crate::feed::strategy_deps::FieldDeps;

/// Steps per field when the search settings do not say.
pub const DEFAULT_STEPS: u32 = 20;
/// The fewest steps per field the setting takes.
pub const MIN_STEPS: u32 = 3;
/// The most steps per field the setting takes — and the most points a typed range may give one
/// field: the search scans every point of a field on every pass, so its time grows with the sum.
pub const MAX_STEPS: u32 = 200;

/// Fewer values than this for a field among the scope's kinds, and its range is taken over every
/// kind: the percentiles of a handful of values are the values (Drops — 5 strategies here).
const MIN_POPULATION: usize = 20;
/// The share cut off each end of the population.
const TAIL: f64 = 0.05;
/// The finest precision a field is gridded at, decimals.
const MAX_DECIMALS: u32 = 6;

/// The steps-per-field setting as the search takes it: the default when unset, within
/// [`MIN_STEPS`]..=[`MAX_STEPS`].
pub fn steps_of(setting: Option<u32>) -> u32 {
    setting.unwrap_or(DEFAULT_STEPS).clamp(MIN_STEPS, MAX_STEPS)
}

/// The user's own edges of one field's range. A slot left `None` stays automatic.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TickRange {
    pub from: Option<f64>,
    pub to: Option<f64>,
    pub step: Option<f64>,
}

impl TickRange {
    /// Whether every slot is automatic.
    pub fn is_auto(&self) -> bool {
        self.from.is_none() && self.to.is_none() && self.step.is_none()
    }

    /// The range with every slot that is not a finite number emptied — a hand edit of the saved
    /// layout reads as automatic there rather than as a range of NaN.
    fn finite(self) -> Self {
        let keep = |v: Option<f64>| v.filter(|v| v.is_finite());
        Self {
            from: keep(self.from),
            to: keep(self.to),
            step: keep(self.step),
        }
    }
}

/// What the live strategies hold of each number field, by strategy kind (`SignalType`, the
/// spelling a deal's kind has). A value counts only where the field is in effect under the
/// strategy's own switches and differs from the schema default.
#[derive(Clone, Debug, Default)]
pub struct Population {
    by_kind: HashMap<String, HashMap<&'static str, Vec<f64>>>,
}

impl Population {
    /// Args:
    ///     strategies: The live strategies, one per distinct content
    ///         ([`crate::db::tuner::live_strategies`]).
    ///     defaults: Schema defaults, lowercase key → number.
    ///     deps: The Strategies window's field rules — which switch a field hangs on.
    pub fn of(
        strategies: &[LiveStrategy],
        defaults: &HashMap<String, f64>,
        deps: &FieldDeps,
    ) -> Self {
        let mut by_kind: HashMap<String, HashMap<&'static str, Vec<f64>>> = HashMap::new();
        for strategy in strategies {
            let values =
                crate::db::tuner::ticks::search::strategy_values(&strategy.values, defaults);
            for field in number_fields() {
                let lower = field.key.to_ascii_lowercase();
                let Some(value) = strategy.values.get(&lower).and_then(|s| parse_num(s)) else {
                    continue;
                };
                if defaults.get(&lower).is_some_and(|d| same(*d, value))
                    || !deps.field_active(field.key, &values)
                {
                    continue;
                }
                by_kind
                    .entry(strategy.kind.clone())
                    .or_default()
                    .entry(field.key)
                    .or_default()
                    .push(value);
            }
        }
        Self { by_kind }
    }

    /// The values of `key` among the strategies of `kinds` — or of every kind, when those hold
    /// fewer than [`MIN_POPULATION`] of them or `kinds` is empty.
    pub fn values(&self, kinds: &[String], key: &str) -> Vec<f64> {
        let of = |map: &HashMap<&'static str, Vec<f64>>| map.get(key).cloned().unwrap_or_default();
        let scoped: Vec<f64> = kinds
            .iter()
            .filter_map(|kind| self.by_kind.get(kind))
            .flat_map(of)
            .collect();
        if !kinds.is_empty() && scoped.len() >= MIN_POPULATION {
            return scoped;
        }
        self.by_kind.values().flat_map(of).collect()
    }
}

/// The number knobs of the axis, each once.
fn number_fields() -> impl Iterator<Item = &'static TickParam> {
    TICK_PARAMS
        .iter()
        .enumerate()
        .filter(|(i, f)| {
            f.kind == ParamKind::Num && !TICK_PARAMS[..*i].iter().any(|g| g.key == f.key)
        })
        .map(|(_, f)| f)
}

/// Where one field's automatic range stands before it is cut into steps: the population's tails
/// widened to the default and the selected strategies' values, and the precision the values
/// carry.
#[derive(Clone, Debug, PartialEq)]
pub struct FieldSpan {
    pub lo: f64,
    pub hi: f64,
    /// The finest decimal digit among the values, the default and the selected ones.
    pub decimals: u32,
    /// The selected strategies' own values, sorted and distinct — each joins the grid exactly.
    pub selected: Vec<f64>,
}

/// One field's span, or `None` when nothing is known of it — no strategy sets it, it has no
/// default and no selected strategy holds it.
///
/// Args:
///     population: The field's values among the live strategies ([`Population::values`]).
///     default: The schema default.
///     selected: The selected strategies' values, a strategy that leaves it out at the default.
pub fn field_span(population: &[f64], default: Option<f64>, selected: &[f64]) -> Option<FieldSpan> {
    let mut sorted: Vec<f64> = population
        .iter()
        .copied()
        .filter(|v| v.is_finite())
        .collect();
    sorted.sort_by(f64::total_cmp);
    let mut own: Vec<f64> = selected.iter().copied().filter(|v| v.is_finite()).collect();
    own.sort_by(f64::total_cmp);
    own.dedup_by(|a, b| same(*a, *b));
    let default = default.filter(|v| v.is_finite());
    let tails = (!sorted.is_empty())
        .then(|| [percentile(&sorted, TAIL), percentile(&sorted, 1.0 - TAIL)])
        .into_iter()
        .flatten();
    let anchors: Vec<f64> = tails.chain(default).chain(own.iter().copied()).collect();
    let lo = anchors.iter().copied().reduce(f64::min)?;
    let hi = anchors.iter().copied().reduce(f64::max)?;
    let decimals = sorted
        .iter()
        .chain(&own)
        .chain(&default)
        .map(|v| decimals_of(*v))
        .max()
        .unwrap_or(0);
    Some(FieldSpan {
        lo,
        hi,
        decimals,
        selected: own,
    })
}

/// The value at quantile `q` of an ascending, non-empty slice, by the nearest rank.
fn percentile(sorted: &[f64], q: f64) -> f64 {
    let at = (q * (sorted.len() - 1) as f64).round() as usize;
    sorted[at.min(sorted.len() - 1)]
}

/// The edges and the step a field's grid is shown with and cut by; `step` 0 is a one-point grid.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Shown {
    pub from: f64,
    pub to: f64,
    pub step: f64,
}

impl FieldSpan {
    /// The automatic range: the span cut into about `steps` steps of a round size, the edges
    /// moved out to a multiple of it — but never across zero. A span all below zero ends where
    /// it ends rather than at 0, and one all above starts where it starts: 0 is a meaning of its
    /// own for the fields that have a sign — no stop armed (`exit::stops::stop_pct`), a corridor
    /// of nothing — which no strategy of the span holds. The grid between stays on the multiples
    /// of the step either way ([`cut`]), the edge that is not one tried beside them.
    pub fn auto(&self, steps: u32) -> Shown {
        let steps = steps.clamp(MIN_STEPS, MAX_STEPS);
        if self.hi - self.lo <= f64::EPSILON * self.hi.abs().max(1.0) {
            return Shown {
                from: self.lo,
                to: self.hi,
                step: 0.0,
            };
        }
        let step = round_step((self.hi - self.lo) / f64::from(steps - 1), self.decimals);
        let places = self.decimals.max(decimals_of(step));
        // A floor off by the division's noise would take a step too many ([`near`]).
        let (lo_q, hi_q) = (self.lo / step, self.hi / step);
        let mut from = snap((lo_q + near(lo_q)).floor() * step, places);
        let mut to = snap((hi_q - near(hi_q)).ceil() * step, places);
        if self.lo > 0.0 && from <= 0.0 {
            from = self.lo;
        }
        if self.hi < 0.0 && to >= 0.0 {
            to = self.hi;
        }
        Shown { from, to, step }
    }
}

/// The smallest round step — 1, 2, 2.5 or 5 times a power of ten — at least `raw` and a whole
/// multiple of the field's precision, so every point of the grid is a value the field holds.
fn round_step(raw: f64, decimals: u32) -> f64 {
    let quantum = quantum(decimals);
    let raw = raw.max(quantum);
    let mut power = raw.log10().floor() as i32 - 1;
    // Past ten decades up there is always 10^power itself, a multiple of any quantum ≤ 1.
    for _ in 0..12 {
        for mantissa in [1.0, 2.0, 2.5, 5.0] {
            let candidate = snap(mantissa * 10f64.powi(power), MAX_DECIMALS + 4);
            let multiple = candidate / quantum;
            if candidate >= raw * (1.0 - 1e-9) && (multiple - multiple.round()).abs() < 1e-6 {
                return candidate;
            }
        }
        power += 1;
    }
    raw
}

/// Why a typed range is not used; the search then takes the automatic one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RangeError {
    /// "from" above "to".
    Inverted,
    /// A step of zero or below over a range wider than a point.
    BadStep,
    /// More points than [`MAX_STEPS`].
    TooMany,
    /// A step typed without "from" and "to" on a field nothing is known of: there is no range to
    /// cut it over.
    NoEdges,
}

/// One field's grid as the search takes it.
#[derive(Clone, Debug, PartialEq)]
pub struct Resolved {
    /// The edges and the step the grid was cut by, `None` when nothing is known of the field.
    pub shown: Option<Shown>,
    /// The candidate values, ascending and distinct.
    pub points: Arc<[f64]>,
    /// Why the typed range was set aside for the automatic one.
    pub error: Option<RangeError>,
}

/// One field's grid: the typed slots over the automatic ones.
///
/// A typed "from" or "to" without a typed step is cut by a round step for `steps` over the new
/// span. The values are rounded to the field's precision — whole numbers for an integer field
/// of the schema, else the finest digit among the values and the typed slots — and the grid
/// deduplicated. The selected strategies' own values inside the range join it.
///
/// Args:
///     span: The field's automatic span, `None` when nothing is known of it.
///     typed: The user's slots.
///     integer: Whether the schema types the field as an integer.
///     steps: Steps per field ([`steps_of`]).
pub fn resolve(span: Option<&FieldSpan>, typed: &TickRange, integer: bool, steps: u32) -> Resolved {
    let steps = steps.clamp(MIN_STEPS, MAX_STEPS);
    let typed = typed.finite();
    let auto = span.map(|s| s.auto(steps));
    let selected: &[f64] = span.map_or(&[], |s| s.selected.as_slice());
    let data_places = if integer {
        0
    } else {
        span.map_or(0, |s| s.decimals)
    };
    let automatic = |error: Option<RangeError>| Resolved {
        shown: auto,
        points: auto
            .map_or_else(
                || selected.to_vec(),
                |a| cut(a, data_places, selected, None, true),
            )
            .into(),
        error,
    };
    if typed.is_auto() {
        return automatic(None);
    }
    let places = if integer {
        0
    } else {
        [typed.from, typed.to, typed.step]
            .into_iter()
            .flatten()
            .map(decimals_of)
            .fold(data_places, u32::max)
    };
    let (from, to) = match (
        typed.from.or(auto.map(|a| a.from)),
        typed.to.or(auto.map(|a| a.to)),
    ) {
        (Some(from), Some(to)) => (from, to),
        (Some(one), None) | (None, Some(one)) => (one, one),
        // Only a typed step is left, and nothing to cut it over.
        (None, None) => return automatic(Some(RangeError::NoEdges)),
    };
    if from > to {
        return automatic(Some(RangeError::Inverted));
    }
    let step = if to - from <= f64::EPSILON * to.abs().max(1.0) {
        0.0
    } else {
        match (typed.step, auto) {
            (Some(step), _) => step,
            (None, Some(a)) if typed.from.is_none() && typed.to.is_none() => a.step,
            _ => round_step((to - from) / f64::from(steps - 1), places),
        }
    };
    if step <= 0.0 && to > from {
        return automatic(Some(RangeError::BadStep));
    }
    let shown = Shown { from, to, step };
    // A typed "from" anchors the steps; without one the automatic "from" stands and the grid
    // keeps to the multiples of the step, as the automatic one does.
    let on_multiples = typed.from.is_none();
    // Counted before anything is cut: a step typed a million times too fine is refused, not
    // allocated.
    if step > 0.0 && (to - from) / step > f64::from(MAX_STEPS) + 2.0 {
        return automatic(Some(RangeError::TooMany));
    }
    if cut(shown, places, &[], None, on_multiples).len() > MAX_STEPS as usize {
        return automatic(Some(RangeError::TooMany));
    }
    Resolved {
        shown: Some(shown),
        points: cut(shown, places, selected, Some((from, to)), on_multiples).into(),
        error: None,
    }
}

/// A tolerance for a quotient of the range by its step, scaled to it: at 1000 over a step of
/// 1e-5 the division itself is off by more than any fixed epsilon.
fn near(q: f64) -> f64 {
    1e-9 * q.abs().max(1.0)
}

/// The points of a range, rounded to `places`, the selected values inside `within` added (all of
/// them when `None`) exactly as the strategies hold them, ascending and distinct.
///
/// The steps run from `from` — or, `on_multiples`, over the multiples of the step between the
/// edges, the edges themselves tried beside them where they are not ones (an automatic edge
/// kept off zero, [`FieldSpan::auto`]). `to` is tried where the steps do not land on it, so both
/// edges always are. The steps are rounded before they are compared; a selected value, which is
/// how a strategy spells it, equals the step that spells the same.
fn cut(
    shown: Shown,
    places: u32,
    selected: &[f64],
    within: Option<(f64, f64)>,
    on_multiples: bool,
) -> Vec<f64> {
    let mut points: Vec<f64> = Vec::new();
    if shown.step > 0.0 {
        if on_multiples {
            let (lo_q, hi_q) = (shown.from / shown.step, shown.to / shown.step);
            let first = (lo_q - near(lo_q)).ceil() as i64;
            let last = (hi_q + near(hi_q)).floor() as i64;
            points.extend((first..=last).map(|k| snap(k as f64 * shown.step, places)));
        } else {
            let span = (shown.to - shown.from) / shown.step;
            let count = (span + near(span)).floor() as usize + 1;
            points.extend((0..count).map(|i| snap(shown.from + i as f64 * shown.step, places)));
        }
    }
    points.push(snap(shown.from, places));
    points.push(snap(shown.to, places));
    let inside = |v: f64| within.is_none_or(|(lo, hi)| v >= lo - 1e-12 && v <= hi + 1e-12);
    points.extend(selected.iter().copied().filter(|v| inside(*v)));
    points.sort_by(f64::total_cmp);
    points.dedup();
    points
}

/// Each number knob's candidate values for one search. A number field without an entry has no
/// value to try and is not varied.
#[derive(Clone, Debug, Default)]
pub struct Grids(HashMap<&'static str, Arc<[f64]>>);

impl Grids {
    /// Grids out of `(key, points)` pairs; the points are taken as they come.
    pub fn of(entries: impl IntoIterator<Item = (&'static str, Arc<[f64]>)>) -> Self {
        Self(entries.into_iter().collect())
    }

    /// Set one field's points.
    pub fn insert(&mut self, key: &'static str, points: Arc<[f64]>) {
        self.0.insert(key, points);
    }

    /// A number field's points; empty for any other field, or one without a grid.
    pub fn values(&self, field: &TickParam) -> &[f64] {
        match field.kind {
            ParamKind::Num => self.0.get(field.key).map_or(&[], |points| points),
            ParamKind::Bool | ParamKind::Enum(_) => &[],
        }
    }

    /// How many values a field's grid offers.
    pub fn arity(&self, field: &TickParam) -> usize {
        match field.kind {
            ParamKind::Num => self.values(field).len(),
            ParamKind::Bool => 2,
            ParamKind::Enum(options) => options.len(),
        }
    }

    /// The spelling of one grid value in the strategy's format. `index` is below
    /// [`Self::arity`].
    pub fn spell(&self, field: &TickParam, index: usize) -> String {
        match field.kind {
            ParamKind::Num => spell_number(self.values(field)[index]),
            ParamKind::Bool => (if index == 0 { "NO" } else { "YES" }).to_string(),
            ParamKind::Enum(options) => options[index].to_string(),
        }
    }
}

/// A number in the strategy's spelling: no fraction for a whole one, else the shortest form.
pub fn spell_number(v: f64) -> String {
    if v.fract() == 0.0 {
        format!("{v:.0}")
    } else {
        format!("{v}")
    }
}

/// The decimal digits `v` carries in its shortest form, at most [`MAX_DECIMALS`].
fn decimals_of(v: f64) -> u32 {
    let text = format!("{}", v.abs());
    text.split_once('.')
        .map_or(0, |(_, fraction)| fraction.len() as u32)
        .min(MAX_DECIMALS)
}

/// `10^-decimals`.
fn quantum(decimals: u32) -> f64 {
    10f64.powi(-(decimals.min(MAX_DECIMALS) as i32))
}

/// `v` rounded to `places` decimals — the nearest double to that decimal, which prints short.
fn snap(v: f64, places: u32) -> f64 {
    let scale = 10f64.powi(places as i32);
    let out = (v * scale).round() / scale;
    // No negative zero in a grid: it spells "-0".
    if out == 0.0 { 0.0 } else { out }
}

/// Whether two values are the same point of a grid.
fn same(a: f64, b: f64) -> bool {
    (a - b).abs() <= 1e-9 * a.abs().max(b.abs()).max(1.0)
}

#[cfg(test)]
mod tests;
