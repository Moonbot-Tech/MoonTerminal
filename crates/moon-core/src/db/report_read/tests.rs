//! Regression tests for exact Report strategy filtering.

use rusqlite::types::Value;
use rusqlite::Connection;

use super::{
    distinct_strategies, query_chart_trade_history, query_reports, query_totals, QuoteCurrency,
    ReportFilter, ReportStrategyKey, SideFilter,
};

/// Removing the exact core, exact coin, or inclusive close-date predicate from
/// `query_chart_trade_history` admits one of the independently seeded decoys, while filtering on
/// buy date drops record 11 even though its close date is inside the selected Report interval.
#[test]
fn chart_history_keeps_the_exact_core_coin_and_report_close_window() {
    let conn = Connection::open_in_memory().expect("open chart-history fixture");
    conn.execute_batch(
        "CREATE TABLE orders_rep (
             core_uid INTEGER NOT NULL,
             core_name TEXT,
             newrecid INTEGER NOT NULL,
             coin TEXT,
             buydate INTEGER,
             closedate INTEGER,
             buyprice REAL,
             sellprice REAL,
             quantity REAL,
             isshort INTEGER,
             isemulator INTEGER,
             isdeleted INTEGER
         );
         INSERT INTO orders_rep VALUES
             (7, 'A', 11, 'BTCUSDT', 50, 100, 10.0, 12.0, 2.0, 0, 0, 0),
             (7, 'A', 12, 'BTCUSDT', 100, 200, 20.0, 18.0, 3.0, 1, 0, 0),
             (8, 'B', 13, 'BTCUSDT', 110, 150, 30.0, 31.0, 4.0, 0, 0, 0),
             (7, 'A', 14, 'BTCUSDT-PERP', 120, 160, 40.0, 41.0, 5.0, 0, 0, 0),
             (7, 'A', 15, 'BTCUSDT', 130, 201, 50.0, 51.0, 6.0, 0, 0, 0);",
    )
    .expect("seed chart-history fixture");

    let filter = ReportFilter {
        date_from: Some(100),
        date_to: Some(200),
        ..ReportFilter::default()
    };
    let result = query_chart_trade_history(&conn, 7, &["btcusdt".to_string()], Some(&filter), 10)
        .expect("query exact chart history");

    assert_eq!(
        result
            .records
            .iter()
            .map(|record| (record.record_id, record.buy_date, record.close_date))
            .collect::<Vec<_>>(),
        vec![(12, 100, 200), (11, 50, 100)]
    );
    assert!(!result.truncated);
}

/// `report_read.rs:query_chart_trade_history` — replacing `quote::effective_ordinal_expr` with
/// the raw `basecurrency` column reads a COIN-M row's mislabeled persisted currency (USDT)
/// instead of its market-derived one (BTC), so the hover card would show a BTC amount labeled
/// USDT — wrong by the BTC price, and presented as a precise figure.
#[test]
fn chart_history_quote_settles_a_coin_m_liquidation_in_its_true_currency() {
    let conn = Connection::open_in_memory().expect("open coin-m chart-history fixture");
    conn.execute_batch(
        "CREATE TABLE orders_rep (
             core_uid INTEGER NOT NULL,
             newrecid INTEGER NOT NULL,
             coin TEXT,
             fname TEXT,
             buydate INTEGER,
             closedate INTEGER,
             buyprice REAL,
             sellprice REAL,
             quantity REAL,
             isshort INTEGER,
             profitbtc REAL,
             spentbtc REAL,
             basecurrency INTEGER
         );
         INSERT INTO orders_rep VALUES
             (7, 31, 'ETH_0926', 'BinanceQ_USD-ETH_0926_09-09-2025 07-57-19_2.bin',
              100, 200, 3600.0, 3650.0, 0.5, 0, 12.5, 1800.0, 1);",
    )
    .expect("seed coin-m chart-history fixture");

    let result = query_chart_trade_history(&conn, 7, &["ETH_0926".to_string()], None, 10)
        .expect("query coin-m chart history");

    assert_eq!(result.records.len(), 1);
    assert_eq!(
        result.records[0].quote,
        Some(QuoteCurrency::btc()),
        "a COIN-M row's stored basecurrency (USDT) must not override the market-derived quote"
    );
}

/// Removing the limit-plus-one read or stable newest-first order makes the independently seeded
/// record 22 disappear from the first page or hides the fact that record 21 was omitted.
#[test]
fn chart_history_reports_a_deterministic_newest_first_cap() {
    let conn = Connection::open_in_memory().expect("open chart-history cap fixture");
    conn.execute_batch(
        "CREATE TABLE orders_rep (
             core_uid INTEGER NOT NULL, newrecid INTEGER NOT NULL, coin TEXT,
             buydate INTEGER, closedate INTEGER, buyprice REAL, sellprice REAL,
             quantity REAL, isshort INTEGER
         );
         INSERT INTO orders_rep VALUES
             (9, 21, 'ETHUSDT', 10, 20, 1.0, 2.0, 3.0, 0),
             (9, 22, 'ETHUSDT', 11, 30, 2.0, 3.0, 4.0, 1);",
    )
    .expect("seed chart-history cap fixture");

    let result = query_chart_trade_history(&conn, 9, &["ETHUSDT".to_string()], None, 1)
        .expect("query capped chart history");
    assert_eq!(
        result
            .records
            .iter()
            .map(|record| record.record_id)
            .collect::<Vec<_>>(),
        vec![22]
    );
    assert!(result.truncated);
}

/// Keeping the legacy source's synthetic zero rec-id instead of its projected `db_id` makes a
/// Report click unable to focus the independently seeded legacy trade 77.
#[test]
fn chart_history_preserves_legacy_identity_for_clicked_trade_focus() {
    let conn = Connection::open_in_memory().expect("open legacy chart-history fixture");
    conn.execute_batch(
        "CREATE TABLE closed_sell_reports (
             core_uid INTEGER NOT NULL, db_id INTEGER NOT NULL, coin TEXT,
             buydate INTEGER, closedate INTEGER, buyprice REAL, sellprice REAL,
             quantity REAL, isshort INTEGER
         );
         CREATE TABLE orders_rep (core_uid INTEGER NOT NULL, newrecid INTEGER NOT NULL);
         INSERT INTO closed_sell_reports VALUES
             (5, 77, 'SOLUSDT', 100, 200, 9.0, 11.0, 4.0, 0);",
    )
    .expect("seed legacy chart-history fixture");

    let result = query_chart_trade_history(&conn, 5, &["SOLUSDT".to_string()], None, 10)
        .expect("query legacy chart history");
    assert_eq!(
        result
            .records
            .iter()
            .map(|record| record.record_id)
            .collect::<Vec<_>>(),
        vec![77]
    );
}

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

/// Seed long, short, open, Funding, and out-of-filter trades for the volume oracle.
///
/// Args:
///     quote: Persisted quote ordinal assigned to every fixture row.
///
/// Returns:
///     An in-memory report replica whose stored spend is deliberately unrelated to notional.
fn traded_volume_report(quote: i64) -> Connection {
    let conn = Connection::open_in_memory().expect("open traded-volume report fixture");
    conn.execute_batch(
        "CREATE TABLE orders_rep (
             core_uid INTEGER NOT NULL, newrecid INTEGER NOT NULL, closedate INTEGER,
             basecurrency INTEGER, profitbtc REAL, spentbtc REAL, boughtq REAL,
             buyprice REAL, sellprice REAL, sellreason TEXT, isshort INTEGER
         );",
    )
    .expect("create traded-volume report schema");
    let rows = [
        // Long: 2 * 100 entry + 2 * 110 exit = 420.
        (
            1,
            1,
            1_700_000_000,
            20.0,
            1.0,
            2.0,
            100.0,
            110.0,
            "Sell Price",
            0,
        ),
        // Short: 3 * 80 entry + 3 * 70 exit = 450, still unsigned.
        (
            1,
            2,
            1_700_000_060,
            30.0,
            2.0,
            3.0,
            80.0,
            70.0,
            "Buy Price",
            1,
        ),
        // Report profit/count include these two rows; volume eligibility does not.
        (1, 3, 0, 40.0, 3.0, 5.0, 10.0, 20.0, "Sell Price", 0),
        (1, 4, 1_700_000_120, 50.0, 4.0, 4.0, 5.0, 6.0, "Funding", 0),
        // The Report filter excludes this otherwise valid closed trade from every aggregate.
        (
            2,
            5,
            1_700_000_180,
            60.0,
            5.0,
            10.0,
            1.0,
            2.0,
            "Sell Price",
            0,
        ),
    ];
    for (core, id, closedate, profit, spent, qty, buy, sell, reason, short) in rows {
        conn.execute(
            "INSERT INTO orders_rep VALUES
             (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            rusqlite::params![
                core, id, closedate, quote, profit, spent, qty, buy, sell, reason, short
            ],
        )
        .expect("seed traded-volume row");
    }
    conn
}

/// Removing the exit leg, signing short quantity, reusing `spentbtc`, moving the closed/Funding
/// predicates into `build_where`, or coupling volume completeness to profit coverage turns one of
/// these exact assertions red and would misstate the current filtered Report footer.
#[test]
fn filtered_traded_volume_is_unsigned_two_sided_and_uses_the_active_rate_mode() {
    let filter = |valuation| ReportFilter {
        core_uids: vec![1],
        valuation,
        ..ReportFilter::default()
    };

    let current_conn = traded_volume_report(1);
    current_conn
        .execute("UPDATE orders_rep SET profitbtc=NULL WHERE newrecid=2", [])
        .expect("remove profit without changing the short trade's price legs");
    let current = query_totals(&current_conn, &filter(super::ValuationMode::Current))
        .expect("query current-rate traded volume");
    assert_eq!(
        current.orders, 4,
        "volume eligibility must not change Report count"
    );
    assert_eq!(
        current.totals[0].profit, 110.0,
        "profit behavior stays unchanged"
    );
    assert_eq!(current.traded_volume.eligible_orders, 2);
    assert_eq!(current.traded_volume.reconstructed_orders, 2);
    assert_eq!(current.traded_volume.totals[0].amount, Some(870.0));
    assert_eq!(
        current.traded_volume.usdt,
        Some(870.0),
        "current USDT identity rate is 1.0"
    );

    let _health = super::super::valuation::test_health_guard();
    let dir = per_row_dir("traded-volume");
    let valuation_path = dir.join("valuation.sqlite");
    let store = super::super::valuation::open_store(&valuation_path)
        .expect("initialize traded-volume valuation fixture");
    store
        .execute_batch(
            "INSERT INTO trade_values (
                 source_kind, core_uid, row_id, algorithm_version, closedate, quote_ordinal,
                 profit_quote, spent_quote, rate_minute_utc, rate_usdt, profit_usdt, spent_usdt,
                 valued_at_ms
             ) VALUES
                 (0, 1, 1, 2, 1700000000, 8, 20.0, 1.0, 1699999980, 0.5, 10.0, 0.5, 1700000100000),
                 (0, 1, 2, 2, 1700000060, 8, 30.0, 2.0, 1700000040, 0.5, 15.0, 1.0, 1700000160000);",
        )
        .expect("seed historical traded-volume rates");
    drop(store);
    let historical_conn = traded_volume_report(8);
    attach_valuation(&historical_conn, &valuation_path);
    let historical = query_totals(&historical_conn, &filter(super::ValuationMode::Historical))
        .expect("query historical traded volume");
    assert_eq!(historical.orders, 4);
    assert_eq!(historical.totals[0].profit, 140.0);
    assert_eq!(historical.traded_volume.totals[0].amount, Some(870.0));
    assert_eq!(
        historical.traded_volume.usdt,
        Some(435.0),
        "historical fixture rate is deliberately 0.5 rather than current identity 1.0"
    );

    drop(historical_conn);
    std::fs::remove_dir_all(dir).expect("remove traded-volume fixture directory");
}

/// Publishing a SUM when one eligible row is inverse-denominated or an explicit liquidation
/// would present a confident partial or dimensionally invalid amount as the whole filter total.
#[test]
fn traded_volume_withholds_unprovable_rows_instead_of_publishing_a_partial_sum() {
    let conn = Connection::open_in_memory().expect("open incomplete-volume fixture");
    conn.execute_batch(
        "CREATE TABLE orders_rep (
             core_uid INTEGER NOT NULL, newrecid INTEGER NOT NULL, closedate INTEGER,
             basecurrency INTEGER, profitbtc REAL, spentbtc REAL, boughtq REAL,
             buyprice REAL, sellprice REAL, sellreason TEXT, coin TEXT, fname TEXT
         );
         INSERT INTO orders_rep VALUES
             (2, 1, 1700000000, 1, 1.0, 9.0, 2.0, 100.0, 110.0,
              'Sell Price', 'BTC', NULL),
             (2, 2, 1700000060, 1, 2.0, 8.0, 3.0, 80.0, 70.0,
              'LIQUIDATION', 'ETH', NULL),
             (1, 3, 1700000120, 1, 3.0, 7.0, 5.0, 3000.0, 3100.0,
              'Sell Price', 'ETH_0927', 'Pump_USD-ETH_0927_x'),
             (1, 4, 1700000180, 1, 4.0, 6.0, 7.0, 2000.0, 2100.0,
              'Sell Price', NULL, NULL),
             (2, 6, 1700000300, 1, 6.0, 4.0, 2.0, 40.0, 50.0,
              NULL, 'SOL', NULL);
         CREATE TABLE closed_sell_reports (
             core_uid INTEGER NOT NULL, db_id INTEGER NOT NULL, closedate INTEGER,
             basecurrency INTEGER, profitbtc REAL, spentbtc REAL, boughtq REAL,
             buyprice REAL, sellprice REAL
         );
         INSERT INTO closed_sell_reports VALUES
             (3, 5, 1700000240, 1, 5.0, 5.0, 8.0, 10.0, 11.0);",
    )
    .expect("seed incomplete-volume rows");

    let totals = query_totals(&conn, &ReportFilter::default()).expect("query incomplete volume");
    assert_eq!(totals.orders, 6);
    assert_eq!(totals.traded_volume.eligible_orders, 6);
    assert_eq!(totals.traded_volume.reconstructed_orders, 1);
    assert_eq!(totals.traded_volume.usdt, None);
    assert!(
        totals
            .traded_volume
            .totals
            .iter()
            .all(|bucket| bucket.amount.is_none()),
        "neither the ordinary-row subtotal nor contract quantity times USD prices is publishable"
    );
}

/// Build two typed cores with the same signed strategy id plus an unidentifiable legacy row.
///
/// Returns:
///     An in-memory report database with strategy metadata and attribution edge cases.
fn strategy_fixture() -> Connection {
    let conn = Connection::open_in_memory().expect("open in-memory report database");
    super::super::init_db(&conn).expect("initialize report database");
    conn.execute_batch(
        r#"CREATE TABLE IF NOT EXISTS orders_rep (
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
              'LIQUIDATION', 'EMA_01 ( Kind )', 1),
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
             (2, 'CORE-B', 109, 0, 1.0, 'OPEN-MATCH', 109, 0, 0, 0, '', '', 1),
             (2, 'CORE-B', 110, 220, 11.0, 'MASK-LOWER', 110, 0, 0, 0, '', '', 1),
             (2, 'CORE-B', 111, 220, 12.0, 'MASK-UNDER', 111, 0, 0, 0, '', '', 1),
             (2, 'CORE-B', 112, 220, 13.0, 'MASK-PERCENT', 112, 0, 0, 0, '', '', 1),
             (2, 'CORE-B', 113, 220, 14.0, 'MASK-SLASH', 113, 0, 0, 0, '', '', 1),
             (2, 'CORE-B', 114, 220, 15.0, 'MASK-NO-UNDER', 114, 0, 0, 0, '', '', 1),
             (2, 'CORE-B', 116, 220, 17.0, 'MASK-UNICODE', 116, 0, 0, 0, '', '', 1),
             (2, 'CORE-B', 117, 220, 18.0, 'MASK-FULL-FOLD', 117, 0, 0, 0, '', '', 1),
             (1, 'CORE-A', 115, 220, 16.0, 'MASK-WRONG-CORE', 115, 0, 0, 0, '', '', 1);
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
         VALUES (2, -7, 'EMA_01', 0),
                (2, -9, 'ONLY-LIQ', 0),
                (2, 110, 'ema_03', 0),
                (2, 111, 'EMA_04', 0),
                (2, 112, 'EMA%05', 0),
                (2, 113, 'EMA\05', 0),
                (2, 114, 'EMAX01', 0),
                (2, 116, 'ЕМА_РУС', 0),
                (2, 117, 'STRASSE_PLAN', 0),
                (1, 115, 'EMA_07', 0);"#,
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
    assert_eq!(choice.name, "EMA_01");
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

/// `report_read::append_strategy_name_mask` must keep user text literal, case-insensitive, and
/// correlated to the current core plus effective strategy id.
///
/// Plausible breakages: replacing `instr` with an unescaped `LIKE` makes `_` or `%` a wildcard;
/// dropping Unicode-aware folding loses lower-case or non-ASCII strategy names; matching raw
/// `strategyid` loses attributed
/// liquidations; dropping `core_uid` includes a same-pattern strategy from another core; treating
/// missing strategy metadata as implicit All exposes unrelated rows; and leaving the mask in
/// `distinct_strategies` makes the exact picker self-lock to the typed mask.
///
/// Consequence: Auto Report rows, totals, export, and its exact strategy picker disagree about
/// which trades `EMA_` represents.
#[test]
fn strategy_name_mask_is_literal_scoped_and_shared_by_rows_totals() {
    let conn = strategy_fixture();
    let coins = |filter: &ReportFilter| {
        let table = query_reports(&conn, filter, "closedate", false, 100)
            .expect("query strategy-name mask");
        let coin = table
            .cols
            .iter()
            .position(|column| column == "coin")
            .expect("coin column");
        table
            .rows
            .iter()
            .map(|row| match &row[coin] {
                Value::Text(text) => text.clone(),
                value => panic!("expected coin text, got {value:?}"),
            })
            .collect::<std::collections::BTreeSet<_>>()
    };
    let filter = ReportFilter {
        core_uids: vec![2],
        closed_only: true,
        strategy_name_mask: " eMa_ ".to_string(),
        ..ReportFilter::default()
    };

    assert_eq!(
        coins(&filter),
        std::collections::BTreeSet::from([
            "ATTRIBUTED".to_string(),
            "EXPECTED".to_string(),
            "MASK-LOWER".to_string(),
            "MASK-UNDER".to_string(),
        ])
    );
    let totals = query_totals(&conn, &filter).expect("query masked totals");
    assert_eq!(totals.orders, 4);
    assert_eq!(totals.totals.len(), 1);
    assert_eq!(totals.totals[0].profit, 38.0);

    let exact_and_mask = ReportFilter {
        strategies: Some(vec![
            ReportStrategyKey {
                core_uid: 2,
                strategy_id: -7,
            },
            ReportStrategyKey {
                core_uid: 2,
                strategy_id: 8,
            },
        ]),
        ..filter.clone()
    };
    assert_eq!(
        coins(&exact_and_mask),
        std::collections::BTreeSet::from(["ATTRIBUTED".to_string(), "EXPECTED".to_string(),])
    );

    for (mask, expected) in [("%", "MASK-PERCENT"), ("\\", "MASK-SLASH")] {
        let special = ReportFilter {
            strategy_name_mask: mask.to_string(),
            ..filter.clone()
        };
        assert_eq!(
            coins(&special),
            std::collections::BTreeSet::from([expected.to_string()]),
            "{mask:?} must remain a literal substring"
        );
    }

    let unicode = ReportFilter {
        strategy_name_mask: "ема_".to_string(),
        ..filter.clone()
    };
    assert_eq!(
        coins(&unicode),
        std::collections::BTreeSet::from(["MASK-UNICODE".to_string()]),
        "strategy-name case folding must not stop at ASCII"
    );

    let full_fold = ReportFilter {
        strategy_name_mask: "straße_".to_string(),
        ..filter.clone()
    };
    assert_eq!(
        coins(&full_fold),
        std::collections::BTreeSet::from(["MASK-FULL-FOLD".to_string()]),
        "full case folding must match multi-character Unicode equivalents"
    );

    let blank = ReportFilter {
        strategy_name_mask: " \t ".to_string(),
        ..filter.clone()
    };
    let no_mask = ReportFilter {
        strategy_name_mask: String::new(),
        ..filter.clone()
    };
    assert_eq!(coins(&blank), coins(&no_mask));

    let catalog = distinct_strategies(&conn, &filter).expect("load mask-independent catalog");
    assert!(catalog.iter().any(|strategy| {
        strategy.key
            == ReportStrategyKey {
                core_uid: 2,
                strategy_id: 8,
            }
    }));

    let no_metadata = Connection::open_in_memory().expect("open report database without names");
    super::super::init_db(&no_metadata).expect("initialize report metadata");
    no_metadata
        .execute_batch(
            "CREATE TABLE orders_rep (
                 core_uid INTEGER NOT NULL,
                 core_name TEXT NOT NULL,
                 newrecid INTEGER NOT NULL,
                 closedate INTEGER,
                 profitbtc REAL,
                 strategyid INTEGER,
                 basecurrency INTEGER,
                 PRIMARY KEY (core_uid, newrecid)
             );
             INSERT INTO orders_rep VALUES (2, 'CORE-B', 1, 100, 99.0, 7, 1);",
        )
        .expect("seed report without strategy metadata");
    assert!(
        query_reports(&no_metadata, &filter, "closedate", false, 100)
            .expect("a name mask without metadata must stay a valid empty query")
            .rows
            .is_empty(),
        "missing strategy metadata must not broaden a non-empty mask"
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
        exact_coins: None,
        side: SideFilter::Long,
        emulator: Some(false),
        deleted_only: false,
        closed_only: true,
        strategies: Some(vec![ReportStrategyKey {
            core_uid: 2,
            strategy_id: -7,
        }]),
        strategy_name_mask: "ignored-catalog-mask".to_string(),
        valuation: Default::default(),
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
        vec![
            "coin",
            "profitbtc",
            super::PROFIT_PERCENT_COLUMN,
            super::VALUATION_PROFIT_COLUMN
        ],
        "the oldest key predates both later columns and must restore them in order"
    );

    super::super::save_visible(&conn, &["coin", "profitbtc"]);
    assert_eq!(
        super::super::load_visible(&conn).expect("current visibility"),
        vec!["coin", "profitbtc"],
        "the current key respects a deliberate hide"
    );
}

/// A `v2` set predates the USDT profit column, so reading one must restore it — exactly once, and
/// never again after the user has saved a set that deliberately omits it.
///
/// Breakage: shipping `valuation_profit_usdt` without adding a `report_visible_v3` step, so the
/// column is invisible to every user who has ever opened the Columns menu — the developer who
/// asked for it first among them. Or letting the migration key off column membership rather than
/// the schema key, which would re-add the column on every load and make hiding it impossible.
#[test]
fn report_visibility_restores_the_usdt_profit_column_from_a_v2_set() {
    let conn = Connection::open_in_memory().expect("open metadata database");
    super::super::init_db(&conn).expect("initialize metadata database");
    conn.execute(
        "INSERT INTO app_meta(key, value) VALUES ('report_visible_v2', 'coin,profitbtc,profitpct')",
        [],
    )
    .expect("seed a v2 visibility set");

    assert_eq!(
        super::super::load_visible(&conn).expect("migrated visibility"),
        vec![
            "coin",
            "profitbtc",
            super::PROFIT_PERCENT_COLUMN,
            super::VALUATION_PROFIT_COLUMN
        ]
    );

    super::super::save_visible(&conn, &["coin", "profitbtc"]);
    assert_eq!(
        super::super::load_visible(&conn).expect("current visibility"),
        vec!["coin", "profitbtc"],
        "a v3 set is authoritative, so hiding the new column sticks"
    );
}

/// Create a private directory for one per-row valuation fixture.
fn per_row_dir(tag: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "moonterminal-report-per-row-{tag}-{}-{}",
        std::process::id(),
        crate::util::now_unix_ms_i64()
    ));
    std::fs::create_dir_all(&dir).expect("create per-row fixture directory");
    dir
}

/// Seed a report replica with a prepared trade, an identity trade, an uncovered trade, and two
/// legacy trades whose natural order disagrees with their converted profit.
///
/// The typed table deliberately carries a `status` column: the valuation joins bring their own
/// `status`, `closedate` and `core_uid` into scope, so an unqualified projection would stop
/// compiling as SQL rather than merely returning the wrong number.
///
/// Returns:
///     Open report connection, before the derived cache is attached.
fn seed_per_row_report() -> Connection {
    let conn = Connection::open_in_memory().expect("open per-row report fixture");
    conn.execute_batch(
        "CREATE TABLE orders_rep (
             core_uid INTEGER NOT NULL, core_name TEXT NOT NULL, newrecid INTEGER NOT NULL,
             closedate INTEGER, buydate INTEGER, profitbtc REAL, spentbtc REAL,
             basecurrency INTEGER, coin TEXT, status INTEGER,
             PRIMARY KEY (core_uid, newrecid)
         );
         INSERT INTO orders_rep VALUES
             (1, 'A', 1, 1700000000, 1699999700, 20.0, 100.0, 8, 'ETHUSDC', 3),
             (1, 'A', 2, 1700000060, 1699999760, 7.5, 50.0, 1, 'BTCUSDT', 3),
             (1, 'A', 3, 1700000120, 1699999820, -1.0, 10.0, 8, 'SOLUSDC', 3);
         CREATE TABLE closed_sell_reports (
             core_uid INTEGER NOT NULL, db_id INTEGER NOT NULL, closedate INTEGER,
             basecurrency INTEGER, profitbtc REAL, spentbtc REAL, coin TEXT
         );
         INSERT INTO closed_sell_reports VALUES
             (1, 2, 1700000700, 8, 2.0, 20.0, 'ADAUSDC'),
             (1, 1, 1700000600, 8, 400.0, 4000.0, 'XRPUSDC');",
    )
    .expect("seed per-row report sources");
    conn
}

/// Seed the derived cache matching [`seed_per_row_report`].
///
/// The rate is a deliberately unrealistic 0.5 USDT per USDC so every converted figure is exactly
/// half its native one — an arithmetic relation the test can check rather than a number it has to
/// take on trust.
///
/// Args:
///     path: Valuation store path inside the fixture directory.
///
/// Returns:
///     Open valuation store, so a caller may damage it.
fn seed_per_row_valuation(path: &std::path::Path) -> Connection {
    let store = super::super::valuation::open_store(path).expect("open per-row valuation fixture");
    store
        .execute_batch(
            "INSERT INTO trade_values (
                 source_kind, core_uid, row_id, algorithm_version, closedate, quote_ordinal,
                 profit_quote, spent_quote, rate_minute_utc, rate_usdt, profit_usdt, spent_usdt,
                 valued_at_ms
             ) VALUES
                 (0, 1, 1, 2, 1700000000, 8, 20.0, 100.0, 1699999980, 0.5, 10.0, 50.0, 1700000100000),
                 (1, 1, 1, 2, 1700000600, 8, 400.0, 4000.0, 1699999980, 0.5, 200.0, 2000.0, 1700000100000),
                 (1, 1, 2, 2, 1700000700, 8, 2.0, 20.0, 1699999980, 0.5, 1.0, 10.0, 1700000100000);
             INSERT INTO rates (
                 algorithm_version, quote_ordinal, minute_utc, resolved_minute_utc, rate_usdt,
                 price_basis, provider, symbol, orientation, candle_open_ms, candle_close_ms,
                 leg1_rate, leg2_provider, leg2_symbol, leg2_orientation, leg2_rate, fetched_at_ms
             ) VALUES (2, 8, 1699999980, 1700000100, 0.5, 1, 'binance', 'USDCBTC', 0,
                       1700000100000, 1700000159999, 0.00001,
                       'bybit', 'BTCUSDT', 0, 50000.0, 1700000200000);",
        )
        .expect("seed prepared values and their rate provenance");
    store
}

/// Attach a valuation store to a report connection under the reader-facing schema name.
fn attach_valuation(conn: &Connection, path: &std::path::Path) {
    let sql = format!(
        "ATTACH DATABASE '{}' AS {}",
        path.to_string_lossy()
            .replace('\\', "/")
            .replace('\'', "''"),
        super::super::valuation::SCHEMA,
    );
    conn.execute(&sql, []).expect("attach valuation fixture");
}

/// Read one report cell by runtime column name.
fn cell<'a>(table: &'a super::ReportTable, row: usize, column: &str) -> &'a Value {
    let index = table
        .cols
        .iter()
        .position(|name| name == column)
        .unwrap_or_else(|| panic!("column {column} is present in the runtime schema"));
    &table.rows[row][index]
}

/// Read one report cell as a number.
fn number(table: &super::ReportTable, row: usize, column: &str) -> Option<f64> {
    match cell(table, row, column) {
        Value::Real(value) => Some(*value),
        Value::Integer(value) => Some(*value as f64),
        _ => None,
    }
}

/// Read one report cell as text.
fn text(table: &super::ReportTable, row: usize, column: &str) -> Option<String> {
    match cell(table, row, column) {
        Value::Text(value) => Some(value.clone()),
        _ => None,
    }
}

/// A prepared row reports its converted profit, the rate that produced it, and where that rate
/// came from; an identity row answers from the trade itself; an unvalued row stays blank.
///
/// Breakage: reading provenance through an unresolved-search row instead of the ready-rate join
/// blanks the source column on every valued row. Breakage: dropping the identity arm from any of the three
/// expressions and treating a NULL provider as "not valued", which blanks that column for USDT
/// trades — the majority. Breakage: emitting the projection unqualified once the valuation joins
/// are in the `FROM`, which makes `status` ambiguous and fails the whole read. Breakage: handing
/// the row query the aggregate join set, which omits the `ra` provenance alias this projection
/// names.
#[test]
fn per_row_columns_report_converted_profit_and_rate_provenance() {
    let _health = super::super::valuation::test_health_guard();
    let dir = per_row_dir("values");
    let valuation_path = dir.join("valuation.sqlite");
    drop(seed_per_row_valuation(&valuation_path));

    let conn = seed_per_row_report();
    attach_valuation(&conn, &valuation_path);
    let table = query_reports(&conn, &ReportFilter::default(), "closedate", false, 100)
        .expect("read rows with the derived cache attached");

    let coins: Vec<Option<String>> = (0..table.rows.len())
        .map(|row| text(&table, row, "coin"))
        .collect();
    let row_of = |coin: &str| {
        coins
            .iter()
            .position(|name| name.as_deref() == Some(coin))
            .unwrap_or_else(|| panic!("fixture row {coin} is present"))
    };

    let prepared = row_of("ETHUSDC");
    let profit = number(&table, prepared, super::VALUATION_PROFIT_COLUMN)
        .expect("a prepared row carries a converted profit");
    let rate =
        number(&table, prepared, super::VALUATION_RATE_COLUMN).expect("and the rate applied to it");
    let native = number(&table, prepared, "profitbtc").expect("beside its native profit");
    assert_ne!(
        rate, 1.0,
        "a prepared row must report the CACHED rate; rate 1.0 would mean it fell into the \
         identity arm and the relation below would hold for the wrong reason"
    );
    assert_eq!(
        profit,
        native * rate,
        "the converted profit must be the native profit at the reported rate"
    );
    assert_eq!(
        text(&table, prepared, super::VALUATION_SOURCE_COLUMN).as_deref(),
        Some("binance USDCBTC -> bybit BTCUSDT +2m"),
        "provenance names both markets and the successor delay that produced the rate"
    );

    let identity = row_of("BTCUSDT");
    assert_eq!(
        number(&table, identity, super::VALUATION_PROFIT_COLUMN),
        number(&table, identity, "profitbtc"),
        "a USDT trade is already valued and needs no cached rate"
    );
    assert_eq!(
        number(&table, identity, super::VALUATION_RATE_COLUMN),
        Some(1.0)
    );
    assert_eq!(
        text(&table, identity, super::VALUATION_SOURCE_COLUMN).as_deref(),
        Some("identity")
    );

    let uncovered = row_of("SOLUSDC");
    for column in [
        super::VALUATION_PROFIT_COLUMN,
        super::VALUATION_RATE_COLUMN,
        super::VALUATION_SOURCE_COLUMN,
    ] {
        assert!(
            matches!(cell(&table, uncovered, column), Value::Null),
            "an unvalued row must stay blank in {column} rather than invent a figure"
        );
    }

    drop(conn);
    std::fs::remove_dir_all(&dir).expect("remove per-row fixture directory");
}

/// Sorting by a synthetic valuation column must return the true global maximum, not each source's
/// arbitrary first row.
///
/// Every physical source is truncated by `LIMIT` before the Rust merge, so a column that projects
/// but cannot sort degrades that source's `ORDER BY` to the constant `1` — silently, with no error.
/// The legacy fixture rows are stored in the opposite order to their converted profit precisely so
/// that the constant ordering picks the wrong one.
///
/// Breakage: adding a synthetic column to `source_select` and forgetting `source_sort_expression`,
/// which returns 10.0 (the typed source's best) as the global maximum instead of 200.0.
#[test]
fn sorting_by_converted_profit_crosses_both_physical_sources() {
    let _health = super::super::valuation::test_health_guard();
    let dir = per_row_dir("sort");
    let valuation_path = dir.join("valuation.sqlite");
    drop(seed_per_row_valuation(&valuation_path));

    let conn = seed_per_row_report();
    attach_valuation(&conn, &valuation_path);
    let table = query_reports(
        &conn,
        &ReportFilter::default(),
        super::VALUATION_PROFIT_COLUMN,
        true,
        1,
    )
    .expect("sort across both sources by converted profit");

    assert_eq!(table.rows.len(), 1);
    assert_eq!(
        number(&table, 0, super::VALUATION_PROFIT_COLUMN),
        Some(200.0)
    );
    assert_eq!(text(&table, 0, "coin").as_deref(), Some("XRPUSDC"));

    drop(conn);
    std::fs::remove_dir_all(&dir).expect("remove per-row fixture directory");
}

/// Every synthetic column must be expressible for BOTH projection and sorting on a source that
/// carries the inputs.
///
/// Breakage: the next synthetic column wired into `source_select` only. That half-wiring produces
/// no error and no warning — just a wrong top-N — so this structural check is the only thing
/// standing between it and a release.
#[test]
fn every_synthetic_column_can_both_project_and_sort() {
    let columns: std::collections::HashSet<String> = [
        "core_uid",
        "newrecid",
        "closedate",
        "basecurrency",
        "profitbtc",
        "spentbtc",
    ]
    .iter()
    .map(|name| (*name).to_string())
    .collect();
    let source = super::super::ReadSource {
        table: "orders_rep",
        cols: columns.clone(),
        legacy: false,
    };
    let valuation = super::super::valuation::coverage_sql(
        "r",
        &columns,
        super::super::valuation::TradeSource::Typed,
    );

    assert!(
        !super::SYNTHETIC.is_empty(),
        "the registry must not be empty, or this test asserts nothing at all"
    );
    for entry in super::SYNTHETIC {
        let column = entry.name;
        let names = vec![column.to_string()];
        let select = super::source_select(&source, &names, Some(&valuation));
        assert!(
            !select.starts_with("NULL"),
            "{column} must project an expression on a full-schema source"
        );
        // No expression means `query_reports_attempt` falls back to the constant `1`, which drops
        // this source's ordering entirely and hands the merge an arbitrary top-N.
        super::source_sort_expression(&source, column, Some(&valuation))
            .unwrap_or_else(|| panic!("{column} must be sortable on a full-schema source"));
        assert!(
            super::super::DISPLAY_COLUMNS.contains(&column),
            "{column} is registered but never offered, so nothing can ever render it"
        );
    }
}

/// A corrupt derived cache must cost the USDT columns and nothing else.
///
/// `query_reports` is the read behind both the table and the export, so failing it closed would
/// blank the panel and refuse the export over a cache the user never asked for — while the report
/// replica itself is perfectly healthy. The retry must also keep the SAME column list, or the
/// panel's `cols` would desynchronise from its `rows`.
///
/// Breakage: adding the valuation join to `query_reports` without routing it through
/// `with_valuation_fallback`, which turns every row read into `Failed`. Breakage: gating
/// `display_columns` on `valuation::is_attached`, which changes the column set between the two
/// attempts.
#[test]
fn corrupt_derived_cache_blanks_the_usdt_columns_not_the_rows() {
    let _health = super::super::valuation::test_health_guard();
    let _integrity = super::super::integrity::test_state_guard();
    super::super::integrity::reset_test_state();
    let dir = per_row_dir("corrupt");
    let valuation_path = dir.join("valuation.sqlite");
    let store = seed_per_row_valuation(&valuation_path);
    // Enough rows that the damaged leaf sits away from the header, so the failure happens when a
    // query reaches that page rather than when the file opens.
    let transaction = store.unchecked_transaction().expect("begin bulk seed");
    for row_id in 100..2_100i64 {
        transaction
            .execute(
                "INSERT INTO trade_values (
                     source_kind, core_uid, row_id, algorithm_version, closedate, quote_ordinal,
                     profit_quote, spent_quote, rate_minute_utc, rate_usdt, profit_usdt,
                     spent_usdt, valued_at_ms
                 ) VALUES (0, 1, ?1, 1, 1700000000, 8, 20.0, 100.0, 1699999980,
                           0.5, 10.0, 50.0, 1700000100000)",
                [row_id],
            )
            .expect("seed filler prepared value");
    }
    transaction.commit().expect("commit filler rows");

    let conn = seed_per_row_report();
    attach_valuation(&conn, &valuation_path);
    super::super::valuation::is_attached(&conn);
    super::super::test_support::corrupt_leaf_page(
        store,
        &valuation_path,
        "sqlite_autoindex_trade_values_1",
    );

    let table = query_reports(&conn, &ReportFilter::default(), "closedate", false, 100)
        .expect("rows survive a corrupt derived cache");
    assert_eq!(table.rows.len(), 5, "every fixture row is still returned");
    assert!(
        table
            .cols
            .iter()
            .any(|name| name == super::VALUATION_PROFIT_COLUMN),
        "the column set is resolved from the report schema and does not change on retry"
    );
    for row in 0..table.rows.len() {
        assert!(
            matches!(
                cell(&table, row, super::VALUATION_PROFIT_COLUMN),
                Value::Null
            ),
            "a cache that cannot be read supplies no conversion"
        );
    }
    assert!(!super::super::integrity::writes_blocked());

    drop(conn);
    super::super::integrity::reset_test_state();
    std::fs::remove_dir_all(&dir).expect("remove per-row fixture directory");
}

/// Current-rate coverage must be published even when the historical cache is unavailable.
///
/// Breakage: `report_read.rs::query_totals_attempt` gating `with_valuation` on `include_valuation`
/// — the derived cache's attach state — instead of on the projection actually existing. The
/// current-rate mode needs no cache, so a detached or recovering `valuation.sqlite` would make the
/// footer compute a perfectly good USDT total and then throw it away, degrading a convertible
/// scope to split per-currency totals for a reason that has nothing to do with it.
#[test]
fn current_rate_coverage_survives_an_unavailable_historical_cache() {
    let conn = Connection::open_in_memory().expect("in-memory database");
    conn.execute_batch(
        "CREATE TABLE orders_rep (core_uid INTEGER NOT NULL, closedate INTEGER,
                                  basecurrency INTEGER, profitbtc REAL, spentbtc REAL,
                                  newrecid INTEGER, deleted INTEGER);
         INSERT INTO orders_rep VALUES (1, 1000, 1, 7.5, 10.0, 1, 0)",
    )
    .expect("schema");
    let sources = super::read_sources_res(&conn).expect("sources");
    let filter = |mode| ReportFilter {
        valuation: mode,
        ..Default::default()
    };

    // `false` is the cache-unavailable case both modes are asked about.
    let current = super::query_totals_attempt(
        &conn,
        &filter(super::ValuationMode::Current),
        &sources,
        false,
    )
    .expect("current-rate totals");
    assert!(
        current.valuation.is_some(),
        "the in-memory conversion does not depend on the derived cache"
    );
    assert_eq!(current.valuation.expect("coverage").valued_orders, 1);

    let historical = super::query_totals_attempt(
        &conn,
        &filter(super::ValuationMode::Historical),
        &sources,
        false,
    )
    .expect("historical totals");
    assert!(
        historical.valuation.is_none(),
        "the historical conversion has nothing to report without its cache"
    );
}

/// The USDT profit column is offered BEFORE the percentage one.
///
/// Breakage this pins: swapping the entries in `report_read.rs:DISPLAY_COLUMNS` would move the
/// amount to the right in the table, Columns menu, and CSV export. The runtime schema-derived list
/// is the shared consumer rather than a second reading of the constant.
#[test]
fn the_usdt_profit_column_precedes_the_percentage_one() {
    let conn = Connection::open_in_memory().expect("open column-order fixture");
    super::super::init_db(&conn).expect("initialize report database");
    // The two synthetic columns are offered on their INPUTS, so the fixture carries exactly those.
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS orders_rep (
             core_uid INTEGER NOT NULL,
             core_name TEXT NOT NULL,
             newrecid INTEGER NOT NULL,
             PRIMARY KEY (core_uid, newrecid)
         );
         ALTER TABLE orders_rep ADD COLUMN closedate INTEGER;
         ALTER TABLE orders_rep ADD COLUMN basecurrency INTEGER;
         ALTER TABLE orders_rep ADD COLUMN profitbtc REAL;
         ALTER TABLE orders_rep ADD COLUMN spentbtc REAL;",
    )
    .expect("create typed report source");

    let cols = super::display_columns(&conn).expect("a healthy schema lists its columns");
    let at = |name: &str| {
        cols.iter()
            .position(|col| col == name)
            .unwrap_or_else(|| panic!("{name} must be offered on a schema carrying its inputs"))
    };
    assert!(
        at(super::VALUATION_PROFIT_COLUMN) < at(super::PROFIT_PERCENT_COLUMN),
        "the USDT amount reads first and the percentage sits to its right: {cols:?}"
    );
}

/// Build a report replica carrying every column the purge predicate can name, with an attached
/// (in-memory) strategy database so liquidation attribution is live.
///
/// Returns an open connection; the caller seeds rows itself so each test states its own fixture.
fn purge_fixture() -> Connection {
    let conn = Connection::open_in_memory().expect("open purge fixture");
    super::super::init_db(&conn).expect("initialize report database");
    // The replica table is created by the schema-driven replication path, not by `init_db`, so the
    // fixture builds it the way an upgraded database looks: bare skeleton, then core-added columns.
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS orders_rep (core_uid INTEGER NOT NULL,
             core_name TEXT NOT NULL, newrecid INTEGER NOT NULL,
             PRIMARY KEY (core_uid, newrecid));
         ALTER TABLE orders_rep ADD COLUMN closedate INTEGER;
         ALTER TABLE orders_rep ADD COLUMN strategyid INTEGER;
         ALTER TABLE orders_rep ADD COLUMN deleted INTEGER;
         ALTER TABLE orders_rep ADD COLUMN channelname TEXT;
         ALTER TABLE orders_rep ADD COLUMN comment TEXT;",
    )
    .expect("extend the typed replica");
    super::super::test_support::rep_init(&conn);
    conn.execute("ATTACH DATABASE ':memory:' AS strat", [])
        .expect("attach strategy metadata");
    conn.execute_batch(
        "CREATE TABLE strat.strategies (core_uid INTEGER NOT NULL, strategy_id INTEGER NOT NULL,
             name TEXT NOT NULL, deleted INTEGER NOT NULL DEFAULT 0);",
    )
    .expect("create strategy metadata");
    conn
}

/// Seed one typed report row.
fn seed_row(
    conn: &Connection,
    core_uid: i64,
    rec_id: i64,
    strategy_id: Option<i64>,
    deleted: i64,
    channel: &str,
    comment: &str,
) {
    conn.execute(
        "INSERT INTO orders_rep (core_uid, core_name, newrecid, closedate, strategyid, deleted,
             channelname, comment) VALUES (?1, 'CORE', ?2, 1000, ?3, ?4, ?5, ?6)",
        rusqlite::params![core_uid, rec_id, strategy_id, deleted, channel, comment],
    )
    .expect("seed report row");
}

/// Scoping the purge by core alone, or forgetting the core half of the key, would hand one core's
/// rec ids to another core's soft-delete command and hide trades the user never chose.
#[test]
fn only_the_named_core_and_strategy_are_returned() {
    let conn = purge_fixture();
    seed_row(&conn, 1, 10, Some(7), 0, "", "");
    seed_row(&conn, 1, 11, Some(8), 0, "", "");
    seed_row(&conn, 2, 12, Some(7), 0, "", "");

    let rows = super::strategy_purge_rows(
        &conn,
        ReportStrategyKey {
            core_uid: 1,
            strategy_id: 7,
        },
    )
    .expect("a healthy replica reads");

    assert_eq!(rows.rec_ids, vec![10]);
    assert_eq!(rows.legacy_rows, 0);
}

/// Rows already soft-deleted are gone from the user's report, so re-addressing them would inflate
/// the count the confirmation promises above what actually disappears.
#[test]
fn already_deleted_rows_are_excluded() {
    let conn = purge_fixture();
    seed_row(&conn, 1, 20, Some(7), 0, "", "");
    seed_row(&conn, 1, 21, Some(7), 1, "", "");

    let rows = super::strategy_purge_rows(
        &conn,
        ReportStrategyKey {
            core_uid: 1,
            strategy_id: 7,
        },
    )
    .expect("a healthy replica reads");

    assert_eq!(rows.rec_ids, vec![20]);
}

/// A liquidation physically carries `strategyid = 0` and is attributed by name. Matching the raw
/// column would leave it behind, so the strategy would keep a non-zero trade count after a
/// "complete" purge and its losses would reappear under "Manual".
#[test]
fn attributed_liquidation_rows_are_included() {
    let conn = purge_fixture();
    conn.execute(
        "INSERT INTO strat.strategies (core_uid, strategy_id, name) VALUES (1, 7, 'EMA_01')",
        [],
    )
    .expect("name the strategy");
    seed_row(&conn, 1, 30, Some(7), 0, "", "");
    seed_row(&conn, 1, 31, Some(0), 0, "LIQUIDATION", "EMA_01");
    // A liquidation naming a different strategy must stay out of this purge.
    seed_row(&conn, 1, 32, Some(0), 0, "LIQUIDATION", "OTHER");

    let rows = super::strategy_purge_rows(
        &conn,
        ReportStrategyKey {
            core_uid: 1,
            strategy_id: 7,
        },
    )
    .expect("a healthy replica reads");

    assert_eq!(rows.rec_ids, vec![30, 31]);
}

/// Legacy rows have no rec id, so the protocol cannot address them. Returning their `0` placeholder
/// as a rec id would build a soft-delete command for a row that does not exist.
#[test]
fn legacy_rows_are_counted_and_never_returned() {
    let conn = purge_fixture();
    conn.execute_batch(
        "CREATE TABLE closed_sell_reports (core_uid INTEGER NOT NULL, db_id INTEGER NOT NULL,
             closedate INTEGER, strategyid INTEGER, deleted INTEGER,
             PRIMARY KEY (core_uid, db_id));
         INSERT INTO closed_sell_reports (core_uid, db_id, closedate, strategyid, deleted)
             VALUES (1, 5, 1000, 7, 0);",
    )
    .expect("create a legacy source");
    seed_row(&conn, 1, 40, Some(7), 0, "", "");

    let rows = super::strategy_purge_rows(
        &conn,
        ReportStrategyKey {
            core_uid: 1,
            strategy_id: 7,
        },
    )
    .expect("a healthy replica reads");

    assert_eq!(
        rows.rec_ids,
        vec![40],
        "no zero placeholder may leak through"
    );
    assert_eq!(rows.legacy_rows, 1);
}

/// Plausible regression: correcting a stored column in `source_select` while `source_sort_expression`
/// keeps serving it raw. Nothing fails to compile and no query errors — but each source is
/// `ORDER BY … LIMIT`-truncated in SQL BEFORE the Rust merge re-sorts by the projected value, so
/// sorting the Report by that column would order rows by one number and display another, and the
/// visible top-N would be missing rows that belong in it.
///
/// The assertion is deliberately generic over the corrected-column registry rather than naming
/// `basecurrency`: the next column to need a correction is the one that would otherwise reintroduce
/// exactly this bug.
#[test]
fn every_corrected_column_sorts_by_the_same_sql_it_projects() {
    let columns: std::collections::HashSet<String> = [
        "core_uid",
        "newrecid",
        "closedate",
        "basecurrency",
        "coin",
        "fname",
        "profitbtc",
    ]
    .iter()
    .map(|name| (*name).to_string())
    .collect();
    let source = super::super::ReadSource {
        table: "orders_rep",
        cols: columns.clone(),
        legacy: false,
    };

    let corrected: Vec<&str> = columns
        .iter()
        .map(String::as_str)
        .filter(|column| super::corrected_column_expression(&source, column).is_some())
        .collect();
    assert!(
        !corrected.is_empty(),
        "the fixture must carry at least one corrected column, or this test asserts nothing"
    );

    for column in corrected {
        let names = vec![column.to_string()];
        let select = super::source_select(&source, &names, None);
        let sort = super::source_sort_expression(&source, column, None)
            .expect("a corrected column must remain sortable");
        assert_eq!(
            select,
            format!("{sort} AS \"{column}\""),
            "`{column}` is projected and sorted by different SQL"
        );
        assert_ne!(
            sort,
            format!("r.\"{column}\""),
            "`{column}` must not sort by the raw stored value"
        );
    }
}
