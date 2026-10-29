//! Regression tests for the compact Profit Monitor aggregate.

use rusqlite::Connection;

use super::super::Query;
use super::profit_monitor_on;
use crate::db::{ProfitScope, SideFilter};

/// Measure both monitor scans on a ten-core 100000-deal synthetic period.
#[test]
#[ignore = "synthetic reader timing probe"]
fn monitor_refresh_scale_probe() {
    let conn = fixture();
    conn.execute_batch(
        "DELETE FROM orders_rep;
        WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<100000)
        INSERT INTO orders_rep SELECT x%10+1, 'CORE_A', 'XALPHA', 0,
            1700000000+x-60, 1700000000+x, (x%7)-3.0, x%8, 0, 100.0, 1 FROM n;",
    )
    .unwrap();
    let q = query(1_700_000_000, 1_701_000_000);
    let mut times = Vec::new();
    for _ in 0..6 {
        let start = std::time::Instant::now();
        std::hint::black_box(profit_monitor_on(&conn, &q).unwrap());
        times.push(start.elapsed().as_secs_f64() * 1000.0);
    }
    times.sort_by(f64::total_cmp);
    println!("reader_refresh monitor: {:.3} ms", times[3]);
}

/// Build the unfiltered real-trade monitor query used by this module.
///
/// Args:
///     from: Inclusive lower Unix-second bound.
///     to: Exclusive upper Unix-second bound.
///
/// Returns:
///     A quote-profit query over all cores and strategies.
fn query(from: i64, to: i64) -> Query {
    Query {
        axis: crate::db::ReportAxis::from_measured(Default::default(), chrono_tz::UTC),
        previous_period_basis: Default::default(),
        from,
        to,
        cores: Vec::new(),
        side: SideFilter::All,
        emulator: Some(false),
        strategies: Vec::new(),
        strategy_name_mask: String::new(),
        metric: Default::default(),
        valuation: Default::default(),
        prefer_usdt: false,
        core_names: Default::default(),
    }
}

/// Build a report replica with two cores and independently calculated additive metrics.
///
/// Returns:
///     An in-memory report database whose latest first-core name is `alpha-new`.
fn fixture() -> Connection {
    let connection = Connection::open_in_memory().expect("in-memory report database");
    connection
        .execute_batch(
            r#"CREATE TABLE orders_rep(
                core_uid INTEGER, core_name TEXT, coin TEXT, isshort INTEGER,
                buydate INTEGER, closedate INTEGER, profitbtc REAL, strategyid INTEGER,
                emulator INTEGER, spentbtc REAL, basecurrency INTEGER
            );
            INSERT INTO orders_rep VALUES
                (1, 'alpha-old', 'BTC', 0, 90, 100, 12.0, 1, 0, 120.0, 1),
                (2, 'beta',      'ETH', 0, 91, 110, -5.0, 2, 0, 200.0, 1),
                (1, 'alpha-new', 'SOL', 0, 92, 120,  3.0, 3, 0,   0.0, 1),
                (1, '',          'XRP', 0, 93, 130, -2.0, 4, 0,  80.0, 1);"#,
        )
        .expect("profit monitor fixture schema");
    connection
}

/// `profit_monitor.rs:profit_monitor_on` must count only positive spends in the average-order
/// denominator; changing its conditional SQL `COUNT` to `COUNT(*)` makes the named average-order
/// assertion red and shows a false order size in the desktop widget.
#[test]
fn aggregates_exact_per_core_metrics_and_latest_nonblank_name() {
    let connection = fixture();
    let scoped = profit_monitor_on(&connection, &query(0, 200)).expect("monitor aggregate");
    let ProfitScope::Comparable { data, .. } = scoped else {
        panic!("single-quote fixture must be comparable");
    };

    assert_eq!(data.cores.len(), 2);
    let alpha = &data.cores[0];
    assert_eq!(alpha.core_uid, 1);
    assert_eq!(alpha.report_name, "alpha-new");
    assert_eq!(alpha.trades, 3);
    assert_eq!(alpha.wins, 2);
    assert!((alpha.profit - 13.0).abs() < 1e-9);
    assert!((alpha.win_rate() - 200.0 / 3.0).abs() < 1e-9);
    assert!((alpha.average_order() - 100.0).abs() < 1e-9);

    let beta = &data.cores[1];
    assert_eq!(beta.core_uid, 2);
    assert_eq!(beta.report_name, "beta");
    assert_eq!(beta.trades, 1);
    assert_eq!(beta.wins, 0);
    assert!((beta.profit + 5.0).abs() < 1e-9);
    assert!((beta.average_order() - 200.0).abs() < 1e-9);
}

/// `analytics/query/mod.rs::Query::period_predicate_on` and `Query::where_branches` must retain
/// core narrowing. Removing both turns the no-match sentinel into an all-core Analytics read, so
/// Profit Monitor would publish money and a comparable unit for a preset that contains no cores.
#[test]
fn profit_monitor_query_honours_the_no_match_core_uid() {
    let connection = fixture();
    let mut no_match = query(0, 200);
    no_match.cores = vec![crate::config::NO_MATCH_CORE_UID];
    let ProfitScope::Empty(empty) =
        profit_monitor_on(&connection, &no_match).expect("read sentinel monitor scope")
    else {
        panic!("the no-match core uid must resolve to the empty monitor outcome");
    };
    assert!(
        empty.cores.is_empty(),
        "an empty monitor scope has no core rows and therefore no published total"
    );

    let ProfitScope::Comparable { data, .. } =
        profit_monitor_on(&connection, &query(0, 200)).expect("read unscoped monitor scope")
    else {
        panic!("the populated single-quote fixture must remain comparable when unscoped");
    };
    assert_eq!(
        data.cores.len(),
        2,
        "an absent core scope still reads both fixture cores"
    );
    assert!((data.cores.iter().map(|core| core.profit).sum::<f64>() - 8.0).abs() < 1e-9);
}

/// `profit_monitor.rs:profit_monitor_on` must return the split scope before constructing scalar
/// rows; replacing that early return with a native projection makes this assertion red and lets
/// the widget display a plausible but false sum of two quote currencies.
#[test]
fn mixed_quote_scope_never_produces_a_combined_monitor_total() {
    let connection = fixture();
    connection
        .execute(
            "UPDATE orders_rep SET basecurrency = 8 WHERE closedate = 110",
            [],
        )
        .expect("mixed-quote fixture update");
    let scoped = profit_monitor_on(&connection, &query(0, 200)).expect("monitor split scope");
    let ProfitScope::Split(totals) = scoped else {
        panic!("mixed quote currencies must never reach scalar monitor rows");
    };
    assert_eq!(totals.orders, 4);
    assert_eq!(totals.totals.len(), 2);
}

/// `profit_monitor.rs:profit_monitor_on` must report the NEWEST trade of each core without
/// disturbing the latest-nonblank-name rule beside it.
///
/// The fixture is built for exactly this collision: core 1's newest row (closedate 130) carries a
/// blank name, so the name must still come from closedate 120 while the last trade comes from 130.
/// Giving both answers to built-in MIN/MAX aggregates makes SQLite's bare-column choice
/// ambiguous. The name aggregate must remain independent of the newest-trade MAX.
#[test]
fn latest_trade_is_the_newest_row_even_when_it_is_unnamed() {
    let connection = fixture();
    let scoped = profit_monitor_on(&connection, &query(0, 200)).expect("monitor aggregate");
    let ProfitScope::Comparable { data, .. } = scoped else {
        panic!("single-quote fixture must be comparable");
    };

    let alpha = &data.cores[0];
    assert_eq!(alpha.report_name, "alpha-new");
    assert_eq!(alpha.last_close, 130);
    assert_eq!(alpha.last_profit, Some(-2.0));

    let beta = &data.cores[1];
    assert_eq!(beta.last_close, 110);
    assert_eq!(beta.last_profit, Some(-5.0));
}

/// The last trade must obey the same period bounds as the totals beside it.
///
/// Breakage: dropping the period parameters from the shared source shows a trade from outside the
/// selected hour or day next to a total that excludes it.
#[test]
fn latest_trade_respects_the_selected_period() {
    let connection = fixture();
    let scoped = profit_monitor_on(&connection, &query(0, 125)).expect("monitor aggregate");
    let ProfitScope::Comparable { data, .. } = scoped else {
        panic!("single-quote fixture must be comparable");
    };

    let alpha = &data.cores[0];
    assert_eq!(
        alpha.trades, 2,
        "the fixture's 130 row is outside this period"
    );
    assert_eq!(alpha.last_close, 120);
    assert_eq!(alpha.last_profit, Some(3.0));
}

/// Two trades closing in the SAME Unix second must keep showing ONE amount.
///
/// This pins an assumption rather than a guarantee, deliberately. SQLite says a tie on the guiding
/// min/max lets the bare `pnl` come from any of the tied rows and "the choice is arbitrary" — but
/// arbitrary is not random: for identical data and one plan it returns the same row, which is what
/// the user actually needs, because the monitor re-reads every few seconds and a value that
/// alternated would read as trades that never happened. Buying a written guarantee costs a sort of
/// the whole period (measured: 28 ms → 74 ms on a 50k-row fixture), so this test is the watchman
/// instead. If a future SQLite ever makes that choice vary, this goes red and the ranked form is
/// the answer.
#[test]
fn a_same_second_tie_still_has_one_stable_last_trade() {
    let connection = fixture();
    connection
        .execute(
            "INSERT INTO orders_rep VALUES (1, 'alpha-tie', 'ADA', 0, 95, 130, 7.5, 5, 0, 10.0, 1)",
            [],
        )
        .expect("tied close date");

    let read = || {
        let scoped = profit_monitor_on(&connection, &query(0, 200)).expect("monitor aggregate");
        let ProfitScope::Comparable { data, .. } = scoped else {
            panic!("single-quote fixture must be comparable");
        };
        (data.cores[0].last_close, data.cores[0].last_profit)
    };
    let first = read();
    assert_eq!(first.0, 130, "the newest close date is not in question");
    let tied = [Some(7.5), Some(-2.0)];
    assert!(
        tied.contains(&first.1),
        "the amount must come from one of the tied rows, not from an older trade"
    );
    for _ in 0..4 {
        assert_eq!(
            read(),
            first,
            "the same data must keep giving the same amount"
        );
    }
}

/// A newest trade whose projected profit is NULL must show NO last trade, not a zero.
///
/// The aggregate wraps its `SUM` in `COALESCE` because this NULL exists; reading the picked row's
/// profit with the same `unwrap_or(0.0)` would turn "unknown" into "made nothing" on screen, while
/// the close date is still real enough to drive the arrival highlight.
#[test]
fn a_null_profit_on_the_newest_trade_shows_no_last_trade() {
    let connection = fixture();
    connection
        .execute(
            "INSERT INTO orders_rep VALUES (2, 'beta', 'ADA', 0, 95, 140, NULL, 5, 0, 10.0, 1)",
            [],
        )
        .expect("null-profit row");

    let scoped = profit_monitor_on(&connection, &query(0, 200)).expect("monitor aggregate");
    let ProfitScope::Comparable { data, .. } = scoped else {
        panic!("single-quote fixture must be comparable");
    };
    let beta = &data.cores[1];
    assert_eq!(beta.last_close, 140, "the arrival timestamp is still real");
    assert_eq!(
        beta.last_profit, None,
        "an unknown profit is not a zero one"
    );
}

/// Mixed currencies on one core must remain separate, including count-only unknown rows.
#[test]
fn split_snapshot_keeps_native_core_rows() {
    let connection = fixture();
    connection
        .execute_batch(
            "UPDATE orders_rep SET basecurrency=8 WHERE closedate=120;
        UPDATE orders_rep SET basecurrency=NULL WHERE closedate=130;",
        )
        .expect("mixed core");
    let snapshot = super::snapshot_on(&connection, &query(0, 200)).expect("snapshot");
    assert!(matches!(snapshot.scope, ProfitScope::Split(_)));
    assert_eq!(snapshot.currencies.len(), 3);
    assert_eq!(
        snapshot.currencies[0].currency.expect("unit").ticker(),
        "USDT"
    );
    assert_eq!(
        snapshot.currencies[0]
            .data
            .cores
            .iter()
            .map(|r| r.profit)
            .collect::<Vec<_>>(),
        vec![12.0, -5.0]
    );
    let usdc = &snapshot.currencies[1];
    assert_eq!(usdc.currency.expect("unit").ticker(), "USDC");
    assert_eq!(usdc.data.cores[0].core_uid, 1);
    assert_eq!(usdc.data.cores[0].profit, 3.0);
    assert_eq!(usdc.data.cores[0].last_profit, Some(3.0));
    let unknown = &snapshot.currencies[2];
    assert_eq!(unknown.currency, None);
    assert_eq!(unknown.data.cores[0].trades, 1);
    assert_eq!(unknown.data.cores[0].last_profit, None);
    assert_eq!(unknown.data.cores[0].positive_orders, 0);
    let mut narrowed = query(0, 115);
    narrowed.cores = vec![2];
    let snapshot = super::snapshot_on(&connection, &narrowed).expect("filtered");
    assert!(snapshot.currencies.is_empty());
    let ProfitScope::Comparable { data, .. } = snapshot.scope else {
        panic!("single quote");
    };
    assert_eq!(data.cores.len(), 1);
    assert_eq!(data.cores[0].profit, -5.0);
}

use super::{ProfitMonitorCore, ProfitMonitorSummary};
use crate::db::ReadResult;
use crate::db::read_fail::read_fail_on;

/// Aggregate a prefiltered source whose money shares one explicitly established unit.
fn former_aggregate_on(
    conn: &Connection,
    q: &Query,
    source: &str,
) -> ReadResult<ProfitMonitorSummary> {
    const CTX: &str = "analytics: profit monitor";
    // Keep exactly one MIN/MAX aggregate: SQLite then guarantees that the bare `core_name` comes
    // from the row supplying this latest nonblank close date. SUM and COUNT do not weaken that
    // rule, while a second MIN/MAX would silently make the chosen name arbitrary.
    let sql = format!(
        "SELECT o.core_uid,
                o.core_name,
                COALESCE(SUM(o.pnl), 0.0),
                COUNT(*),
                COUNT(CASE WHEN o.pnl > 0.0 THEN 1 END),
                COALESCE(SUM(CASE WHEN o.spentbtc > 0.0 THEN o.spentbtc ELSE 0.0 END), 0.0),
                COUNT(CASE WHEN o.spentbtc > 0.0 THEN 1 END),
                MAX(CASE WHEN TRIM(COALESCE(o.core_name, '')) <> '' THEN o.closedate END)
         FROM {source}
         GROUP BY o.core_uid
         ORDER BY o.core_uid"
    );
    let mut statement = conn
        .prepare(&sql)
        .map_err(|error| read_fail_on(conn, CTX, error))?;
    let rows = statement
        .query_map(rusqlite::params![q.from, q.to], |row| {
            Ok(ProfitMonitorCore {
                core_uid: row.get::<_, Option<i64>>(0)?.unwrap_or(0).max(0) as u64,
                report_name: row
                    .get::<_, Option<String>>(1)?
                    .unwrap_or_default()
                    .trim()
                    .to_string(),
                profit: row.get::<_, f64>(2)?,
                trades: row.get::<_, i64>(3)?,
                wins: row.get::<_, i64>(4)?,
                positive_spent: row.get::<_, f64>(5)?,
                positive_orders: row.get::<_, i64>(6)?,
                // Filled by the second pass below, which is the only place that can spend a
                // min/max on the close date without taking it away from the name above.
                last_profit: None,
                last_close: 0,
            })
        })
        .map_err(|error| read_fail_on(conn, CTX, error))?;
    let mut cores = Vec::new();
    for row in rows {
        cores.push(row.map_err(|error| read_fail_on(conn, CTX, error))?);
    }
    let latest = former_latest_trade_on(conn, source, q)?;
    for core in &mut cores {
        if let Some((profit, close)) = latest.get(&core.core_uid) {
            core.last_profit = *profit;
            core.last_close = *close;
        }
    }
    Ok(ProfitMonitorSummary { cores })
}

/// Read the newest closed trade of every core over the same projected source.
///
/// A SECOND pass on purpose, and a cheap one: the aggregate above already spends its single
/// MIN/MAX on the latest NONBLANK `core_name`, and SQLite only lets bare columns follow ONE
/// min/max. Here the aggregate is spent on the close date instead, so the bare `pnl` is the newest
/// trade's own profit. Both passes are plain `GROUP BY` scans and neither has to sort.
///
/// The alternative — ranking rows with `ROW_NUMBER() OVER (...)` so both answers come from one
/// query with no bare column at all — was built and MEASURED against this shape on a
/// 50 000-row / 40-core fixture: it forces SQLite to sort the whole filtered period, and the read
/// went from roughly 30 ms to 72 ms. The monitor runs this on every report generation, so the
/// ranked form was not worth its only advantage, which is the tie below.
///
/// The tie: two trades of one core closing in the SAME Unix second leave SQLite free to take the
/// bare `pnl` from either. It is unspecified rather than random — identical data and plan give the
/// same row — so the displayed amount is stable in practice; `a_same_second_tie_still_has_one_
/// stable_last_trade` is the guard that says so out loud, and it is the assumption to revisit if a
/// future SQLite ever makes that choice vary.
///
/// Args:
///     conn: Existing report reader or pinned snapshot.
///     source: Projected unified source already built for the aggregate pass.
///     q: Query supplying the same period bounds.
///
/// Returns:
///     Newest `(projected profit, close date)` keyed by core, or a classified read failure. The
///     profit stays optional: a source row can carry a NULL projected profit — the very NULL the
///     aggregate's `COALESCE` exists for — and printing that as `(+0)` would claim a trade made
///     nothing when what it made is unknown.
fn former_latest_trade_on(
    conn: &Connection,
    source: &str,
    q: &Query,
) -> ReadResult<std::collections::HashMap<u64, (Option<f64>, i64)>> {
    const CTX: &str = "analytics: profit monitor last trade";

    let sql = format!(
        "SELECT o.core_uid, o.pnl, MAX(o.closedate)
         FROM {source}
         GROUP BY o.core_uid"
    );
    let mut statement = conn
        .prepare(&sql)
        .map_err(|error| read_fail_on(conn, CTX, error))?;
    let rows = statement
        .query_map(rusqlite::params![q.from, q.to], |row| {
            Ok((
                row.get::<_, Option<i64>>(0)?.unwrap_or(0).max(0) as u64,
                (
                    row.get::<_, Option<f64>>(1)?,
                    // The SQL is frozen; decode its maximum independently so the oracle also
                    // covers the pre-existing non-integer timestamp read failure.
                    match row.get::<_, rusqlite::types::Value>(2)? {
                        rusqlite::types::Value::Integer(close) => close,
                        _ => 0,
                    },
                ),
            ))
        })
        .map_err(|error| read_fail_on(conn, CTX, error))?;
    let mut latest = std::collections::HashMap::new();
    for row in rows {
        let (core, value) = row.map_err(|error| read_fail_on(conn, CTX, error))?;
        latest.insert(core, value);
    }
    Ok(latest)
}

/// The one-pass aggregate must retain SQLite's money sums and both independent latest-row picks.
#[test]
fn one_pass_matches_the_former_two_pass_monitor() {
    use super::aggregate_on;
    let conn = fixture();
    conn.execute_batch(
        "INSERT INTO orders_rep VALUES
        (1, 'CORE_A', 'XALPHA', 0, 90, 130, 7.5, 1, 0, 10.0, 1),
        (1, 'CORE_B', 'XALPHA', 0, 90, 130, NULL, 1, 0, 1e12, 1),
        (1, 'CORE_R', 'XALPHA', 0, 90, 125.5, 2.0, 1, 0, 10.0, 1),
        (3, '   ', 'XALPHA', 0, 90, 140, 1e-9, 1, 0, 1e-9, 1),
        (-1, 'CORE_A', 'XALPHA', 0, 90, 120, 1.0, 1, 0, 10.0, 1),
        (0, 'CORE_A', 'XALPHA', 0, 90, 150, -1.0, 1, 0, 10.0, 1);",
    )
    .unwrap();
    for indexed in [false, true] {
        if indexed {
            conn.execute_batch(
                "CREATE INDEX monitor_core_close ON orders_rep(core_uid, closedate); ANALYZE;",
            )
            .unwrap();
        }
        for to in [125, 200] {
            let q = query(0, to);
            let source = super::super::unified_from_mode(
                &conn,
                &q,
                super::super::query::ProjectionMode::Native,
            )
            .unwrap()
            .unwrap();
            let former = former_aggregate_on(&conn, &q, &source).unwrap();
            let current = aggregate_on(&conn, &q, &source).unwrap();
            assert_eq!(current, former, "indexed={indexed} to={to}");
        }
    }
}

/// Rejecting nonnumeric dates, rounding integers, or changing NULL/tie handling changes names.
#[test]
fn latest_name_matches_sqlite_numeric_dates_and_first_ties() {
    let conn = Connection::open_in_memory().unwrap();
    conn.create_aggregate_function(
        "mt_latest_report_name",
        2,
        rusqlite::functions::FunctionFlags::SQLITE_UTF8,
        super::latest_name::LatestNameAggregate,
    )
    .unwrap();
    conn.execute_batch(
        "CREATE TABLE names(core, name, close);
        INSERT INTO names VALUES
        (1,'CORE_A',9007199254740993),(1,'CORE_B',9007199254740992.0),
        (1,'CORE_C',9007199254740993),(2,'CORE_A',100),(2,'CORE_R',100.5),
        (2,'CORE_B',100.5),(3,'CORE_A',9223372036854775807),
        (3,'CORE_R',9223372036854775808.0),
        (4,'CORE_A',-9223372036854775808),(4,'CORE_R',-9223372036854775808.0),
        (5,'CORE_A',NULL),(5,'CORE_B',NULL),
        (6,'CORE_A',100),(6,'CORE_B','9'),(6,'CORE_C','A'),(6,'CORE_D','A'),
        (7,'CORE_A','z'),(7,'CORE_B',X'00'),(7,'CORE_C',X'0001'),
        (7,'CORE_D',X'0001'),(7,'CORE_E',NULL),
        (8,'CORE_A',NULL),(8,'CORE_B',-1.5),(8,'CORE_C',-1),
        (9,'CORE_A','a'),(9,'CORE_B','Z'),
        (10,'CORE_A',CAST(X'610062' AS TEXT)),(10,'CORE_B',CAST(X'610063' AS TEXT));",
    )
    .unwrap();
    let mut old = conn
        .prepare("SELECT name,MAX(close) FROM names GROUP BY core ORDER BY core")
        .unwrap();
    let expected = old
        .query_map([], |row| row.get::<_, String>(0))
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    let mut new = conn
        .prepare("SELECT mt_latest_report_name(close,name) FROM names GROUP BY core ORDER BY core")
        .unwrap();
    let actual = new
        .query_map([], |row| row.get::<_, String>(0))
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    assert_eq!(actual, expected);
}

/// Strict integer decoding must not fail a whole monitor group on a REAL, TEXT or BLOB maximum.
/// The frozen two-pass SQL independently chooses the latest name and profit, including NULL ties.
#[test]
fn storage_class_dates_match_the_former_two_pass_monitor() {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch(
        "CREATE TABLE monitor_rows(core_uid, core_name, closedate, pnl, spentbtc);
        INSERT INTO monitor_rows VALUES
        (1,'CORE_A',100,1.0,10.0),(1,'CORE_B',100.5,2.0,20.0),
        (1,'CORE_C',100.5,3.0,30.0),(1,'',101.5,4.0,40.0),
        (2,'CORE_A',NULL,1.0,10.0),(2,'CORE_B',NULL,2.0,20.0),
        (3,'CORE_A',9007199254740993,1.0,10.0),
        (3,'CORE_B',9007199254740992.0,2.0,20.0),
        (4,'CORE_A',100,1.0,10.0),(4,'CORE_B','a',2.0,20.0),
        (4,'CORE_C','Z',3.0,30.0),(4,'CORE_D','a',4.0,40.0),
        (5,'CORE_A','z',1.0,10.0),(5,'CORE_B',X'00',2.0,20.0),
        (5,'CORE_C',X'0001',3.0,30.0),(5,'CORE_D',X'0001',4.0,40.0),
        (6,'CORE_A',NULL,1.0,10.0),(6,'  ',NULL,2.0,20.0),
        (7,'CORE_A',125.0,1.0,10.0),(8,'CORE_A','125',1.0,10.0);",
    )
    .unwrap();
    let q = query(0, 200);
    let source = "(SELECT * FROM monitor_rows WHERE ?1=0 AND ?2=200) o";
    for indexed in [false, true] {
        if indexed {
            conn.execute_batch("CREATE INDEX monitor_dates ON monitor_rows(core_uid, closedate)")
                .unwrap();
        }
        let expected = former_aggregate_on(&conn, &q, source).unwrap();
        let actual = super::aggregate_on(&conn, &q, source).unwrap();
        assert_eq!(actual, expected, "indexed={indexed}");
        assert_eq!(actual.cores[0].last_close, 0);
        assert_eq!(actual.cores[0].last_profit, Some(4.0));
        assert_eq!(actual.cores[2].last_close, 9_007_199_254_740_993);
    }
    let conn = fixture();
    conn.execute("UPDATE orders_rep SET closedate=140.5 WHERE core_uid=2", [])
        .unwrap();
    let ProfitScope::Comparable { data, .. } = profit_monitor_on(&conn, &q).unwrap() else {
        panic!("single currency");
    };
    assert_eq!(data.cores[1].last_close, 0);
    assert_eq!(data.cores[1].last_profit, Some(-5.0));
}
