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

/// A budget of `bytes` of its own, for a test.
fn budget(bytes: usize) -> &'static Budget {
    Box::leak(Box::new(Budget::new(bytes)))
}

/// Points pushed out by newer ones must replay, the newest stays, the budget is never passed;
/// a key heavier than a shard's share bypasses storage.
#[test]
fn eviction_and_large_keys_remain_bounded() {
    let mut params = vec![(EntryParams::Fact, ExitParams::default()); 8];
    let entry = ReplayCache::entry_bytes(&params);
    // Room for ten entries a shard's share.
    let limit = budget(entry * 10 * SHARDS);
    let cache = ReplayCache::within(limit);
    let calls = AtomicUsize::new(0);
    let replay = || {
        calls.fetch_add(1, Ordering::Relaxed);
        Replayed::default()
    };
    let points = 400;
    for i in 0..points {
        params[0].1.model.latency_ms = i as f64;
        cache.replay(&params, replay);
        assert!(limit.held() <= entry * 10 * SHARDS, "held {}", limit.held());
    }
    assert_eq!(calls.load(Ordering::Relaxed), points);
    // The newest point is still kept.
    cache.replay(&params, replay);
    assert_eq!(calls.load(Ordering::Relaxed), points);
    // 400 points do not fit in 160: asking all of them again replays some.
    for i in 0..points {
        params[0].1.model.latency_ms = i as f64;
        cache.replay(&params, replay);
    }
    assert!(calls.load(Ordering::Relaxed) > points);
    let oversized = vec![params[0].clone(); 10 * 9];
    assert!(ReplayCache::entry_bytes(&oversized) > entry * 10);
    let before = calls.load(Ordering::Relaxed);
    cache.replay(&oversized, replay);
    cache.replay(&oversized, replay);
    assert_eq!(calls.load(Ordering::Relaxed), before + 2);
}

/// Two caches drawing on one budget never hold more than it together, and a dropped cache
/// gives back everything it held.
#[test]
fn caches_share_one_budget_and_release_it_when_dropped() {
    let mut params = vec![(EntryParams::Fact, ExitParams::default()); 2];
    let entry = ReplayCache::entry_bytes(&params);
    let total = entry * 4 * SHARDS;
    let limit = budget(total);
    let first = ReplayCache::within(limit);
    let second = ReplayCache::within(limit);
    for i in 0..300 {
        params[0].1.model.latency_ms = i as f64;
        first.replay(&params, Replayed::default);
        second.replay(&params, Replayed::default);
        assert!(limit.held() <= total, "held {} of {total}", limit.held());
    }
    assert!(limit.held() > 0);
    drop(first);
    let left: usize = second.shards.iter().map(|s| s.lock().unwrap().bytes).sum();
    assert_eq!(limit.held(), left);
    drop(second);
    assert_eq!(limit.held(), 0);
}

/// Equal keys hash alike, a signed zero included; the hash reads every field.
#[test]
fn equal_keys_hash_alike_and_every_field_counts() {
    let mut zero = vec![(EntryParams::Fact, ExitParams::default())];
    zero[0].1.stop_loss_pct = 0.0;
    let mut negative_zero = zero.clone();
    negative_zero[0].1.stop_loss_pct = -0.0;
    assert_eq!(zero, negative_zero);
    assert_eq!(key_hash(&zero), key_hash(&negative_zero));
    let mut moved = zero.clone();
    moved[0].1.model.latency_ms += 1.0;
    assert_ne!(key_hash(&zero), key_hash(&moved));
}

/// A cache big enough for every point answers exactly what no cache answers, point by point.
#[test]
fn a_cache_that_keeps_everything_answers_what_no_cache_answers() {
    let deals: Vec<_> = (1..=6)
        .map(|uid| prepared(uid, 95.0 + uid as f64))
        .collect();
    let bases = Bases::of(&deals);
    let fill_cache = fills::FillCache::default();
    let kept = ReplayCache::default();
    let none = ReplayCache::within(budget(0));
    let mut seen = Vec::new();
    for round in 0..3 {
        for step in 0..20 {
            let params = vec![(
                EntryParams::Fact,
                ExitParams {
                    stop_loss_pct: -1.0 - f64::from(step) * 0.5,
                    sell_price_pct: 0.2 + f64::from(step % 4) * 0.3,
                    ..Default::default()
                },
            )];
            let fills = fill_cache.fills(&deals, &bases.of_deal, &params);
            let fold = || closing::replay(&deals, &bases.of_deal, &params, &fills);
            let a = kept.replay(&params, fold);
            let b = none.replay(&params, fold);
            assert_eq!(exact(&a), exact(&b), "round {round} step {step}");
            seen.push(exact(&a));
        }
    }
    assert_eq!(seen[..20], seen[20..40]);
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
