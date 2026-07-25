use super::*;
use rusqlite::Connection;

/// In-memory `orders_rep` with closedate, core_uid, and profitbtc; `unified_from` builds
/// its branch from the columns that EXIST.
fn seed(rows: &[(i64, i64, f64)]) -> Connection {
    let c = Connection::open_in_memory().unwrap();
    c.execute_batch(
        "CREATE TABLE orders_rep(closedate INTEGER, core_uid INTEGER, profitbtc REAL);",
    )
    .unwrap();
    for (d, uid, p) in rows {
        c.execute(
            "INSERT INTO orders_rep(closedate, core_uid, profitbtc) VALUES (?1, ?2, ?3)",
            rusqlite::params![d, uid, p],
        )
        .unwrap();
    }
    c
}

/// `orders_rep` carrying `spentbtc` too, so the Percent metric can form `profit / spent`.
fn seed_spent(rows: &[(i64, i64, f64, f64)]) -> Connection {
    let c = Connection::open_in_memory().unwrap();
    c.execute_batch(
        "CREATE TABLE orders_rep(closedate INTEGER, core_uid INTEGER, profitbtc REAL, spentbtc REAL);",
    )
    .unwrap();
    for (d, uid, p, s) in rows {
        c.execute(
            "INSERT INTO orders_rep(closedate, core_uid, profitbtc, spentbtc) VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![d, uid, p, s],
        )
        .unwrap();
    }
    c
}

// 2021-01-01 00:00:00 UTC.
const D0: i64 = 1_609_459_200;

#[test]
fn buckets_by_utc_day_fills_gaps_and_counts_wins() {
    // Day 0: two profitable trades (+10, +5); day 1: empty; day 2: one loss (-3).
    let c = seed(&[
        (D0 + 3_600, 1, 10.0),
        (D0 + 7_200, 2, 5.0),
        (D0 + 2 * 86_400 + 100, 1, -3.0),
    ]);
    let q = Query {
        from: D0,
        to: D0 + 3 * 86_400,
        ..Default::default()
    };
    let days = calendar_cells_from(&c, &q).unwrap();
    // Dense range: exactly three days, including empty day 1.
    assert_eq!(
        days.iter().map(|d| d.start).collect::<Vec<_>>(),
        vec![D0, D0 + 86_400, D0 + 2 * 86_400]
    );
    assert_eq!((days[0].trades, days[0].wins, days[0].profit), (2, 2, 15.0));
    assert_eq!((days[1].trades, days[1].profit), (0, 0.0)); // The gap is filled.
    assert_eq!((days[2].trades, days[2].wins, days[2].profit), (1, 0, -3.0));
}

/// `calendar/mod.rs:calendar_cells_from` must return `Ok(empty)` for an initialized source with no
/// trades; treating it as a read failure leaves a new Calendar tab in a retry loop.
#[test]
fn empty_period_is_successful_empty() {
    let c = seed(&[]);
    let q = Query {
        from: D0,
        to: D0 + 86_400,
        ..Default::default()
    };
    // A schema with no trades yields a successful empty calendar, not an infinite fill.
    assert_eq!(calendar_cells_from(&c, &q).unwrap().len(), 0);
}

#[test]
fn respects_period_bounds_excluding_to() {
    // The day-3 trade is outside [from, to), while the trailing empty day 2 is present.
    let c = seed(&[(D0 + 100, 1, 7.0), (D0 + 3 * 86_400 + 100, 1, 99.0)]);
    let q = Query {
        from: D0,
        to: D0 + 3 * 86_400,
        ..Default::default()
    };
    let days = calendar_cells_from(&c, &q).unwrap();
    assert_eq!(days.len(), 3);
    assert_eq!(days[0].trades, 1);
    assert!(days.iter().all(|d| (d.profit - 99.0).abs() > 1e-9)); // Day 3 is excluded.
}

/// `calendar/mod.rs:calendar_period_from` must query the supplied comparison period on the same
/// connection; accidentally reusing the current query makes the Month KPI compare a month with
/// itself and report a false zero delta.
#[test]
fn calendar_period_keeps_current_and_comparison_scopes_distinct() {
    let c = seed(&[
        (D0 - 86_400 + 100, 1, -5.0),
        (D0 + 100, 1, 7.0),
        (D0 + 200, 1, 2.0),
    ]);
    let current = Query {
        from: D0,
        to: D0 + 86_400,
        ..Default::default()
    };
    let previous = Query {
        from: D0 - 86_400,
        to: D0,
        ..Default::default()
    };

    let period = calendar_period_from(&c, &current, Some(&previous), false).unwrap();

    assert_eq!(period.current.len(), 1);
    assert_eq!(
        (
            period.current[0].profit,
            period.current[0].trades,
            period.current[0].wins,
        ),
        (9.0, 2, 2)
    );
    assert_eq!(period.previous, Some((-5.0, 1, 0)));
}

#[test]
fn percent_metric_is_profit_over_spent() {
    // Same day, two trades: +10 on 200 spent = +5%, -3 on 60 spent = -5%.
    let c = seed_spent(&[(D0 + 3_600, 1, 10.0, 200.0), (D0 + 7_200, 1, -3.0, 60.0)]);
    let base = Query {
        from: D0,
        to: D0 + 86_400,
        ..Default::default()
    };
    // USDT (default): raw money, 10 - 3 = 7.
    let usd = calendar_cells_from(&c, &base).unwrap();
    assert!((usd[0].profit - 7.0).abs() < 1e-9, "usd={}", usd[0].profit);
    // Percent: each trade as profit/spent*100, summed: +5 + (-5) = 0.
    let pct = calendar_cells_from(
        &c,
        &Query {
            metric: crate::db::ProfitMetric::Percent,
            ..base.clone()
        },
    )
    .unwrap();
    assert!((pct[0].profit - 0.0).abs() < 1e-9, "pct={}", pct[0].profit);
    // Sign is preserved, so win/loss classification is unchanged by the metric.
    assert_eq!((pct[0].trades, pct[0].wins), (2, 1));
}

#[test]
fn percent_metric_excludes_zero_spent() {
    // A trade with no spent has no percent, so in percent mode it is EXCLUDED entirely — never
    // a divide-by-zero, and never a phantom zero-profit trade that would skew count/winrate.
    let c = seed_spent(&[(D0 + 3_600, 1, 10.0, 200.0), (D0 + 7_200, 1, 4.0, 0.0)]);
    let pct = calendar_cells_from(
        &c,
        &Query {
            from: D0,
            to: D0 + 86_400,
            metric: crate::db::ProfitMetric::Percent,
            ..Default::default()
        },
    )
    .unwrap();
    // Only the +5% trade survives: one trade, one win, +5% — the zero-spent row is gone from
    // COUNT and SUM alike, so the two agree.
    assert!((pct[0].profit - 5.0).abs() < 1e-9, "pct={}", pct[0].profit);
    assert_eq!((pct[0].trades, pct[0].wins), (1, 1));

    // In USDT mode the same zero-spent row is still counted (no spent filter there).
    let usd = calendar_cells_from(
        &c,
        &Query {
            from: D0,
            to: D0 + 86_400,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(usd[0].trades, 2);
}

#[test]
fn hour_profile_buckets_by_hour_of_day_across_days() {
    // Hour 1: +10 and -4 on day 0, plus +3 on day 1, aggregated by hour of day.
    // Hour 22: +7 on day 0. All other hours are empty.
    let c = seed(&[
        (D0 + 3_600 + 60, 1, 10.0),
        (D0 + 3_600 + 120, 1, -4.0),
        (D0 + 22 * 3_600, 1, 7.0),
        (D0 + 86_400 + 3_600, 1, 3.0),
    ]);
    let prof = hour_profile_one(&c, &Query::default(), D0, D0 + 3 * 86_400).unwrap();
    // Hour 1 combines both days: profit 10 - 4 + 3 = 9, three trades, two wins.
    assert_eq!((prof[1].trades, prof[1].wins), (3, 2));
    assert!(
        (prof[1].profit - 9.0).abs() < 1e-9,
        "profit={}",
        prof[1].profit
    );
    // Hour 22: one +7 trade.
    assert_eq!((prof[22].trades, prof[22].wins), (1, 1));
    assert!((prof[22].profit - 7.0).abs() < 1e-9);
    // An hour without trades is all zeroes in the dense 24-element array.
    assert_eq!((prof[0].trades, prof[0].profit), (0, 0.0));
    // A trade outside [from, to) does not enter the profile, so the fresh period is empty.
    let empty = hour_profile_one(&c, &Query::default(), D0 - 5 * 86_400, D0 - 4 * 86_400).unwrap();
    assert!(empty.iter().all(|h| h.trades == 0));
}
