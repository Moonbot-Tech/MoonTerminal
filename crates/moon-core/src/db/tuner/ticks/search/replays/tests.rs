//! Cache equivalence, collision, eviction and concurrent-miss regressions.

use super::*;
use crate::db::tuner::ticks::search::{Bases, closing, fills, tests::prepared};
use std::sync::atomic::{AtomicUsize, Ordering};

/// An independently replayed score, including the private cumulative peak via Debug.
fn exact(replayed: &Replayed) -> String {
    format!("{replayed:?}")
}

/// Raw point spelling must not split a built key, and a held exit edit must not reuse it.
#[test]
fn spelling_aliases_share_a_replay_but_held_parameters_do_not() {
    use super::super::{ModelSettings, Point, params_of};
    use std::collections::HashMap;

    let deals = vec![prepared(1, 101.0)];
    let bases = Bases::of(&deals);
    let defaults = HashMap::new();
    let cache = ReplayCache::default();
    let fills_cache = fills::FillCache::default();
    let calls = AtomicUsize::new(0);
    let mut held = HashMap::new();
    for (spelling, changed) in [("0.2", false), ("0.20", false), ("0.20", true)] {
        if changed {
            held.insert("SellDelay".into(), "2000".into());
        }
        let point: Point = [("SellPrice", spelling.into())].into();
        let params = vec![params_of(
            &deals[0].own,
            &held,
            &defaults,
            &point,
            "PumpsDetection",
            ModelSettings::default(),
        )];
        let fills = fills_cache.fills(&deals, &bases.of_deal, &params);
        let expected = closing::replay(&deals, &bases.of_deal, &params, &fills);
        let actual = cache.replay(&params, || {
            calls.fetch_add(1, Ordering::Relaxed);
            closing::replay(&deals, &bases.of_deal, &params, &fills)
        });
        assert_eq!(exact(&actual), exact(&expected));
    }
    assert_eq!(calls.load(Ordering::Relaxed), 2);
}

/// Dropping either half of a built key, or returning only closed deals, changes this oracle.
#[test]
fn built_keys_preserve_closed_open_and_colliding_replays() {
    let mut deals = vec![prepared(1, 101.0), prepared(2, 90.0)];
    std::sync::Arc::make_mut(&mut deals[1].own).insert("SellPrice".into(), "0.5".into());
    let bases = Bases::of(&deals);
    let cache = ReplayCache::default();
    let fill_cache = fills::FillCache::default();
    let mut params = vec![
        (
            EntryParams::Fact,
            ExitParams {
                stop_loss_pct: -5.0,
                ..Default::default()
            }
        );
        2
    ];
    let calls = AtomicUsize::new(0);
    let check = |params: &[(EntryParams, ExitParams)]| {
        let fills = fill_cache.fills(&deals, &bases.of_deal, params);
        let expected = closing::replay(&deals, &bases.of_deal, params, &fills);
        for _ in 0..2 {
            let actual = cache.replay(params, || {
                calls.fetch_add(1, Ordering::Relaxed);
                closing::replay(&deals, &bases.of_deal, params, &fills)
            });
            assert_eq!(exact(&actual), exact(&expected));
        }
    };
    check(&params);
    // Same fingerprint, different model: the whole exit key must distinguish them.
    params[0].1.model.latency_ms = 1_500.0;
    check(&params);
    params[0].1.sell_price_pct = 5.0;
    check(&params);
    // A different entry must not inherit the fact's fills or closed tally.
    params[0].0 = EntryParams::MoonShot(super::super::MshotParams {
        price_pct: 5.0,
        ..Default::default()
    });
    check(&params);
    // The second base participates in the key even when the first did not move.
    params[1].1.model.latency_ms = 1_500.0;
    check(&params);
    assert_eq!(calls.load(Ordering::Relaxed), 5);
}

/// An entry pushed out by newer points must replay; exceeding the base budget bypasses storage.
#[test]
fn eviction_and_large_keys_remain_bounded() {
    let cache = ReplayCache::default();
    let mut params = vec![(EntryParams::Fact, ExitParams::default()); 8];
    let capacity = PARAMS_PER_SHARD / params.len();
    let calls = AtomicUsize::new(0);
    let replay = || {
        calls.fetch_add(1, Ordering::Relaxed);
        Replayed::default()
    };
    for i in 0..=capacity {
        params[0].1.model.latency_ms = i as f64;
        cache.replay(&params, replay);
    }
    cache.replay(&params, replay);
    assert_eq!(calls.load(Ordering::Relaxed), capacity + 1);
    params[0].1.model.latency_ms = 0.0;
    cache.replay(&params, replay);
    assert_eq!(calls.load(Ordering::Relaxed), capacity + 2);
    let oversized = vec![params[0].clone(); PARAMS_PER_SHARD + 1];
    cache.replay(&oversized, replay);
    cache.replay(&oversized, replay);
    assert_eq!(calls.load(Ordering::Relaxed), capacity + 4);
}

/// Parallel misses must neither block nested replays nor replace another point's score.
#[test]
fn concurrent_misses_keep_the_same_fold() {
    let deals: Vec<_> = (1..=8).map(|uid| prepared(uid, 101.0)).collect();
    let bases = Bases::of(&deals);
    let params = vec![(EntryParams::Fact, ExitParams::default())];
    let fills = fills::FillCache::default().fills(&deals, &bases.of_deal, &params);
    let expected = closing::replay(&deals, &bases.of_deal, &params, &fills);
    let cache = ReplayCache::default();
    super::super::install(|| {
        use rayon::prelude::*;
        (0..100).into_par_iter().for_each(|_| {
            let actual = cache.replay(&params, || {
                closing::replay(&deals, &bases.of_deal, &params, &fills)
            });
            assert_eq!(exact(&actual), exact(&expected));
        });
    });
}
