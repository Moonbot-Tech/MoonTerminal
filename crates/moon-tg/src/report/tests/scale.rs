//! Scale probes for a heavy account: 10 cores, 20 000 deals in the report period, 5 000 closes in
//! the notification window. Ignored by default; run with `--ignored --nocapture` to print timings.

use std::time::Instant;

use moon_core::config::telegram_access::TelegramReportAccess;
use moon_core::config::telegram_menu::ReportBasis;
use moon_core::telegram::report::{Period, ReportRequest};

const CORES: i64 = 10;
const ROUNDS: u32 = 5;

thread_local! {
    static CLOSING_SQL: std::cell::RefCell<Vec<String>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// Capture the production handoff SELECT, so its plan cannot be confused with a smaller surrogate.
fn capture_closing_statement(event: rusqlite::trace::TraceEvent<'_>) {
    if let rusqlite::trace::TraceEvent::Stmt(statement, _) = event
        && let Some(sql) = statement.expanded_sql()
        && sql.contains("mt_chart_entry_matches(")
    {
        CLOSING_SQL.with(|rows| rows.borrow_mut().push(sql));
    }
}

/// Time an older same-coin closing identity among 100000 trades, including the full projection.
#[test]
#[ignore = "manual closing identity scale measurement"]
fn probe_closing_identity() {
    let conn = source(100_000, 1_700_000_000, 1_700_200_000);
    let indexing = Instant::now();
    conn.execute_batch(
        "CREATE INDEX idx_rep_coin_entry ON orders_rep(core_uid, coin COLLATE NOCASE, buydate) WHERE buydate > 0",
    )
    .unwrap();
    println!(
        "[scale] closing identity index build (100000 rows): {:.3} ms",
        indexing.elapsed().as_secs_f64() * 1000.0
    );
    CLOSING_SQL.with(|rows| rows.borrow_mut().clear());
    conn.trace_v2(
        rusqlite::trace::TraceEventCodes::SQLITE_TRACE_STMT,
        Some(capture_closing_statement),
    );
    super::super::read_mini_chart_trade_on(
        &conn,
        chrono_tz::UTC,
        moon_core::db::CoreNames::default(),
        1,
        Err(("XALPHA".into(), 1_700_000_000_000)),
    )
    .unwrap();
    conn.trace_v2(rusqlite::trace::TraceEventCodes::SQLITE_TRACE_STMT, None);
    let statements = CLOSING_SQL.with(|rows| rows.borrow().clone());
    assert!(!statements.is_empty());
    for sql in statements {
        let plan = conn
            .prepare(&format!("EXPLAIN QUERY PLAN {sql}"))
            .unwrap()
            .query_map([], |row| row.get::<_, String>(3))
            .unwrap()
            .map(Result::unwrap)
            .collect::<Vec<_>>()
            .join("; ");
        assert!(plan.contains("idx_rep_coin_entry"), "{plan}");
        println!("[scale] production closing identity plan: {plan}");
    }
    let mut samples = Vec::new();
    for _ in 0..7 {
        let start = Instant::now();
        let result = super::super::read_mini_chart_trade_on(
            &conn,
            chrono_tz::UTC,
            moon_core::db::CoreNames::default(),
            1,
            Err(("XALPHA".into(), 1_700_000_000_000)),
        )
        .unwrap();
        assert!(result.is_some(), "an older same-coin trade must resolve");
        samples.push(start.elapsed().as_secs_f64() * 1000.0);
    }
    samples.sort_by(f64::total_cmp);
    println!("[scale] closing identity median: {:.3} ms", samples[3]);
}

/// A report source with the replica's indexes and `deals` rows inside both period bases across
/// [`CORES`], so a table scan or an out-of-period entry cannot distort the measured workload.
fn source(deals: i64, from: i64, to: i64) -> rusqlite::Connection {
    source_on(
        rusqlite::Connection::open_in_memory().unwrap(),
        deals,
        from,
        to,
    )
}

/// Populate either an in-memory report source or the isolated end-to-end replica.
pub(super) fn source_on(
    conn: rusqlite::Connection,
    deals: i64,
    from: i64,
    to: i64,
) -> rusqlite::Connection {
    conn.execute_batch(
        "CREATE TABLE orders_rep (core_uid INTEGER, core_name TEXT, newrecid INTEGER, coin TEXT,
            buydate INTEGER, closedate INTEGER, channelname TEXT, emulator INTEGER,
            buyprice REAL, sellprice REAL, profitbtc REAL, spentbtc REAL, basecurrency INTEGER,
            PRIMARY KEY (core_uid, newrecid));
         CREATE INDEX idx_rep_closedate ON orders_rep(closedate);
         CREATE INDEX idx_rep_core_close ON orders_rep(core_uid, closedate)",
    )
    .unwrap();
    let tx = conn.unchecked_transaction().unwrap();
    {
        let mut insert = tx
            .prepare(
                "INSERT INTO orders_rep VALUES (?1, ?2, ?3, 'XALPHA', ?4, ?5, '', 0, 1.0, 1.01, ?6, 100, 0)",
            )
            .unwrap();
        for deal in 0..deals {
            let core = deal % CORES + 1;
            let close = from + 30 + deal * (to - from - 31) / deals;
            insert
                .execute(rusqlite::params![
                    core,
                    format!("CORE_{core}"),
                    deal,
                    close - 30,
                    close,
                    (deal % 7) as f64 - 3.0
                ])
                .unwrap();
        }
    }
    tx.commit().unwrap();
    conn
}

/// Mean milliseconds of `ROUNDS` runs of `run`.
fn mean_ms(mut run: impl FnMut()) -> f64 {
    run();
    let start = Instant::now();
    for _ in 0..ROUNDS {
        run();
    }
    start.elapsed().as_secs_f64() * 1000.0 / f64::from(ROUNDS)
}

/// Skipping a core or re-announcing seen rows would lose or duplicate cards; time the complete
/// 72-hour read and the decision with every row already recorded.
#[test]
#[ignore = "scale probe"]
fn probe_notify_window_read() {
    let now = 1_700_000_000_i64;
    let from = now - 72 * 3600;
    let conn = source(5_000, from, now);
    let names = moon_core::db::CoreNames::default();
    let mut len = 0;
    let ms = mean_ms(|| {
        len = super::super::read_closed_since_on(&conn, chrono_tz::UTC, &names, from)
            .unwrap()
            .len();
    });
    eprintln!("probe notify read: {len} closes, {ms:.2} ms");
    assert_eq!(len, 5_000, "the probe must read the complete window");
    let trades = super::super::read_closed_since_on(&conn, chrono_tz::UTC, &names, from).unwrap();
    let mut sorted = trades.clone();
    let sort_ms = mean_ms(|| {
        sorted.clone_from(&trades);
        sorted.sort_by(|left, right| {
            left.close_utc
                .cmp(&right.close_utc)
                .then(left.rec_id.cmp(&right.rec_id))
        });
    });
    eprintln!("probe notify clone+sort: {sort_ms:.3} ms");
    let rule = moon_core::telegram::notify::TradeRule {
        on: true,
        ..Default::default()
    };
    let mut ledger = moon_core::telegram::notify::NotifyLedger {
        trades_enabled_utc: Some(from),
        ..Default::default()
    };
    let visible = (1..=CORES as u64).collect::<Vec<_>>();
    let announced = crate::notify::trades::decide(&rule, &mut ledger, &visible, &trades, now);
    assert_eq!(announced.len(), 5_000);
    let decide_ms = mean_ms(|| {
        assert!(
            crate::notify::trades::decide(&rule, &mut ledger, &visible, &trades, now).is_empty()
        );
    });
    eprintln!("probe notify already-seen decision: {decide_ms:.3} ms");
}

/// By-core, by-exchange and daily reports over the period, both bases; losing a slice's rows
/// would produce a timing without exercising the full heavy-user report.
#[test]
#[ignore = "scale probe"]
fn probe_report_totals() {
    let _locale = crate::test_locale::force("en");
    let from = 1_700_000_000_i64;
    let to = from + 30 * 86_400;
    let conn = source(20_000, from, to);
    let venues: std::collections::HashMap<u64, moon_core::venue::CoreVenue> = (1..=CORES as u64)
        .map(|core| {
            (
                core,
                moon_core::venue::CoreVenue::identify((core % 5 + 1) as u8, "", None),
            )
        })
        .collect();
    for basis in [ReportBasis::Close, ReportBasis::Open] {
        for (by_exchange, daily) in [(false, false), (true, false), (false, true)] {
            let mut orders = 0;
            let ms = mean_ms(|| {
                let mut request = ReportRequest::new(Period::Month, daily);
                request.by_exchange = by_exchange;
                orders = super::super::read_page_on(
                    &conn,
                    request,
                    from,
                    to,
                    chrono_tz::UTC,
                    basis,
                    &Default::default(),
                    |_| (venues.clone(), TelegramReportAccess::Owner),
                )
                .unwrap()
                .total
                .orders;
                assert_eq!(orders, 20_000, "every report must cover the entire period");
            });
            eprintln!(
                "probe report {basis:?} by_exchange={by_exchange} daily={daily}: {orders} orders, {ms:.2} ms"
            );
        }
    }
}

/// Time the actual read and render at the interactive workload, retaining every deal in totals.
#[test]
#[ignore = "100000-deal interactive scale probe"]
fn probe_interactive_report() {
    let _locale = crate::test_locale::force("en");
    let from = 1_700_000_000_i64;
    let to = from + 30 * 86_400;
    let conn = source(100_000, from, to);
    let mut reads = Vec::new();
    let mut renders = Vec::new();
    for _ in 0..20 {
        let start = Instant::now();
        let page = super::super::read_page_on(
            &conn,
            ReportRequest::preset(Period::Today),
            from,
            to,
            chrono_tz::UTC,
            ReportBasis::Close,
            &Default::default(),
            |_| (Default::default(), TelegramReportAccess::Owner),
        )
        .unwrap();
        reads.push(start.elapsed().as_secs_f64() * 1000.0);
        assert_eq!(page.total.orders, 100_000);
        let start = Instant::now();
        let response = super::super::render::render(
            &page,
            crate::HostKind::Station,
            moon_core::telegram::api::ReplyMarkup::Inline(
                moon_core::telegram::api::InlineKeyboardMarkup::from_rows(vec![]),
            ),
        );
        assert!(matches!(
            response,
            moon_core::telegram::runtime::Response::Rich { .. }
        ));
        renders.push(start.elapsed().as_secs_f64() * 1000.0);
    }
    reads.sort_by(f64::total_cmp);
    renders.sort_by(f64::total_cmp);
    eprintln!(
        "interactive 100000 deals / 10 cores: read p50={:.3} p95={:.3} ms; render p50={:.3} p95={:.3} ms",
        reads[9], reads[18], renders[9], renders[18]
    );
}

/// Measure the exact Mini App top-N read over 100000 deals on ten differently shifted cores.
#[test]
#[ignore = "synthetic reader timing probe"]
fn mini_app_utc_top_n_scale_probe() {
    let conn = source(100_000, 1_700_000_000, 1_700_200_000);
    conn.execute_batch(
        "CREATE TABLE core_time_offset (
            core_uid INTEGER, from_utc INTEGER, offset_secs INTEGER,
            observed_at INTEGER, source TEXT
        );
        INSERT INTO core_time_offset
            SELECT core_uid, 0, (core_uid%5-2)*900, 0, 'fixture' FROM orders_rep
            GROUP BY core_uid;
        ANALYZE;",
    )
    .unwrap();
    let mut samples = Vec::new();
    for _ in 0..8 {
        let started = Instant::now();
        let trades = super::super::read_mini_trades_on(
            &conn,
            chrono_tz::UTC,
            TelegramReportAccess::Owner,
            Default::default(),
            50,
            None,
        )
        .unwrap();
        assert_eq!(trades.len(), 50);
        std::hint::black_box(trades);
        samples.push(started.elapsed().as_secs_f64() * 1000.0);
    }
    samples.sort_by(f64::total_cmp);
    println!("mini_app utc top-n 100000 deals: {:.3} ms", samples[4]);
}
