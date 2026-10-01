//! How much a search will score, known before it runs — what the axis shows under its grid and
//! what asks before a long run (LinKvo, 2026-09-25: "how many variants, and roughly how long").
//!
//! The count is the descent's own arithmetic: one pass tries every other value of each field it
//! varies, a pass that moves nothing then tries the Entry pairs, and a search of both groups
//! runs a whole exit descent per entry point it scores ([`super::nested`]) — with the screen on,
//! per entry move the screen keeps ([`super::screen`]). What the count cannot
//! know is how many passes a descent takes before one changes nothing; it takes the typical
//! number the bench measured ([`PASSES`], [`INNER_PASSES`]). The time is the count by what one
//! point costs on this sample ([`point_cost`]), measured, never assumed.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use rayon::prelude::*;

use super::{PreparedDeal, SearchParams, train_len, variant_tally, varied};
use crate::db::tuner::threshold_search::search::install;
use crate::db::tuner::ticks::params::{ParamGroup, ParamKind};
use crate::db::tuner::ticks::settings::ModelSettings;

/// Passes one descent is counted at: the pass that moves, the one that moves what the first made
/// worth moving, and the one that finds nothing left.
const PASSES: f64 = 3.0;

/// Passes the exit descent under one entry point is counted at: it starts from the exit found
/// under the best entry point so far, and one pass that moves and one that does not usually end
/// it.
const INNER_PASSES: f64 = 2.0;

/// What a point scored on the entry fills of a point before costs, as a share of a whole replay
/// ([`super::fills`]): the exit replayed alone. Measured 2026-10-02 — 0.53–0.61 on three MoonShot
/// strategies, 210 µs against 381 µs a point on the bench's search of both groups.
const EXIT_SHARE: f64 = 0.55;

/// Whole replays `scored` points come to when `reused` of them read the entry fills of a point
/// before — what a point cost is a share of ([`point_cost`]).
pub fn full_replays(scored: f64, reused: f64) -> f64 {
    let reused = reused.clamp(0.0, scored.max(0.0));
    (scored - reused) + reused * EXIT_SHARE
}

/// How much one search scores.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SearchSize {
    /// Points scored, each a replay of the training slice.
    pub points: f64,
    /// Entry points each scored by a whole search of the exit under them; zero for a search of
    /// one group.
    pub entry_points: f64,
    /// Fields of the Entry group the search varies.
    pub entry_fields: usize,
    /// Fields of the Exit group the search varies.
    pub exit_fields: usize,
    /// Points that replay their entry anew; the rest read the fills of a point before
    /// ([`full_replays`]).
    pub fresh: f64,
}

impl SearchSize {
    /// Whether the search nests the exit under every entry point.
    pub fn nested(&self) -> bool {
        self.entry_points > 0.0
    }

    /// Roughly how long it runs, at `per_point` a whole replay ([`point_cost`]).
    pub fn time(&self, per_point: Duration) -> Duration {
        let replays = full_replays(self.points, self.points - self.fresh);
        Duration::from_secs_f64((replays * per_point.as_secs_f64()).min(u32::MAX as f64))
    }
}

/// The size of the search `params` describe; the held values, the defaults, the seed and the
/// sample play no part in it.
pub fn search_size(params: &SearchParams<'_>) -> SearchSize {
    let fields = varied(params);
    let of = |group: ParamGroup| fields.iter().filter(move |f| f.group == group);
    // One pass tries every value of a field but the one it stands on.
    let span = |group: ParamGroup| -> f64 {
        of(group)
            .map(|f| params.grids.arity(f).saturating_sub(1) as f64)
            .sum()
    };
    let numbers = of(ParamGroup::Entry)
        .filter(|f| f.kind == ParamKind::Num)
        .count() as f64;
    // Every ordered pair of two Entry number fields, once, in the pass that moves nothing.
    let pairs = numbers * (numbers - 1.0).max(0.0);
    let restarts = params.restarts.max(1) as f64;
    let cap = params.max_passes.max(1) as f64;
    let passes = PASSES.min(cap);
    let (entry_fields, exit_fields) = (of(ParamGroup::Entry).count(), of(ParamGroup::Exit).count());
    let both = entry_fields > 0 && exit_fields > 0;
    if both {
        let per_entry = INNER_PASSES.min(cap) * span(ParamGroup::Exit);
        let tried = restarts * (passes * span(ParamGroup::Entry) + pairs);
        if params.screen_entry {
            // Every move of a step scored once, under the exit found so far; the best few of
            // each field, and of the pair moves, by a whole exit descent ([`super::screen`]).
            let keep = super::screen::KEEP as f64;
            let kept_fields: f64 = of(ParamGroup::Entry)
                .map(|f| (params.grids.arity(f).saturating_sub(1) as f64).min(keep))
                .sum();
            let entry_points = restarts * (passes * kept_fields + pairs.min(keep));
            // Each move the screen scores is a new entry; the exit descents under the kept ones
            // read the fills their screening read.
            return SearchSize {
                points: tried + entry_points * per_entry,
                entry_points,
                entry_fields,
                exit_fields,
                fresh: tried,
            };
        }
        let entry_points = tried;
        // The first point of each exit descent replays its entry; the rest of it reads the fills.
        SearchSize {
            points: entry_points * per_entry,
            entry_points,
            entry_fields,
            exit_fields,
            fresh: entry_points,
        }
    } else {
        let points =
            restarts * (passes * (span(ParamGroup::Entry) + span(ParamGroup::Exit)) + pairs);
        SearchSize {
            points,
            entry_points: 0.0,
            entry_fields,
            exit_fields,
            // Every point of an entry search moves the entry; an exit search reads its start's.
            fresh: if entry_fields > 0 { points } else { restarts },
        }
    }
}

/// What one whole replay costs on this sample: the strategies as they stand replayed over the
/// training slice, `parallel` replays side by side on the search's own pool — the way a search
/// runs its restarts — so the figure is the same quantity a finished search's time over the
/// whole replays its scored points come to is ([`full_replays`]), and the two can stand in for
/// each other.
///
/// Args:
///     deals: The sample, chronological, cut at its horizon (`clip_to_horizon`).
///     defaults, kind, model: As for [`super::variant_tally`].
///     train_frac: [`SearchParams::train_frac`].
///     parallel: Replays run side by side — the search's restarts; at least one.
///
/// Returns:
///     The time of one replay of the training slice, amortized over the replays run side by
///     side; zero for an empty sample.
pub fn point_cost(
    deals: &[PreparedDeal],
    defaults: &HashMap<String, f64>,
    kind: &str,
    model: ModelSettings,
    train_frac: f64,
    parallel: usize,
) -> Duration {
    // More side by side than the pool has threads adds nothing but waiting.
    let runs = parallel.clamp(1, 64);
    let closes: Vec<i64> = deals.iter().map(|d| d.deal.close_ms).collect();
    let train = &deals[..train_len(&closes, train_frac)];
    if train.is_empty() {
        return Duration::ZERO;
    }
    let started = Instant::now();
    install(|| {
        (0..runs).into_par_iter().for_each(|_| {
            let _ = variant_tally(train, defaults, kind, &[], model);
        })
    });
    started.elapsed() / runs as u32
}

#[cfg(test)]
mod tests;
