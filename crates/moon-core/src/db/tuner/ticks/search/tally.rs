//! tally items for tick searches.

use super::*;

/// The fact of `deals`, in order: each deal's reported result — the same per-deal value
/// [`crate::db::tuner::ticks::fact_stats`] folds into the "Fact" column, so the two cannot drift. Every deal
/// counts, as there.
pub fn fact_tally(deals: &[PreparedDeal]) -> Tally {
    crate::db::tuner::ticks::stats::fact_tally_of(deals.iter().map(|d| &d.deal)).0
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
    let cut = install_column(|| cut(deals, params));
    (cut.kept, cut.left_open.len())
}

/// The sample's bases, their completion, and the sample less the deals the strategies as they
/// stand leave open inside the tape — the one cut both the search and the variant columns
/// ([`comparable`]) make.
pub(super) struct Cut<'a> {
    /// The strategies of the whole sample, the dropped deals' among them.
    pub(super) whole: Bases<'a>,
    /// Where each number field starts on its grid ([`deps::dependents_of`]).
    pub(super) start: HashMap<&'static str, usize>,
    /// What completes a point over `whole`.
    pub(super) deps: deps::Dependents,
    /// The kept deals, in order.
    pub(super) kept: Vec<PreparedDeal>,
    /// Each kept deal's index into `whole.owns`.
    pub(super) of_kept: Vec<usize>,
    /// The dropped deals' ids.
    pub(super) left_open: Vec<i64>,
}

/// Make the [`Cut`] of `deals` under `params` (`closing::closable_at_base`).
pub(super) fn cut<'a>(deals: &'a [PreparedDeal], params: &SearchParams<'_>) -> Cut<'a> {
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
    install_column(|| variant_tally_here(deals, defaults, kind, values, model))
}

/// [`variant_tally`] in whatever pool the caller runs in — for [`super::point_cost`], which
/// measures a replay the way a search runs it, on the search's own pool: through the columns'
/// pool every replay of its batch would queue on two threads, and the time it reports would be
/// that queue's.
pub(super) fn variant_tally_here(
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
    score(deals, &bases.of_deal, &per_base)
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
    let own = |params: &crate::db::tuner::ticks::MshotParams| {
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
/// ([`crate::db::tuner::ticks::Outcome::profit_money`]) —
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
    install_column(|| {
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
pub(super) fn point_of(values: &[(String, String)]) -> Point {
    let mut point = Point::new();
    for (key, value) in values {
        if let Some(field) = TICK_PARAMS.iter().find(|f| f.key == key) {
            point.insert(field.key, value.clone());
        }
    }
    point
}
