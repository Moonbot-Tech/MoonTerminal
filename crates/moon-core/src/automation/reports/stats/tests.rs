//! Metric rules: Funding, unvalued trades, zero results and the shared formulas.

use super::*;
use crate::automation::reports::trade::Trade;

/// Regression target: counting Funding as a trade, or dropping its money.
#[test]
fn funding_is_money_but_not_a_trade() {
    let mut funding = Trade::sample(1, 0, "ACE", 2, -0.5);
    funding.reason = Reason::Funding;
    let stats = of([Trade::sample(1, 7, "ACE", 1, 2.0), funding].iter());
    assert_eq!(stats.value("trades"), Some(1.0));
    assert_eq!(stats.value("winrate_pct"), Some(100.0));
    assert_eq!(stats.value("profit_usdt_sum"), Some(1.5));
    assert_eq!(stats.value("funding_usdt"), Some(-0.5));
}

/// Regression target: reading a trade without a USDT rate as zero, which invents a drawdown.
#[test]
fn an_unvalued_trade_is_counted_apart_and_withholds_drawdown() {
    let mut unvalued = Trade::sample(1, 7, "ACE", 2, -3.0);
    unvalued.profit_usdt = None;
    let stats = of([Trade::sample(1, 7, "ACE", 1, 2.0), unvalued].iter());
    assert_eq!(stats.value("trades"), Some(2.0));
    assert_eq!(stats.value("unvalued"), Some(1.0));
    assert_eq!(stats.value("profit_usdt_sum"), Some(2.0));
    assert_eq!(
        stats.value("losses"),
        Some(1.0),
        "the sign is read in native money"
    );
    assert_eq!(stats.value("max_drawdown_usdt"), None);
}

/// A zero result is a loss and profit factor without losses is 99, as in Analytics.
#[test]
fn zero_is_a_loss_and_no_losses_is_99() {
    let stats = of([
        Trade::sample(1, 7, "A", 1, 0.0),
        Trade::sample(1, 7, "A", 2, 1.0),
    ]
    .iter());
    assert_eq!(stats.value("winrate_pct"), Some(50.0));
    let winners = of([Trade::sample(1, 7, "A", 1, 1.0)].iter());
    assert_eq!(winners.value("profit_factor"), Some(99.0));
}

/// The drawdown and the losing run follow close order.
#[test]
fn drawdown_and_losing_run_follow_close_order() {
    let trades = [5.0, -2.0, -3.0, 4.0, -1.0].map(|p| p);
    let list: Vec<Trade> = trades
        .iter()
        .enumerate()
        .map(|(i, p)| Trade::sample(1, 7, "A", i as i64, *p))
        .collect();
    let stats = of(list.iter());
    assert_eq!(stats.value("max_drawdown_usdt"), Some(5.0));
    assert_eq!(stats.value("max_consecutive_losses"), Some(2.0));
    assert_eq!(stats.value("profit_usdt_median"), Some(-1.0));
}

/// Unknown metric names fail with the known list, so a client does not probe.
#[test]
fn an_unknown_metric_names_the_known_ones() {
    let error = metric_names(&["profit".into()]).unwrap_err().to_string();
    assert!(error.contains("profit_usdt_sum"));
    assert_eq!(metric_names(&[]).unwrap().len(), DEFAULT_METRICS.len());
}

/// Output rounding: USDT to cents, shares to three places; comparisons stay exact.
#[test]
fn output_rounds_by_unit() {
    let stats = of([Trade::sample(1, 7, "A", 1, 1.23456)].iter());
    assert_eq!(stats.output("profit_usdt_sum"), json!(1.23));
    assert_eq!(stats.value("profit_usdt_sum"), Some(1.23456));
}

/// Regression target: an unvalued Funding row summed as zero instead of counted.
#[test]
fn an_unvalued_funding_row_is_counted_not_summed() {
    let mut funding = Trade::sample(1, 0, "ACE", 2, -0.5);
    funding.reason = Reason::Funding;
    funding.profit_usdt = None;
    let stats = of([Trade::sample(1, 7, "ACE", 1, 2.0), funding].iter());
    assert_eq!(stats.value("unvalued"), Some(1.0));
    assert_eq!(stats.value("funding_usdt"), Some(0.0));
    assert_eq!(
        stats.value("max_drawdown_usdt"),
        Some(0.0),
        "Funding is not on the trade curve"
    );
}

/// A set that never lost has no largest loss, and one that never won has no largest win.
#[test]
fn extremes_exist_only_on_their_side() {
    let winners = of([Trade::sample(1, 7, "A", 1, 3.0)].iter());
    assert_eq!(winners.value("max_loss_usdt"), None);
    assert_eq!(winners.value("max_win_usdt"), Some(3.0));
    let losers = of([Trade::sample(1, 7, "A", 1, -3.0)].iter());
    assert_eq!(losers.value("max_win_usdt"), None);
    assert!(numeric_metric("native_profit").is_err());
}
