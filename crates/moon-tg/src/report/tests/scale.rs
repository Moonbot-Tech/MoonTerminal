//! Scale probes for a heavy account: 10 cores, 20 000 deals in the report period, 5 000 closes in
//! the notification window. Ignored by default; run with `--ignored --nocapture` to print timings.

use std::time::Instant;

use moon_core::config::telegram_access::TelegramReportAccess;
use moon_core::config::telegram_menu::ReportBasis;
use moon_core::telegram::report::{Period, ReportRequest};

const CORES: i64 = 10;
const ROUNDS: u32 = 5;

/// A report source with the replica's indexes and `deals` rows inside both period bases across
/// [`CORES`], so a table scan or an out-of-period entry cannot distort the measured workload.
fn source(deals: i64, from: i64, to: i64) -> rusqlite::Connection {
    let conn = rusqlite::Connection::open_in_memory().unwrap();
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
        let step = ((to - from) / deals).max(1);
        for deal in 0..deals {
            let core = deal % CORES + 1;
            let close = from + 30 + deal * step;
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
