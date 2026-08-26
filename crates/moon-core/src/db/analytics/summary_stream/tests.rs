//! Summary current-stream statement-count regression tests.

use std::sync::{Mutex, MutexGuard};

use rusqlite::trace::{TraceEvent, TraceEventCodes};

use super::*;

/// Serializes the process-global function-pointer trace sink used by rusqlite.
static TRACE_GUARD: Mutex<()> = Mutex::new(());
/// SQL statements executed while the local test connection has tracing enabled.
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

/// Acquire the trace sink and clear any statements from an earlier failed test.
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

/// Adding another current-period query inside `summary_stream.rs:read` must raise the traced
/// `SELECT` count above one even when the immutable fixture makes every visible value agree.
#[test]
fn current_summary_rows_execute_one_statement() {
    let _trace = start_trace();
    let conn = Connection::open_in_memory().expect("in-memory database");
    conn.execute_batch(
        "CREATE TABLE current_rows(
            closedate INTEGER, buydate INTEGER, pnl REAL, core_uid INTEGER,
            core_name TEXT, coin TEXT, strategyid INTEGER, isshort INTEGER,
            profitbtc REAL, spentbtc REAL, basecurrency INTEGER
         );
         INSERT INTO current_rows VALUES
            (100, 90, 2.0, 1, 'alpha', 'BTC', 7, 0, 2.0, 20.0, 1),
            (200, 180, -1.0, 1, 'alpha', 'ETH', 7, 1, -1.0, 20.0, 1);",
    )
    .expect("summary trace fixture");
    let query = Query {
        from: 1,
        to: 300,
        ..Default::default()
    };
    let source = "(SELECT * FROM current_rows WHERE closedate >= ?1 AND closedate < ?2) o";

    conn.trace_v2(TraceEventCodes::SQLITE_TRACE_STMT, Some(record_sql));
    let result = read(&conn, source, None, &query, &query.axis, 3_600, true, false);
    conn.trace_v2(TraceEventCodes::empty(), None);
    result.expect("single current-period stream");

    let statements = TRACE_SQL
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone();
    assert_eq!(
        statements.len(),
        1,
        "current Summary statements: {statements:#?}"
    );
    assert!(statements[0].contains("FROM (SELECT * FROM current_rows"));
}

/// A non-integer SQLite storage class keeps the historical string strategy fallback.
///
/// Decoding `summary_stream.rs:TradeRow::strategy_id` directly as `Option<i64>` makes one legacy
/// text value fail the entire Summary instead of producing the key and top-row label used before.
#[test]
fn textual_strategy_storage_keeps_group_and_top_fallbacks() {
    let conn = Connection::open_in_memory().expect("in-memory database");
    conn.execute_batch(
        "CREATE TABLE current_rows(
            closedate INTEGER, buydate INTEGER, pnl REAL, core_uid INTEGER,
            core_name TEXT, coin TEXT, strategyid INTEGER, isshort INTEGER,
            profitbtc REAL, spentbtc REAL, basecurrency INTEGER
         );
         INSERT INTO current_rows VALUES
            (100, 90, 2.0, 1, 'alpha', 'BTC', 'odd-id', 0, 2.0, 20.0, 1);",
    )
    .expect("textual strategy fixture");
    let query = Query {
        from: 1,
        to: 300,
        ..Default::default()
    };
    let source = "(SELECT * FROM current_rows WHERE closedate >= ?1 AND closedate < ?2) o";

    let result = read(&conn, source, None, &query, &query.axis, 3_600, true, false)
        .expect("textual strategy remains readable");

    assert_eq!(result.strategies.len(), 1);
    assert_eq!(result.strategies[0].key, "odd-id@1");
    assert_eq!(result.strategies[0].name, "odd-id");
    assert_eq!(result.best[0].strategy, "odd-id");
    assert_eq!(result.worst[0].strategy, "odd-id");
}
