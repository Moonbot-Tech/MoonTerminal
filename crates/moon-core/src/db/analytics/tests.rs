//! Summary, Strategies, and quote-scope analytics regression tests.

use super::super::test_support::{
    build_replica, corrupt_leaf_page, remove_db, spread_rows, temp_db,
};
use super::super::{QuoteCurrency, SideFilter};
use super::*;

/// Build the minimal real-trade query used by analytics regression tests.
fn q(from: i64, to: i64) -> Query {
    Query {
        from,
        to,
        cores: Vec::new(),
        side: SideFilter::All,
        emulator: Some(false),
        strategies: Vec::new(),
        metric: Default::default(),
    }
}

/// A single-day period switches the series grid to HOURS and reports the profit split
/// by strategy type — and that split must reconcile with the period total, or the two
/// charts on screen contradict each other.
#[test]
fn single_day_uses_hourly_grid_and_kinds_reconcile() {
    let path = temp_db("oneday");
    let day = 1_780_000_000i64 / 86_400 * 86_400;
    // Four trades spread across three different hours of ONE day.
    let conn = build_replica(
        &path,
        &[
            (day + 3_600, 10.0, "BTCUSDT"),
            (day + 3_700, -4.0, "BTCUSDT"),
            (day + 7_200, 6.0, "ETHUSDT"),
            (day + 18_000, -2.0, "ETHUSDT"),
        ],
    );

    let scoped = summary_on(&conn, &q(day, day + 86_400), false, false).expect("healthy DB reads");
    let s = scoped.data().expect("single-quote summary is comparable");
    assert_eq!(s.bucket_secs, 3_600, "a day or less → hourly grid");
    // The grid steps by exactly an hour with no holes (empty hours are zero-filled —
    // a chart's X axis has to be continuous), and three of them hold the trades.
    assert!(
        s.days.windows(2).all(|w| w[1].start - w[0].start == 3_600),
        "grid step must be one hour: {:?}",
        s.days
    );
    assert_eq!(
        s.days.iter().filter(|d| d.trades > 0).count(),
        3,
        "three hours with trades: {:?}",
        s.days
    );

    // Without the strategies DB every trade folds into ONE unnamed type — the chart
    // still has something to draw instead of vanishing.
    assert_eq!(s.kinds.len(), 1, "no strategies DB → a single group");
    assert_eq!(s.kinds[0].kind, "");

    // The split must add up to the period total, both in money and in count.
    let ksum: f64 = s.kinds.iter().map(|k| k.profit).sum();
    let kn: i64 = s.kinds.iter().map(|k| k.trades).sum();
    assert!(
        (ksum - s.cur.profit).abs() < 1e-9,
        "Σ of types {ksum} != period total {}",
        s.cur.profit
    );
    assert_eq!(kn, s.cur.n, "Σ of type trades != the period's n");

    // And inside a type, the per-core rows the popup lists must add up to its own bar.
    let csum: f64 = s.kinds[0].cores.iter().map(|c| c.profit).sum();
    assert!(
        (csum - s.kinds[0].profit).abs() < 1e-9,
        "Σ of cores {csum} != the type's profit {}",
        s.kinds[0].profit
    );

    // A longer period keeps the daily grid and computes no type split at all.
    let wide_scope =
        summary_on(&conn, &q(day - 86_400, day + 86_400), false, false).expect("reads");
    let wide = wide_scope
        .data()
        .expect("single-quote wide summary is comparable");
    assert_eq!(wide.bucket_secs, 86_400, "two days → daily grid");
    assert!(wide.kinds.is_empty(), "no type split on a long period");

    drop(conn);
    remove_db(&path);
}

/// A healthy replica retains exact summary metrics and empty-period semantics.
#[test]
fn healthy_summary_exact_values() {
    let path = temp_db("healthy");
    // Four same-day trades: +10, -4, +6, -2 => profit 10 and two wins.
    let day = 1_780_000_000i64 / 86_400 * 86_400 + 3_600;
    let conn = build_replica(
        &path,
        &[
            (day, 10.0, "BTCUSDT"),
            (day + 60, -4.0, "BTCUSDT"),
            (day + 120, 6.0, "ETHUSDT"),
            (day + 180, -2.0, "ETHUSDT"),
        ],
    );

    let scoped = summary_on(&conn, &q(day - 86_400, day + 86_400), false, true)
        .expect("healthy database must remain readable");
    let s = scoped.data().expect("single-quote summary is comparable");
    assert_eq!(s.cur.n, 4);
    assert_eq!(s.cur.wins, 2);
    assert_eq!(s.cur.losses, 2);
    assert!(
        (s.cur.profit - 10.0).abs() < 1e-9,
        "profit={}",
        s.cur.profit
    );
    assert!((s.cur.winrate() - 50.0).abs() < 1e-9);
    // Profit factor is total wins divided by total losses: 16 / 6.
    assert!((s.cur.pf - 16.0 / 6.0).abs() < 1e-9, "pf={}", s.cur.pf);
    // Cumulative profit 10 -> 6 -> 12 -> 10 has a maximum drawdown of 4.
    assert!((s.cur.max_dd - 4.0).abs() < 1e-9, "max_dd={}", s.cur.max_dd);
    assert!((s.cur.avg - 2.5).abs() < 1e-9);
    assert_eq!(s.coins.len(), 2, "two coins");
    assert_eq!(s.cores, vec![(1u64, "CORE-A".to_string())]);
    assert_eq!(s.best.len(), 4);

    // A genuinely empty period succeeds with zero counters.
    let empty_scope = summary_on(&conn, &q(day - 10 * 86_400, day - 9 * 86_400), false, false)
        .expect("an empty period is a successful read");
    let empty = empty_scope.data().expect("empty scope retains empty data");
    assert_eq!(empty.cur.n, 0);

    drop(conn);
    remove_db(&path);
}

/// A homogeneous USDC period must carry USDC through the typed summary boundary.
///
/// Changing `analytics::scope_decision_on` back to the historical implicit-USDT assumption makes
/// the unit assertion fail and would label valid USDC money as USDT in every consumer.
#[test]
fn homogeneous_usdc_summary_carries_its_exact_unit() {
    let path = temp_db("usdc-unit");
    let day = 1_780_000_000i64 / 86_400 * 86_400;
    let conn = build_replica(&path, &[(day + 60, 12.5, "BTCUSDC")]);
    conn.execute("UPDATE orders_rep SET basecurrency = 8", [])
        .expect("mark row as USDC");

    let scoped = summary_on(&conn, &q(day, day + 86_400), false, false).expect("summary");
    match scoped {
        ProfitScope::Comparable {
            unit: ProfitUnit::Quote(currency),
            data,
        } => {
            assert_eq!(currency.ticker(), "USDC");
            assert_eq!((data.cur.profit, data.cur.n), (12.5, 1));
        }
        other => panic!("USDC period must be comparable, got {other:?}"),
    }

    drop(conn);
    remove_db(&path);
}

/// Mixed raw quote money is split, while the same rows remain comparable in Percent mode.
///
/// Removing the raw preflight from `analytics::summary_on` makes the raw branch expose a scalar
/// 15.0 whose operands are USDT and USDC; applying that preflight to Percent breaks the second
/// assertion even though per-trade percentages are dimensionless.
#[test]
fn mixed_quote_summary_splits_raw_money_but_keeps_percent() {
    let path = temp_db("mixed-summary");
    let day = 1_780_000_000i64 / 86_400 * 86_400;
    let conn = build_replica(
        &path,
        &[(day + 60, 10.0, "BTCUSDT"), (day + 120, 5.0, "ETHUSDC")],
    );
    conn.execute(
        "UPDATE orders_rep SET basecurrency = 8 WHERE closedate = ?1",
        [day + 120],
    )
    .expect("mark second row as USDC");

    let raw = summary_on(&conn, &q(day, day + 86_400), false, false).expect("raw summary");
    let split = raw.split().expect("mixed raw money must be split");
    assert_eq!(split.orders, 2);
    assert_eq!(split.totals.len(), 2);
    assert!(
        raw.data().is_none(),
        "split scope must carry no scalar summary"
    );

    let mut percent_query = q(day, day + 86_400);
    percent_query.metric = ProfitMetric::Percent;
    let percent = summary_on(&conn, &percent_query, false, false).expect("percent summary");
    assert!(matches!(
        percent,
        ProfitScope::Comparable {
            unit: ProfitUnit::Percent,
            ..
        }
    ));

    drop(conn);
    remove_db(&path);
}

/// `analytics/query/mod.rs:unified_from_mode` must project a fully covered mixed scope through
/// historical USDT values; using native `profitbtc` would publish 15.0 instead of the independently
/// calculated 14.995 USDT, while exposing coverage before every row is ready would publish partial
/// history as a complete scalar.
#[test]
fn mixed_quote_summary_becomes_usdt_only_after_complete_coverage() {
    let _health = super::super::valuation::test_health_guard();
    let path = temp_db("mixed-valued-summary");
    let day = 1_780_000_000i64 / 86_400 * 86_400;
    let conn = build_replica(
        &path,
        &[
            (day - 60, 7.0, "PREVIOUSUSDT"),
            (day + 60, 10.0, "BTCUSDT"),
            (day + 120, 5.0, "ETHUSDC"),
        ],
    );
    conn.execute(
        "UPDATE orders_rep SET basecurrency = 8 WHERE closedate = ?1",
        [day + 120],
    )
    .expect("mark second row as USDC");
    conn.execute_batch(
        "ATTACH ':memory:' AS valuation;
         CREATE TABLE valuation.rates (
             algorithm_version INTEGER, quote_ordinal INTEGER, minute_utc INTEGER,
             status INTEGER, rate_usdt REAL, provider TEXT, symbol TEXT,
             orientation INTEGER, candle_open_ms INTEGER, candle_close_ms INTEGER,
             fetched_at_ms INTEGER,
             PRIMARY KEY (algorithm_version, quote_ordinal, minute_utc)
         );
         CREATE TABLE valuation.trade_values (
             source_kind INTEGER, core_uid INTEGER, row_id INTEGER,
             algorithm_version INTEGER, closedate INTEGER, quote_ordinal INTEGER,
             profit_quote REAL, spent_quote REAL, rate_minute_utc INTEGER,
             rate_usdt REAL, profit_usdt REAL, spent_usdt REAL, valued_at_ms INTEGER,
             PRIMARY KEY (source_kind, core_uid, row_id)
         );",
    )
    .expect("attach empty valuation cache");

    let partial =
        summary_on(&conn, &q(day, day + 86_400), false, false).expect("partial coverage reads");
    let partial_totals = partial.split().expect("partial mixed scope remains split");
    assert_eq!(
        partial_totals
            .valuation
            .expect("coverage is attached")
            .valued_orders,
        1,
        "the USDT row is identity-valued before USDC is prepared"
    );

    conn.execute(
        "INSERT INTO valuation.trade_values (
             source_kind, core_uid, row_id, algorithm_version, closedate, quote_ordinal,
             profit_quote, spent_quote, rate_minute_utc, rate_usdt,
             profit_usdt, spent_usdt, valued_at_ms
         )
         SELECT 0, core_uid, newrecid, ?1, closedate, basecurrency,
                profitbtc, spentbtc, (closedate/60)*60, 0.999,
                profitbtc*0.999, spentbtc*0.999, 1780001000000
         FROM orders_rep WHERE basecurrency=8",
        [super::super::valuation::ALGORITHM_VERSION],
    )
    .expect("prepare exact USDC valuation");

    let current_query = q(day, day + 86_400);
    let decision = scope_decision_on(&conn, &current_query).expect("resolve current projection");
    let projection = decision.projection().expect("complete mixed projection");
    let source = unified_from_mode(&conn, &current_query, projection)
        .expect("build current USDT source")
        .expect("report source exists");
    let previous_rows = conn
        .query_row(
            "SELECT COUNT(*) FROM orders_rep
             WHERE closedate>=?1 AND closedate<?2 AND basecurrency=1 AND emulator=0",
            rusqlite::params![day - 86_400, day],
            |row| row.get::<_, i64>(0),
        )
        .expect("count raw previous rows");
    assert_eq!(previous_rows, 1);
    let direct_previous = scan_period(&conn, &source, day - 86_400, day, 3_600)
        .expect("scan compatible previous period")
        .0;
    assert_eq!(
        (direct_previous.profit, direct_previous.n),
        (7.0, 1),
        "USDT projection must retain identity-valued previous rows: {source}"
    );

    let complete =
        summary_on(&conn, &q(day, day + 86_400), false, true).expect("complete coverage reads");
    match complete {
        ProfitScope::Comparable {
            unit: ProfitUnit::Quote(currency),
            data,
        } => {
            assert_eq!(currency, QuoteCurrency::usdt());
            assert!(
                (data.cur.profit - 14.995).abs() < 1e-9,
                "profit={}",
                data.cur.profit
            );
            assert_eq!(data.cur.n, 2);
            let previous = data
                .prev
                .expect("single-USDT comparison remains expressible in current USDT");
            assert_eq!((previous.profit, previous.n), (7.0, 1));
        }
        other => panic!("complete mixed scope must be USDT-comparable, got {other:?}"),
    }

    conn.execute(
        "UPDATE orders_rep SET profitbtc=6.0 WHERE basecurrency=8",
        [],
    )
    .expect("replace report inputs under the same row identity");
    let stale =
        summary_on(&conn, &q(day, day + 86_400), false, false).expect("same-key update reads");
    assert!(
        stale.split().is_some(),
        "a prepared value with stale profit inputs must stop being comparable immediately"
    );

    drop(conn);
    remove_db(&path);
}

/// Reusing per-source Analytics accumulators after attached valuation corruption would duplicate
/// the already-scanned typed row; the native retry must rebuild the whole quote breakdown from an
/// empty state and must not classify the healthy report main database as damaged.
#[test]
fn quote_breakdown_restarts_whole_scan_after_valuation_corruption() {
    let _health = super::super::valuation::test_health_guard();
    let _integrity = super::super::integrity::test_state_guard();
    super::super::integrity::reset_test_state();
    let dir = std::env::temp_dir().join(format!(
        "moonterminal-analytics-valuation-retry-{}-{}",
        std::process::id(),
        crate::util::now_unix_ms_i64()
    ));
    std::fs::create_dir_all(&dir).expect("create Analytics retry fixture");
    let valuation_path = dir.join("valuation.sqlite");
    let store = super::super::valuation::open_store(&valuation_path)
        .expect("open Analytics valuation fixture");
    let transaction = store
        .unchecked_transaction()
        .expect("begin Analytics valuation seed");
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
            .expect("seed Analytics prepared value");
    }
    transaction
        .commit()
        .expect("commit Analytics prepared values");

    let conn = rusqlite::Connection::open_in_memory().expect("open Analytics report fixture");
    conn.execute_batch(
        "CREATE TABLE orders_rep (
             core_uid INTEGER NOT NULL, newrecid INTEGER NOT NULL,
             closedate INTEGER, profitbtc REAL
         );
         INSERT INTO orders_rep VALUES (1, 1, 1700000000, 10.0);
         CREATE TABLE closed_sell_reports (
             core_uid INTEGER NOT NULL, db_id INTEGER NOT NULL, closedate INTEGER,
             basecurrency INTEGER, profitbtc REAL, spentbtc REAL
         );
         INSERT INTO closed_sell_reports VALUES (1, 1, 1700000000, 8, 20.0, 100.0);",
    )
    .expect("seed Analytics physical sources");
    let attach = format!(
        "ATTACH DATABASE '{}' AS valuation",
        valuation_path
            .to_string_lossy()
            .replace('\\', "/")
            .replace('\'', "''")
    );
    conn.execute(&attach, [])
        .expect("attach Analytics valuation");
    assert!(super::super::valuation::is_attached(&conn));
    corrupt_leaf_page(store, &valuation_path, "sqlite_autoindex_trade_values_1");

    let totals = quote_breakdown_on(&conn, &q(1_699_999_000, 1_700_001_000))
        .expect("fall back to complete native Analytics totals");
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
    std::fs::remove_dir_all(dir).expect("remove Analytics retry fixture");
}

/// The lightweight Strategies payload enforces the same boundary before building its groups.
///
/// Omitting the preflight from `analytics::strategy_base_on` would let the Strategies tab bypass
/// the safe Summary path and expose cross-currency totals and tuner inputs after a tab switch.
#[test]
fn mixed_quote_strategy_base_never_bypasses_the_scope_boundary() {
    let path = temp_db("mixed-strategy-base");
    let day = 1_780_000_000i64 / 86_400 * 86_400;
    let conn = build_replica(
        &path,
        &[(day + 60, 10.0, "BTCUSDT"), (day + 120, 5.0, "ETHUSDC")],
    );
    conn.execute(
        "UPDATE orders_rep SET basecurrency = 8 WHERE closedate = ?1",
        [day + 120],
    )
    .expect("mark second row as USDC");

    let raw = strategy_base_on(&conn, &q(day, day + 86_400), false).expect("raw base");
    assert!(raw.data().is_none());
    assert_eq!(raw.split().expect("mixed base must split").orders, 2);

    let mut percent_query = q(day, day + 86_400);
    percent_query.metric = ProfitMetric::Percent;
    assert!(matches!(
        strategy_base_on(&conn, &percent_query, false).expect("percent base"),
        ProfitScope::Comparable {
            unit: ProfitUnit::Percent,
            ..
        }
    ));

    drop(conn);
    remove_db(&path);
}

/// Unknown quote identity quarantines raw money instead of guessing from the current core.
///
/// Treating NULL as USDT in `quote_breakdown_on` makes this become comparable and would silently
/// assign the row's amount to a currency that is absent from its persisted report data.
#[test]
fn unknown_quote_summary_never_guesses_a_currency() {
    let path = temp_db("unknown-summary");
    let day = 1_780_000_000i64 / 86_400 * 86_400;
    let conn = build_replica(&path, &[(day + 60, 10.0, "BTC")]);
    conn.execute("UPDATE orders_rep SET basecurrency = NULL", [])
        .expect("remove quote identity");

    let raw = summary_on(&conn, &q(day, day + 86_400), false, false).expect("raw summary");
    let split = raw.split().expect("unknown raw money must be split");
    assert_eq!((split.orders, split.unknown_orders), (1, 1));
    assert!(split.totals.is_empty());

    drop(conn);
    remove_db(&path);
}

/// Previous-period money is comparable only when its quote matches the current period.
///
/// Dropping the independent previous-period quote check in `analytics::previous_stats` makes the
/// USDC current KPI compare against a USDT scalar and produces a dimensionally invalid delta.
#[test]
fn summary_omits_previous_stats_when_quote_changes() {
    let path = temp_db("previous-quote");
    let day = 1_780_000_000i64 / 86_400 * 86_400;
    let conn = build_replica(
        &path,
        &[(day - 60, 4.0, "BTCUSDT"), (day + 60, 7.0, "BTCUSDC")],
    );
    conn.execute(
        "UPDATE orders_rep SET basecurrency = 8 WHERE closedate >= ?1",
        [day],
    )
    .expect("mark current row as USDC");

    let scoped = summary_on(&conn, &q(day, day + 86_400), false, false).expect("summary");
    let summary = scoped.data().expect("current USDC period is comparable");
    assert!(
        summary.prev.is_none(),
        "USDT previous must not compare with USDC"
    );

    drop(conn);
    remove_db(&path);
}

/// Index-page corruption surfaces as an error rather than an empty period.
#[test]
fn corrupt_replica_surfaces_error_not_empty() {
    // The corruption latch this test trips is process-global, and `test_state_guard` is what
    // serializes it against the tests that assert on it; without it this test can flip
    // `WRITES_BLOCKED` under an unrelated assertion.
    let _integrity = super::super::integrity::test_state_guard();
    super::super::integrity::reset_test_state();
    let path = temp_db("corrupt");
    // Enough rows keep the target index leaf away from the header page so
    // corruption surfaces during the period scan rather than file opening.
    let day = 1_780_000_000i64 / 86_400 * 86_400;
    let conn = build_replica(&path, &spread_rows(day, 2000));

    // Prove the fixture is healthy before introducing damage.
    let before_scope = summary_on(&conn, &q(day - 86_400, day + 10 * 86_400), false, false)
        .expect("до порчи БД читается");
    let before = before_scope
        .data()
        .expect("single-quote summary is comparable before corruption");
    assert_eq!(before.cur.n, 2000);

    // The scan must use the index whose leaf page is about to be damaged.
    let plan: String = conn
        .query_row(
            "EXPLAIN QUERY PLAN SELECT closedate FROM orders_rep
             WHERE closedate >= 1 AND closedate < 2 AND closedate > 0",
            [],
            |r| r.get(3),
        )
        .unwrap();
    assert!(
        plan.contains("idx_rep_closedate"),
        "план без индекса: {plan}"
    );

    corrupt_leaf_page(conn, &path, "idx_rep_closedate");

    // The intact header allows opening; the period read reaches the damage.
    let conn = Connection::open(&path).expect("битая БД всё ещё открывается");
    let wide = q(day - 86_400, day + 10 * 86_400);

    // Pin the period scan itself so another query cannot mask skipped rows
    // by failing later in the summary pipeline.
    let src = unified_from(&conn, &wide)
        .expect("схема читается")
        .expect("источник есть");
    assert!(
        scan_period(&conn, &src, wide.from, wide.to, 86_400).is_err(),
        "скан периода обязан вернуть ошибку, а не усечённую статистику"
    );

    let res = summary_on(&conn, &wide, false, false);

    assert!(
        !matches!(res, Ok(_)),
        "ошибка чтения не должна превращаться в успешный — в том числе \
         пустой или частичный — период: это и есть чинимый баг"
    );
    match res {
        Err(ReadFail::Failed { kind, .. }) => assert_eq!(
            kind,
            super::super::FailKind::Corrupt,
            "порча должна классифицироваться как Corrupt"
        ),
        Err(ReadFail::NotReady) => {
            panic!("порча не должна выглядеть как «реплика не готова»")
        }
        Err(ReadFail::IncomparableQuote) => {
            panic!("a single-quote corruption fixture must not become a quote-scope error")
        }
        Ok(_) => unreachable!("уже проверено выше"),
    }

    remove_db(&path);
}
