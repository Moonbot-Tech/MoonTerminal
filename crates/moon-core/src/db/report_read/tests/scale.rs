//! Scale probe of the Report reads at a heavy user's size: 10 cores, tens of thousands of closed
//! deals, several strategies and coins. Ignored: it prints timings, it asserts nothing about them.
//!
//! Run with `cargo test -p moon-core --lib scale_probe -- --ignored --nocapture`.
//! `MOON_SCALE_ROWS` overrides the deal count (default 20 000). All data is synthetic.

use std::time::Instant;

use super::super::{
    ReportFilter, RowScope, distinct_strategies, query_chart_trade_history_for_cores,
    query_reports, query_totals,
};
use super::seeks::{plans_of, replica, seed};

/// Median wall time of `iterations` runs of `read`, in milliseconds.
fn median_ms(iterations: usize, mut read: impl FnMut()) -> f64 {
    read();
    let mut samples: Vec<f64> = (0..iterations)
        .map(|_| {
            let started = Instant::now();
            read();
            started.elapsed().as_secs_f64() * 1000.0
        })
        .collect();
    samples.sort_by(f64::total_cmp);
    samples[samples.len() / 2]
}

/// Keep the full-scale read costs inspectable without imposing machine-dependent CI budgets.
#[test]
#[ignore = "scale probe: prints timings"]
fn scale_probe() {
    let rows: i64 = std::env::var("MOON_SCALE_ROWS")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(20_000);
    let conn = replica();
    seed(&conn, rows, 10, 60, 1_000, 8);
    // Chart records require valid prices; NULL prices benchmark discarded rows instead.
    conn.execute_batch("UPDATE orders_rep SET buyprice=10.0, sellprice=11.0, quantity=1.0;")
        .expect("seed chart prices");
    let all = ReportFilter {
        rows: RowScope::Closed,
        ..ReportFilter::default()
    };
    let iterations = 7;
    let report = |name: &str, ms: f64| println!("scale[{rows}] {name}: {ms:.2} ms");
    for (name, plans) in [
        (
            "totals",
            plans_of(&conn, || {
                query_totals(&conn, &all).unwrap();
            }),
        ),
        (
            "strategies",
            plans_of(&conn, || {
                distinct_strategies(&conn, &all).unwrap();
            }),
        ),
        (
            "closedate",
            plans_of(&conn, || {
                query_reports(&conn, &all, "closedate", true, 500).unwrap();
            }),
        ),
        (
            "profitbtc",
            plans_of(&conn, || {
                query_reports(&conn, &all, "profitbtc", true, 500).unwrap();
            }),
        ),
        (
            "coin",
            plans_of(&conn, || {
                query_reports(&conn, &all, "coin", true, 500).unwrap();
            }),
        ),
    ] {
        for (sql, plan) in plans {
            println!("plan[{rows}] {name}: {plan}; SQL: {sql}");
        }
    }

    report(
        "1 query_totals",
        median_ms(iterations, || {
            query_totals(&conn, &all).expect("totals");
        }),
    );
    report(
        "2 distinct_strategies",
        median_ms(iterations, || {
            distinct_strategies(&conn, &all).expect("strategies");
        }),
    );
    for sort in ["closedate", "profitbtc", "profitpct", "coin"] {
        report(
            &format!("3 query_reports sort={sort}"),
            median_ms(iterations, || {
                query_reports(&conn, &all, sort, true, 500).expect("rows");
            }),
        );
    }
    let cores: Vec<u64> = (1..=10).collect();
    for coins in [
        vec!["C7USDT".to_string()],
        vec!["C7USDT".to_string(), "C9USDT".to_string()],
        vec![
            "C7USDT".to_string(),
            "C9USDT".to_string(),
            "C11USDT".to_string(),
        ],
    ] {
        let history = query_chart_trade_history_for_cores(&conn, &cores, &coins, None, 1000)
            .expect("valid chart history");
        assert!(
            !history.records.is_empty(),
            "chart probe must parse real records"
        );
        for (sql, plan) in plans_of(&conn, || {
            query_chart_trade_history_for_cores(&conn, &cores, &coins, None, 1000).unwrap();
        }) {
            println!(
                "plan[{rows}] chart coins={}: {plan}; SQL: {sql}",
                coins.len()
            );
        }
        report(
            &format!("4 chart history coins={}", coins.len()),
            median_ms(iterations, || {
                query_chart_trade_history_for_cores(&conn, &cores, &coins, None, 1000)
                    .expect("chart history");
            }),
        );
    }
}
