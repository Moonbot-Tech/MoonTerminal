//! Index seeks of the Report reads over the replica: open rows, one coin's trades and the
//! strategy-name mask must reach `orders_rep` through a named index, never a table scan, and the
//! rewritten predicates must select exactly what the replaced SQL did.
//!
//! Plans are read off `EXPLAIN QUERY PLAN` of the statement each public read actually prepared
//! (captured by a trace hook); result sets are compared against the replaced SQL, kept here as
//! literal oracles. All data is synthetic.

use std::cell::RefCell;
use std::collections::{BTreeSet, HashMap};

use rusqlite::Connection;

use super::super::{
    PeriodBasis, ReportFilter, RowScope, closed_test_sql, query_chart_trade_history_for_cores,
    query_reports, query_totals,
};
use crate::db::{OffsetSegment, ReportAxis};

thread_local! {
    static CAPTURED: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
}

/// Keep every expanded statement that reads `orders_rep`.
fn capture(event: rusqlite::trace::TraceEvent<'_>) {
    let rusqlite::trace::TraceEvent::Stmt(statement, _) = event else {
        return;
    };
    let Some(sql) = statement.expanded_sql() else {
        return;
    };
    if sql.to_ascii_uppercase().contains("ORDERS_REP") && sql.trim_start().starts_with("SELECT") {
        CAPTURED.with(|slot| slot.borrow_mut().push(sql));
    }
}

/// Run `read` and return every captured `orders_rep` SELECT with its query plan.
fn plans_of(conn: &Connection, read: impl FnOnce()) -> Vec<(String, String)> {
    CAPTURED.with(|slot| slot.borrow_mut().clear());
    conn.trace_v2(
        rusqlite::trace::TraceEventCodes::SQLITE_TRACE_STMT,
        Some(capture),
    );
    read();
    conn.trace_v2(rusqlite::trace::TraceEventCodes::SQLITE_TRACE_STMT, None);
    let statements = CAPTURED.with(|slot| slot.borrow().clone());
    statements
        .into_iter()
        .map(|sql| {
            let plan = explain(conn, &sql);
            (sql, plan)
        })
        .collect()
}

/// `EXPLAIN QUERY PLAN` details of one statement, joined by ` | `.
fn explain(conn: &Connection, sql: &str) -> String {
    let mut stmt = conn
        .prepare(&format!("EXPLAIN QUERY PLAN {sql}"))
        .unwrap_or_else(|error| panic!("explain failed: {error}; sql: {sql}"));
    stmt.query_map([], |row| row.get::<_, String>(3))
        .expect("read the query plan")
        .map(|row| row.expect("plan row"))
        .collect::<Vec<_>>()
        .join(" | ")
}

/// Assert one statement searches `index` and never scans the report table.
fn assert_seeks(plan: &str, sql: &str, index: &str, what: &str) {
    assert!(
        plan.contains(index),
        "{what}: plan does not use {index}: {plan}; sql: {sql}"
    );
    // `SCAN r USING INDEX idx_rep_open` walks the partial index, which holds only open rows.
    assert!(
        !plan
            .split(" | ")
            .any(|step| step.starts_with("SCAN ") && !step.contains(" USING ")),
        "{what}: plan scans orders_rep: {plan}; sql: {sql}"
    );
}

/// The head of the open test, whatever spelling the column takes inside it.
const OPEN_TEXT: &str = "(NOT (typeof(";

/// The replica schema the core sends, with the production indexes `rep::init` creates.
fn replica() -> Connection {
    let conn = Connection::open_in_memory().expect("open replica");
    crate::db::init_db(&conn).expect("init report database");
    replica_columns(&conn);
    conn
}

/// Add the replica's columns the way the core schema arrives, then run `rep::init`.
fn replica_columns(conn: &Connection) {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS orders_rep (core_uid INTEGER NOT NULL,
                core_name TEXT NOT NULL, newrecid INTEGER NOT NULL,
                PRIMARY KEY (core_uid, newrecid));
         ALTER TABLE orders_rep ADD COLUMN closedate INTEGER;
         ALTER TABLE orders_rep ADD COLUMN buydate INTEGER;
         ALTER TABLE orders_rep ADD COLUMN profitbtc REAL;
         ALTER TABLE orders_rep ADD COLUMN spentbtc REAL;
         ALTER TABLE orders_rep ADD COLUMN coin TEXT;
         ALTER TABLE orders_rep ADD COLUMN strategyid INTEGER;
         ALTER TABLE orders_rep ADD COLUMN isshort INTEGER;
         ALTER TABLE orders_rep ADD COLUMN emulator INTEGER;
         ALTER TABLE orders_rep ADD COLUMN deleted INTEGER;
         ALTER TABLE orders_rep ADD COLUMN basecurrency INTEGER;
         ALTER TABLE orders_rep ADD COLUMN buyprice REAL;
         ALTER TABLE orders_rep ADD COLUMN sellprice REAL;
         ALTER TABLE orders_rep ADD COLUMN quantity REAL;",
    )
    .expect("replica columns");
    crate::db::test_support::rep_init(conn);
}

/// Fill the replica with `rows` trades over `cores` cores and `coins` coins, one in `open_every`
/// still open, then attach `strategies` strategies per core.
fn seed(conn: &Connection, rows: i64, cores: i64, coins: i64, open_every: i64, strategies: i64) {
    conn.execute_batch("BEGIN").expect("begin seed");
    {
        let mut insert = conn
            .prepare(
                "INSERT INTO orders_rep (core_uid, core_name, newrecid, closedate, buydate,
                        profitbtc, spentbtc, coin, strategyid, isshort, emulator, deleted,
                        basecurrency)
                 VALUES (?1, 'CORE', ?2, ?3, ?4, 1.0, 10.0, ?5, ?6, 0, 0, 0, 1)",
            )
            .expect("prepare seed insert");
        for i in 0..rows {
            let core = 1 + i % cores;
            let buy = 1_700_000_000 + i * 7;
            let close = if i % open_every == 0 { 0 } else { buy + 60 };
            let coin = format!("C{}USDT", i % coins);
            let sid = 1 + (i / cores) % strategies;
            insert
                .execute(rusqlite::params![core, i + 1, close, buy, coin, sid])
                .expect("seed row");
        }
    }
    conn.execute_batch(
        "ATTACH DATABASE ':memory:' AS strat;
         CREATE TABLE strat.strategies (core_uid, strategy_id, name, deleted);",
    )
    .expect("attach strategies");
    {
        let mut insert = conn
            .prepare("INSERT INTO strat.strategies VALUES (?1, ?2, ?3, 0)")
            .expect("prepare strategy insert");
        for core in 1..=cores {
            for sid in 1..=strategies {
                let name = if sid % 50 == 0 {
                    format!("Alpha{sid}")
                } else {
                    format!("Beta{sid}")
                };
                insert
                    .execute(rusqlite::params![core, sid, name])
                    .expect("seed strategy");
            }
        }
    }
    conn.execute_batch("COMMIT; ANALYZE").expect("commit seed");
}

/// Two cores on different clocks, so an `OpenIfCurrent` read builds two offset groups.
fn two_offset_axis() -> ReportAxis {
    ReportAxis::from_measured(
        HashMap::from([
            (
                1,
                vec![OffsetSegment {
                    from_utc: 0,
                    offset_secs: -14_400,
                }],
            ),
            (
                2,
                vec![OffsetSegment {
                    from_utc: 0,
                    offset_secs: 10_800,
                }],
            ),
        ]),
        chrono_tz::UTC,
    )
}

/// `report_read/scope.rs::append_row_scope` (the `OpenIfCurrent` hoist) and
/// `append_open_basis_scope` must spell every open-row read with ONE top-level open test.
///
/// Breakage: folding the open test back into each offset group's branch
/// (`A AND g1 OR A AND g2`), or passing it through `close_test_off_index` (`+r."closedate"`),
/// stops SQLite matching the `idx_rep_open` partial index, so every 5-second Report refresh and
/// totals footer scans the whole report table to find a handful of open positions.
#[test]
fn open_row_reads_seek_the_open_partial_index() {
    let conn = replica();
    seed(&conn, 6_000, 20, 40, 500, 10);
    let now = crate::util::now_unix_ms_i64().div_euclid(1_000);

    let reads: Vec<(&str, ReportFilter, bool)> = vec![
        (
            "totals open pass, no core filter",
            ReportFilter {
                rows: RowScope::Open,
                ..ReportFilter::default()
            },
            true,
        ),
        (
            "totals open pass, with cores",
            ReportFilter {
                rows: RowScope::Open,
                core_uids: vec![1, 2, 3],
                ..ReportFilter::default()
            },
            true,
        ),
        (
            "rows Open pass",
            ReportFilter {
                rows: RowScope::Open,
                ..ReportFilter::default()
            },
            false,
        ),
        (
            "rows OpenIfCurrent, two offset groups",
            ReportFilter {
                rows: RowScope::OpenIfCurrent,
                core_uids: vec![1, 2],
                date_to: Some(now + 86_400),
                axis: two_offset_axis(),
                ..ReportFilter::default()
            },
            false,
        ),
        (
            "rows OpenDate-basis Open",
            ReportFilter {
                rows: RowScope::Open,
                period_basis: PeriodBasis::OpenDate,
                date_from: Some(1_700_000_000),
                core_uids: vec![1, 2],
                axis: two_offset_axis(),
                ..ReportFilter::default()
            },
            false,
        ),
    ];
    for (what, filter, totals) in reads {
        let plans = plans_of(&conn, || {
            if totals {
                query_totals(&conn, &filter).expect("totals read");
            } else {
                query_reports(&conn, &filter, "closedate", true, 100).expect("rows read");
            }
        });
        let open: Vec<_> = plans
            .iter()
            .filter(|(sql, _)| sql.contains(OPEN_TEXT))
            .collect();
        assert!(!open.is_empty(), "{what}: no open-row statement prepared");
        for (sql, plan) in open {
            assert_seeks(plan, sql, "idx_rep_open", what);
        }
    }
}

/// `report_read/scope.rs::build_where` (exact ticker) and chart history must seek
/// `idx_rep_coin_close`.
///
/// Breakage: dropping the NOCASE range around the exact-ticker test, or a core-filter rewrite
/// that makes the planner prefer `idx_rep_core_close`, sends a one-coin chart or Report filter
/// through every trade the selected cores ever made.
#[test]
fn coin_reads_seek_the_coin_index() {
    let conn = replica();
    seed(&conn, 6_000, 4, 200, 500, 10);
    let one = ["C7USDT".to_string()];
    let window = ReportFilter {
        date_from: Some(1_700_000_000),
        date_to: Some(1_800_000_000),
        ..ReportFilter::default()
    };
    let chart_reads: Vec<(&str, &[String], Option<&ReportFilter>)> = vec![
        ("single coin", &one, Some(&window)),
        ("single coin, no window", &one, None),
    ];
    for (what, coins, window) in chart_reads {
        let plans = plans_of(&conn, || {
            query_chart_trade_history_for_cores(&conn, &[1, 2], coins, window, 1000)
                .expect("chart history");
        });
        assert!(!plans.is_empty(), "chart history {what}: nothing prepared");
        for (sql, plan) in &plans {
            assert_seeks(plan, sql, "idx_rep_coin_close", what);
        }
    }
    for (what, date_from) in [
        ("exact ticker", None),
        ("exact ticker + period", Some(1_700_010_000)),
    ] {
        let filter = ReportFilter {
            rows: RowScope::Closed,
            coin: "C7USDT ".to_string(),
            date_from,
            ..ReportFilter::default()
        };
        let plans = plans_of(&conn, || {
            query_reports(&conn, &filter, "closedate", true, 100).expect("exact coin rows");
        });
        assert!(!plans.is_empty(), "{what}: nothing prepared");
        for (sql, plan) in &plans {
            assert_seeks(plan, sql, "idx_rep_coin_close", what);
        }
    }
}

/// Chart history over TWO coins must seek `idx_rep_coin_close` like the single-coin read.
///
/// Open finding, ignored until the production fix lands: the `OR` of two `coin COLLATE NOCASE =`
/// terms is not turned into a multi-index seek; the planner walks `idx_rep_closedate` over every
/// closed trade of the selected cores instead, so a chart showing a coin and its alias stays a
/// full-history read.
#[test]
#[ignore = "open finding: multi-coin chart history walks idx_rep_closedate"]
fn multi_coin_chart_history_seeks_the_coin_index() {
    let conn = replica();
    seed(&conn, 6_000, 4, 200, 500, 10);
    let two = ["C7USDT".to_string(), "C9USDT".to_string()];
    let plans = plans_of(&conn, || {
        query_chart_trade_history_for_cores(&conn, &[1, 2], &two, None, 1000)
            .expect("chart history");
    });
    assert!(!plans.is_empty(), "chart history: nothing prepared");
    for (sql, plan) in &plans {
        assert_seeks(plan, sql, "idx_rep_coin_close", "two coins");
    }
}

/// `report_read/strategy_mask.rs::append_strategy_name_mask` must resolve the mask once, not per row.
///
/// Breakage: restoring the correlated `EXISTS (SELECT .. FROM strat.strategies ..)` re-runs the
/// name matcher for every report row the period holds.
#[test]
fn positive_name_mask_is_not_a_correlated_subquery() {
    let conn = replica();
    seed(&conn, 2_000, 4, 20, 500, 100);
    let filter = ReportFilter {
        rows: RowScope::Closed,
        strategy_name_mask: "Alpha".to_string(),
        ..ReportFilter::default()
    };
    let plans = plans_of(&conn, || {
        let table = query_reports(&conn, &filter, "closedate", true, 100).expect("mask rows");
        assert!(
            !table.rows.is_empty(),
            "the mask must select the Alpha rows"
        );
    });
    assert!(!plans.is_empty());
    for (sql, plan) in &plans {
        assert!(
            !sql.contains("strat.strategies") && !plan.contains("CORRELATED"),
            "the mask stayed a per-row subquery: {plan}; sql: {sql}"
        );
    }
}

/// `rep.rs::OPEN_ROW_WHERE` must stay the exact negation of `closed_test_sql` over the bare
/// column: SQLite matches a partial index's `WHERE` structurally, so any drift (a changed
/// operator, a reordered type list) leaves `idx_rep_open` built, paid for on every write and
/// never used.
#[test]
fn open_row_index_where_is_the_negated_closed_test() {
    assert_eq!(
        crate::db::rep::OPEN_ROW_WHERE,
        format!("NOT {}", closed_test_sql("closedate"))
    );
}

/// `rep.rs::ensure_indexes` is safe to run on every start: a second run is a no-op, and a
/// replica that has not received `coin` yet gets every other index and no coin index.
#[test]
fn replica_indexes_are_idempotent_and_wait_for_their_columns() {
    let names = |conn: &Connection| {
        let mut stmt = conn
            .prepare(
                "SELECT name FROM sqlite_master WHERE type='index' AND tbl_name='orders_rep'
                 AND name LIKE 'idx_rep_%'",
            )
            .expect("list indexes");
        stmt.query_map([], |row| row.get::<_, String>(0))
            .expect("index names")
            .map(|row| row.expect("index name"))
            .collect::<BTreeSet<_>>()
    };
    let all = BTreeSet::from(
        [
            "idx_rep_closedate",
            "idx_rep_core_close",
            "idx_rep_strat",
            "idx_rep_strategy_close",
            "idx_rep_open",
            "idx_rep_coin_close",
        ]
        .map(String::from),
    );

    let conn = replica();
    assert_eq!(names(&conn), all);
    crate::db::test_support::rep_init(&conn);
    assert_eq!(names(&conn), all, "a second init must change nothing");

    let no_coin = Connection::open_in_memory().expect("open coin-less replica");
    crate::db::init_db(&no_coin).expect("init");
    no_coin
        .execute_batch(
            "CREATE TABLE orders_rep (core_uid INTEGER NOT NULL, core_name TEXT NOT NULL,
                 newrecid INTEGER NOT NULL, closedate INTEGER, buydate INTEGER,
                 strategyid INTEGER, PRIMARY KEY (core_uid, newrecid));",
        )
        .expect("coin-less replica");
    crate::db::test_support::rep_init(&no_coin);
    crate::db::test_support::rep_init(&no_coin);
    let mut expected = all.clone();
    expected.remove("idx_rep_coin_close");
    assert_eq!(names(&no_coin), expected);
}

/// The replaced name-mask predicates, verbatim from before the resolve-once rewrite, over the
/// plain strategy id (the fixture has no attribution columns).
const OLD_MASK_POSITIVE: &str = "EXISTS (SELECT 1 FROM strat.strategies mask_strategy \
     WHERE mask_strategy.core_uid = r.core_uid \
     AND mask_strategy.strategy_id = COALESCE(r.\"strategyid\", 0) \
     AND mt_strategy_name_match(mask_strategy.name, ?1) = 1)";
const OLD_MASK_EXCLUSION: &str = "NOT EXISTS (SELECT 1 FROM strat.strategies mask_strategy \
     WHERE mask_strategy.core_uid = r.core_uid \
     AND mask_strategy.strategy_id = COALESCE(r.\"strategyid\", 0) \
     AND mask_strategy.name IS NOT NULL \
     AND mt_strategy_name_match(mask_strategy.name, ?1) = 0)";
/// The replaced exact-ticker test.
const OLD_EXACT_TICKER: &str = "(r.coin COLLATE NOCASE = ?1 OR r.coin LIKE ?2 ESCAPE '\\')";

/// A small replica with every edge the rewritten predicates must keep: legacy rows with a NULL
/// core, metadata-less rows, sid 0, NULL sid, duplicate and NULL names, a huge core uid stored
/// as a negative i64, an integral-REAL core uid, odd closedate types and look-alike coins.
///
/// `null_core` adds the legacy NULL-core row; a rows read cannot project it (as before this
/// change), so only totals reads take it.
fn edge_fixture(null_core: bool) -> Connection {
    let conn = replica();
    // (core, rec, closedate, coin, sid); closedate as SQL text so TEXT/NULL/REAL stay typed.
    let rows: &[(i64, i64, &str, &str, &str)] = &[
        (1, 1, "100", "SOL", "1"),
        (1, 2, "100", "SOL_RP", "2"),
        (1, 3, "100", "SOL1", "3"),
        (1, 4, "100", "SOLV", "4"),
        (1, 5, "100", "sol", "0"),
        (1, 6, "0", "SOL", "1"),
        (1, 7, "NULL", "SOL_0925", "2"),
        (1, 8, "'200'", "AB_CD", "NULL"),
        (1, 9, "150.5", "ab_cd_x", "5"),
        (2, 10, "120", "BTCUSD_PERP", "1"),
        (2, 11, "120", "SOLUSDT", "7"),
        (3, 12, "130", "SOL", "1"),
        (-5, 13, "140", "SOL", "1"),
        (-5, 14, "0", "ETH", "2"),
        (1, 15, "-3", "SOL", "6"),
    ];
    for (core, rec, close, coin, sid) in rows {
        conn.execute(
            &format!(
                "INSERT INTO orders_rep (core_uid, core_name, newrecid, closedate, buydate,
                        profitbtc, spentbtc, coin, strategyid, isshort, emulator, deleted,
                        basecurrency)
                 VALUES ({core}, 'CORE', {rec}, {close}, 50, {rec}, 10.0, '{coin}', {sid},
                         0, 0, 0, 1)"
            ),
            [],
        )
        .expect("edge row");
    }
    conn.execute_batch(
        "CREATE TABLE closed_sell_reports (core_uid INTEGER, core_name TEXT NOT NULL,
             db_id INTEGER NOT NULL, closedate INTEGER, profitbtc REAL, coin TEXT,
             strategyid INTEGER, updated_ms INTEGER);
         INSERT INTO closed_sell_reports VALUES
             (1, 'LEGACY', 91, 111, 91.0, 'SOL', 2, 111);
         ATTACH DATABASE ':memory:' AS strat;
         CREATE TABLE strat.strategies (core_uid, strategy_id, name, deleted);
         INSERT INTO strat.strategies VALUES
             (1, 1, 'Alpha', 0), (1, 2, 'Alpha', 0), (1, 3, 'Beta', 0),
             (1, 4, NULL, 0), (1, 0, 'Alpha', 0), (2, 1, 'Beta', 0),
             (3.0, 1, 'Alpha', 0), (-5, 1, 'Beta', 0), (-5, 2, 'Alpha', 1);",
    )
    .expect("legacy source and strategies");
    if null_core {
        conn.execute(
            "INSERT INTO closed_sell_reports VALUES (NULL, 'LEGACY', 90, 110, 90.0, 'SOL', 1, 110)",
            [],
        )
        .expect("legacy NULL-core row");
    }
    crate::db::strategy_name_match::install_strategy_name_match(&conn)
        .expect("register the old SQL's matcher");
    conn
}

/// Row ids (the fixture stores each row's id in `profitbtc`) a Report rows read returns.
fn rec_ids(conn: &Connection, filter: &ReportFilter) -> BTreeSet<i64> {
    let table = query_reports(conn, filter, "closedate", true, 1000).expect("rows read");
    let at = table
        .cols
        .iter()
        .position(|c| c == "profitbtc")
        .expect("profitbtc column");
    table
        .rows
        .iter()
        .map(|row| match row[at] {
            rusqlite::types::Value::Real(v) => v as i64,
            ref other => panic!("row id expected, got {other:?}"),
        })
        .collect()
}

/// Rec ids of both physical sources that pass `predicate`, bound to `params`.
fn oracle(conn: &Connection, predicate: &str, params: &[&str]) -> BTreeSet<i64> {
    let mut out = BTreeSet::new();
    for table in ["orders_rep", "closed_sell_reports"] {
        let sql = format!("SELECT CAST(r.profitbtc AS INTEGER) FROM {table} r WHERE {predicate}");
        let mut stmt = conn.prepare(&sql).expect("prepare oracle");
        let ids = stmt
            .query_map(rusqlite::params_from_iter(params), |row| {
                row.get::<_, i64>(0)
            })
            .expect("run oracle");
        out.extend(ids.map(|id| id.expect("oracle id")));
    }
    out
}

/// `report_read/strategy_mask.rs::append_strategy_name_mask` must select exactly what the replaced per-row
/// `EXISTS` / `NOT EXISTS` subqueries did, for positive, exclusion-only and mixed masks.
///
/// Breakage: dropping the exclusion arm's null-safe wrapper (`NOT COALESCE((..), 0)` ->
/// `NOT ((..))`) turns the group test's NULL for a NULL-core legacy row into a dropped row, and
/// losing the empty-set rule would drop every row of an exclusion that matches no strategy:
/// "everything except X" silently hides manual and unknown-strategy trades.
#[test]
fn name_mask_selects_what_the_replaced_subquery_did() {
    let conn = edge_fixture(false);
    let with_null_core = edge_fixture(true);
    let closed = closed_test_sql("r.\"closedate\"");
    for (mask, old) in [
        ("Alpha", OLD_MASK_POSITIVE),
        ("Beta", OLD_MASK_POSITIVE),
        ("!Alpha", OLD_MASK_EXCLUSION),
        ("!Zeta", OLD_MASK_EXCLUSION),
        ("Alpha, Beta !alp", OLD_MASK_POSITIVE),
    ] {
        let filter = ReportFilter {
            rows: RowScope::Closed,
            strategy_name_mask: mask.to_string(),
            ..ReportFilter::default()
        };
        let predicate = format!("{closed} AND {old}");
        assert_eq!(
            rec_ids(&conn, &filter),
            oracle(&conn, &predicate, &[mask]),
            "rows, mask {mask:?}"
        );
        let expected = oracle(&with_null_core, &predicate, &[mask]).len() as u64;
        let totals = query_totals(&with_null_core, &filter).expect("totals read");
        assert_eq!(
            totals.quotes.orders as u64, expected,
            "totals with a NULL-core legacy row, mask {mask:?}"
        );
    }
    // The exclusion keeps rows no strategy metadata covers.
    let kept = rec_ids(
        &conn,
        &ReportFilter {
            rows: RowScope::Closed,
            strategy_name_mask: "!Alpha".to_string(),
            ..ReportFilter::default()
        },
    );
    for id in [9, 11] {
        assert!(
            kept.contains(&id),
            "exclusion dropped metadata-less row {id}: {kept:?}"
        );
    }
}

/// `report_read/scope.rs::build_where` (exact ticker) and the open/closed split must select exactly
/// what the replaced SQL did.
///
/// Breakage: an upper bound of `{ticker}_` instead of `` {ticker}` `` loses `SOL_RP` (`'_'`
/// sorts below `` '`' ``), and a drifted open test changes which rows count as running.
#[test]
fn coin_and_open_predicates_select_what_the_replaced_sql_did() {
    let conn = edge_fixture(false);
    let closed = closed_test_sql("r.\"closedate\"");
    for ticker in ["SOL", "sol", "AB_CD", "BTCUSD"] {
        let filter = ReportFilter {
            rows: RowScope::Closed,
            coin: format!("{ticker} "),
            ..ReportFilter::default()
        };
        let upper = ticker.to_uppercase();
        let tail = format!("{}\\_%", upper.replace('_', "\\_"));
        let expected = oracle(
            &conn,
            &format!("{closed} AND {OLD_EXACT_TICKER}"),
            &[upper.as_str(), tail.as_str()],
        );
        assert!(!expected.is_empty(), "ticker {ticker} must match something");
        assert_eq!(rec_ids(&conn, &filter), expected, "exact ticker {ticker:?}");
    }
    let open = ReportFilter {
        rows: RowScope::Open,
        ..ReportFilter::default()
    };
    // The legacy source has `closedate` too, so its rows take the same test.
    assert_eq!(
        rec_ids(&conn, &open),
        oracle(&conn, &format!("NOT {closed}"), &[]),
        "open rows"
    );
}

/// Warm median of `runs` executions of `f`, in milliseconds.
fn median_ms(runs: usize, mut f: impl FnMut()) -> f64 {
    f();
    let mut times = (0..runs)
        .map(|_| {
            let started = std::time::Instant::now();
            f();
            started.elapsed().as_secs_f64() * 1000.0
        })
        .collect::<Vec<_>>();
    times.sort_by(|a, b| a.total_cmp(b));
    times[runs / 2]
}

/// Before/after timings of the six reads this change reworked, on a 200k-row file replica.
///
/// BEFORE drops `idx_rep_open` / `idx_rep_coin_close`; the name-mask reads run the replaced
/// correlated SQL as a literal, every other read runs the production function (whose SQL for
/// those passes differs from the old one only by the index-friendly spelling). AFTER runs the
/// production reads with every index present. Prints only; run by hand with `--ignored`.
#[test]
#[ignore = "timing probe, run by hand in release"]
fn seek_timing_200k() {
    let path = crate::db::test_support::temp_db("seek-timing");
    let conn = Connection::open(&path).expect("open file replica");
    crate::db::init_db(&conn).expect("init");
    replica_columns(&conn);
    seed(&conn, 200_000, 100, 400, 500, 50);
    crate::db::strategy_name_match::install_strategy_name_match(&conn).expect("matcher");
    let strategies: i64 = conn
        .query_row("SELECT COUNT(*) FROM strat.strategies", [], |r| r.get(0))
        .expect("count strategies");
    let coin = ["C7USDT".to_string()];
    let cores = (1..=100).collect::<Vec<u64>>();
    let open = ReportFilter {
        rows: RowScope::Open,
        ..ReportFilter::default()
    };
    let exact = ReportFilter {
        rows: RowScope::Closed,
        coin: "C7USDT ".to_string(),
        ..ReportFilter::default()
    };
    let mask = |m: &str| ReportFilter {
        rows: RowScope::Closed,
        strategy_name_mask: m.to_string(),
        ..ReportFilter::default()
    };
    let closed = closed_test_sql("r.\"closedate\"");
    let old_mask = |old: &str, m: &str| {
        let sql =
            format!("SELECT SUM(r.profitbtc), COUNT(*) FROM orders_rep r WHERE {closed} AND {old}");
        let mut stmt = conn.prepare_cached(&sql).expect("old mask sql");
        let n: i64 = stmt.query_row([m], |row| row.get(1)).expect("old mask");
        assert!(n > 0);
    };
    let reads = |label: &str, old: bool| {
        let t = [
            median_ms(5, || {
                query_totals(&conn, &open).expect("totals open");
            }),
            median_ms(5, || {
                query_reports(&conn, &open, "closedate", true, 100).expect("rows open");
            }),
            median_ms(5, || {
                query_chart_trade_history_for_cores(&conn, &cores, &coin, None, 1000)
                    .expect("chart");
            }),
            median_ms(5, || {
                query_reports(&conn, &exact, "closedate", true, 100).expect("exact coin");
            }),
            median_ms(5, || {
                if old {
                    old_mask(OLD_MASK_POSITIVE, "Alpha");
                } else {
                    query_totals(&conn, &mask("Alpha")).expect("mask positive");
                }
            }),
            median_ms(5, || {
                if old {
                    old_mask(OLD_MASK_EXCLUSION, "!Alpha");
                } else {
                    query_totals(&conn, &mask("!Alpha")).expect("mask exclusion");
                }
            }),
        ];
        println!(
            "[{label}] totals-open {:.1} ms | rows-open {:.1} ms | chart-1coin {:.1} ms | \
             exact-coin {:.1} ms | mask+ {:.1} ms | mask- {:.1} ms",
            t[0], t[1], t[2], t[3], t[4], t[5]
        );
    };
    println!("[seek_timing_200k] rows 200000, cores 100, coins 400, strategies {strategies}");
    reads("AFTER ", false);
    conn.execute_batch("DROP INDEX idx_rep_open; DROP INDEX idx_rep_coin_close; ANALYZE")
        .expect("drop the new indexes");
    reads("BEFORE", true);
    drop(conn);
    crate::db::test_support::remove_db(&path);
}
