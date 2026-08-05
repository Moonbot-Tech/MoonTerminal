//! Calendar aggregation and quote-scope regression tests.

use super::*;
use rusqlite::Connection;

/// In-memory `orders_rep` with closedate, core_uid, and profitbtc; `unified_from` builds
/// its branch from the columns that EXIST.
///
/// Args:
///     rows: Close time, core id, and raw profit fixtures.
///
/// Returns:
///     Seeded USDT report connection.
fn seed(rows: &[(i64, i64, f64)]) -> Connection {
    let c = Connection::open_in_memory().unwrap();
    c.execute_batch(
        "CREATE TABLE orders_rep(
            closedate INTEGER, core_uid INTEGER, profitbtc REAL, basecurrency INTEGER
         );",
    )
    .unwrap();
    for (d, uid, p) in rows {
        c.execute(
            "INSERT INTO orders_rep(closedate, core_uid, profitbtc, basecurrency)
             VALUES (?1, ?2, ?3, 1)",
            rusqlite::params![d, uid, p],
        )
        .unwrap();
    }
    c
}

/// `orders_rep` carrying `spentbtc` too, so the Percent metric can form `profit / spent`.
///
/// Args:
///     rows: Close time, core id, raw profit, and spend fixtures.
///
/// Returns:
///     Seeded USDT report connection.
fn seed_spent(rows: &[(i64, i64, f64, f64)]) -> Connection {
    let c = Connection::open_in_memory().unwrap();
    c.execute_batch(
        "CREATE TABLE orders_rep(
            closedate INTEGER, core_uid INTEGER, profitbtc REAL, spentbtc REAL,
            basecurrency INTEGER
         );",
    )
    .unwrap();
    for (d, uid, p, s) in rows {
        c.execute(
            "INSERT INTO orders_rep(
                closedate, core_uid, profitbtc, spentbtc, basecurrency
             ) VALUES (?1, ?2, ?3, ?4, 1)",
            rusqlite::params![d, uid, p, s],
        )
        .unwrap();
    }
    c
}

// 2021-01-01 00:00:00 UTC.
const D0: i64 = 1_609_459_200;

/// Removing dense UTC-day filling from `calendar_cells_from` would lose the empty middle day or
/// corrupt the independently asserted trade/win counts around it.
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
    let days = calendar_cells_from(&c, &q, ProjectionMode::Native).unwrap();
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
    assert_eq!(
        calendar_cells_from(&c, &q, ProjectionMode::Native)
            .unwrap()
            .len(),
        0
    );
}

/// Changing the period predicate from half-open to inclusive would admit the independently seeded
/// trade exactly beyond `to` and make the three-day result contain its 99-unit profit.
#[test]
fn respects_period_bounds_excluding_to() {
    // The day-3 trade is outside [from, to), while the trailing empty day 2 is present.
    let c = seed(&[(D0 + 100, 1, 7.0), (D0 + 3 * 86_400 + 100, 1, 99.0)]);
    let q = Query {
        from: D0,
        to: D0 + 3 * 86_400,
        ..Default::default()
    };
    let days = calendar_cells_from(&c, &q, ProjectionMode::Native).unwrap();
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

    let period_scope = calendar_period_from(&c, &current, Some(&previous), false).unwrap();
    let period = period_scope
        .data()
        .expect("single-quote Calendar periods are comparable");

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

/// Adding daily `GROUP BY` back to `calendar/mod.rs:calendar_total_from` makes `query_row` return
/// only the first seeded day and turns the visible previous-period KPI from `(6, 3, 2)` into
/// `(4, 1, 1)`.
#[test]
fn previous_period_total_aggregates_multiple_days_directly() {
    let conn = seed(&[
        (D0 - 3 * 86_400 + 100, 1, 4.0),
        (D0 - 2 * 86_400 + 100, 1, -3.0),
        (D0 - 86_400 + 100, 1, 5.0),
    ]);
    let previous = Query {
        from: D0 - 3 * 86_400,
        to: D0,
        ..Default::default()
    };

    assert_eq!(
        calendar_total_from(&conn, &previous, ProjectionMode::Native)
            .expect("direct previous-period total"),
        (6.0, 3, 2)
    );
}

/// Calendar raw money uses the same split boundary as Summary, without blocking Percent mode.
///
/// Removing the preflight in `calendar_period_from` exposes a scalar 15.0 across USDT and USDC;
/// applying the raw-money rule to Percent would incorrectly hide valid dimensionless returns.
#[test]
fn mixed_quote_calendar_splits_raw_money_but_keeps_percent() {
    let c = seed_spent(&[(D0 + 100, 1, 10.0, 100.0), (D0 + 200, 1, 5.0, 100.0)]);
    c.execute(
        "UPDATE orders_rep SET basecurrency = 8 WHERE closedate = ?1",
        [D0 + 200],
    )
    .expect("mark second row as USDC");
    let raw_query = Query {
        from: D0,
        to: D0 + 86_400,
        ..Default::default()
    };

    let raw = calendar_period_from(&c, &raw_query, None, false).expect("raw Calendar");
    let split = raw.split().expect("mixed Calendar money must be split");
    assert_eq!((split.orders, split.totals.len()), (2, 2));
    assert!(raw.data().is_none());

    let percent = calendar_period_from(
        &c,
        &Query {
            metric: crate::db::ProfitMetric::Percent,
            ..raw_query
        },
        None,
        false,
    )
    .expect("percent Calendar");
    assert!(matches!(
        percent,
        ProfitScope::Comparable {
            unit: ProfitUnit::Percent,
            ..
        }
    ));
}

/// Calendar comparison data is omitted when the previous period has another quote.
///
/// Reusing only the current period's quote decision in `calendar_period_from` makes this expose
/// the previous USDT aggregate beside the current USDC aggregate as though their delta were valid.
#[test]
fn calendar_omits_previous_when_quote_changes() {
    let c = seed(&[(D0 - 100, 1, 4.0), (D0 + 100, 1, 7.0)]);
    c.execute(
        "UPDATE orders_rep SET basecurrency = 8 WHERE closedate >= ?1",
        [D0],
    )
    .expect("mark current row as USDC");
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

    let scoped = calendar_period_from(&c, &current, Some(&previous), false).expect("Calendar");
    let period = scoped.data().expect("current USDC Calendar is comparable");
    assert!(
        period.previous.is_none(),
        "USDT previous must not compare with USDC"
    );
}

/// Replacing Percent projection with native profit in `calendar_cells_from` would yield 7 instead
/// of the independently calculated sum of +5% and -5% while changing neither win count nor rows.
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
    let usd = calendar_cells_from(&c, &base, ProjectionMode::Native).unwrap();
    assert!((usd[0].profit - 7.0).abs() < 1e-9, "usd={}", usd[0].profit);
    // Percent: each trade as profit/spent*100, summed: +5 + (-5) = 0.
    let pct = calendar_cells_from(
        &c,
        &Query {
            metric: crate::db::ProfitMetric::Percent,
            ..base.clone()
        },
        ProjectionMode::Percent,
    )
    .unwrap();
    assert!((pct[0].profit - 0.0).abs() < 1e-9, "pct={}", pct[0].profit);
    // Sign is preserved, so win/loss classification is unchanged by the metric.
    assert_eq!((pct[0].trades, pct[0].wins), (2, 1));
}

/// Percent aggregation excludes a zero-spent row instead of inventing a zero-percent trade.
///
/// Removing the positive-spent predicate from the Percent source makes the independent trade
/// count include the second fixture row and skews Calendar win rate.
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
        ProjectionMode::Percent,
    )
    .unwrap();
    // Only the +5% trade survives: one trade, one win, +5% — the zero-spent row is gone from
    // COUNT and SUM alike, so the two agree.
    assert!((pct[0].profit - 5.0).abs() < 1e-9, "pct={}", pct[0].profit);
    assert_eq!((pct[0].trades, pct[0].wins), (1, 1));

    // In raw quote mode the same zero-spent row is still counted (no spent filter there).
    let usd = calendar_cells_from(
        &c,
        &Query {
            from: D0,
            to: D0 + 86_400,
            ..Default::default()
        },
        ProjectionMode::Native,
    )
    .unwrap();
    assert_eq!(usd[0].trades, 2);
}

/// Changing `hour_profile_one` to group by complete calendar timestamp instead of UTC hour would
/// split the independently seeded hour-one trades across days and lose the expected aggregate.
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
