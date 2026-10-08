use std::time::Duration;

use super::{fetch_wait_text, holdout_part};
use moon_core::db::tuner::ticks::SearchResult;
use moon_core::db::tuner::ticks::search::MIN_HOLDOUT;

fn result(holdout_n: Option<i64>, holdout_open: usize) -> SearchResult {
    SearchResult {
        values: Vec::new(),
        searched: Vec::new(),
        train: Default::default(),
        holdout: holdout_n.map(|n| {
            let mut tally = moon_core::db::metrics::Tally::default();
            tally.n = n;
            tally
        }),
        holdout_open,
        holdout_open_profit: 0.0,
        fact_train: Default::default(),
        fact_holdout: None,
        holdout_loses: false,
        seed: 1,
        stats: Default::default(),
    }
}

/// Every search answer states its out-of-sample status: no holdout and a holdout too small to
/// check both warn as "no check", an open one warns, a real one is printed against the fact.
#[test]
fn every_answer_states_its_out_of_sample_status() {
    let _locale = crate::test_locale::force("en");
    let none = holdout_part(&result(None, 0));
    let small = holdout_part(&result(Some(MIN_HOLDOUT - 1), 0));
    // A 4-deal holdout the answer left a deal open in is still no check: the size speaks first.
    let small_open = holdout_part(&result(Some(1), 1));
    let open = holdout_part(&result(Some(MIN_HOLDOUT), 1));
    let real = holdout_part(&result(Some(MIN_HOLDOUT), 0));
    let whole = rust_i18n::t!("analytics.ticks.whole_period_short").to_string();
    for part in [&none, &small, &small_open] {
        assert!(part.warn);
        assert_eq!(part.short, whole);
    }
    assert!(open.warn && open.short != whole);
    // The open deals' tape-end estimate and their count follow the holdout's profit.
    let mut with_estimate = result(Some(MIN_HOLDOUT), 2);
    with_estimate.holdout_open_profit = 20.0;
    let estimate = holdout_part(&with_estimate);
    assert!(estimate.warn);
    assert!(estimate.short.contains("(+20 (2))"), "{}", estimate.short);
    assert!(!real.warn && real.short != whole && !real.short.is_empty());
}

/// The wait before the fill is shown only where the core filed a placement that precedes the
/// entry fill: an absent, zero or later placement gives no line.
#[test]
fn order_wait_needs_a_placement_before_the_fill() {
    use super::order_wait_ms;
    assert_eq!(order_wait_ms(10_000, Some(7_600)), Some(2_400));
    assert_eq!(order_wait_ms(10_000, None), None);
    assert_eq!(order_wait_ms(10_000, Some(0)), None);
    assert_eq!(order_wait_ms(10_000, Some(-5)), None);
    assert_eq!(order_wait_ms(10_000, Some(12_000)), None);
    assert_eq!(order_wait_ms(10_000, Some(10_000)), None);
}

/// Printing a 90s pause as minutes would hide the seconds the next try honours, and printing
/// a 10 minute pause as raw seconds would read as a stuck counter. The one-second floor of
/// the search estimate must not turn a deadline that has just passed into "1 s".
#[test]
fn a_venue_wait_stays_in_seconds_through_ninety_then_reads_as_minutes() {
    let _locale = crate::test_locale::force("en");
    assert_eq!(fetch_wait_text(Duration::from_secs(0)), "0 s");
    assert_eq!(fetch_wait_text(Duration::from_secs(30)), "30 s");
    assert_eq!(fetch_wait_text(Duration::from_secs(90)), "90 s");
    assert_eq!(fetch_wait_text(Duration::from_secs(91)), "2 min");
    assert_eq!(fetch_wait_text(Duration::from_secs(600)), "10 min");
    assert_eq!(
        fetch_wait_text(Duration::from_secs(3 * 3600 + 5 * 60)),
        "3 h 5 min"
    );
    assert_eq!(fetch_wait_text(Duration::from_millis(500)), "1 s");
}
