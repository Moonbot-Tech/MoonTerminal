//! scoring items for tick searches.

use super::*;

/// Every deal's result under one point, in order — `(result, spent)`, the result in the scope's
/// metric as the "Fact" column holds it ([`crate::db::tuner::ticks::Outcome::profit_metric`]), `None` where the
/// point makes no trade of the deal — whether it bought the deal and left it open, and what
/// such a deal would make at the tape's end ([`crate::db::tuner::ticks::Outcome::open_metric_at_tape_end`]).
///
/// Args:
///     deals: The deals.
///     of_deal: Each deal's index into `params` ([`Bases::of_deal`]), as long as `deals`.
///     params: The point's parameters per base ([`Bases::params`]).
///     fills: Each deal's entry fill under `params`, when read already ([`fills::FillCache`]);
///         `None` replays the entry too.
pub(super) fn results(
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
    /// tally's metric ([`crate::db::tuner::ticks::Outcome::open_metric_at_tape_end`]). An estimate shown beside
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
    pub(super) fn push(
        &mut self,
        result: Option<(f64, f64)>,
        left_open: bool,
        at_tape_end: Option<f64>,
    ) {
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
pub(super) fn score(
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
pub(super) struct CorridorGuard {
    /// `(deal index, own corridor, deltas)`; a deal without a MoonShot entry of its own holds
    /// no corridor to keep.
    pub(super) deals: Vec<(usize, MshotParams, Vec<Deltas>)>,
}

impl CorridorGuard {
    pub(super) fn of(deals: &[PreparedDeal]) -> Self {
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
    pub(super) fn holds(&self, of_deal: &[usize], params: &[(EntryParams, ExitParams)]) -> bool {
        self.deals
            .iter()
            .all(|(i, own, deltas)| keeps_corridor(&params[of_deal[*i]].0, own, deltas))
    }
}

/// Whether an entry keeps a trade's own corridor ([`MshotParams::never_closer_than`]); an entry
/// of the fact's has none to move. The near bound is held only where the entry method reads it.
pub(super) fn keeps_corridor(entry: &EntryParams, own: &MshotParams, deltas: &[Deltas]) -> bool {
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
pub(super) fn ordered(entry: &EntryParams) -> bool {
    match entry {
        EntryParams::MoonShot(params) => params.is_ordered(),
        EntryParams::Fact => true,
    }
}

/// Whether a point's parameters invert the corridor fields of a base that started in order
/// (`start_ordered`, one flag per base, in the bases' order).
pub(super) fn inverts(start_ordered: &[bool], params: &[(EntryParams, ExitParams)]) -> bool {
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
pub(super) fn better_score(a: &Option<Tally>, b: &Option<Tally>, min_n: i64) -> bool {
    match (a, b) {
        (Some(a), Some(b)) => better(a, b, min_n),
        (Some(_), None) => true,
        (None, _) => false,
    }
}

/// Whether `a` beats `b` under the objective, with the sample floor.
pub(super) fn better(a: &Tally, b: &Tally, min_n: i64) -> bool {
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
pub(super) fn drop_passengers(
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
pub(super) fn next_random(state: &mut u64) -> u64 {
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
