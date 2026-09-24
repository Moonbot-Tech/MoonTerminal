//! The Strategies window's field dependencies inside the search (`assets/param_deps.toml`): a
//! field is in effect only while its rule holds — `TakeProfit` under `UseTrailing=YES` and
//! `UseTakeProfit=YES`, the ladder's fields under `UseStopLoss=YES` and their own switch.
//!
//! Two rules keep a point honest about them:
//!
//! - a number field the VARIANT puts in effect — the point or its held edits — on a strategy that
//!   holds no value for it is set on the point: the step the search starts it from, the median of
//!   the values the strategies hold. A switch is never turned on with nothing behind it (a
//!   `UseTakeProfit = YES` whose per cent is on no record: the developer, 2026-09-24). Like every
//!   value of a variant it is one value for every strategy of the point — Save writes it to each —
//!   so a strategy that held its own is scored at it too, and the point is scored as it will be
//!   written;
//! - a field in effect on no strategy is left out of the answer: it moves nothing.
//!
//! A field already in effect on the strategy as it stands is never completed, however its value
//! is missing from the dump: the core leaves out a field at its default, so absent there means
//! "at the core's default", not "no value". Completing it with the other strategies' median
//! rewrote the strategy itself — `MShotAddBTCDelta` 0.03 laid on a strategy at 0 moved its
//! corridor nearer the price than its own trades ran, and the corridor rule refused the strategy
//! as it stands (24.09, 53 of 136 deals).
//!
//! The first rule reaches the fields the search does not vary too ([`offered`]): a search of
//! `UseTakeProfit` alone locks every other field, `TakeProfit` among them, and a switch it turns on
//! must still bring its per cent.

use std::collections::HashMap;

use super::{Point, SearchParams, spell};
use crate::db::tuner::ticks::TICK_PARAMS;
use crate::db::tuner::ticks::params::{ParamGroup, ParamKind, TickParam};
use crate::feed::strategy_deps::{FieldDeps, Values};

/// The value a condition field reads when a strategy's dump leaves it out and no live schema
/// says otherwise — the model's own fallbacks (`params::exit_params`, `ExitParams::default`), so
/// "in effect" is the same question the model answers when it reads the field. Every condition a
/// number knob's rule reads is here — a unit test holds this against the bundled rules: one left
/// out reads as absent, which does not block, and the strategy as it stands would seem to have
/// the field in effect behind a switch that is off.
const CONDITION_FALLBACKS: &[(&str, &str)] = &[
    ("hodlmode", "NO"),
    ("autosell", "YES"),
    ("usestoploss", "YES"),
    ("usesecondstop", "NO"),
    ("usestoploss3", "NO"),
    ("usetrailing", "NO"),
    ("usetakeprofit", "NO"),
    ("pricedowntimer", "0"),
    ("sellleveldelay", "0"),
    ("sellleveltime", "0"),
    ("mshotsellatlastprice", "NO"),
];

/// The fields of the searched groups this kind and entry method read, the locked ones among
/// them: what a point can switch on, and so what [`Dependents`] completes.
pub(super) fn offered<'a>(p: &SearchParams<'a>) -> Vec<&'static TickParam> {
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
        .collect()
}

/// Where each number field of the search starts on its grid, and the completion over those
/// steps.
///
/// A field starts at the median of what the strategies hold (the held value over all of them;
/// the schema default for one that leaves it out), snapped to the nearest grid step: what a pair
/// move and a perturbed start step from while the point leaves the field alone. A move sets one
/// value for every strategy, so it steps from the middle of theirs rather than from whichever
/// came first. The locked fields get one too: it is where a switch the variant turns on
/// completes them.
///
/// Args:
///     params: The search's parameters.
///     owns: Each strategy's own values, once per strategy (`Bases::owns`).
pub(super) fn dependents_of(
    params: &SearchParams<'_>,
    owns: &[&HashMap<String, String>],
) -> (HashMap<&'static str, usize>, Dependents) {
    let parse = |text: &String| text.trim().replace(',', ".").parse::<f64>().ok();
    let offered = offered(params);
    let start: HashMap<&'static str, usize> = offered
        .iter()
        .filter_map(|f| {
            let ParamKind::Num { grid } = &f.kind else {
                return None;
            };
            // The schema's defaults are keyed lowercase (`strategy_field_defaults`).
            let default = params.defaults.get(&f.key.to_ascii_lowercase()).copied();
            let mut values: Vec<f64> = match params.held.get(f.key).and_then(parse) {
                Some(held) => vec![held],
                None => owns
                    .iter()
                    .filter_map(|own| own.get(f.key).and_then(parse).or(default))
                    .collect(),
            };
            values.sort_by(f64::total_cmp);
            let value = values.get(values.len() / 2).copied().or(default)?;
            Some((f.key, super::nearest_step(grid, value)))
        })
        .collect();
    let dependents = Dependents::new(&offered, &start);
    (start, dependents)
}

/// The number fields a point may complete and the step each is completed at.
pub(super) struct Dependents {
    rules: FieldDeps,
    numbers: Vec<(&'static TickParam, String)>,
}

impl Dependents {
    /// Args:
    ///     fields: The fields the search offers ([`offered`]), varied or locked.
    ///     start: Where each number field starts on its grid; one without a start is never
    ///         completed.
    pub(super) fn new(fields: &[&'static TickParam], start: &HashMap<&'static str, usize>) -> Self {
        let numbers = fields
            .iter()
            .filter(|f| matches!(f.kind, ParamKind::Num { .. }))
            .filter_map(|f| Some((*f, spell(&f.kind, *start.get(f.key)?))))
            .collect();
        Self {
            rules: FieldDeps::bundled(),
            numbers,
        }
    }

    /// `point` with every number field the variant puts in effect on a strategy that holds no
    /// value for it set at its start step — for every strategy, as a variant's values are. A
    /// field the strategy as it stands already has in effect is at the core's default there, and
    /// is left alone.
    pub(super) fn complete(
        &self,
        point: &Point,
        owns: &[&HashMap<String, String>],
        held: &HashMap<String, String>,
        defaults: &HashMap<String, f64>,
    ) -> Point {
        let mut out = point.clone();
        // Each strategy's values as it stands, before the variant — read once per strategy, and
        // only when a field comes into question: they do not depend on the point.
        let mut stored: Vec<Option<Values>> = vec![None; owns.len()];
        // A completed number can be another's condition (`PriceDownTimer<>0`) on any strategy:
        // every strategy is read again until nothing more comes into effect.
        loop {
            let before = out.len();
            for (index, own) in owns.iter().enumerate() {
                let values = effective(own, held, &out, defaults);
                for (field, at_start) in &self.numbers {
                    let missing = !out.contains_key(field.key)
                        && !held.contains_key(field.key)
                        && !own.contains_key(field.key);
                    if !missing || !self.rules.field_active(field.key, &values) {
                        continue;
                    }
                    let base = stored[index].get_or_insert_with(|| {
                        effective(own, &HashMap::new(), &Point::new(), defaults)
                    });
                    if !self.rules.field_active(field.key, base) {
                        out.insert(field.key, at_start.clone());
                    }
                }
            }
            if out.len() == before {
                break;
            }
        }
        out
    }

    /// `point` without the fields in effect on none of the strategies.
    pub(super) fn prune(
        &self,
        point: &Point,
        owns: &[&HashMap<String, String>],
        held: &HashMap<String, String>,
        defaults: &HashMap<String, f64>,
    ) -> Point {
        let values: Vec<Values> = owns
            .iter()
            .map(|own| effective(own, held, point, defaults))
            .collect();
        point
            .iter()
            .filter(|(key, _)| values.iter().any(|v| self.rules.field_active(key, v)))
            .map(|(key, value)| (*key, value.clone()))
            .collect()
    }
}

/// One strategy's values as the rules read them: its own, `held` over them, the point over that,
/// lowercase; a condition field none of them spells reads the live schema's default, else the
/// model's fallback.
fn effective(
    own: &HashMap<String, String>,
    held: &HashMap<String, String>,
    point: &Point,
    defaults: &HashMap<String, f64>,
) -> Values {
    let mut values: Values = own
        .iter()
        .chain(held.iter())
        .map(|(k, v)| (k.to_ascii_lowercase(), v.clone()))
        .collect();
    for (key, value) in point {
        values.insert(key.to_ascii_lowercase(), value.clone());
    }
    for (key, fallback) in CONDITION_FALLBACKS {
        values.entry((*key).to_string()).or_insert_with(|| {
            defaults
                .get(*key)
                .map_or_else(|| (*fallback).to_string(), f64::to_string)
        });
    }
    values
}

#[cfg(test)]
mod tests;
