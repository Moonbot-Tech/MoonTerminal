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

#[test]
fn empty_period_is_some_empty_not_none() {
    let c = seed(&[]);
    let q = Query {
        from: D0,
        to: D0 + 86_400,
        ..Default::default()
    };
    // A schema with no trades yields an empty calendar, NOT None or an infinite fill.
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
