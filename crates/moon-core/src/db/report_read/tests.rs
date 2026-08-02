//! Regression tests for exact Report strategy filtering.

use rusqlite::types::Value;
use rusqlite::Connection;

use super::{
    distinct_strategies, query_reports, query_totals, ReportFilter, ReportStrategyKey, SideFilter,
};

/// Removing mandatory-column gating from `valuation::coverage_sql` would make this query refer to
/// absent `closedate`, `basecurrency`, and `profitbtc` columns while a core schema is still loading.
#[test]
fn totals_tolerate_skeleton_replica_with_valuation_attached() {
    let _health = super::super::valuation::test_health_guard();
    let dir = std::env::temp_dir().join(format!(
        "moonterminal-report-skeleton-{}-{}",
        std::process::id(),
        crate::util::now_unix_ms_i64()
    ));
    std::fs::create_dir_all(&dir).expect("create valuation fixture directory");
    let valuation_path = dir.join("valuation.sqlite");
    drop(
        super::super::valuation::open_store(&valuation_path).expect("initialize valuation fixture"),
    );

    let conn = Connection::open_in_memory().expect("open report fixture");
    super::super::init_db(&conn).expect("initialize skeleton report database");
    super::super::test_support::rep_init(&conn);
    conn.execute(
        "INSERT INTO orders_rep (core_uid, core_name, newrecid) VALUES (1, 'Loading', 7)",
        [],
    )
    .expect("seed one skeleton report row");
    let valuation_sql = format!(
        "ATTACH DATABASE '{}' AS valuation",
        valuation_path.to_string_lossy().replace('\'', "''")
    );
    conn.execute(&valuation_sql, [])
        .expect("attach valuation fixture");

    let totals = query_totals(&conn, &ReportFilter::default())
        .expect("query totals before the complete schema arrives");
    assert_eq!(totals.orders, 1);
    assert_eq!(totals.unknown_orders, 1);
    assert!(totals.totals.is_empty());
    assert_eq!(totals.valuation.unwrap_or_default().eligible_orders, 0);

    drop(conn);
    std::fs::remove_dir_all(&dir).expect("remove valuation fixture directory");
}

/// Retrying only the physical source that hit valuation corruption, or reusing the first attempt's
/// accumulators, would count the skeleton source twice; a whole native retry must return exactly
/// one unknown row and one USDC row while leaving healthy report writes enabled.
#[test]
fn totals_restart_from_empty_accumulators_after_valuation_corruption() {
    let _health = super::super::valuation::test_health_guard();
    let _integrity = super::super::integrity::test_state_guard();
    super::super::integrity::reset_test_state();
    let dir = std::env::temp_dir().join(format!(
        "moonterminal-report-valuation-retry-{}-{}",
        std::process::id(),
        crate::util::now_unix_ms_i64()
    ));
    std::fs::create_dir_all(&dir).expect("create totals retry fixture");
    let valuation_path = dir.join("valuation.sqlite");
    let store =
        super::super::valuation::open_store(&valuation_path).expect("open valuation retry fixture");
    let transaction = store
        .unchecked_transaction()
        .expect("begin prepared-value seed");
    for row_id in 0..2_000i64 {
        transaction
            .execute(
                "INSERT INTO trade_values (
                     source_kind, core_uid, row_id, algorithm_version, closedate,
                     quote_ordinal, profit_quote, spent_quote, rate_minute_utc,
                     rate_usdt, profit_usdt, spent_usdt, valued_at_ms
                 ) VALUES (1, 1, ?1, 1, 1700000000, 8, 20.0, 100.0, 1699999980,
                           1.0, 20.0, 100.0, 1700000100000)",
                [row_id],
            )
            .expect("seed prepared value");
    }
    transaction.commit().expect("commit prepared values");

    let conn = Connection::open_in_memory().expect("open report retry fixture");
    conn.execute_batch(
        "CREATE TABLE orders_rep (
             core_uid INTEGER NOT NULL, newrecid INTEGER NOT NULL, profitbtc REAL
         );
         INSERT INTO orders_rep VALUES (1, 1, 10.0);
         CREATE TABLE closed_sell_reports (
             core_uid INTEGER NOT NULL, db_id INTEGER NOT NULL, closedate INTEGER,
             basecurrency INTEGER, profitbtc REAL, spentbtc REAL
         );
         INSERT INTO closed_sell_reports VALUES (1, 1, 1700000000, 8, 20.0, 100.0);",
    )
    .expect("seed two physical report sources");
    let attach = format!(
        "ATTACH DATABASE '{}' AS valuation",
        valuation_path
            .to_string_lossy()
            .replace('\\', "/")
            .replace('\'', "''")
    );
    conn.execute(&attach, []).expect("attach healthy valuation");
    super::super::valuation::is_attached(&conn);
    super::super::test_support::corrupt_leaf_page(
        store,
        &valuation_path,
        "sqlite_autoindex_trade_values_1",
    );

    let totals = query_totals(&conn, &ReportFilter::default())
        .expect("fall back to a complete native totals retry");
    assert_eq!(totals.orders, 2);
    assert_eq!(totals.unknown_orders, 1);
    assert_eq!(totals.totals.len(), 1);
    assert_eq!(totals.totals[0].currency.ticker(), "USDC");
    assert_eq!(totals.totals[0].orders, 1);
    assert_eq!(totals.totals[0].profit, 20.0);
    assert!(totals.valuation.is_none());
    assert!(!super::super::integrity::writes_blocked());

    drop(conn);
    super::super::integrity::reset_test_state();
    std::fs::remove_dir_all(dir).expect("remove totals retry fixture");
}

/// Plausible regression: replacing the grouped `basecurrency` selector in
/// `report_read::query_totals` with one global SUM must fail the separate USDT/USDC/BTC
/// assertions and would show users a fictitious cross-currency profit. Grouping the raw column
/// instead of its storage-class-guarded projection also merges malformed REAL 1.0 into USDT.
#[test]
fn totals_split_known_quotes_and_quarantine_unknown_money() {
    let conn = Connection::open_in_memory().expect("open report fixture");
    conn.execute_batch(
        "CREATE TABLE orders_rep (
            core_uid INTEGER NOT NULL,
            core_name TEXT NOT NULL,
            newrecid INTEGER NOT NULL,
            profitbtc REAL,
            basecurrency,
            PRIMARY KEY (core_uid, newrecid)
         );
         INSERT INTO orders_rep
            (core_uid, core_name, newrecid, profitbtc, basecurrency)
         VALUES
            (1, 'A', 1, 10.0, 1),
            (1, 'A', 2, -2.0, 1),
            (2, 'B', 3, 3.5, 8),
            (3, 'C', 4, 0.00000125, 0),
            (4, 'D', 5, 999999.0, NULL),
            (4, 'D', 6, 888888.0, 26),
            (4, 'D', 7, 777777.0, 'USDT'),
            (5, 'E', 8, 666666.0, 1.0);",
    )
    .expect("seed mixed report totals");

    let totals = query_totals(&conn, &ReportFilter::default()).expect("query split totals");

    assert_eq!(totals.orders, 8);
    assert_eq!(totals.unknown_orders, 4);
    assert_eq!(totals.totals.len(), 3);
    assert_eq!(totals.totals[0].currency.ticker(), "BTC");
    assert_eq!(totals.totals[0].profit, 0.00000125);
    assert_eq!(totals.totals[1].currency.ticker(), "USDT");
    assert_eq!(totals.totals[1].profit, 8.0);
    assert_eq!(totals.totals[2].currency.ticker(), "USDC");
    assert_eq!(totals.totals[2].profit, 3.5);
}

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
         ALTER TABLE orders_rep ADD COLUMN basecurrency INTEGER;
         INSERT INTO orders_rep
             (core_uid, core_name, newrecid, closedate, profitbtc, coin, strategyid, deleted,
              isshort, emulator, channelname, signaltype, basecurrency)
         VALUES
             (1, 'CORE-A', 1, 100, 10.0, 'WRONG-CORE', -7, 0, 0, 0, '', '', 1),
             (2, 'CORE-B', 1, 200, 20.0, 'EXPECTED', -7, 0, 0, 0, '', '', 1),
             (2, 'CORE-B', 2, 300, 30.0, 'WRONG-STRATEGY', 8, 0, 0, 0, '', '', 1),
             (2, 'CORE-B', 3, 250, -5.0, 'ATTRIBUTED', 0, 0, 0, 0,
              'LIQUIDATION', 'OWNER ( Kind )', 1),
             (2, 'CORE-B', 4, 0, 90.0, 'UNDATED', -7, 0, 0, 0, '', '', 1),
             (2, 'CORE-B', 5, 350, -9.0, 'ONLY-ATTRIBUTED', 0, 0, 0, 0,
              'LIQUIDATION', 'ONLY-LIQ ( Kind )', 1),
             (2, 'CORE-B', 6, 360, 6.0, 'NULL-MANUAL', NULL, 0, 0, 0, '', '', 1),
             (2, 'CORE-B', 101, 220, 1.0, 'MATCH', 101, 0, 0, 0, '', '', 1),
             (3, 'CORE-C', 102, 220, 1.0, 'MATCH', 102, 0, 0, 0, '', '', 1),
             (2, 'CORE-B', 103, 189, 1.0, 'MATCH', 103, 0, 0, 0, '', '', 1),
             (2, 'CORE-B', 104, 311, 1.0, 'MATCH', 104, 0, 0, 0, '', '', 1),
             (2, 'CORE-B', 105, 220, 1.0, 'OTHER', 105, 0, 0, 0, '', '', 1),
             (2, 'CORE-B', 106, 220, 1.0, 'MATCH', 106, 0, 1, 0, '', '', 1),
             (2, 'CORE-B', 107, 220, 1.0, 'MATCH', 107, 0, 0, 1, '', '', 1),
             (2, 'CORE-B', 108, 220, 1.0, 'MATCH', 108, 1, 0, 0, '', '', 1),
             (2, 'CORE-B', 109, 0, 1.0, 'OPEN-MATCH', 109, 0, 0, 0, '', '', 1);
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
    let totals = query_totals(&conn, &filter).expect("query filtered totals");
    assert_eq!(totals.orders, 2);
    assert_eq!(totals.unknown_orders, 0);
    assert_eq!(totals.totals.len(), 1);
    assert_eq!(totals.totals[0].currency.ticker(), "USDT");
    assert_eq!(totals.totals[0].profit, 15.0);
    assert_eq!(totals.totals[0].orders, 2);
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
    let totals = query_totals(&conn, &multiple).expect("query multiple-strategy totals");
    assert_eq!(totals.orders, 2);
    assert_eq!(totals.unknown_orders, 0);
    assert_eq!(totals.totals.len(), 1);
    assert_eq!(totals.totals[0].currency.ticker(), "USDT");
    assert_eq!(totals.totals[0].profit, 40.0);
    assert_eq!(totals.totals[0].orders, 2);

    let empty = ReportFilter {
        strategies: Some(Vec::new()),
        ..ReportFilter::default()
    };
    assert!(query_reports(&conn, &empty, "closedate", false, 100)
        .expect("query explicit empty strategy set")
        .rows
        .is_empty());
    let empty_totals = query_totals(&conn, &empty).expect("query explicit empty totals");
    assert_eq!(empty_totals.orders, 0);
    assert!(empty_totals.totals.is_empty());
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

/// Per-trade Profit % is derived safely and sorted before each source's top-N truncation.
///
/// Breakage this pins: ordering the source by raw `profitbtc` or deriving the percentage only
/// after `query_reports` applies its source-local LIMIT. The returned top trade would then be the
/// larger quote-denominated profit instead of the larger return on spent capital.
#[test]
fn profit_percent_is_derived_and_source_sorted_before_limit() {
    let conn = Connection::open_in_memory().expect("open report database");
    super::super::init_db(&conn).expect("initialize report database");
    conn.execute_batch(
        "CREATE TABLE orders_rep (
             core_uid INTEGER NOT NULL,
             core_name TEXT NOT NULL,
             newrecid INTEGER NOT NULL,
             closedate INTEGER,
             profitbtc REAL,
             spentbtc REAL,
             PRIMARY KEY (core_uid, newrecid)
         );
         INSERT INTO orders_rep VALUES
             (1, 'CORE-A', 1, 100, 20.0, 2000.0),
             (1, 'CORE-A', 2, 200, 10.0, 100.0),
             (1, 'CORE-A', 3, 300, -5.0, 0.0);",
    )
    .expect("seed report rows");
    super::super::test_support::rep_init(&conn);

    let table = query_reports(
        &conn,
        &ReportFilter::default(),
        super::PROFIT_PERCENT_COLUMN,
        true,
        2,
    )
    .expect("query by Profit percent");
    let percent = table
        .cols
        .iter()
        .position(|column| column == super::PROFIT_PERCENT_COLUMN)
        .expect("synthetic Profit percent column");
    let record = table
        .cols
        .iter()
        .position(|column| column == "id")
        .unwrap_or_else(|| {
            table
                .cols
                .iter()
                .position(|column| column == "closedate")
                .expect("stable row identity")
        });

    assert_eq!(table.rows[0][percent], Value::Real(10.0));
    assert_eq!(table.rows[1][percent], Value::Real(1.0));
    assert_eq!(table.rows[0][record], Value::Integer(200));
    assert!(table.rows.iter().all(|row| row[percent] != Value::Null));
}

/// A legacy visibility preference gains Profit % once, while a current preference may hide it.
///
/// Breakage this pins: continuing to read only `report_visible`, which either leaves the new
/// column hidden forever or re-adds it after the user deliberately disables it.
#[test]
fn report_visibility_migrates_profit_percent_only_once() {
    let conn = Connection::open_in_memory().expect("open metadata database");
    super::super::init_db(&conn).expect("initialize metadata database");
    conn.execute(
        "INSERT INTO app_meta(key, value) VALUES ('report_visible', 'coin,profitbtc')",
        [],
    )
    .expect("seed legacy visibility");
    assert_eq!(
        super::super::load_visible(&conn).expect("migrated visibility"),
        vec!["coin", "profitbtc", super::PROFIT_PERCENT_COLUMN]
    );

    super::super::save_visible(&conn, &["coin", "profitbtc"]);
    assert_eq!(
        super::super::load_visible(&conn).expect("current visibility"),
        vec!["coin", "profitbtc"],
        "the current key respects a deliberate hide"
    );
}
