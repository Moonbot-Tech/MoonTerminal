use super::holdout_part;
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
    assert!(!real.warn && real.short != whole && !real.short.is_empty());
}
