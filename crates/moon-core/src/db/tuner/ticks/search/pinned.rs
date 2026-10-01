//! A searched number field answers from its grid. The descent leaves a field at the strategy's
//! own value by not holding it, and keeps a grid step only when it beats what the field holds —
//! so where the strategy's value is off the grid (a typed range that leaves it out: `0..30` over
//! a strategy at 60), that value stood beside the grid as a candidate the range excludes, and
//! won whenever no step beat it (LinKvo, 2026-10-01: "a ticked field with steps is searched by
//! its steps, not at what the strategy holds"). Such a field starts every restart held at its
//! start step instead. A value on the grid is still left to the base: there "leave it" is a step
//! of the range, the answer it always was.

use std::collections::HashMap;

use super::Point;
use crate::db::tuner::ticks::params::range::Grids;
use crate::db::tuner::ticks::params::{ParamKind, TickParam};

/// The searched number fields some strategy holds off their grid, each at its start step.
///
/// Args:
///     fields: The fields the search varies.
///     grids: The search's grids.
///     start: Where each number field starts on its grid (`deps::dependents_of`).
///     owns: Each strategy's own values, once per strategy (`Bases::owns`).
///     defaults: The schema's defaults, keyed lowercase — the value of a field a strategy
///         leaves out.
///
/// Returns:
///     The fields to hold at the start of every restart.
pub(super) fn off_grid(
    fields: &[&'static TickParam],
    grids: &Grids,
    start: &HashMap<&'static str, usize>,
    owns: &[&HashMap<String, String>],
    defaults: &HashMap<String, f64>,
) -> Point {
    let parse = |text: &String| text.trim().replace(',', ".").parse::<f64>().ok();
    fields
        .iter()
        .filter(|f| f.kind == ParamKind::Num)
        .filter_map(|f| {
            let at = *start.get(f.key)?;
            let grid = grids.values(f);
            let default = defaults.get(&f.key.to_ascii_lowercase()).copied();
            let on_grid = |v: f64| {
                grid.iter()
                    .any(|g| (g - v).abs() <= 1e-9 * v.abs().max(1.0))
            };
            owns.iter()
                .filter_map(|own| own.get(f.key).and_then(parse).or(default))
                .any(|v| !on_grid(v))
                .then(|| (f.key, grids.spell(f, at)))
        })
        .collect()
}

#[cfg(test)]
mod tests;
