//! The Delta Modifiers section inside the search: a product, not a set of independent fields.
//!
//! The section moves a level by `coefficient · Min(MaxModifier, |Σ Pn·Dn|)` — the coefficient is
//! `SellModifier` on the sell and `StopLossModifier` on the stop, the `Pn` are the `Add*` terms
//! (`exit::delta_mods`). Coordinate descent turns one field at a time, and on a product that has
//! two consequences:
//!
//! - a field whose partner is zero moves nothing — every `Add*` while both coefficients are zero,
//!   a coefficient while every term is zero, the cap while either is — and scanning its grid
//!   is a replay of the sample per step that cannot change the score. Such a field is skipped
//!   ([`Coupling::inert`]);
//! - from the corner where the coefficient and the terms are all zero — the state of most
//!   strategies — no single move leaves it: each field is inert while the other is zero. The
//!   pass that moves no single field then walks each (coefficient, term) pair from that corner
//!   together, along a diagonal of their grids ([`Coupling::diagonals`]), from the smallest
//!   product to the largest, so the section can be switched on at all.
//!
//! A field is inert only when it is inert on EVERY strategy of the sample: a variant's value is
//! one value for all of them, and one strategy that applies the sum is enough for the field to
//! move a column.

use super::{Point, better_score, restore, spell};
use crate::db::metrics::Tally;
use crate::db::tuner::threshold_search::SearchHandle;
use crate::db::tuner::ticks::params::{ParamKind, ParamSection, TickParam};
use crate::db::tuner::ticks::{EntryParams, ExitParams};

/// The model parameters of every strategy of the sample at a point, completed as a scored
/// point is.
pub(super) type PerBase<'a> = dyn Fn(&Point) -> Vec<(EntryParams, ExitParams)> + Sync + 'a;

/// What a field of the Delta Modifiers section is to the product.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Role {
    /// `SellModifier`: spends the sum on the sell.
    SellCoefficient,
    /// `StopLossModifier`: spends the sum on the stop.
    StopCoefficient,
    /// `MaxModifier`: caps the sum.
    Cap,
    /// One of the `Add*` terms of the sum.
    Term,
}

/// The role of `field`, or `None` for a field outside the section.
fn role(field: &TickParam) -> Option<Role> {
    if field.section != ParamSection::DeltaModifiers {
        return None;
    }
    Some(match field.key {
        "SellModifier" => Role::SellCoefficient,
        "StopLossModifier" => Role::StopCoefficient,
        "MaxModifier" => Role::Cap,
        _ => Role::Term,
    })
}

/// The Delta Modifiers fields of one search and how to read their partners at a point.
pub(super) struct Coupling<'a> {
    /// Every (coefficient, term) pair among the varied fields.
    pairs: Vec<(&'static TickParam, &'static TickParam)>,
    /// The strategies' parameters at a point; `None` couples nothing.
    per_base: Option<&'a PerBase<'a>>,
}

impl<'a> Coupling<'a> {
    /// A search that couples nothing: no field is inert, no diagonal is walked — what the tests
    /// of the plain descent run under.
    #[cfg(test)]
    pub(super) fn none() -> Self {
        Self {
            pairs: Vec::new(),
            per_base: None,
        }
    }

    /// Args:
    ///     fields: The fields the search varies.
    ///     per_base: The strategies' parameters at a point.
    pub(super) fn of(fields: &[&'static TickParam], per_base: &'a PerBase<'a>) -> Self {
        let coefficients = fields
            .iter()
            .filter(|f| matches!(role(f), Some(Role::SellCoefficient | Role::StopCoefficient)));
        let pairs = coefficients
            .flat_map(|&c| {
                fields
                    .iter()
                    .filter(|t| role(t) == Some(Role::Term))
                    .map(move |&t| (c, t))
            })
            .collect();
        Self {
            pairs,
            per_base: Some(per_base),
        }
    }

    /// Whether turning `field` at `point` can move nothing, on every strategy of the sample.
    pub(super) fn inert(&self, field: &TickParam, point: &Point) -> bool {
        let (Some(role), Some(per_base)) = (role(field), self.per_base) else {
            return false;
        };
        per_base(point).iter().all(|(_, exit)| inert_on(role, exit))
    }

    /// The (coefficient, term) pairs stuck at `point` ([`Self::is_stuck`]).
    pub(super) fn stuck(&self, point: &Point) -> Vec<(&'static TickParam, &'static TickParam)> {
        self.pairs
            .iter()
            .filter(|(c, t)| self.is_stuck(c, t, point))
            .copied()
            .collect()
    }

    /// Whether a (coefficient, term) pair is stuck at `point`: the coefficient is off (zero on
    /// every strategy) and the sum has no term on any, so the coefficient's side cannot be
    /// switched on by one field — its own scan moves nothing without a term, and a term's scan
    /// is judged through the other coefficient only, or through nothing — while the coefficient
    /// has a level to spend the sum on: a stop on some strategy for `StopLossModifier`.
    ///
    /// A coefficient already set is not stuck: a term's own scan spends through it. One with no
    /// level anywhere is never walked: its value would move nothing, and a step that pays
    /// through the other coefficient would keep that value in the answer Save writes.
    pub(super) fn is_stuck(
        &self,
        coefficient: &TickParam,
        term: &TickParam,
        point: &Point,
    ) -> bool {
        let (Some(c), Some(Role::Term), Some(per_base)) =
            (role(coefficient), role(term), self.per_base)
        else {
            return false;
        };
        let bases = per_base(point);
        bases
            .iter()
            .all(|(_, exit)| silent(exit) && coefficient_of(c, exit) == 0.0)
            && bases.iter().any(|(_, exit)| can_spend(c, exit))
    }

    /// The paths a stuck pair is walked along, each a list of (coefficient, term) values in
    /// strategy spelling: the coefficient's grid off zero on each side — up, and down where the
    /// grid goes below zero — against the term's grid above zero, both spanned end to end over
    /// the longer of the two, so a path runs from the smallest product to the largest.
    pub(super) fn diagonals(
        coefficient: &TickParam,
        term: &TickParam,
    ) -> Vec<Vec<(String, String)>> {
        let (Some(up), Some(down), Some(terms)) = (
            steps(coefficient, |v| v > 0.0),
            steps(coefficient, |v| v < 0.0),
            steps(term, |v| v > 0.0),
        ) else {
            return Vec::new();
        };
        [up, down]
            .into_iter()
            .filter(|side| !side.is_empty() && !terms.is_empty())
            .map(|side| {
                let n = side.len().max(terms.len());
                (0..n)
                    .map(|k| {
                        let at = |len: usize| {
                            if n == 1 {
                                0
                            } else {
                                (k * (len - 1) + (n - 1) / 2) / (n - 1)
                            }
                        };
                        (
                            spell(&coefficient.kind, side[at(side.len())]),
                            spell(&term.kind, terms[at(terms.len())]),
                        )
                    })
                    .collect::<Vec<_>>()
            })
            .collect()
    }
}

/// Whether a field of `role` moves nothing on one strategy's sell parameters.
fn inert_on(role: Role, exit: &ExitParams) -> bool {
    let sell_spends = exit.sell_modifier != 0.0;
    // `exit::stops::stop_pct`: no stop, or no coefficient on it, spends nothing.
    let stop_spends = exit.stop_loss_modifier != 0.0 && exit.stop_loss_pct != 0.0;
    let no_terms = silent(exit);
    match role {
        Role::SellCoefficient => no_terms,
        Role::StopCoefficient => no_terms || exit.stop_loss_pct == 0.0,
        Role::Term => !sell_spends && !stop_spends,
        Role::Cap => no_terms || (!sell_spends && !stop_spends),
    }
}

/// The value of a coefficient of `role` on one strategy; 0 for a role that is no coefficient.
fn coefficient_of(role: Role, exit: &ExitParams) -> f64 {
    match role {
        Role::SellCoefficient => exit.sell_modifier,
        Role::StopCoefficient => exit.stop_loss_modifier,
        Role::Cap | Role::Term => 0.0,
    }
}

/// Whether a coefficient of `role` has a level to spend the sum on, on one strategy: the sell
/// always, the stop only where there is one (`exit::stops::stop_pct`).
fn can_spend(role: Role, exit: &ExitParams) -> bool {
    match role {
        Role::StopCoefficient => exit.stop_loss_pct != 0.0,
        _ => true,
    }
}

/// Whether the sum has no term: every `Add*` coefficient at zero. Compared against the family
/// with its terms zeroed rather than field by field, so a term added to `Modifiers` later is
/// not left out of the question.
fn silent(exit: &ExitParams) -> bool {
    let m = &exit.sell_mods;
    *m == crate::db::tuner::ticks::mshot::Modifiers {
        market_sign: m.market_sign,
        distance_pct: m.distance_pct,
        pricebug_cap: m.pricebug_cap,
        ..Default::default()
    }
}

/// The grid indices of a number field whose value passes `keep`, nearest zero first; `None`
/// for a field that is not a number.
fn steps(field: &TickParam, keep: impl Fn(f64) -> bool) -> Option<Vec<usize>> {
    let ParamKind::Num { grid } = &field.kind else {
        return None;
    };
    let mut out: Vec<usize> = (0..grid.len()).filter(|&i| keep(grid[i])).collect();
    out.sort_by(|&a, &b| grid[a].abs().total_cmp(&grid[b].abs()));
    Some(out)
}

/// Walk one path of two fields as one move: each step sets both, and a step that beats the
/// score is kept — the walk goes on from it, as a single field's scan does.
///
/// Returns:
///     Whether a step was kept, or `None` when the run was stopped — checked before every
///     step, each a replay of the sample.
pub(super) fn walk_path(
    point: &mut Point,
    (a, b): (&'static TickParam, &'static TickParam),
    path: &[(String, String)],
    evaluate: &(dyn Fn(&Point) -> Option<Tally> + Sync),
    score: &mut Option<Tally>,
    min_n: i64,
    handle: &SearchHandle,
) -> Option<bool> {
    let (mut kept_a, mut kept_b) = (point.get(a.key).cloned(), point.get(b.key).cloned());
    let mut improved = false;
    for (va, vb) in path {
        if handle.is_cancelled() {
            handle.note_abandoned();
            return None;
        }
        point.insert(a.key, va.clone());
        point.insert(b.key, vb.clone());
        let trial = evaluate(point);
        if better_score(&trial, score, min_n) {
            *score = trial;
            improved = true;
            (kept_a, kept_b) = (Some(va.clone()), Some(vb.clone()));
        } else {
            restore(point, a.key, kept_a.clone());
            restore(point, b.key, kept_b.clone());
        }
    }
    Some(improved)
}

#[cfg(test)]
mod tests;
