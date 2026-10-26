//! Synthetic timing and exact search-answer regressions, with no replica or user data.

use super::*;
use std::time::Instant;

/// A cache key or skipped per-request accounting that changes the answer breaks this frozen
/// unmodified-search oracle. Timing is printed, never asserted.
#[test]
fn repeated_point_search_preserves_the_answer() {
    let deals: Vec<_> = (1..=80)
        .map(|uid| {
            let mut d = prepared(uid * 60, 101.0);
            d.deal.coin = "SYNTHETIC".into();
            let t0 = d.deal.buy_ms;
            d.ticks = (0..2_000)
                .map(|i| tick(t0 - 500 + i, if i < 800 { 100.0 } else { 101.0 }))
                .collect::<Vec<_>>()
                .into();
            d
        })
        .collect();
    let held = HashMap::new();
    let defaults = HashMap::new();
    let mut locked: HashSet<String> = TICK_PARAMS.iter().map(|f| f.key.to_string()).collect();
    for key in ["SellPrice", "StopLoss", "PriceDownTimer"] {
        locked.remove(key);
    }
    let params = SearchParams {
        held: &held,
        defaults: &defaults,
        kind: "PumpsDetection",
        vary_entry: false,
        vary_exit: true,
        locked: &locked,
        grids: test_grids::legacy(),
        restarts: 500,
        min_n: Some(20),
        seed: Some(7),
        train_frac: 0.7,
        max_passes: DEFAULT_MAX_PASSES,
        keep_corridor: true,
        risk: Default::default(),
        screen_entry: false,
        model: ModelSettings {
            latency_ms: 0.0,
            ..Default::default()
        },
    };
    let started = Instant::now();
    let found = suggest(&deals, &params, &SearchHandle::new()).expect("synthetic answer");
    eprintln!(
        "synthetic search: {} ms; {found:?}",
        started.elapsed().as_millis()
    );
    // Frozen from the unmodified search: the base's off-grid stop is pinned at -15.
    assert_eq!(
        found.values,
        vec![
            ("SellPrice".into(), "1".into()),
            ("StopLoss".into(), "-15".into())
        ]
    );
    assert_eq!(found.searched, ["PriceDownTimer", "SellPrice", "StopLoss"]);
    assert_eq!(
        format!("{:?}", found.train),
        "Tally { n: 56, wins: 56, profit: 560.0, wsum: 560.0, lsum: 0.0, max_dd: 0.0, peak: 560.0 }"
    );
    assert_eq!(
        format!("{:?}", found.holdout),
        "Some(Tally { n: 24, wins: 24, profit: 240.0, wsum: 240.0, lsum: 0.0, max_dd: 0.0, peak: 240.0 })"
    );
    assert_eq!(
        format!("{:?}", found.fact_train),
        "Tally { n: 56, wins: 56, profit: 112.0, wsum: 112.0, lsum: 0.0, max_dd: 0.0, peak: 112.0 }"
    );
    assert_eq!(
        format!("{:?}", found.fact_holdout),
        "Some(Tally { n: 24, wins: 24, profit: 48.0, wsum: 48.0, lsum: 0.0, max_dd: 0.0, peak: 48.0 })"
    );
    assert_eq!(found.holdout_open, 0);
    assert_eq!(found.holdout_open_profit.to_bits(), 0.0f64.to_bits());
    assert!(!found.holdout_loses);
    assert_eq!(found.seed, 7);
    assert_eq!(
        found.stats,
        SearchStats {
            requested: 500,
            stopped: false,
            restarts: 500,
            best_restart: 0,
            passes: 2,
            converged: true,
            distinct: 39,
            evaluations: 52_210,
            refused: 0,
            left_open: 0,
            left_open_by_held: 0,
            entry_points: 0,
            fills_reused: 52_209,
            cornered: 0,
            unjudged_refused: 0,
        }
    );
    let bases = Bases::of(&deals);
    let point = point_of(&found.values);
    let started = Instant::now();
    for _ in 0..100 {
        std::hint::black_box(bases.params(&held, &defaults, &point, params.kind, params.model));
    }
    let build_us = started.elapsed().as_micros();
    let built = bases.params(&held, &defaults, &point, params.kind, params.model);
    let fills = fills::FillCache::default().fills(&deals, &bases.of_deal, &built);
    let started = Instant::now();
    install(|| {
        for _ in 0..100 {
            std::hint::black_box(closing::replay(&deals, &bases.of_deal, &built, &fills));
        }
    });
    eprintln!(
        "100 point builds: {build_us} us; 100 exit replays: {} us",
        started.elapsed().as_micros()
    );
}
