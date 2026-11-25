//! The totals row's fold: which rows it covers and which cells it may print.
//!
//! Explicit imports, never `use super::*`: the grandparent re-exports `gpui::*`, whose own
//! `test` shadows the built-in attribute and makes `#[test]` expand recursively.

use moon_core::db::analytics::GroupStat;
use moon_core::db::{ProfitUnit, QuoteCurrency, QuoteScope};

use crate::analytics::set_pnl_unit;

use super::super::super::columns::{
    COL_AVG, COL_AVG_ORDER, COL_BEST, COL_PF, COL_PROFIT, COL_PROFIT_PCT, COL_TRADES, COL_WINRATE,
    COL_WORST,
};
use super::super::super::strat_columns::METRIC_COLS;
use super::super::MAX_ROWS;
use super::{StratTotals, widest_totals};

fn usdt() -> QuoteScope {
    QuoteScope::Single(QuoteCurrency::usdt())
}

fn usdc() -> QuoteScope {
    QuoteScope::Single(QuoteCurrency::from_ticker("USDC").expect("USDC is a known quote"))
}

/// One strategy row with the fields the fold reads.
fn row(n: i64, wins: i64, profit: f64, best: f64, worst: f64, quote: QuoteScope) -> GroupStat {
    GroupStat {
        key: format!("{n}-{profit}@1"),
        n,
        wins,
        profit,
        best,
        worst,
        quote,
        ..Default::default()
    }
}

fn text(totals: &StratTotals, col: &super::MetricCol) -> Option<String> {
    totals.cell(col).map(|(text, _)| text)
}

/// The issue's own case: the list draws 500 rows and says "shown 500 of …". The total must count
/// the tail too. Breaks if the fold is handed the drawn head (`idx[..MAX_ROWS]`) instead of the
/// whole admitted list.
#[test]
fn the_total_counts_rows_past_the_draw_cap() {
    let all: Vec<GroupStat> = (0..MAX_ROWS + 100)
        .map(|_| row(2, 1, 1.0, 1.0, 0.0, usdt()))
        .collect();
    let idx: Vec<usize> = (0..all.len()).collect();
    let totals = StratTotals::of(&all, &idx, false);
    assert_eq!(totals.rows, 600);
    assert_eq!(text(&totals, &COL_TRADES).as_deref(), Some("1200"));
    assert_eq!(text(&totals, &COL_PROFIT).as_deref(), Some("+600.00 USDT"));
}

/// Only the admitted rows add up: an index left out by a filter must not reach the sum.
#[test]
fn only_the_admitted_rows_are_summed() {
    let all = vec![
        row(1, 1, 5.0, 5.0, 5.0, usdt()),
        row(1, 0, -100.0, -100.0, -100.0, usdt()),
        row(1, 1, 2.5, 2.5, 2.5, usdt()),
    ];
    let totals = StratTotals::of(&all, &[0, 2], false);
    assert_eq!(totals.rows, 2);
    assert_eq!(text(&totals, &COL_PROFIT).as_deref(), Some("+7.50 USDT"));
}

/// USDT and USDC stay apart, and a row that already prints "—" makes the total "—": a sum of two
/// currencies is no amount. Every money column follows, not only Profit. Breaks if the quote fold
/// keeps the first currency, or if only the Profit formatter checks it.
#[test]
fn money_over_two_quotes_or_an_unknown_row_is_a_dash() {
    for other in [
        usdc(),
        QuoteScope::Mixed,
        QuoteScope::Unknown,
        QuoteScope::Empty,
    ] {
        let all = vec![
            row(2, 1, 4.0, 3.0, 1.0, usdt()),
            row(2, 2, 6.0, 4.0, 2.0, other),
        ];
        let totals = StratTotals::of(&all, &[0, 1], false);
        for col in [&COL_PROFIT, &COL_AVG, &COL_BEST, &COL_WORST] {
            assert_eq!(
                text(&totals, col).as_deref(),
                Some("\u{2014}"),
                "{} over {other:?}",
                col.key
            );
        }
        // Counts and the pooled win rate carry no currency and stay.
        assert_eq!(text(&totals, &COL_TRADES).as_deref(), Some("4"));
        assert_eq!(text(&totals, &COL_WINRATE).as_deref(), Some("75.0%"));
    }
}

/// Under the percent lens the values carry no currency, so rows of different quotes add up —
/// every money column, Profit included. Breaks if the quote gate is applied regardless of the
/// lens, or if one money column keeps its own currency check.
#[test]
fn the_percent_lens_ignores_the_quote() {
    let all = vec![
        row(1, 1, 2.0, 2.0, 2.0, usdt()),
        row(1, 0, -1.0, -1.0, -1.0, QuoteScope::Mixed),
    ];
    let totals = StratTotals::of(&all, &[0, 1], true);
    let _lens = Lens::set(Some(ProfitUnit::Percent));
    assert_eq!(text(&totals, &COL_PROFIT).as_deref(), Some("+1%"));
    assert_eq!(text(&totals, &COL_AVG).as_deref(), Some("+0.5%"));
    assert_eq!(text(&totals, &COL_BEST).as_deref(), Some("+2%"));
    assert_eq!(text(&totals, &COL_WORST).as_deref(), Some("-1%"));
}

/// Win rate pools the trades: 1 win of 1 and 0 of 3 is 25%, not the 50% mean of two row rates.
/// The per-trade average is total profit over total trades the same way.
#[test]
fn ratios_pool_the_trades_instead_of_averaging_rows() {
    let all = vec![
        row(1, 1, 8.0, 8.0, 8.0, usdt()),
        row(3, 0, -2.0, -0.5, -1.0, usdt()),
    ];
    let totals = StratTotals::of(&all, &[0, 1], false);
    assert_eq!(text(&totals, &COL_WINRATE).as_deref(), Some("25.0%"));
    // (8 - 2) / 4 trades = 1.5 per trade; the mean of the row averages would be 3.67.
    assert_eq!(text(&totals, &COL_AVG).as_deref(), Some("+1.5"));
}

/// Best and worst are the extremes of the rows that traded. A funding-only row reports 0 for
/// both; taken in, it would make a set of losers' best "0" and a set of winners' worst "0".
#[test]
fn extremes_skip_rows_without_trades() {
    let all = vec![
        row(2, 0, -6.0, -1.0, -5.0, usdt()),
        row(4, 0, -3.0, -0.5, -2.0, usdt()),
        row(0, 0, 0.4, 0.0, 0.0, usdt()),
    ];
    let totals = StratTotals::of(&all, &[0, 1, 2], false);
    assert_eq!(text(&totals, &COL_BEST).as_deref(), Some("-0.5"));
    assert_eq!(text(&totals, &COL_WORST).as_deref(), Some("-5"));
}

/// Profit %, average order and PF are ratios the row cannot be summed from — the cell stays
/// empty. Breaks if a column's rule is flipped to show the synthetic group's value.
#[test]
fn ratio_columns_stay_empty() {
    let all = vec![row(1, 1, 1.0, 1.0, 1.0, usdt())];
    let totals = StratTotals::of(&all, &[0], false);
    for col in [&COL_PROFIT_PCT, &COL_AVG_ORDER, &COL_PF] {
        assert_eq!(totals.cell(col), None, "{}", col.key);
    }
}

/// The width bound: no subset of the rows produces a total cell longer than the column is
/// measured for — the rows' own texts plus [`widest_totals`], exactly what `metric_col_widths`
/// measures. Checked over every subset of a small mixed set under both lenses, so a bound that
/// forgot the sign, the magnitude of the losses, a currency's own precision, the trimmed
/// decimals of the crypto and percent formatters (`5.5 + 4.55` prints longer than `10.5`) or a
/// funding-only row's money in the pooled average fails here.
#[test]
fn the_width_bound_is_at_least_as_long_as_any_total() {
    let btc = QuoteScope::Single(QuoteCurrency::btc());
    let all = vec![
        row(400, 200, 812.4, 1.0, -1.0, usdt()),
        row(300, 100, -905.75, 1.0, -1.0, usdt()),
        row(2, 1, 5.5, 3.0, -1.0, btc),
        row(3, 2, 4.55, 2.0, -1.0, btc),
        row(1, 0, -0.45, -0.45, -0.45, btc),
        row(1, 1, 1.0, 1.0, 1.0, usdt()),
        row(0, 0, 300.0, 0.0, 0.0, usdt()),
        row(5, 1, 3.0, 1.0, -1.0, QuoteScope::Mixed),
    ];
    for (percent, unit) in [(false, None), (true, Some(ProfitUnit::Percent))] {
        let _lens = Lens::set(unit);
        let bounds = widest_totals(&all);
        let widest = |col: &super::MetricCol| {
            all.iter()
                .chain(bounds.iter())
                .map(|g| (col.text)(g).chars().count())
                .max()
                .unwrap_or(0)
        };
        for mask in 1u32..(1 << all.len()) {
            let idx: Vec<usize> = (0..all.len()).filter(|i| mask & (1 << i) != 0).collect();
            let totals = StratTotals::of(&all, &idx, percent);
            for col in METRIC_COLS {
                if let Some(text) = text(&totals, col) {
                    assert!(
                        text.chars().count() <= widest(col),
                        "{} total {text:?} wider than its bound (rows {idx:?}, percent {percent})",
                        col.key
                    );
                }
            }
        }
    }
}

/// Sets the thread's profit lens for the formatters and puts the quote lens back on drop, so a
/// failed assertion cannot leak the percent lens into the next test on the same thread.
struct Lens;

impl Lens {
    fn set(unit: Option<ProfitUnit>) -> Self {
        set_pnl_unit(unit);
        Lens
    }
}

impl Drop for Lens {
    fn drop(&mut self) {
        set_pnl_unit(None);
    }
}
