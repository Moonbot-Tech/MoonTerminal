//! Regression tests for exact Report strategy filtering.

use rusqlite::types::Value;
use rusqlite::Connection;

use super::{
    distinct_strategies, query_reports, query_totals, ReportFilter, ReportStrategyKey, SideFilter,
};

/// Build two typed cores with the same signed strategy id plus an unidentifiable legacy row.
///
/// Returns:
///     An in-memory report database with strategy metadata and attribution edge cases.
fn strategy_fixture() -> Connection {
    let conn = Connection::open_in_memory().expect("open in-memory report database");
    super::super::init_db(&conn).expect("initialize report database");
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS orders_rep (
             core_uid INTEGER NOT NULL,
             core_name TEXT NOT NULL,
             newrecid INTEGER NOT NULL,
             PRIMARY KEY (core_uid, newrecid)
         );
         ALTER TABLE orders_rep ADD COLUMN closedate INTEGER;
         ALTER TABLE orders_rep ADD COLUMN profitbtc REAL;
         ALTER TABLE orders_rep ADD COLUMN coin TEXT;
         ALTER TABLE orders_rep ADD COLUMN strategyid INTEGER;
         ALTER TABLE orders_rep ADD COLUMN deleted INTEGER;
         ALTER TABLE orders_rep ADD COLUMN isshort INTEGER;
         ALTER TABLE orders_rep ADD COLUMN emulator INTEGER;
         ALTER TABLE orders_rep ADD COLUMN channelname TEXT;
         ALTER TABLE orders_rep ADD COLUMN signaltype TEXT;
         INSERT INTO orders_rep
             (core_uid, core_name, newrecid, closedate, profitbtc, coin, strategyid, deleted,
              isshort, emulator, channelname, signaltype)
         VALUES
             (1, 'CORE-A', 1, 100, 10.0, 'WRONG-CORE', -7, 0, 0, 0, '', ''),
             (2, 'CORE-B', 1, 200, 20.0, 'EXPECTED', -7, 0, 0, 0, '', ''),
             (2, 'CORE-B', 2, 300, 30.0, 'WRONG-STRATEGY', 8, 0, 0, 0, '', ''),
             (2, 'CORE-B', 3, 250, -5.0, 'ATTRIBUTED', 0, 0, 0, 0,
              'LIQUIDATION', 'OWNER ( Kind )'),
             (2, 'CORE-B', 4, 0, 90.0, 'UNDATED', -7, 0, 0, 0, '', ''),
             (2, 'CORE-B', 5, 350, -9.0, 'ONLY-ATTRIBUTED', 0, 0, 0, 0,
              'LIQUIDATION', 'ONLY-LIQ ( Kind )'),
             (2, 'CORE-B', 6, 360, 6.0, 'NULL-MANUAL', NULL, 0, 0, 0, '', ''),
             (2, 'CORE-B', 101, 220, 1.0, 'MATCH', 101, 0, 0, 0, '', ''),
             (3, 'CORE-C', 102, 220, 1.0, 'MATCH', 102, 0, 0, 0, '', ''),
             (2, 'CORE-B', 103, 189, 1.0, 'MATCH', 103, 0, 0, 0, '', ''),
             (2, 'CORE-B', 104, 311, 1.0, 'MATCH', 104, 0, 0, 0, '', ''),
             (2, 'CORE-B', 105, 220, 1.0, 'OTHER', 105, 0, 0, 0, '', ''),
             (2, 'CORE-B', 106, 220, 1.0, 'MATCH', 106, 0, 1, 0, '', ''),
             (2, 'CORE-B', 107, 220, 1.0, 'MATCH', 107, 0, 0, 1, '', ''),
             (2, 'CORE-B', 108, 220, 1.0, 'MATCH', 108, 1, 0, 0, '', ''),
             (2, 'CORE-B', 109, 0, 1.0, 'OPEN-MATCH', 109, 0, 0, 0, '', '');
         CREATE TABLE closed_sell_reports (
             core_uid INTEGER NOT NULL,
             core_name TEXT NOT NULL,
             db_id INTEGER NOT NULL,
             closedate INTEGER,
             profitbtc REAL,
             coin TEXT,
             updated_ms INTEGER
         );
         INSERT INTO closed_sell_reports
             (core_uid, core_name, db_id, closedate, profitbtc, coin, updated_ms)
         VALUES (2, 'CORE-B', 99, 400, 40.0, 'NO-STRATEGY-COLUMN', 400);
         ATTACH DATABASE ':memory:' AS strat;
         CREATE TABLE strat.strategies (
             core_uid INTEGER NOT NULL,
             strategy_id INTEGER NOT NULL,
             name TEXT NOT NULL,
             deleted INTEGER NOT NULL
         );
         INSERT INTO strat.strategies (core_uid, strategy_id, name, deleted)
         VALUES (2, -7, 'OWNER', 0),
                (2, -9, 'ONLY-LIQ', 0);",
    )
    .expect("create exact-strategy fixture");
    super::super::test_support::rep_init(&conn);
    conn
}

/// Removing the `core_uid` predicate from `report_read:build_where` must expose
/// `WRONG-CORE`, while treating a source without `strategyid` as a match must expose
/// `NO-STRATEGY-COLUMN`; replacing the effective id with raw `strategyid` must also drop
/// `ATTRIBUTED`; dropping the positive-close constraint must expose `UNDATED`, and raw selector
/// discovery must omit `ONLY-LIQ`. Those edits make Report disagree with the Analytics row.
///
/// Returns:
///     Nothing; exact rows, totals, attribution, and selector discovery are asserted.
#[test]
fn exact_strategy_filters_rows_totals_and_unidentifiable_sources() {
    let conn = strategy_fixture();
    let filter = ReportFilter {
        strategies: Some(vec![ReportStrategyKey {
            core_uid: 2,
            strategy_id: -7,
        }]),
        closed_only: true,
        ..ReportFilter::default()
    };

    let table =
        query_reports(&conn, &filter, "closedate", false, 100).expect("query filtered reports");
    let coin = table
        .cols
        .iter()
        .position(|column| column == "coin")
        .expect("coin column");

    assert_eq!(table.core_uids, vec![2, 2]);
    assert_eq!(table.rows.len(), 2);
    assert_eq!(table.rows[0][coin], Value::Text("EXPECTED".to_string()));
    assert_eq!(table.rows[1][coin], Value::Text("ATTRIBUTED".to_string()));
    assert_eq!(
        query_totals(&conn, &filter).expect("query filtered totals"),
        (15.0, 2)
    );
    let choice = distinct_strategies(&conn, &ReportFilter::default())
        .expect("load strategy choices")
        .into_iter()
        .find(|strategy| strategy.key == filter.strategies.as_ref().expect("exact filter")[0])
        .expect("selected strategy choice");
    assert_eq!(choice.name, "OWNER");
    assert!(
        distinct_strategies(&conn, &ReportFilter::default())
            .expect("load attributed-only strategy choice")
            .iter()
            .any(|strategy| {
                strategy.key
                    == ReportStrategyKey {
                        core_uid: 2,
                        strategy_id: -9,
                    }
                    && strategy.name == "ONLY-LIQ"
            }),
        "an attribution-only owner must be selectable"
    );

    let manual = ReportFilter {
        strategies: Some(vec![ReportStrategyKey {
            core_uid: 2,
            strategy_id: 0,
        }]),
        closed_only: true,
        ..ReportFilter::default()
    };
    let manual_rows =
        query_reports(&conn, &manual, "closedate", false, 100).expect("query manual reports");
    let manual_coin = manual_rows
        .cols
        .iter()
        .position(|column| column == "coin")
        .expect("manual coin column");
    assert_eq!(manual_rows.rows.len(), 1);
    assert_eq!(
        manual_rows.rows[0][manual_coin],
        Value::Text("NULL-MANUAL".to_string())
    );
}

/// Removing `build_where` from `report_read:distinct_strategies` must expose one of the core,
/// date, coin, side, emulator, or deleted decoys. Applying `filter.strategies` there instead must
/// hide strategy 101, even though it matches every non-strategy predicate. Ignoring `closed_only`
/// must expose the open strategy 109.
///
/// Returns:
///     Nothing; exact catalog identities are asserted from independent fixture literals.
#[test]
fn strategy_choices_follow_report_scope_without_self_filtering() {
    let conn = strategy_fixture();
    let scoped = ReportFilter {
        core_uids: vec![2],
        date_from: Some(190),
        date_to: Some(310),
        coin: " match ".to_string(),
        side: SideFilter::Long,
        emulator: Some(false),
        deleted_only: false,
        closed_only: true,
        strategies: Some(vec![ReportStrategyKey {
            core_uid: 2,
            strategy_id: -7,
        }]),
    };
    let keys = distinct_strategies(&conn, &scoped)
        .expect("load scoped strategy choices")
        .into_iter()
        .map(|strategy| (strategy.key.core_uid, strategy.key.strategy_id))
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(keys, std::collections::BTreeSet::from([(2, 101)]));

    let deleted = ReportFilter {
        deleted_only: true,
        strategies: None,
        ..scoped.clone()
    };
    let deleted_keys = distinct_strategies(&conn, &deleted)
        .expect("load deleted strategy choices")
        .into_iter()
        .map(|strategy| (strategy.key.core_uid, strategy.key.strategy_id))
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(deleted_keys, std::collections::BTreeSet::from([(2, 108)]));

    let open = ReportFilter {
        core_uids: vec![2],
        coin: "OPEN-MATCH".to_string(),
        closed_only: false,
        ..ReportFilter::default()
    };
    let open_keys = distinct_strategies(&conn, &open)
        .expect("load open strategy choice")
        .into_iter()
        .map(|strategy| (strategy.key.core_uid, strategy.key.strategy_id))
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(open_keys, std::collections::BTreeSet::from([(2, 109)]));
    assert!(distinct_strategies(
        &conn,
        &ReportFilter {
            closed_only: true,
            ..open
        },
    )
    .expect("load closed-only strategy choices")
    .is_empty());
}

/// Replacing the grouped exact-key predicate with only its first key must lose one selected row,
/// while treating `Some(vec![])` as implicit All must expose every fixture row. Either regression
/// makes checkbox state disagree with rows, totals, and export.
///
/// Returns:
///     Nothing; multiple and empty explicit filter semantics are asserted.
#[test]
fn multiple_and_explicit_empty_strategy_filters_remain_exact() {
    let conn = strategy_fixture();
    let multiple = ReportFilter {
        strategies: Some(vec![
            ReportStrategyKey {
                core_uid: 1,
                strategy_id: -7,
            },
            ReportStrategyKey {
                core_uid: 2,
                strategy_id: 8,
            },
        ]),
        closed_only: true,
        ..ReportFilter::default()
    };

    let table = query_reports(&conn, &multiple, "closedate", false, 100)
        .expect("query multiple strategies");
    let coin = table
        .cols
        .iter()
        .position(|column| column == "coin")
        .expect("coin column");
    let coins = table
        .rows
        .iter()
        .map(|row| row[coin].clone())
        .collect::<Vec<_>>();
    assert_eq!(
        coins,
        vec![
            Value::Text("WRONG-CORE".to_string()),
            Value::Text("WRONG-STRATEGY".to_string()),
        ]
    );
    assert_eq!(
        query_totals(&conn, &multiple).expect("query multiple-strategy totals"),
        (40.0, 2)
    );

    let empty = ReportFilter {
        strategies: Some(Vec::new()),
        ..ReportFilter::default()
    };
    assert!(query_reports(&conn, &empty, "closedate", false, 100)
        .expect("query explicit empty strategy set")
        .rows
        .is_empty());
    assert_eq!(
        query_totals(&conn, &empty).expect("query explicit empty totals"),
        (0.0, 0)
    );
}

/// Returning early from `report_read:append_strategy_filter` for a multi-key complete selector
/// universe must expose `NO-STRATEGY-COLUMN`, a legacy source with no checkbox identity.
///
/// Returns:
///     Nothing; a complete explicit selector universe remains an exact database predicate.
#[test]
fn complete_explicit_strategy_universe_excludes_unidentifiable_sources() {
    let conn = strategy_fixture();
    let complete = ReportFilter {
        strategies: Some(
            distinct_strategies(&conn, &ReportFilter::default())
                .expect("load complete identifiable strategy universe")
                .into_iter()
                .map(|strategy| strategy.key)
                .collect(),
        ),
        closed_only: true,
        ..ReportFilter::default()
    };
    let table = query_reports(&conn, &complete, "closedate", false, 100)
        .expect("query complete explicit strategy universe");
    let coin = table
        .cols
        .iter()
        .position(|column| column == "coin")
        .expect("complete-filter coin column");

    assert!(
        table
            .rows
            .iter()
            .all(|row| row[coin] != Value::Text("NO-STRATEGY-COLUMN".to_string())),
        "a complete explicit checkbox set must still exclude sources without strategy identity"
    );
}
