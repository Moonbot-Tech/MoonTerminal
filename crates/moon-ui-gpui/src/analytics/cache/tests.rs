//! Cache correctness probes independent of Analytics widgets and persistent user data.

use std::collections::HashMap;
use std::hint::black_box;
use std::mem::size_of;
use std::sync::Arc;
use std::time::{Duration, Instant};

use moon_core::db::analytics::{
    AnalyticsRead, CalendarRead, CoreSeries, DayPoint, GroupStat, PreviousPeriodBasis, Query,
    StrategyBase, Summary, UndatedCloses,
};
use moon_core::db::valuation::ValuationMode;
use moon_core::db::{
    CoreNames, OffsetSegment, ProfitMetric, ProfitScope, QuoteBreakdown, ReadFail, ReportAxis,
    SideFilter, ValuationCoverage,
};

use super::{
    AnalyticsCache, CacheKey, CacheRevisions, CachedCalendar, CachedRead, CachedResult,
    CachedScope, Entry,
};
use crate::analytics::refresh::{BusyRetryBudget, CatchUpOutcome, preserve_on_catch_up};

/// A stable resolved range allows revisions and scopes to vary independently in each probe.
fn query() -> Query {
    Query {
        from: 1_000,
        to: 2_000,
        ..Query::default()
    }
}

/// Payloads expose their identity through an independent period profit marker.
fn summary(marker: f64, name_bytes: usize) -> CachedResult {
    let mut data = Summary::default();
    data.cur.profit = marker;
    if name_bytes > 0 {
        data.cores.push((1, "x".repeat(name_bytes)));
    }
    CachedResult::Summary(Arc::new(CachedRead {
        data: CachedScope::Empty(Arc::new(data)),
        undated: Arc::new(UndatedCloses::default()),
        cores: Some(Arc::new(vec![(1, "CORE_A".to_string())])),
        cores_at: Some(Instant::now()),
    }))
}

/// Derive a complete Summary key without hiding its revisions behind a global fixture.
fn key(query: &Query) -> CacheKey {
    CacheKey::summary(query, None, revisions(), true)
}

/// Ordinary cache probes start with explicit certified stamps; Default remains fail-closed.
fn revisions() -> CacheRevisions {
    CacheRevisions {
        read_state: Some(0),
        valuation_attachment: Some(0),
        ..CacheRevisions::default()
    }
}

/// Read a marker and prove a cache lookup shares the original allocation.
fn scalar(result: &CachedResult) -> &Arc<Summary> {
    match result {
        CachedResult::Summary(read) => match &read.data {
            CachedScope::Comparable { data, .. } | CachedScope::Empty(data) => data,
            CachedScope::Split(_) => panic!("scalar fixture returned split money"),
        },
        _ => panic!("summary fixture returned another page"),
    }
}

/// Omitting any query operand from the key would serve figures for another visible scope.
#[test]
fn every_query_operand_prevents_a_false_hit() {
    let base = query();
    let mut cache = AnalyticsCache::default();
    assert!(cache.insert(key(&base), summary(17.0, 0)));
    let mut variants = Vec::new();
    let mut changed = base.clone();
    changed.from += 1;
    variants.push(changed);
    let mut changed = base.clone();
    changed.to += 1;
    variants.push(changed);
    let mut changed = base.clone();
    changed.axis = ReportAxis::from_measured(HashMap::new(), chrono_tz::Europe::Warsaw);
    variants.push(changed);
    let mut changed = base.clone();
    changed.previous_period_basis = PreviousPeriodBasis::Elapsed;
    variants.push(changed);
    let mut changed = base.clone();
    changed.cores = vec![moon_core::config::NO_MATCH_CORE_UID];
    variants.push(changed);
    let mut changed = base.clone();
    changed.side = SideFilter::Long;
    variants.push(changed);
    let mut changed = base.clone();
    changed.emulator = Some(false);
    variants.push(changed);
    let mut changed = base.clone();
    changed.strategies = vec![(11, Some(1))];
    variants.push(changed);
    let mut changed = base.clone();
    changed.strategy_name_mask = "EXAMPLE*".to_string();
    variants.push(changed);
    let mut changed = base.clone();
    changed.metric = ProfitMetric::Percent;
    variants.push(changed);
    let mut changed = base.clone();
    changed.valuation = ValuationMode::Current;
    variants.push(changed);
    let mut changed = base.clone();
    changed.prefer_usdt = true;
    variants.push(changed);
    let mut changed = base.clone();
    changed.core_names = CoreNames::from_pairs([(1, "RENAMED")]);
    variants.push(changed);
    for (operand, changed) in variants.iter().enumerate() {
        assert!(
            cache.lookup(&key(changed), true).is_none(),
            "query operand {operand} aliased cached data"
        );
    }
    let hit = cache.lookup(&key(&base), false).expect("unchanged query");
    assert_eq!(scalar(&hit.result).cur.profit, 17.0);
}

/// Each data publisher can independently invalidate rows while stale reuse remains explicit.
#[test]
fn revisions_are_independent_and_stale_is_opt_in() {
    let base = key(&query());
    let original = summary(23.0, 0);
    let mut cache = AnalyticsCache::default();
    assert!(cache.insert(base.clone(), original.clone()));
    for revisions in [
        CacheRevisions {
            reports: 1,
            ..revisions()
        },
        CacheRevisions {
            valuation: 1,
            ..revisions()
        },
        CacheRevisions {
            strategies: 1,
            ..revisions()
        },
    ] {
        let mut changed = base.clone();
        changed.revisions = revisions;
        assert!(cache.lookup(&changed, false).is_none());
        let hit = cache
            .lookup(&changed, true)
            .expect("explicit same-scope stale snapshot");
        assert!(!hit.fresh);
        assert!(Arc::ptr_eq(scalar(&original), scalar(&hit.result)));
    }
    assert!(cache.lookup(&base, false).expect("exact revision").fresh);
}

/// Reading cached data must preserve its original age across the exact freshness boundary.
#[test]
fn expiry_never_slides_or_relabels_an_old_snapshot_as_fresh() {
    let mut cache = AnalyticsCache::default();
    let key = key(&query());
    let inserted = Instant::now();
    assert!(cache.insert_at(key.clone(), summary(43.0, 0), inserted, 12_345));
    let before = cache
        .lookup_at(&key, false, inserted + Duration::from_secs(59))
        .expect("within freshness limit");
    assert!(before.fresh);
    assert_eq!(before.as_of, 12_345);
    assert!(
        cache
            .lookup_at(&key, false, inserted + Duration::from_secs(60))
            .is_none()
    );
    let expired = cache
        .lookup_at(&key, true, inserted + Duration::from_secs(60))
        .expect("explicit stale display");
    assert!(!expired.fresh);
    assert_eq!(expired.as_of, 12_345);
    assert!(Arc::ptr_eq(scalar(&before.result), scalar(&expired.result)));
    assert!(
        cache
            .lookup_at(&key, false, inserted + Duration::from_secs(119))
            .is_none(),
        "stale lookup cannot renew the TTL"
    );
}

/// SQL set permutations and replaced bootstrap lists must share the same computed result.
#[test]
fn set_order_and_discovery_bootstrap_do_not_trigger_duplicate_computations() {
    let mut cache = AnalyticsCache::default();
    let mut computations = 0;
    for cores in [vec![3, 1], vec![1, 3], vec![3, 1, 3], vec![1, 3]] {
        let mut query = query();
        query.cores = cores;
        query.strategies = if computations == 0 {
            vec![(12, Some(3)), (11, None)]
        } else {
            vec![(11, None), (12, Some(3)), (11, None)]
        };
        let key = key(&query);
        if cache.lookup(&key, false).is_none() {
            computations += 1;
            assert!(cache.insert(key.clone(), summary(47.0, 0)));
        }
        assert_eq!(
            scalar(
                &cache
                    .lookup(&key, false)
                    .expect("same semantic request")
                    .result
            )
            .cur
            .profit,
            47.0
        );
    }
    assert_eq!(
        computations, 1,
        "four equivalent requests must share one computed payload"
    );
    let mut bootstrap = query();
    bootstrap.cores = vec![1];
    let mut discovered = bootstrap.clone();
    discovered.cores = vec![3, 1, 2];
    let revisions = revisions();
    let a = CacheKey::summary(&bootstrap, Some(&[9, 8]), revisions, true);
    let b = CacheKey::summary(&discovered, Some(&[8, 9, 8]), revisions, true);
    assert_eq!(
        a, b,
        "discovery replaces bootstrap inclusions before reading rows"
    );
    assert_ne!(
        key(&bootstrap),
        key(&discovered),
        "without discovery explicit inclusion sets still matter"
    );
    let a = CacheKey::calendar(&bootstrap, Some(&bootstrap), false, Some(&[9]), revisions);
    let b = CacheKey::calendar(&discovered, Some(&discovered), false, Some(&[9]), revisions);
    assert_eq!(a, b, "comparison also resolves its replica core universe");
}

/// A delayed speculative result must never evict a newer publication for the same filters.
#[test]
fn late_completion_cannot_replace_a_more_recent_snapshot() {
    let mut cache = AnalyticsCache::default();
    let mut current = key(&query());
    current.revisions = CacheRevisions {
        reports: 4,
        valuation: 7,
        strategies: 2,
        ..revisions()
    };
    assert!(cache.insert(current.clone(), summary(41.0, 0)));
    let mut old = current.clone();
    old.revisions.valuation -= 1;
    assert!(!cache.insert(old, summary(37.0, 0)));
    assert_eq!(
        scalar(
            &cache
                .lookup(&current, false)
                .expect("latest snapshot retained")
                .result
        )
        .cur
        .profit,
        41.0
    );
}

/// Missing reader or attachment certification forbids reuse rather than merely marking it stale.
#[test]
fn uncertified_process_state_refuses_cache_admission_and_every_lookup() {
    let valid = key(&query());
    let mut cache = AnalyticsCache::default();
    assert!(cache.insert(valid.clone(), summary(53.0, 0)));
    for revisions in [
        CacheRevisions {
            read_state: None,
            ..revisions()
        },
        CacheRevisions {
            valuation_attachment: None,
            ..revisions()
        },
        CacheRevisions::default(),
    ] {
        let mut uncertified = valid.clone();
        uncertified.revisions = revisions;
        assert!(!uncertified.is_reusable());
        assert!(cache.lookup(&uncertified, false).is_none());
        assert!(cache.lookup(&uncertified, true).is_none());
        assert!(!cache.insert(uncertified, summary(59.0, 0)));
    }
    assert_eq!(
        scalar(
            &cache
                .lookup(&valid, false)
                .expect("valid snapshot was not overwritten")
                .result
        )
        .cur
        .profit,
        53.0
    );
}

/// A changed money-expression or attachment policy invalidates old numbers even for stale display.
#[test]
fn changed_read_certification_refuses_fresh_and_explicit_stale_reuse() {
    let original = key(&query());
    let mut cache = AnalyticsCache::default();
    assert!(cache.insert(original.clone(), summary(71.0, 0)));
    for revisions in [
        CacheRevisions {
            read_state: Some(1),
            ..revisions()
        },
        CacheRevisions {
            valuation_attachment: Some(1),
            ..revisions()
        },
    ] {
        let mut changed = original.clone();
        changed.revisions = revisions;
        assert!(!changed.same_read_state(&original));
        assert!(
            changed.is_reusable(),
            "the new policy is certified but differs from the old one"
        );
        assert!(cache.lookup(&changed, false).is_none());
        assert!(cache.lookup(&changed, true).is_none());
    }
    let mut newer_rows = original.clone();
    newer_rows.revisions.reports += 1;
    assert!(newer_rows.same_read_state(&original));
    assert!(
        !cache
            .lookup(&newer_rows, true)
            .expect("row-only revisions retain unit-safe stale display")
            .fresh
    );
}

/// A request begun before money knowledge or attachment recovery cannot overwrite certified data.
#[test]
fn older_read_knowledge_and_attachment_completions_cannot_replace_newer_admission() {
    let mut newest = key(&query());
    newest.revisions.read_state = Some(3);
    newest.revisions.valuation_attachment = Some(5);
    let mut cache = AnalyticsCache::default();
    assert!(cache.insert(newest.clone(), summary(61.0, 0)));
    for revisions in [
        CacheRevisions {
            read_state: Some(2),
            ..newest.revisions
        },
        CacheRevisions {
            valuation_attachment: Some(4),
            ..newest.revisions
        },
    ] {
        let mut old = newest.clone();
        old.revisions = revisions;
        assert!(!cache.insert(old, summary(67.0, 0)));
    }
    assert_eq!(
        scalar(
            &cache
                .lookup(&newest, false)
                .expect("newest certified snapshot")
                .result
        )
        .cur
        .profit,
        61.0
    );
}

/// Page type, comparison range and discovery policy each change a result's meaning.
#[test]
fn pages_comparison_and_replica_discovery_never_alias() {
    let query = query();
    let revisions = revisions();
    let mut cache = AnalyticsCache::default();
    assert!(cache.insert(key(&query), summary(29.0, 0)));
    for other in [
        CacheKey::summary(&query, None, revisions, false),
        CacheKey::summary(&query, Some(&[]), revisions, true),
        CacheKey::summary(&query, Some(&[1]), revisions, true),
        CacheKey::strategies(&query, None, revisions),
        CacheKey::calendar(&query, None, false, None, revisions),
    ] {
        assert!(cache.lookup(&other, true).is_none());
    }
    let calendar = CacheKey::calendar(&query, None, false, None, revisions);
    let mut previous = query.clone();
    previous.from = 1;
    previous.to = query.from;
    assert_ne!(
        calendar,
        CacheKey::calendar(&query, None, true, None, revisions)
    );
    assert_ne!(
        calendar,
        CacheKey::calendar(&query, Some(&previous), false, None, revisions)
    );
    assert!(!cache.insert(calendar, summary(31.0, 0)));
}

/// Serving a snapshot changes eviction order without broadening explicit refresh invalidation.
#[test]
fn hits_change_lru_order_and_refresh_only_removes_its_scope() {
    let mut cache = AnalyticsCache::new(2, usize::MAX);
    let a = query();
    let mut b = a.clone();
    b.from += 1;
    let mut c = a.clone();
    c.from += 2;
    assert!(cache.insert(key(&a), summary(1.0, 0)));
    assert!(cache.insert(key(&b), summary(2.0, 0)));
    assert!(cache.lookup(&key(&a), false).is_some());
    assert!(cache.insert(key(&c), summary(3.0, 0)));
    assert!(cache.lookup(&key(&b), false).is_none());
    assert_eq!(
        scalar(&cache.lookup(&key(&a), false).expect("recently used").result)
            .cur
            .profit,
        1.0
    );
    cache.remove_scope(&key(&a));
    assert!(cache.lookup(&key(&a), true).is_none());
    assert!(cache.lookup(&key(&c), false).is_some());
}

/// Memory bounds must include variable-length text rather than only counting payload objects.
#[test]
fn retained_text_is_charged_and_oversized_results_preserve_existing_entries() {
    let mut cache = AnalyticsCache::new(12, 8 * 1024);
    let a = query();
    let mut b = a.clone();
    b.from += 1;
    assert!(cache.insert(key(&a), summary(7.0, 0)));
    assert!(!cache.insert(key(&b), summary(8.0, 32 * 1024)));
    assert!(cache.lookup(&key(&a), false).is_some());
    assert!(cache.lookup(&key(&b), false).is_none());
    for index in 1..20 {
        let mut q = a.clone();
        q.from += index;
        cache.insert(key(&q), summary(index as f64, 1_024));
        assert!(cache.bytes <= 8 * 1024);
        assert!(cache.entries.len() <= 12);
    }
    assert!(
        cache.entries.len() < 12,
        "byte limit must evict before entry limit"
    );
}

/// Independent metadata failures cannot become successful cached pages or erase typed split data.
#[test]
fn partial_failures_and_not_ready_never_become_cached_successes() {
    for failure in 0..3 {
        let read = AnalyticsRead {
            data: if failure == 0 {
                Err(ReadFail::NotReady)
            } else {
                Ok(ProfitScope::Empty(Summary::default()))
            },
            undated: if failure == 1 {
                Err(ReadFail::NotReady)
            } else {
                Ok(UndatedCloses::default())
            },
            cores: Some(if failure == 2 {
                Err(ReadFail::NotReady)
            } else {
                Ok(Vec::new())
            }),
        };
        assert!(CachedRead::from_analytics(read).is_err());
    }
    let calendar = CalendarRead {
        period: Err(ReadFail::NotReady),
        undated: Ok(UndatedCloses::default()),
        cores: None,
    };
    assert!(CachedCalendar::from_calendar(calendar).is_err());
    let split = AnalyticsRead::<ProfitScope<Summary>> {
        data: Ok(ProfitScope::Split(Default::default())),
        undated: Ok(UndatedCloses::default()),
        cores: None,
    };
    let cached = CachedRead::from_analytics(split)
        .ok()
        .expect("successful split retains its typed result");
    assert!(matches!(cached.data, CachedScope::Split(_)));
}

/// Refreshing cells without fetching core metadata must not recertify an older selector snapshot.
#[test]
fn metadata_observation_time_exists_only_for_cores_read_by_this_request() {
    let observed = Instant::now();
    for cores_included in [false, true] {
        let read = AnalyticsRead {
            data: Ok(ProfitScope::Empty(Summary::default())),
            undated: Ok(UndatedCloses::default()),
            cores: cores_included.then(|| Ok(vec![(1, "CORE_A".to_string())])),
        };
        let cached = CachedRead::from_analytics_at(read, observed)
            .ok()
            .expect("successful summary");
        assert_eq!(cached.cores_at, cores_included.then_some(observed));
        assert_eq!(cached.cores.is_some(), cores_included);
        let read = CalendarRead {
            period: Ok(ProfitScope::Empty(Default::default())),
            undated: Ok(UndatedCloses::default()),
            cores: cores_included.then(|| Ok(vec![(1, "CORE_A".to_string())])),
        };
        let cached = CachedCalendar::from_calendar_at(read, observed + Duration::from_secs(1))
            .ok()
            .expect("successful calendar");
        assert_eq!(
            cached.cores_at,
            cores_included.then_some(observed + Duration::from_secs(1))
        );
        assert_eq!(cached.cores.is_some(), cores_included);
    }
}

/// Valuation-in-progress must return to the bounded catch-up path instead of becoming a cache hit.
#[test]
fn transient_split_preserves_original_reads_and_only_a_scheduled_correction_keeps_snapshot() {
    let pending = QuoteBreakdown {
        orders: 5,
        valuation: Some(ValuationCoverage {
            eligible_orders: 5,
            valued_orders: 3,
            unavailable_orders: 1,
            ..Default::default()
        }),
        ..Default::default()
    };
    let raw = AnalyticsRead::<ProfitScope<Summary>> {
        data: Ok(ProfitScope::Split(pending.clone())),
        undated: Ok(UndatedCloses::default()),
        cores: Some(Ok(vec![(7, "CORE_B".to_string())])),
    };
    let returned = CachedRead::from_analytics(raw)
        .err()
        .expect("pending valuation must bypass cache");
    assert_eq!(
        returned.data.as_ref().ok().and_then(ProfitScope::split),
        Some(&pending)
    );
    assert_eq!(
        returned
            .cores
            .as_ref()
            .and_then(|read| read.as_ref().ok())
            .map(|cores| cores.as_slice()),
        Some([(7, "CORE_B".to_string())].as_slice())
    );
    let outcome = CatchUpOutcome::of_scope(&returned.data);
    let mut retries = BusyRetryBudget::default();
    for _ in 0..3 {
        assert!(preserve_on_catch_up(
            true,
            outcome,
            retries.has_allowance(),
            false
        ));
        assert!(retries.claim());
    }
    assert!(
        !preserve_on_catch_up(true, outcome, retries.has_allowance(), false),
        "exhausted retries must publish the actual outcome"
    );
    assert!(!retries.claim());
    assert!(
        preserve_on_catch_up(true, outcome, false, true),
        "a newer generation still supplies a correction path"
    );
    assert!(
        !preserve_on_catch_up(false, outcome, true, true),
        "another scope cannot inherit old figures"
    );

    let raw = CalendarRead {
        period: Ok(ProfitScope::Split(pending.clone())),
        undated: Ok(UndatedCloses::default()),
        cores: Some(Ok(vec![(7, "CORE_B".to_string())])),
    };
    let returned = CachedCalendar::from_calendar(raw)
        .err()
        .expect("calendar pending valuation must bypass cache");
    assert_eq!(
        returned.period.as_ref().ok().and_then(ProfitScope::split),
        Some(&pending)
    );
    assert!(CatchUpOutcome::of_scope(&returned.period).is_transient());
    assert_eq!(
        returned
            .cores
            .as_ref()
            .and_then(|read| read.as_ref().ok())
            .map(|cores| cores.as_slice()),
        Some([(7, "CORE_B".to_string())].as_slice())
    );
}

/// Unknown denominations and completely attempted unavailable valuations are durable safety results.
#[test]
fn durable_unknown_and_unavailable_splits_remain_cacheable_for_both_pages() {
    for (unknown_orders, unavailable_orders) in [(1, 1), (0, 2)] {
        let totals = QuoteBreakdown {
            orders: 5,
            unknown_orders,
            valuation: Some(ValuationCoverage {
                eligible_orders: 5,
                valued_orders: 3,
                unavailable_orders,
                ..Default::default()
            }),
            ..Default::default()
        };
        let raw = AnalyticsRead::<ProfitScope<Summary>> {
            data: Ok(ProfitScope::Split(totals.clone())),
            undated: Ok(UndatedCloses::default()),
            cores: None,
        };
        let cached = CachedRead::from_analytics(raw)
            .ok()
            .expect("durable summary split");
        match cached.data {
            CachedScope::Split(data) => assert_eq!(*data, totals),
            _ => panic!("durable split lost its unit-safe result"),
        }
        let raw = CalendarRead {
            period: Ok(ProfitScope::Split(totals.clone())),
            undated: Ok(UndatedCloses::default()),
            cores: None,
        };
        let cached = CachedCalendar::from_calendar(raw)
            .ok()
            .expect("durable calendar split");
        match cached.data {
            CachedScope::Split(data) => assert_eq!(*data, totals),
            _ => panic!("durable calendar split lost its unit-safe result"),
        }
    }
}

/// Build aggregate rows with neutral short names; no database, application, or user data is read.
fn publication_fixture(groups: usize, cores: usize, strategies: bool) -> (CacheKey, CachedResult) {
    let names: Vec<_> = (1..=cores as u64)
        .map(|uid| (uid, format!("CORE_{uid:03}")))
        .collect();
    let rows = |count: usize, coins: bool| {
        (0..count)
            .map(|index| GroupStat {
                key: format!("{index}@{}", index % cores + 1),
                name: format!("UNIT_{index:06}"),
                kind: if coins { String::new() } else { "Drops".into() },
                core: names[index % cores].1.clone(),
                lastedit: if coins {
                    String::new()
                } else {
                    "2026-01-01".into()
                },
                n: 10,
                profit: 2.0,
                ..GroupStat::default()
            })
            .collect()
    };
    let mut query = query();
    query.from = 100_000;
    query.cores = names.iter().map(|(uid, _)| *uid).collect();
    query.core_names = CoreNames::from_pairs(names.clone());
    query.axis = ReportAxis::from_measured(
        names
            .iter()
            .map(|(uid, _)| {
                (
                    *uid,
                    vec![
                        OffsetSegment {
                            from_utc: 1,
                            offset_secs: 3_600,
                        };
                        3
                    ],
                )
            })
            .collect(),
        chrono_tz::Europe::Warsaw,
    );
    let strategy_rows = rows(groups * 9 / 10, false);
    let coin_rows = rows(groups - groups * 9 / 10, true);
    let wrap = |data| CachedRead {
        data: CachedScope::Empty(Arc::new(data)),
        undated: Arc::new(UndatedCloses::default()),
        cores: Some(Arc::new(names.clone())),
        cores_at: Some(Instant::now()),
    };
    if strategies {
        let data = StrategyBase {
            strategies: strategy_rows,
            coins: coin_rows,
            ..StrategyBase::default()
        };
        let result = CachedResult::Strategies(Arc::new(wrap(data)));
        (CacheKey::strategies(&query, None, revisions()), result)
    } else {
        let data = Summary {
            strategies: strategy_rows,
            coins: coin_rows,
            cores: names.clone(),
            days: (0..90)
                .map(|day| DayPoint {
                    start: day * 86_400,
                    profit: 2.0,
                    trades: 10,
                })
                .collect(),
            core_days: names
                .iter()
                .map(|(uid, name)| CoreSeries {
                    uid: *uid,
                    name: name.clone(),
                    per_bucket: vec![2.0; 90],
                    per_bucket_trades: vec![10; 90],
                    ..CoreSeries::default()
                })
                .collect(),
            ..Summary::default()
        };
        let result = CachedResult::Summary(Arc::new(CachedRead {
            data: CachedScope::Empty(Arc::new(data)),
            undated: Arc::new(UndatedCloses::default()),
            cores: Some(Arc::new(names)),
            cores_at: Some(Instant::now()),
        }));
        (key(&query), result)
    }
}

/// Report nearest-rank p95 alongside the median without a performance pass/fail threshold.
fn publication_percentiles(samples: &mut [Duration]) -> (f64, f64) {
    samples.sort_unstable();
    let micros = |index: usize| samples[index].as_secs_f64() * 1_000_000.0;
    (
        micros(samples.len() / 2),
        micros((samples.len() * 95).div_ceil(100) - 1),
    )
}

/// Manual SYNTHETIC benchmark of current cache publication, in the shipped dev profile.
/// Run with vcvars64 loaded:
/// `cargo test -p moon-ui-gpui --bin moonterminal analytics::cache::tests::synthetic_publication_cost -- --exact --ignored --nocapture`
/// Fixtures and input clones are outside timers. Three arms rotate/reverse order over 42 rounds.
/// Insertion uses an uncontended local mutex and a full 12-entry LRU with shared payload owners;
/// an unlimited byte budget admits the 100k case even when the production 32 MiB cap would refuse.
/// Last-owner removal measures destruction separately (11 freshly built, untimed fixtures).
#[test]
#[ignore = "manual synthetic publication timings; no database or live application"]
fn synthetic_publication_cost() {
    for groups in [1_000, 10_000, 100_000] {
        for cores in [1, 200] {
            for strategies in [false, true] {
                let (key, payload) = publication_fixture(groups, cores, strategies);
                let payload_bytes = payload.estimated_bytes();
                let key_bytes = key.estimated_bytes();
                let charge = size_of::<Entry>() + key_bytes + payload_bytes + 256;
                let mut cache = AnalyticsCache::new(12, usize::MAX);
                for index in 0..12 {
                    let mut initial = key.clone();
                    initial.query.from += index;
                    assert!(cache.insert(initial, payload.clone()));
                }
                let cache = std::sync::Mutex::new(cache);
                let mut samples: [Vec<Duration>; 3] = std::array::from_fn(|_| Vec::new());
                for round in 0..42 {
                    let mut order = [0, 1, 2];
                    order.rotate_left(round % 3);
                    if round % 2 == 1 {
                        order.reverse();
                    }
                    for arm in order {
                        // Admission owns these inputs, but neither input cloning nor a last-owner
                        // payload destructor belongs to its timed shared-publication path.
                        let mut insert_key = key.clone();
                        insert_key.query.from += 100 + round as i64;
                        let insert_payload = payload.clone();
                        let started = Instant::now();
                        let measured = match arm {
                            0 => black_box(&payload).estimated_bytes(),
                            1 => black_box(&key).estimated_bytes(),
                            _ => {
                                let mut cache = cache.lock().expect("local benchmark mutex");
                                assert!(cache.insert(insert_key, insert_payload));
                                cache.bytes
                            }
                        };
                        samples[arm].push(started.elapsed());
                        assert_eq!(
                            black_box(measured),
                            [payload_bytes, key_bytes, 12 * charge][arm]
                        );
                    }
                }
                let mut destruction = Vec::new();
                for _ in 0..11 {
                    let (owned_key, owned_payload) = publication_fixture(groups, cores, strategies);
                    let mut owned_cache = AnalyticsCache::new(1, usize::MAX);
                    assert!(owned_cache.insert(owned_key.clone(), owned_payload));
                    let started = Instant::now();
                    owned_cache.remove_scope(black_box(&owned_key));
                    destruction.push(started.elapsed());
                    assert_eq!(owned_cache.bytes, 0);
                    assert!(owned_cache.entries.is_empty());
                }
                let timings: Vec<_> = samples
                    .iter_mut()
                    .map(|samples| publication_percentiles(samples))
                    .collect();
                println!(
                    "SYNTHETIC {} groups={groups} cores={cores} buckets=90 bytes={charge} default_fits={} us median/p95 payload={:.1}/{:.1} key={:.1}/{:.1} insert_shared={:.1}/{:.1} last_owner_remove={:.1}/{:.1}",
                    if strategies { "strategies" } else { "summary" },
                    charge <= 32 * 1024 * 1024,
                    timings[0].0,
                    timings[0].1,
                    timings[1].0,
                    timings[1].1,
                    timings[2].0,
                    timings[2].1,
                    publication_percentiles(&mut destruction).0,
                    publication_percentiles(&mut destruction).1,
                );
            }
        }
    }
}
