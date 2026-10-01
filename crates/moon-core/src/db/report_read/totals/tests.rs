//! Every slice of a sliced totals read states exactly what its own `query_totals` states.

use rusqlite::Connection;

use super::super::{ReportFilter, RowScope, query_totals};
use super::{TotalsSlice, query_totals_sliced};

/// Day length the fixture's windows are cut on.
const DAY: i64 = 86_400;
/// First day of the fixture period, true UTC.
const START: i64 = 1_790_000_000 - 1_790_000_000 % DAY;

/// A replica with the money columns the totals read: three cores (two on measured clocks one
/// either side of UTC, one never measured), three quotes, reals across magnitudes and signs,
/// Funding and liquidation rows, and closes on both edges of every day.
fn fixture() -> Connection {
    let conn = Connection::open_in_memory().expect("open sliced fixture");
    conn.execute_batch(
        "CREATE TABLE orders_rep (
             core_uid INTEGER NOT NULL, core_name TEXT, newrecid INTEGER NOT NULL,
             coin TEXT, fname TEXT, basecurrency INTEGER, closedate INTEGER, buydate INTEGER,
             profitbtc REAL, spentbtc REAL, boughtq REAL, buyprice REAL, sellprice REAL,
             sellreason TEXT, emulator INTEGER, deleted INTEGER,
             PRIMARY KEY (core_uid, newrecid)
         );
         CREATE INDEX idx_rep_closedate ON orders_rep(closedate);
         CREATE INDEX idx_rep_core_close ON orders_rep(core_uid, closedate);",
    )
    .expect("create sliced fixture");
    let mut state = 0x2545_f491_4f6c_dd1d_u64;
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    let mut insert = conn
        .prepare(
            "INSERT INTO orders_rep VALUES (?1, 'core', ?2, 'COIN', '', ?3, ?4, ?5, ?6, ?7, ?8,
                                            ?9, ?10, ?11, ?12, 0)",
        )
        .expect("prepare fixture insert");
    for rec in 0..3_000_i64 {
        let core = [1_i64, 2, 3][(next() % 3) as usize];
        // Offsets below are +3600 for core 1 and -7200 for core 2, so a close near midnight
        // lands on a different UTC day than its raw value says.
        let close = START - DAY + (next() % (9 * DAY as u64)) as i64;
        let magnitude = [1e-9, 1e-3, 1.0, 1e4, 1e9][(next() % 5) as usize];
        let profit = ((next() % 2_000_001) as f64 - 1_000_000.0) / 999_983.0 * magnitude;
        let spent = (next() % 5_000) as f64 / 7.0 * magnitude;
        let qty = (next() % 900 + 1) as f64 / 3.0;
        let buy = (next() % 50_000 + 1) as f64 / 1_000.0;
        let sell = buy * (1.0 + ((next() % 200) as f64 - 100.0) / 10_000.0);
        let reason = ["Sell", "Stop", "Funding", "LIQUIDATION", ""][(next() % 5) as usize];
        let quote = [0_i64, 1, 5][(next() % 3) as usize];
        let emulator = i64::from(next() % 10 == 0);
        insert
            .execute(rusqlite::params![
                core,
                rec,
                quote,
                close,
                close - 3_600,
                profit,
                spent,
                qty,
                buy,
                sell,
                reason,
                emulator
            ])
            .expect("insert fixture row");
    }
    drop(insert);
    conn
}

/// The fixture's clock axis: core 1 ahead of UTC, core 2 behind, core 3 never measured.
fn axis() -> crate::db::ReportAxis {
    let segment = |offset_secs| {
        vec![crate::db::OffsetSegment {
            from_utc: 0,
            offset_secs,
        }]
    };
    crate::db::ReportAxis::from_measured(
        std::collections::HashMap::from([(1, segment(3_600)), (2, segment(-7_200))]),
        chrono_tz::UTC,
    )
}

/// The slices the Mini App asks for: the period, core groups, single cores, and every day.
fn slices(from: i64, to: i64) -> Vec<TotalsSlice> {
    let mut slices = vec![TotalsSlice {
        core_uids: None,
        date_from: Some(from),
        date_to: Some(to),
    }];
    for cores in [vec![1, 2], vec![3], vec![1], vec![2], vec![2, 3]] {
        slices.push(TotalsSlice {
            core_uids: Some(cores),
            date_from: Some(from),
            date_to: Some(to),
        });
    }
    let mut day = from;
    while day <= to {
        slices.push(TotalsSlice {
            core_uids: None,
            date_from: Some(day),
            date_to: Some((day + DAY - 1).min(to)),
        });
        day += DAY;
    }
    slices
}

/// Owner and scoped reads, over a period with a partial last day: each slice's totals, printed
/// in full, equal the totals its own filter reads.
#[test]
fn every_slice_equals_its_own_totals_read() {
    let conn = fixture();
    let (from, to) = (START, START + 6 * DAY + 50_000);
    for scope in [vec![], vec![1, 2, 3], vec![1, 2]] {
        let base = ReportFilter {
            core_uids: scope.clone(),
            date_from: Some(from),
            date_to: Some(to),
            emulator: Some(false),
            rows: RowScope::Closed,
            axis: axis(),
            ..ReportFilter::default()
        };
        let slices = slices(from, to)
            .into_iter()
            .filter(|slice| {
                slice
                    .core_uids
                    .as_ref()
                    .is_none_or(|cores| scope.is_empty() || cores.iter().all(|c| scope.contains(c)))
            })
            .collect::<Vec<_>>();
        let pass = one_pass(&conn, &base, &slices).expect("clean data takes the single pass");
        assert!(
            pass.iter().all(Option::is_some),
            "clean data refuses no slice"
        );
        let sliced = query_totals_sliced(&conn, &base, &slices).expect("sliced read");
        assert_eq!(sliced.len(), slices.len());
        for (slice, totals) in slices.iter().zip(&sliced) {
            let own = query_totals(&conn, &slice.filter(&base)).expect("own read");
            assert!(own.quotes.orders > 0, "the fixture must populate {slice:?}");
            assert_eq!(
                format!("{totals:?}"),
                format!("{own:?}"),
                "scope {scope:?}, slice {slice:?}"
            );
        }
    }
}

/// A money cell SQLite would coerce from TEXT sends only the slices holding it down their own
/// statements, which still answer exactly; a base filter the single pass does not cover sends
/// every slice there.
#[test]
fn unrepresentable_values_and_shapes_fall_back_to_per_slice_reads() {
    let conn = fixture();
    conn.execute(
        "UPDATE orders_rep SET profitbtc = 'n/a', closedate = ?1 WHERE newrecid = 7",
        [START + DAY],
    )
    .expect("store a TEXT profit inside the window");
    let (from, to) = (START, START + 3 * DAY);
    for rows in [RowScope::Closed, RowScope::ClosedAndOpen] {
        let base = ReportFilter {
            date_from: Some(from),
            date_to: Some(to),
            rows,
            axis: axis(),
            ..ReportFilter::default()
        };
        let slices = slices(from, to);
        if rows == RowScope::Closed {
            let pass = one_pass(&conn, &base, &slices).expect("the pass still places every row");
            let refused = pass.iter().filter(|totals| totals.is_none()).count();
            assert!(
                refused > 0 && refused < slices.len(),
                "only the slices holding the TEXT cell are refused: {refused} of {}",
                slices.len()
            );
        } else {
            assert!(!super::one_pass_serves(&base, &slices));
        }
        let sliced = query_totals_sliced(&conn, &base, &slices).expect("sliced read");
        for (slice, totals) in slices.iter().zip(&sliced) {
            let own = query_totals(&conn, &slice.filter(&base)).expect("own read");
            assert_eq!(
                format!("{totals:?}"),
                format!("{own:?}"),
                "{rows:?} {slice:?}"
            );
        }
    }
}

/// A slice reaching outside the base filter is answered by its own read, never clipped to the
/// base pass.
#[test]
fn slices_outside_the_base_are_not_clipped() {
    let conn = fixture();
    let base = ReportFilter {
        core_uids: vec![1],
        date_from: Some(START + DAY),
        date_to: Some(START + 2 * DAY),
        rows: RowScope::Closed,
        axis: axis(),
        ..ReportFilter::default()
    };
    let slices = [
        TotalsSlice {
            core_uids: Some(vec![2]),
            date_from: base.date_from,
            date_to: base.date_to,
        },
        TotalsSlice {
            core_uids: None,
            date_from: Some(START),
            date_to: base.date_to,
        },
    ];
    assert!(!super::one_pass_serves(&base, &slices));
    let sliced = query_totals_sliced(&conn, &base, &slices).expect("sliced read");
    for (slice, totals) in slices.iter().zip(&sliced) {
        let own = query_totals(&conn, &slice.filter(&base)).expect("own read");
        assert!(own.quotes.orders > 0);
        assert_eq!(format!("{totals:?}"), format!("{own:?}"));
    }
}

/// The single pass alone, without the per-slice fallback behind it.
fn one_pass(
    conn: &Connection,
    base: &ReportFilter,
    slices: &[TotalsSlice],
) -> Option<Vec<Option<super::ReportTotals>>> {
    let sources = super::read_sources_res(conn).expect("discover sources");
    let now = crate::util::now_unix_ms_i64().div_euclid(1_000);
    let meta = super::super::report_strategy_meta(conn, base).expect("strategy meta");
    super::sliced_attempt(conn, base, slices, &sources, &meta, false, now).expect("single pass")
}
