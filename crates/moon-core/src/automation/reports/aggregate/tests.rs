//! Grouping, having and ordering, on trades built the way the read builds them.

use super::*;

/// Regression target: grouping by core+id, which splits one strategy name across 15 cores.
#[test]
fn one_strategy_name_on_several_cores_is_one_group() {
    let mut a = Trade::sample(1, 7, "ACE", 1, 1.0);
    let mut b = Trade::sample(2, 99, "ACE", 2, -2.0);
    a.name = std::sync::Arc::from("MainShotL");
    b.name = std::sync::Arc::from("MainShotL");
    let keys = Keys {
        by: &[GroupBy::StrategyName],
        tz: chrono_tz::UTC,
        bucket: BucketSize::Day,
        bounds: None,
    };
    assert_eq!(keys.of(&a), keys.of(&b));
    let by_strategy = Keys {
        by: &[GroupBy::Strategy],
        ..keys
    };
    assert_ne!(by_strategy.of(&a), by_strategy.of(&b));
}

/// Groups failing `having` are dropped; the rest order by the exact metric, missing last.
#[test]
fn having_and_sort_use_exact_values() {
    let groups: Vec<(Vec<String>, Stats)> = [("A", -1.004), ("B", -1.006), ("C", 2.0)]
        .iter()
        .map(|(k, p)| {
            (
                vec![k.to_string()],
                stats::of([Trade::sample(1, 7, k, 1, *p)].iter()),
            )
        })
        .collect();
    let having = Having {
        metric: "profit_usdt_sum".into(),
        op: Op::Lt,
        value: 0.0,
    };
    let mut kept: Vec<_> = groups
        .into_iter()
        .filter(|(_, s)| {
            having
                .op
                .holds(s.value(&having.metric).unwrap(), having.value)
        })
        .collect();
    let sort = [Sort {
        by: "profit_usdt_sum".into(),
        dir: Dir::Asc,
    }];
    order(&mut kept, &sort, &[GroupBy::Coin]).unwrap();
    let order: Vec<_> = kept.iter().map(|(k, _)| k[0].as_str()).collect();
    // Both print as -1.0 after rounding; the exact values still decide.
    assert_eq!(order, ["B", "A"]);
    assert!(
        super::order(
            &mut kept,
            &[Sort {
                by: "nope".into(),
                dir: Dir::Asc
            }],
            &[]
        )
        .is_err()
    );
}

/// Hours and weekdays read in the requested zone, not in UTC.
#[test]
fn hour_of_day_follows_the_zone() {
    // 2026-11-02 23:30 UTC is Tuesday 01:30 in Kyiv.
    let trade = Trade::sample(1, 7, "A", 1_793_662_200_000, 1.0);
    let keys = Keys {
        by: &[GroupBy::HourOfDay, GroupBy::Weekday],
        tz: "Europe/Kyiv".parse().unwrap(),
        bucket: BucketSize::Day,
        bounds: None,
    };
    assert_eq!(keys.of(&trade), ["1", "2"]);
}

/// Quantile bounds split the values evenly; a trade without the value is its own group.
#[test]
fn entry_quantiles_split_evenly_and_keep_the_missing_apart() {
    let mut trades: Vec<Trade> = (1..=10)
        .map(|i| {
            let mut t = Trade::sample(1, 7, "A", i, 1.0);
            t.entry = vec![Some(i as f64)];
            t
        })
        .collect();
    let bounds = quantile_bounds(&trades, 2);
    assert_eq!(bounds, [5.0, 10.0]);
    trades[0].entry = vec![None];
    let keys = Keys {
        by: &[GroupBy::EntryQuantile],
        tz: chrono_tz::UTC,
        bucket: BucketSize::Day,
        bounds: Some(&bounds),
    };
    assert_eq!(keys.of(&trades[0]), ["none"]);
    assert_eq!(keys.of(&trades[4]), ["q1"]);
    assert_eq!(keys.of(&trades[9]), ["q2"]);
}

/// Regression target: comparing hours as text ("10" before "2") and printing "known" for a trade
/// without a holding time.
#[test]
fn numeric_groups_order_as_numbers_and_unknown_hold_prints_whole() {
    // Through the sort itself: hours 10, 2 and 0 ascending come back as 0, 2, 10.
    let mut groups: Vec<(Vec<String>, Stats)> = ["10", "2", "0"]
        .iter()
        .map(|h| (vec![h.to_string()], Stats::default()))
        .collect();
    let sort = [Sort {
        by: "hour_of_day".into(),
        dir: Dir::Asc,
    }];
    order(&mut groups, &sort, &[GroupBy::HourOfDay]).unwrap();
    let hours: Vec<_> = groups.iter().map(|(k, _)| k[0].as_str()).collect();
    assert_eq!(hours, ["0", "2", "10"]);
    // Names that look like numbers stay text, so the order is total: q10 after q2 only for
    // quantiles, where every key is an index.
    use std::cmp::Ordering;
    assert_eq!(key_order(GroupBy::StrategyName, "5", "15m"), "5".cmp("15m"));
    assert_eq!(
        key_order(GroupBy::EntryQuantile, "q2", "q10"),
        Ordering::Less
    );
    assert_eq!(
        key_order(GroupBy::EntryQuantile, "none", "q1"),
        Ordering::Less
    );
    assert_eq!(
        key_value(GroupBy::HoldBucket, hold_bucket(None)),
        json!("unknown")
    );
    assert_eq!(
        key_value(GroupBy::HoldBucket, hold_bucket(Some(5))),
        json!("under_10s")
    );
}
