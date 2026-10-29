//! Synthetic Report reader scale probes and frozen strategy-discovery parity.
//! Probes use 10 cores and tens of thousands of deals; timings carry no CI budget assertions.
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

/// Measure the dispatched reader paths on the same 100000-row, ten-core replica.
#[test]
#[ignore = "synthetic reader timing probe"]
fn reader_refresh_scale_probe() {
    use crate::db::{TotalsSlice, query_totals_sliced};
    let conn = replica();
    seed(&conn, 100_000, 10, 60, 1_000, 8);
    let base = ReportFilter {
        rows: RowScope::Closed,
        ..Default::default()
    };
    let slices = std::iter::once(TotalsSlice {
        core_uids: None,
        date_from: None,
        date_to: None,
    })
    .chain((0..31).map(|day| TotalsSlice {
        core_uids: None,
        date_from: Some(1_700_000_000 + day * 86_400),
        date_to: Some(1_700_000_000 + (day + 1) * 86_400 - 1),
    }))
    .collect::<Vec<_>>();
    for (name, ms) in [
        (
            "strategies",
            median_ms(5, || {
                std::hint::black_box(distinct_strategies(&conn, &base).unwrap());
            }),
        ),
        (
            "sliced_totals",
            median_ms(5, || {
                std::hint::black_box(query_totals_sliced(&conn, &base, &slices).unwrap());
            }),
        ),
    ] {
        println!("reader_refresh {name}: {ms:.3} ms");
    }
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

use super::super::*;

/// Frozen strategy-discovery statements before index-served identity probes.
fn former_distinct_strategies(
    conn: &Connection,
    filter: &ReportFilter,
) -> ReadResult<Vec<ReportStrategy>> {
    const CTX: &str = "reports: distinct_strategies";
    let mut scope = filter.clone();
    scope.strategies = None;
    scope.strategy_name_mask.clear();
    let has_strategy_names = crate::db::analytics::strategies_attached(conn);
    let mut names = std::collections::HashMap::new();
    if has_strategy_names {
        let mut stmt = conn
            .prepare("SELECT core_uid, strategy_id, name FROM strat.strategies")
            .map_err(|e| read_fail(CTX, e))?;
        let rows = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, i64>(0)? as u64,
                    row.get::<_, i64>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })
            .map_err(|e| read_fail(CTX, e))?;
        for row in rows {
            let (core_uid, strategy_id, name) = row.map_err(|e| read_fail(CTX, e))?;
            names.insert((core_uid, strategy_id), name);
        }
    }

    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for src in read_sources_res(conn)? {
        if !src.cols.contains("core_uid") || !src.cols.contains("strategyid") {
            continue;
        }
        let strategy_id =
            crate::db::analytics::effective_sid_expr("r", &src.cols, has_strategy_names);
        let (where_sql, params) = build_where(
            &scope,
            &src.cols,
            &StrategyMeta::without_mask(has_strategy_names),
        );
        // `COALESCE(r."strategyid",0)` never yields NULL, so `<> 0` / `= 0` partition every row
        // with no third case, and identity is set equality (this function sorts its whole output
        // in Rust below, so SQL row order never matters). When `strategy_id` is already the plain
        // column, both arms would be identical and the split would buy nothing — keep the single
        // statement in that case.
        //
        // `UNION`, not `UNION ALL`, and this is MEASURED rather than reasoned. The arms are
        // disjoint by construction and the caller below dedups every pair through `seen`, so
        // `UNION ALL` looks like the free choice — it is the opposite. `UNION` lets SQLite
        // dedup the compound ONCE, and its left arm then needs no `DISTINCT` of its own;
        // `UNION ALL` makes each arm materialise its own `USE TEMP B-TREE FOR DISTINCT`.
        // On the 600k-row measurement fixture: 2057 ms before this split, 1236 ms with `UNION`,
        // 6599 ms with `UNION ALL` — three times WORSE than doing nothing at all.
        //
        // ONE `build_where`, its parameters bound TWICE. Calling it a second time would re-read
        // the wall clock — `append_row_scope` resolves "does this window still reach the
        // present" against `now`, and its own doc says that must be ONE reading for the whole
        // predicate — so two calls can straddle a second boundary and scope the two arms to
        // genuinely different row states. Binding the same vector twice also keeps the bound
        // parameter count where it was instead of doubling it.
        let two_arms = strategy_id != "r.\"strategyid\"";
        let sql = if two_arms {
            format!(
                "SELECT DISTINCT r.core_uid, COALESCE(r.\"strategyid\",0) FROM {table} r{where_sql} AND COALESCE(r.\"strategyid\",0) <> 0 \
                 UNION \
                 SELECT DISTINCT r.core_uid, COALESCE({strategy_id}, 0) FROM {table} r{where_sql} AND COALESCE(r.\"strategyid\",0) = 0",
                table = src.table,
            )
        } else {
            format!(
                "SELECT DISTINCT r.core_uid, COALESCE({strategy_id}, 0) FROM {} r{where_sql}",
                src.table,
            )
        };
        let mut refs: Vec<&dyn rusqlite::types::ToSql> =
            params.iter().map(|value| value.as_ref()).collect();
        if two_arms {
            let second: Vec<&dyn rusqlite::types::ToSql> =
                params.iter().map(|value| value.as_ref()).collect();
            refs.extend(second);
        }
        let mut stmt = conn.prepare(&sql).map_err(|e| read_fail(CTX, e))?;
        let rows = stmt
            .query_map(refs.as_slice(), |row| {
                Ok((row.get::<_, i64>(0)? as u64, row.get::<_, i64>(1)?))
            })
            .map_err(|e| read_fail(CTX, e))?;
        for row in rows {
            let (core_uid, strategy_id) = row.map_err(|e| read_fail(CTX, e))?;
            if seen.insert((core_uid, strategy_id)) {
                out.push(ReportStrategy {
                    key: ReportStrategyKey {
                        core_uid,
                        strategy_id,
                    },
                    name: names
                        .get(&(core_uid, strategy_id))
                        .cloned()
                        .unwrap_or_else(|| strategy_id.to_string()),
                });
            }
        }
    }
    out.sort_by_cached_key(|strategy| {
        (
            strategy.name.to_lowercase(),
            strategy.key.core_uid,
            strategy.key.strategy_id,
        )
    });
    Ok(out)
}

/// Index-served identity discovery must offer exactly the same Manual, liquidation and deleted choices.
#[test]
fn strategy_discovery_matches_the_former_statements() {
    let conn = replica();
    seed(&conn, 1200, 10, 60, 19, 8);
    conn.execute_batch("ALTER TABLE orders_rep ADD COLUMN channelname TEXT;
        ALTER TABLE orders_rep ADD COLUMN signaltype TEXT;
        ALTER TABLE orders_rep ADD COLUMN comment TEXT;
        UPDATE strat.strategies SET name='UNIT_01' WHERE strategy_id=1;
        UPDATE orders_rep SET strategyid=0, channelname='LIQUIDATION', signaltype='UNIT_01 (Synthetic)' WHERE newrecid%13=0;
        UPDATE orders_rep SET strategyid=NULL WHERE newrecid%17=0;
        UPDATE orders_rep SET deleted=1 WHERE newrecid%23=0;
        CREATE TABLE closed_sell_reports AS SELECT *, newrecid AS id FROM orders_rep WHERE newrecid%3=0;
        CREATE INDEX legacy_identity ON closed_sell_reports(core_uid,strategyid,closedate);
        ANALYZE;").unwrap();
    for filter in [
        ReportFilter::default(),
        ReportFilter {
            rows: RowScope::Closed,
            ..Default::default()
        },
        ReportFilter {
            rows: RowScope::Open,
            ..Default::default()
        },
        ReportFilter {
            deleted_only: true,
            ..Default::default()
        },
        ReportFilter {
            core_uids: vec![1, 4],
            date_from: Some(1_700_000_000),
            date_to: Some(1_700_004_000),
            ..Default::default()
        },
        ReportFilter {
            coin: "C3USDT ".into(),
            ..Default::default()
        },
    ] {
        assert_eq!(
            distinct_strategies(&conn, &filter).unwrap(),
            former_distinct_strategies(&conn, &filter).unwrap(),
            "{filter:?}"
        );
    }
    let (_, plan) = plans_of(&conn, || {
        distinct_strategies(&conn, &ReportFilter::default()).unwrap();
    })
    .into_iter()
    .find(|(sql, _)| sql.contains("WITH identities") && sql.contains("FROM orders_rep"))
    .expect("capture the replica strategy-discovery statement");
    assert!(plan.contains("COVERING INDEX idx_rep_"), "{plan}");
    assert!(plan.contains("SEARCH r USING INDEX idx_rep_"), "{plan}");
    assert!(!plan.contains("TEMP B-TREE FOR DISTINCT"), "{plan}");
}

/// Measure liquidation discovery against the frozen two-arm query on the same ten-core replica.
#[test]
#[ignore = "synthetic reader timing probe"]
fn strategy_attribution_refresh_scale_probe() {
    let conn = replica();
    seed(&conn, 100_000, 10, 60, 1_000, 8);
    conn.execute_batch("ALTER TABLE orders_rep ADD COLUMN channelname TEXT;
        ALTER TABLE orders_rep ADD COLUMN signaltype TEXT;
        ALTER TABLE orders_rep ADD COLUMN comment TEXT;
        UPDATE strat.strategies SET name='UNIT_01' WHERE strategy_id=1;
        UPDATE orders_rep SET strategyid=0, channelname='LIQUIDATION', signaltype='UNIT_01 (Synthetic)' WHERE newrecid%997=0;
        UPDATE orders_rep SET strategyid=NULL WHERE newrecid%2111=0;
        ANALYZE;").unwrap();
    let filter = ReportFilter {
        rows: RowScope::Closed,
        ..Default::default()
    };
    assert_eq!(
        former_distinct_strategies(&conn, &filter).unwrap(),
        distinct_strategies(&conn, &filter).unwrap()
    );
    println!(
        "reader_refresh strategies attribution former: {:.3} ms",
        median_ms(5, || {
            std::hint::black_box(former_distinct_strategies(&conn, &filter).unwrap());
        })
    );
    println!(
        "reader_refresh strategies attribution current: {:.3} ms",
        median_ms(5, || {
            std::hint::black_box(distinct_strategies(&conn, &filter).unwrap());
        })
    );
}
