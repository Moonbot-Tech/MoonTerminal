//! The Strategies window's field dependencies inside the search (`assets/param_deps.toml`): a
//! field is in effect only while its rule holds — `TakeProfit` under `UseTrailing=YES` and
//! `UseTakeProfit=YES`, the ladder's fields under `UseStopLoss=YES` and their own switch.
//!
//! Two rules keep a point honest about them:
//!
//! - a number field the point puts in effect on a strategy that holds no value for it is set on
//!   the point — the step the search starts it from, the median of the values the strategies
//!   hold — so a switch is never turned on with nothing behind it (a `UseTakeProfit = YES` whose
//!   per cent is on no record: the developer, 2026-09-24). Like every value of a variant it is
//!   one value for every strategy of the point — Save writes it to each — so a strategy that held
//!   its own is scored at it too, and the point is scored as it will be written;
//! - a field in effect on no strategy is left out of the answer: it moves nothing.

use std::collections::HashMap;

use super::{Point, spell};
use crate::db::tuner::ticks::params::{ParamKind, TickParam};
use crate::feed::strategy_deps::{FieldDeps, Values};

/// The value a condition field reads when a strategy's dump leaves it out and no live schema
/// says otherwise — the model's own fallbacks (`params::exit_params`, `ExitParams::default`), so
/// "in effect" is the same question the model answers when it reads the field.
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
];

/// The varied number fields and the step each is completed at.
pub(super) struct Dependents {
    rules: FieldDeps,
    numbers: Vec<(&'static TickParam, String)>,
}

impl Dependents {
    /// Args:
    ///     fields: The fields the search varies.
    ///     start: Where each number field starts on its grid.
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

    /// `point` with every varied number field it puts in effect on a strategy that holds no value
    /// for it set at its start step — for every strategy, as a variant's values are.
    pub(super) fn complete(
        &self,
        point: &Point,
        owns: &[&HashMap<String, String>],
        held: &HashMap<String, String>,
        defaults: &HashMap<String, f64>,
    ) -> Point {
        let mut out = point.clone();
        // A completed number can be another's condition (`PriceDownTimer<>0`) on any strategy:
        // every strategy is read again until nothing more comes into effect.
        loop {
            let before = out.len();
            for own in owns {
                let values = effective(own, held, &out, defaults);
                for (field, at_start) in &self.numbers {
                    let missing = !out.contains_key(field.key)
                        && !held.contains_key(field.key)
                        && !own.contains_key(field.key);
                    if missing && self.rules.field_active(field.key, &values) {
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
