//! Filter-tuner query, snapshot, and predicate regression tests.

use std::sync::{Mutex, MutexGuard};

use rusqlite::trace::{TraceEvent, TraceEventCodes};

use super::*;

/// Serializes the process-global function-pointer trace sink used by rusqlite.
static TRACE_GUARD: Mutex<()> = Mutex::new(());
/// SQL statements executed while a tuner test connection has tracing enabled.
static TRACE_SQL: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// Record one statement reported by SQLite's execution trace.
fn record_sql(event: TraceEvent<'_>) {
    if let TraceEvent::Stmt(statement, _) = event {
        TRACE_SQL
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push(
                statement
                    .expanded_sql()
                    .unwrap_or_else(|| statement.sql().into_owned()),
            );
    }
}

/// Acquire the trace sink and clear statements from an earlier failed test.
fn start_trace() -> MutexGuard<'static, ()> {
    let guard = TRACE_GUARD
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    TRACE_SQL
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clear();
    guard
}

/// Raw tuner scans reject mixed quotes before optimization, while Percent scans remain valid.
///
/// Removing the guard from `tuner_source_on` lets threshold and variant optimizers rank a scalar
/// whose operands are USDT and USDC. Applying the same guard to Percent breaks the second branch.
#[test]
fn mixed_quote_tuner_rejects_raw_money_but_accepts_percent() {
    let conn = Connection::open_in_memory().expect("in-memory database");
    conn.execute_batch(
        "CREATE TABLE orders_rep(
            closedate INTEGER, core_uid INTEGER, profitbtc REAL, spentbtc REAL,
            basecurrency INTEGER
         );
         INSERT INTO orders_rep VALUES (100, 1, 10.0, 100.0, 1);
         INSERT INTO orders_rep VALUES (200, 1, 5.0, 100.0, 8);",
    )
    .expect("mixed quote fixture");
    let raw = Query {
        from: 1,
        to: 300,
        ..Default::default()
    };

    assert!(matches!(
        tuner_source_on(&conn, &raw),
        Err(ReadFail::IncomparableQuote)
    ));

    let percent = Query {
        metric: crate::db::ProfitMetric::Percent,
        ..raw
    };
    assert!(tuner_source_on(&conn, &percent).is_ok());
}

/// Quote preflight and tuner row materialization observe one WAL snapshot.
///
/// Replacing `tuner_read_on`'s `with_read_snapshot` call with a direct callback lets the second
/// query observe the USDC commit below after preflight accepted the original USDT-only scope.
#[test]
fn tuner_materialization_cannot_cross_the_quote_preflight_snapshot() {
    let path = super::super::test_support::temp_db("tuner-quote-snapshot");
    let writer = Connection::open(&path).expect("open writer");
    writer
        .execute_batch(
            "PRAGMA journal_mode=WAL;
             CREATE TABLE orders_rep(
                closedate INTEGER, core_uid INTEGER, profitbtc REAL, spentbtc REAL,
                basecurrency INTEGER
             );
             INSERT INTO orders_rep VALUES (100, 1, 10.0, 100.0, 1);",
        )
        .expect("seed WAL fixture");
    let reader = Connection::open(&path).expect("open reader");
    let query = Query {
        from: 1,
        to: 300,
        ..Default::default()
    };

    let rows = tuner_read_on(&reader, &query, |snapshot, query, source| {
        writer
            .execute("INSERT INTO orders_rep VALUES (200, 1, 5.0, 100.0, 8)", [])
            .expect("commit USDC row after quote preflight");
        snapshot
            .query_row(
                &format!("SELECT COUNT(*) FROM {source}"),
                rusqlite::params![query.from, query.to],
                |row| row.get::<_, i64>(0),
            )
            .map_err(|error| read_fail_on(snapshot, "test: tuner snapshot materialization", error))
    })
    .expect("materialize pinned tuner rows");

    assert_eq!(rows, 1);
    drop(reader);
    drop(writer);
    super::super::test_support::remove_db(&path);
}

/// Replacing `tuner/mod.rs:variant_stats_from_source` with one statement per variant must make
/// this parity check fail when any filtered subsequence or total ordering differs from the
/// independently evaluated reference matrix shown to the user.
#[test]
fn one_ordered_variant_statement_matches_independent_variant_scans() {
    let _trace = start_trace();
    let conn = Connection::open_in_memory().expect("in-memory database");
    conn.execute_batch(
        "CREATE TABLE trades(
            closedate INTEGER, buydate INTEGER, pnl REAL, spentbtc REAL, d1h REAL, coin TEXT,
            core_uid INTEGER
         );
         INSERT INTO trades VALUES (300, 100,  4.0, 40.0,  3.0, 'BTC', 1);
         INSERT INTO trades VALUES (100,  50, -5.0, 25.0, -2.0, 'ETH', 1);
         INSERT INTO trades VALUES (200, 150,  2.0, 20.0,  1.0, 'BTC', 1);
         INSERT INTO trades VALUES (200, 150, -1.0, 10.0,  4.0, 'SOL', 1);",
    )
    .expect("variant fixture");
    let query = Query {
        from: 1,
        to: 400,
        ..Default::default()
    };
    let source = "(SELECT * FROM trades WHERE closedate >= ?1 AND closedate < ?2) o";
    let variants = vec![
        Variant::default(),
        Variant {
            bounds: vec![Bound {
                field: "d1h".into(),
                from: Some(1.0),
                to: Some(3.0),
            }],
            ..Default::default()
        },
        Variant {
            coins_in: Some(vec!["BTC".into()]),
            ..Default::default()
        },
        Variant {
            tod: Some(TimeWindow::Hour(30, 50)),
            ..Default::default()
        },
    ];

    conn.trace_v2(TraceEventCodes::SQLITE_TRACE_STMT, Some(record_sql));
    let actual = variant_stats_from_source(&conn, &query, source, &variants);
    conn.trace_v2(TraceEventCodes::empty(), None);
    let actual = actual.expect("single ordered variant scan");
    let statements = TRACE_SQL
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone();
    assert_eq!(statements.len(), 1, "variant statements: {statements:#?}");
    let expected = vec![
        VarStats {
            n: 4,
            wins: 2,
            profit: 0.0,
            pf: 1.0,
            avg: 0.0,
            avg_win: 3.0,
            avg_loss: 3.0,
            avg_spent: 23.75,
            max_dd: 6.0,
        },
        VarStats {
            n: 2,
            wins: 2,
            profit: 6.0,
            pf: 99.0,
            avg: 3.0,
            avg_win: 3.0,
            avg_loss: 0.0,
            avg_spent: 30.0,
            max_dd: 0.0,
        },
        VarStats {
            n: 2,
            wins: 2,
            profit: 6.0,
            pf: 99.0,
            avg: 3.0,
            avg_win: 3.0,
            avg_loss: 0.0,
            avg_spent: 30.0,
            max_dd: 0.0,
        },
        VarStats::default(),
    ];

    assert_eq!(format!("{actual:?}"), format!("{expected:?}"));
    let sql = variant_stats_sql("trades o", &variants);
    assert_eq!(sql.matches("FROM trades o").count(), 1, "one source scan");
    assert_eq!(sql.matches("ORDER BY").count(), 1, "one shared sort");
    assert_eq!(sql.matches("CASE WHEN").count(), variants.len());
}

/// Replacing the source-consuming calls inside `tuner/mod.rs:coin_tuner_data` with
/// `coin_groups_on` or `variant_stats_on` must fail this routing contract because the Coin Tuner
/// would repeat quote preflight and unified-source construction before showing one refresh.
#[test]
fn coin_tuner_reuses_one_validated_source_for_table_and_kpi() {
    let source = include_str!("mod.rs");
    let body = source
        .split_once("pub fn coin_tuner_data")
        .expect("coin tuner entry point")
        .1
        .split_once("/// Build the unified tuner source")
        .expect("coin tuner function boundary")
        .0;

    assert_eq!(body.matches("tuner_source_on(").count(), 1);
    assert_eq!(body.matches("coin_groups_from_source(").count(), 1);
    assert_eq!(body.matches("variant_stats_from_source(").count(), 1);
    assert!(!body.contains("coin_groups_on("));
    assert!(!body.contains("variant_stats_on("));
}

/// Replacing either source-consuming call in `tuner/mod.rs:filter_tuner_data` with the public
/// standalone reader must fail this routing contract because one Filters refresh would repeat
/// quote preflight and unified-source construction for KPI and histogram.
#[test]
fn filter_tuner_reuses_one_validated_source_for_kpi_and_histogram() {
    let source = include_str!("mod.rs");
    let body = source
        .split_once("pub fn filter_tuner_data")
        .expect("filter tuner entry point")
        .1
        .split_once("/// Read every report-derived By-time result")
        .expect("filter tuner function boundary")
        .0;

    assert_eq!(body.matches("tuner_source_on(").count(), 1);
    assert_eq!(body.matches("variant_stats_from_source(").count(), 1);
    assert_eq!(body.matches("histogram_from_source(").count(), 1);
    assert!(!body.contains("variant_stats("));
    assert!(!body.contains("histogram("));
}

#[test]
fn variant_where_whitelists_fields() {
    let v = Variant {
        bounds: vec![
            Bound {
                field: "d1h".into(),
                from: Some(1.5),
                to: Some(10.0),
            },
            Bound {
                field: "evil\"; DROP TABLE x;--".into(),
                from: Some(1.0),
                to: None,
            },
            Bound {
                field: "hvol".into(),
                from: None,
                to: None,
            },
        ],
        ..Default::default()
    };
    let w = v.where_sql();
    assert!(w.contains("COALESCE(o.\"d1h\",0) >= 1.5"));
    assert!(w.contains("<= 10"));
    assert!(!w.contains("DROP"));
    assert!(!w.contains("hvol"), "пустые границы не добавляют условий");
}

/// The variant's only STRING condition: a coin name reaches SQL as a literal,
/// so its apostrophe must be doubled — otherwise one such coin breaks the whole
/// WHERE and "Fact vs v1" quietly scores a different set of trades.
#[test]
fn variant_coins_quote_is_escaped() {
    let v = Variant {
        coins_in: Some(vec!["BTC".into(), "O'BRIEN' OR 1=1--".into()]),
        ..Default::default()
    };
    let w = v.where_sql();
    assert!(w.contains("IN ('BTC','O''BRIEN'' OR 1=1--')"), "w={w}");
    assert!(!v.is_empty(), "a coin variant is not the fact");
    // No dangling quote: their count in the condition stays even.
    assert_eq!(w.matches('\'').count() % 2, 0, "unbalanced quote: {w}");
    assert!(
        Variant::default().where_sql().is_empty(),
        "an empty coin list adds no condition"
    );
    // The blacklist side EXCLUDES, and both sides may apply at once.
    let both = Variant {
        coins_in: Some(vec!["BTC".into()]),
        coins_out: vec!["ETH".into()],
        ..Default::default()
    };
    let w = both.where_sql();
    assert!(w.contains("IN ('BTC')"), "w={w}");
    assert!(w.contains("NOT IN ('ETH')"), "w={w}");

    // A whitelist that no traded coin satisfies keeps NOTHING — it must not quietly
    // degrade into "no whitelist", which would score the untouched fact as the plan.
    let unmatched = Variant {
        coins_in: Some(Vec::new()),
        ..Default::default()
    };
    assert!(unmatched.where_sql().contains("0=1"));
    assert!(
        !unmatched.is_empty(),
        "an unmatched whitelist is not the fact"
    );
}

#[test]
fn variant_week_span_predicate() {
    // Mon 00:00 -> Sat 23:59 (week minutes 0..8639): continuous -> BETWEEN, excluding Sun.
    let v = Variant {
        week_span: Some((0, 8639)),
        ..Default::default()
    };
    let w = v.where_sql();
    assert!(w.contains("o.__mt_week BETWEEN 0 AND 8639"), "w={w}");
    assert!(!v.is_empty(), "week_span-вариант не равен «Факту»");

    // Wrap Sun -> Mon (from > to): Sat 12:00 (8640-720=7920) -> Mon 12:00 (720).
    let v = Variant {
        week_span: Some((7920, 720)),
        ..Default::default()
    };
    let w = v.where_sql();
    assert!(
        w.contains("<= 720 OR"),
        "через воскресенье — до Пн 12:00: {w}"
    );
    assert!(
        w.contains(">= 7920"),
        "через воскресенье — от Сб 12:00: {w}"
    );
    assert!(!w.contains("BETWEEN"), "перевёрнутое окно не BETWEEN: {w}");
}

#[test]
fn variant_time_window_predicate() {
    // WorkingTime `Day` 09:00-21:00 -> minute-of-day BETWEEN.
    let v = Variant {
        tod: Some(TimeWindow::Day(9 * 60, 21 * 60)),
        ..Default::default()
    };
    let w = v.where_sql();
    assert!(w.contains("o.__mt_day BETWEEN 540 AND 1260"), "w={w}");
    assert!(!v.is_empty());

    // `Day` wrapping past midnight (22:00-06:00) -> `<= 360 OR >= 1320`.
    let v = Variant {
        tod: Some(TimeWindow::Day(22 * 60, 6 * 60)),
        ..Default::default()
    };
    let w = v.where_sql();
    assert!(w.contains("<= 360 OR"), "через полночь до 06:00: {w}");
    assert!(w.contains(">= 1320"), "через полночь от 22:00: {w}");

    // WorkingTime `Hour` 1-50 -> minute-within-hour (mod 60) BETWEEN.
    let v = Variant {
        tod: Some(TimeWindow::Hour(1, 50)),
        ..Default::default()
    };
    let w = v.where_sql();
    assert!(w.contains("% 60) BETWEEN 1 AND 50"), "w={w}");

    // `week_span` AND `tod` combine into one WHERE clause (both axes).
    let v = Variant {
        week_span: Some((0, 8639)),
        tod: Some(TimeWindow::Day(1, 1430)),
        ..Default::default()
    };
    let w = v.where_sql();
    assert!(w.contains("BETWEEN 0 AND 8639"));
    assert!(w.contains("BETWEEN 1 AND 1430"));
}

/// Removing the query-zone UDF from `Variant::where_sql` would evaluate this 09:00 Warsaw
/// schedule as 08:00 UTC and drop the trade the user sees inside the window.
#[test]
fn time_variant_uses_the_selected_zone_for_open_time() {
    let conn = Connection::open_in_memory().expect("in-memory database");
    conn.execute_batch(
        "CREATE TABLE trades(
            closedate INTEGER, buydate INTEGER, pnl REAL, spentbtc REAL, core_uid INTEGER
         );
         INSERT INTO trades VALUES (1767258000, 1767256200, 5.0, 10.0, 1);",
    )
    .expect("seed trade");
    let source = "(SELECT closedate, buydate, pnl, spentbtc, core_uid FROM trades
                   WHERE closedate >= ?1 AND closedate < ?2) o";
    let query = Query {
        axis: crate::db::ReportAxis::from_measured(Default::default(), chrono_tz::Europe::Warsaw),
        from: 1_767_200_000,
        to: 1_767_300_000,
        ..Default::default()
    };
    let variants = [
        Variant::default(),
        Variant {
            tod: Some(TimeWindow::Day(9 * 60, 10 * 60)),
            ..Default::default()
        },
    ];

    let stats = variant_stats_from_source(&conn, &query, source, &variants)
        .expect("selected-zone variant evaluates");

    assert_eq!(stats[0].n, 1, "fact sees the seeded trade");
    assert_eq!(stats[1].n, 1, "Warsaw 09:30 stays inside 09:00-10:00");

    let mut civil_rows = Vec::new();
    visit_time_rows(
        &conn,
        &query,
        source,
        "test: selected-zone time profile",
        |weekday, minute, _profit| civil_rows.push((weekday, minute)),
    )
    .expect("selected-zone profile evaluates");
    assert_eq!(civil_rows, vec![(3, 570)], "Thursday 09:30 in Warsaw");
}

#[test]
fn best_range_skips_noop_full_range() {
    // Every trade is profitable: no subrange beats no filter, so a no-op min/max pair
    // must not be suggested.
    let mut vals: Vec<(f64, f64)> = (0..100).map(|i| (i as f64, 1.0)).collect();
    assert!(best_range(&mut vals, 1, 16, false).is_none());
    // The lower third loses: suggest a range that is NOT the full extent.
    let mut vals: Vec<(f64, f64)> = (0..99)
        .map(|i| (i as f64, if i < 33 { -1.0 } else { 1.0 }))
        .collect();
    let s = best_range(&mut vals, 1, 16, false).expect("фильтр должен найтись");
    assert!(
        s.from.unwrap() > 0.0,
        "нижняя граница должна отрезать минус"
    );
    assert_eq!(s.to.unwrap(), 98.0, "верхняя пара — фактический max данных");
}

#[test]
fn round_falls_back_when_pair_stops_cutting() {
    // The range cuts only two losing tail values at the data extremes. Outward rounding would
    // move BOTH bounds beyond min/max and make a no-op, so keep the raw filtering values.
    let mut vals: Vec<(f64, f64)> = (0..100)
        .map(|i| (1000.0 + i as f64, if i < 2 { -5.0 } else { 1.0 }))
        .collect();
    let s = best_range(&mut vals, 1, 50, true).expect("фильтр должен найтись");
    let from = s.from.unwrap();
    assert!(from > 1000.0, "граница обязана резать данные, got {from}");
}

#[test]
fn empty_variant_is_fact() {
    assert!(Variant::default().is_empty());
    let v = Variant {
        bounds: vec![Bound {
            field: "d1h".into(),
            from: Some(0.0),
            to: None,
        }],
        ..Default::default()
    };
    assert!(!v.is_empty());
}
