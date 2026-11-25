//! The strategy list's totals row (#1455): one aggregate over every row the list's own filters
//! admit — period, name search, kind, coin lists, "Enabled only", conditions and the Auto /
//! Tuning filters — including the rows past the [`super::MAX_ROWS`] draw cap.
//!
//! The aggregate is a synthetic [`GroupStat`], so each column formats it through its own
//! descriptor and the total can never be printed in a different shape than the cells above it.
//! Whether a column shows that text at all is its [`TotalRule`]: counts and pooled ratios
//! always, money only when the set is in one unit, and the ratios a row cannot be summed from
//! (Profit %, average order, profit factor) never.
//!
//! Strategies are never merged across cores here (#642): the fold runs over the list's own
//! rows, one per strategy per core, and only produces the numbers the footer prints.

use moon_core::db::QuoteScope;
use moon_core::db::analytics::GroupStat;

use super::super::columns::{MetricCol, TotalRule};

/// What the totals row draws, folded once per change of the list's filters or data.
#[derive(Clone, Debug, Default)]
pub(in crate::analytics) struct StratTotals {
    /// How many strategy rows were folded in: every admitted row, drawn or not.
    pub(in crate::analytics::tuner) rows: usize,
    /// The synthetic aggregate: summed trades, wins and profit; best and worst are the row
    /// extremes; `quote` is the one currency every row shares, or `Mixed` / `Unknown`.
    group: GroupStat,
    /// Whether the money columns may be printed: every row's money fields are finite and, in
    /// the quote lens, every row carries the same single quote.
    money: bool,
}

impl StratTotals {
    /// Fold the admitted rows into the footer's aggregate.
    ///
    /// Args:
    ///     all: The complete group set of the current load.
    ///     idx: Indices of the admitted rows — the WHOLE filtered list, not just its drawn head.
    ///     percent: Whether the active profit lens is the per-trade percent one. Its values
    ///         carry no currency, so only finiteness gates the money columns there.
    ///
    /// Returns:
    ///     The totals to draw under the list.
    pub(in crate::analytics::tuner) fn of(all: &[GroupStat], idx: &[usize], percent: bool) -> Self {
        let mut group = GroupStat::default();
        let mut quote: Option<QuoteScope> = None;
        let mut finite = true;
        // Over rows that HAVE trades only: a funding-only row reports 0 for both extremes
        // (`COALESCE` in `db::analytics::groups`), which is no trade and must not become the
        // set's best loss or worst win.
        let mut extremes: Option<(f64, f64)> = None;
        let mut rows = 0;
        for g in idx.iter().filter_map(|i| all.get(*i)) {
            rows += 1;
            group.n += g.n;
            group.wins += g.wins;
            group.profit += g.profit;
            finite &= g.profit.is_finite() && g.best.is_finite() && g.worst.is_finite();
            if g.n > 0 {
                extremes = Some(match extremes {
                    None => (g.best, g.worst),
                    Some((best, worst)) => (best.max(g.best), worst.min(g.worst)),
                });
            }
            quote = Some(match quote {
                None => g.quote,
                Some(acc) => fold_quote(acc, g.quote),
            });
        }
        let quote = quote.unwrap_or_default();
        (group.best, group.worst) = extremes.unwrap_or((0.0, 0.0));
        group.quote = quote;
        // Never read through a column (their rules are `Blank`), but NaN rather than a zero
        // that could pass for a real figure if one ever were.
        group.raw_profit = f64::NAN;
        group.avg_order = f64::NAN;
        group.pf = f64::NAN;
        let money = finite && (percent || matches!(quote, QuoteScope::Single(_)));
        Self { rows, group, money }
    }

    /// The footer cell under one metric column.
    ///
    /// Args:
    ///     col: The column the cell sits under.
    ///
    /// Returns:
    ///     `None` for an empty cell; otherwise the text and the value its sign colours by
    ///     (`None` = neutral). A money column over a set it cannot add up reads as an em dash.
    pub(in crate::analytics::tuner) fn cell(
        &self,
        col: &MetricCol,
    ) -> Option<(String, Option<f64>)> {
        let shown = || {
            (
                (col.text)(&self.group),
                col.signed.map(|value| value(&self.group)),
            )
        };
        match col.total {
            TotalRule::Blank => None,
            TotalRule::Plain => Some(shown()),
            TotalRule::Money if self.money => Some(shown()),
            TotalRule::Money => Some(("\u{2014}".to_string(), None)),
        }
    }
}

/// The widest total any filter could produce, as a group the columns can format.
///
/// The numeric columns are measured once per data load over every row, so they do not jump
/// while the user types in the search box; a total over hundreds of rows is wider than any of
/// them, and measured only against the rows it would be cut. This is the bound to measure as
/// well: no subset's profit is further from zero than the sum of every row's magnitude (signed
/// negative, so the sign is counted), no subset trades more than all of them, and an extreme of
/// a subset is one of the rows' own.
///
/// One bound per quote currency, because a money total is printed only over rows of one
/// currency and each currency has its own precision (eight decimals for BTC, two for USDT);
/// one over every row with no currency, which is the percent lens's bound (its cells carry no
/// ticker) and reads as an em dash in the quote lens; and one for the per-trade average.
///
/// Each bound is printed at its formatter's every decimal ([`full_digits`]): the crypto and
/// percent formatters trim trailing zeros, so `-19.5` would print shorter than a subset's
/// `+18.50000001`.
///
/// Args:
///     all: The complete group set of the current load.
///
/// Returns:
///     Groups whose formatted cells are together at least as long as any footer total's.
pub(in crate::analytics::tuner) fn widest_totals(all: &[GroupStat]) -> Vec<GroupStat> {
    // The percent-lens formatter (`summary::fmt_signed`) rounds to two decimals.
    const PLAIN_DECIMALS: usize = 2;
    let mut bounds = vec![(
        GroupStat {
            quote: QuoteScope::Empty,
            ..Default::default()
        },
        0.0f64,
    )];
    let mut widest_avg = 0.0f64;
    let mut funding_only = 0.0f64;
    let mut fewest_trades: Option<i64> = None;
    for g in all {
        let magnitude = if g.profit.is_finite() {
            g.profit.abs()
        } else {
            0.0
        };
        bounds[0].0.n += g.n;
        bounds[0].1 += magnitude;
        if let QuoteScope::Single(c) = g.quote {
            let at = match bounds.iter().position(|(b, _)| b.quote == g.quote) {
                Some(at) => at,
                None => {
                    let bound = GroupStat {
                        quote: QuoteScope::Single(c),
                        ..Default::default()
                    };
                    bounds.push((bound, 0.0));
                    bounds.len() - 1
                }
            };
            bounds[at].0.n += g.n;
            bounds[at].1 += magnitude;
        }
        if g.n > 0 {
            if g.avg().is_finite() {
                widest_avg = widest_avg.max(g.avg().abs());
            }
            fewest_trades = Some(fewest_trades.map_or(g.n, |n| n.min(g.n)));
        } else {
            funding_only += magnitude;
        }
    }
    let mut bounds: Vec<GroupStat> = bounds
        .into_iter()
        .map(|(mut bound, magnitude)| {
            let decimals = match bound.quote {
                QuoteScope::Single(c) => c.display_decimals(),
                _ => PLAIN_DECIMALS,
            };
            bound.profit = -full_digits(magnitude, decimals);
            bound
        })
        .collect();
    // A pooled per-trade average is a new number and can print more digits than any row's
    // ("+1.33" between "+1" and "+2"). The trading rows pool to within their widest average; a
    // funding-only row adds profit but no trades (`n` counts trades only), at most its whole
    // money over the fewest trades any subset can divide by. A subset of funding rows alone
    // has no trades and prints 0.
    let widest_avg = widest_avg + fewest_trades.map_or(0.0, |n| funding_only / n as f64);
    bounds.push(GroupStat {
        n: 1,
        profit: -full_digits(widest_avg, PLAIN_DECIMALS),
        quote: QuoteScope::Empty,
        ..Default::default()
    });
    bounds
}

/// The longest number a formatter prints for any magnitude up to `magnitude`: the same whole
/// part (one more if rounding to `decimals` carries into it) with every decimal non-zero.
fn full_digits(magnitude: f64, decimals: usize) -> f64 {
    let step = 10f64.powi(decimals as i32);
    (magnitude + 0.5 / step).floor() + (1.0 - 1.0 / step)
}

/// Combine the running quote scope with one more row's.
///
/// One shared currency survives; two different known currencies make the set `Mixed` (USDT and
/// USDC stay apart); a row with no trustworthy quote, or with none at all, poisons the sum the
/// way it already blanks its own cell.
fn fold_quote(acc: QuoteScope, next: QuoteScope) -> QuoteScope {
    match (acc, next) {
        (QuoteScope::Single(a), QuoteScope::Single(b)) if a == b => QuoteScope::Single(a),
        (QuoteScope::Unknown, _) | (_, QuoteScope::Unknown) => QuoteScope::Unknown,
        _ => QuoteScope::Mixed,
    }
}

// Explicit imports, never `use super::*`: the parent re-exports `gpui::*`, whose own `test`
// shadows the built-in attribute and makes `#[test]` expand recursively.
#[cfg(test)]
mod tests;
