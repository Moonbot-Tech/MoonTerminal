//! descent items for tick searches.

use super::*;

/// Where one descent stopped.
pub(super) struct Walked {
    pub(super) point: Point,
    pub(super) score: Option<Tally>,
    pub(super) passes: usize,
    pub(super) converged: bool,
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
pub(super) fn descend(
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
pub(super) fn restore(point: &mut Point, key: &'static str, was: Option<String>) {
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
pub(super) fn grid_index(
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
pub(super) fn nearest_step(grid: &[f64], value: f64) -> usize {
    grid.iter()
        .enumerate()
        .min_by(|(_, a), (_, b)| (*a - value).abs().total_cmp(&(*b - value).abs()))
        .map_or(0, |(i, _)| i)
}

/// Fisher–Yates over the restart's own stream.
pub(super) fn shuffle<T>(items: &mut [T], state: &mut u64) {
    for i in (1..items.len()).rev() {
        let j = (next_random(state) % (i as u64 + 1)) as usize;
        items.swap(i, j);
    }
}

/// Move the base a little: one to three fields of `order`, a number field one to three grid
/// steps either way from where it stands (`start`), any other field to a value of its own.
pub(super) fn perturb(
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
pub(super) struct Run {
    pub(super) restart: usize,
    pub(super) point: Point,
    pub(super) score: Option<Tally>,
    pub(super) passes: usize,
    pub(super) converged: bool,
}

/// One point of the grid: the varied fields' values, in strategy spelling.
pub(super) type Point = HashMap<&'static str, String>;

/// The model parameters a point comes to on a deal whose strategy holds `own`, with `held` laid
/// over it first.
pub(super) fn params_of(
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
pub(super) fn varied<'a>(
    p: &SearchParams<'a>,
) -> Vec<&'static crate::db::tuner::ticks::params::TickParam> {
    deps::offered(p)
        .into_iter()
        .filter(|f| !p.locked.contains(f.key))
        .filter(|f| p.grids.arity(f) > 0)
        .collect()
}

/// The distinct strategy bases of a sample ([`PreparedDeal::own`]) and which one each deal runs
/// over: a point's parameters are built once per strategy, not once per deal.
pub(super) struct Bases<'a> {
    pub(super) owns: Vec<&'a HashMap<String, String>>,
    /// Each deal's index into `owns`, in the deals' order.
    pub(super) of_deal: Vec<usize>,
}

impl<'a> Bases<'a> {
    pub(super) fn of(deals: &'a [PreparedDeal]) -> Self {
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
    pub(super) fn params(
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
    pub(super) fn moves(&self, held: &HashMap<String, String>, key: &str, value: &str) -> bool {
        self.owns.iter().any(|own| {
            held.get(key)
                .or_else(|| own.get(key))
                .filter(|base| !base.trim().is_empty())
                .is_none_or(|base| !same_value(base, value))
        })
    }
}
