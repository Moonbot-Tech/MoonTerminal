//! Lifecycle checks for coin-suggestion expiry and group-window teardown.

use std::time::{Duration, Instant};

use super::{SuggestCache, prune_expired_suggestions, remove_group_suggestions};
use crate::controls::coin_search::CoinSuggestEntry;

/// Build one suggestion entry with an independently controlled age and owner.
fn entry(
    group: &str,
    bucket: &str,
    age: Duration,
) -> (
    (String, Option<moon_core::config::ChartBucket>),
    CoinSuggestEntry,
) {
    (
        (
            group.to_string(),
            Some(moon_core::config::ChartBucket::Bundle(bucket.to_string())),
        ),
        CoinSuggestEntry {
            at: Instant::now().checked_sub(age).unwrap(),
            sig: Vec::new(),
            markets: vec![(1, "XALPHAUSDT".to_string())],
        },
    )
}

/// Expired rows leave on a cache touch, and a closed group releases even its fresh rows.
#[test]
fn suggestions_expire_on_touch_and_close_with_their_group() {
    let mut cache = SuggestCache::new();
    let (old_key, old) = entry("group-a", "old", Duration::from_secs(31));
    let (fresh_key, fresh) = entry("group-b", "fresh", Duration::ZERO);
    cache.insert(old_key, old);
    cache.insert(fresh_key, fresh);

    prune_expired_suggestions(&mut cache);
    assert_eq!(cache.len(), 1);
    assert!(cache.contains_key(&(
        "group-b".to_string(),
        Some(moon_core::config::ChartBucket::Bundle("fresh".to_string()))
    )));

    remove_group_suggestions(&mut cache, "group-b");
    assert!(cache.is_empty());
}

/// Measure a synthetic day of 100,000 expired buckets shrinking to the ten still fresh.
#[test]
#[ignore = "manual coin-suggestion retention scale measurement"]
fn bench_coin_suggestion_expiry_day() {
    let mut samples = Vec::new();
    for _ in 0..5 {
        let mut cache = SuggestCache::new();
        for uid in 0..100_000 {
            let (key, entry) = entry("group-a", &format!("old-{uid}"), Duration::from_secs(31));
            cache.insert(key, entry);
        }
        for uid in 0..10 {
            let (key, entry) = entry("group-a", &format!("fresh-{uid}"), Duration::ZERO);
            cache.insert(key, entry);
        }
        let before = cache.len();
        let start = Instant::now();
        prune_expired_suggestions(&mut cache);
        samples.push(start.elapsed().as_secs_f64() * 1_000.0);
        assert_eq!((before, cache.len()), (100_010, 10));
    }
    samples.sort_by(f64::total_cmp);
    println!(
        "[bench] coin suggestions retained=100010 -> 10 prune median={:.6} ms",
        samples[2]
    );
}
