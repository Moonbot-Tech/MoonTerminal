//! Offline recovery scheduling, replica paging and catalog-readiness regressions.

use super::*;
use crate::db::{ReportAxis, ReportStamp};
use crate::market::tape_recorder::TradeId;

/// A later resolved or empty page must not erase the protecting identity skipped earlier.
#[test]
fn skipped_rows_keep_the_listing_incomplete() {
    let mut listing = ListingProgress::default();
    listing.page(1000, 1000);
    assert!(listing.complete());
    listing.page(1000, 999);
    listing.page(1000, 1000);
    listing.page(0, 0);
    assert!(!listing.complete());
    let mut complete = ListingProgress::default();
    complete.page(1000, 1000);
    complete.page(40, 40);
    assert!(complete.complete());
}

/// Older replicas without the emulator column must remain readable, while null means plain.
#[test]
fn catchup_sql_reads_emulator_flag() {
    for with_column in [false, true] {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE orders_rep(core_uid INTEGER, newrecid INTEGER, coin TEXT, buydate INTEGER, closedate INTEGER); INSERT INTO orders_rep VALUES(1,1,'XALPHA',10,20),(1,2,'XALPHA',10,20),(1,3,'XALPHA',10,20);").unwrap();
        if with_column {
            conn.execute_batch("ALTER TABLE orders_rep ADD COLUMN emulator INTEGER; UPDATE orders_rep SET emulator=1 WHERE newrecid=1; UPDATE orders_rep SET emulator=0 WHERE newrecid=2;").unwrap();
        }
        let rows = catchup_page(&conn, &catchup_sql(&conn).unwrap(), 1, 0, None).unwrap();
        assert_eq!(
            rows.iter()
                .map(|row| (row.rec_id, row.emulator))
                .collect::<Vec<_>>(),
            vec![(3, false), (2, false), (1, with_column)]
        );
    }
}

/// Losing the sync trigger or hourly boundary would leave closed trades without recovery.
#[test]
fn sync_edge_and_hourly_boundary_are_due() {
    let now = Instant::now();
    assert!(catchup_due(None, now, false));
    assert!(catchup_due(Some(now), now, true));
    assert!(!catchup_due(
        Some(now),
        now + Duration::from_secs(3599),
        false
    ));
    assert!(catchup_due(
        Some(now),
        now + Duration::from_secs(3600),
        false
    ));
}

/// Open rows and rows past the 365-day boundary must never reach the tape planner.
#[test]
fn mapping_excludes_open_and_old_rows() {
    let _guard = crate::market::tape_recorder::catchup::tests::SWITCH_LOCK
        .lock()
        .unwrap();
    let now = 40_000_000_000;
    let axis = ReportAxis::identity_core_local();
    let mut row = CatchupRow {
        rec_id: 4,
        coin: "SYN".into(),
        buy: ReportStamp::Seconds(1),
        close: ReportStamp::Millis(now - 364 * 86_400_000),
        closedate: 1,
        emulator: false,
    };
    assert!(catchup_stamps(&row, &axis, 1, now).is_some());
    row.close = ReportStamp::Millis(now - 366 * 86_400_000);
    assert!(catchup_stamps(&row, &axis, 1, now).is_none());
    row.closedate = 0;
    row.close = ReportStamp::Millis(now);
    assert!(catchup_stamps(&row, &axis, 1, now).is_none());
}

/// A report sync before the catalog arrives must not permanently reject its coins.
#[test]
fn catalog_pending_does_not_latch_unknown() {
    let mut latched = HashSet::new();
    let id = TradeId { core: 1, rec_id: 5 };
    assert!(!latch_unknown(&mut latched, id, false, true));
    assert!(!latch_unknown(&mut latched, id, true, false));
    assert!(latched.is_empty());
}

/// An actually unknown coin must count once across all later hourly passes.
#[test]
fn ready_unknown_coin_latches_once() {
    let mut latched = HashSet::new();
    let id = TradeId { core: 1, rec_id: 5 };
    assert!(latch_unknown(&mut latched, id, true, true));
    assert!(!latch_unknown(&mut latched, id, true, true));
    assert_eq!(latched.len(), 1);
}

/// Equal close times must not starve the rows after the first thousand replica IDs.
#[test]
fn keyset_pages_cover_equal_close_times_and_skip_deleted() {
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    conn.execute_batch("CREATE TABLE orders_rep (core_uid INTEGER, newrecid INTEGER, coin TEXT, buydate INTEGER, closedate INTEGER, deleted INTEGER);
        WITH RECURSIVE ids(n) AS (SELECT 1 UNION ALL SELECT n+1 FROM ids WHERE n < 1005)
        INSERT INTO orders_rep SELECT 1, n, 'SYN', 10, 20, 0 FROM ids;
        INSERT INTO orders_rep VALUES (1,1006,'SYN',10,20,1),(1,1007,'SYN',10,0,0),(1,1008,'SYN',10,19,0);").unwrap();
    let sql = catchup_sql(&conn).unwrap();
    let first = catchup_page(&conn, &sql, 1, 20, None).unwrap();
    assert_eq!(first.len(), 1000);
    assert_eq!(first[0].rec_id, 1005);
    let last = first.last().unwrap();
    let second = catchup_page(&conn, &sql, 1, 20, Some((last.closedate, last.rec_id))).unwrap();
    assert_eq!(
        second.iter().map(|row| row.rec_id).collect::<Vec<_>>(),
        vec![5, 4, 3, 2, 1]
    );
}

/// A partially replicated row must not abort the whole page and starve valid neighboring trades.
#[test]
fn nullable_partial_rows_do_not_abort_replica_pages() {
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    conn.execute_batch("CREATE TABLE orders_rep (core_uid INTEGER, newrecid INTEGER, coin TEXT, buydate INTEGER, closedate INTEGER);
        INSERT INTO orders_rep VALUES (1,2,NULL,NULL,20),(1,1,'SYN',10,20);").unwrap();
    let rows = catchup_page(&conn, &catchup_sql(&conn).unwrap(), 1, 20, None).unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].coin, "");
    assert!(catchup_stamps(&rows[0], &ReportAxis::identity_core_local(), 1, 20_000).is_none());
    assert_eq!(
        catchup_stamps(&rows[1], &ReportAxis::identity_core_local(), 1, 20_000),
        Some((10_000, 20_000))
    );
}

/// A core-local close near the depth boundary must be judged on UTC rather than raw seconds.
#[test]
fn depth_bound_and_millisecond_stamps_follow_measured_axis() {
    let _guard = crate::market::tape_recorder::catchup::tests::SWITCH_LOCK
        .lock()
        .unwrap();
    let now = 40_000_000_000;
    let cutoff = now - crate::market::trade_replay::venue_caps::ARCHIVE_DEPTH_MS;
    let axis = ReportAxis::from_measured(
        HashMap::from([(
            1,
            vec![crate::db::OffsetSegment {
                from_utc: 0,
                offset_secs: 7200,
            }],
        )]),
        chrono_tz::UTC,
    );
    assert_eq!(axis.from_utc(cutoff / 1000, 1), cutoff / 1000 + 7200);
    let row = CatchupRow {
        rec_id: 4,
        coin: "SYN".into(),
        buy: ReportStamp::Seconds((cutoff - 60_000) / 1000 + 7200),
        close: ReportStamp::Millis(cutoff + 7_200_001),
        closedate: cutoff / 1000 + 7200,
        emulator: false,
    };
    assert_eq!(
        catchup_stamps(&row, &axis, 1, now),
        Some((cutoff - 60_000, cutoff + 1))
    );
}

/// A boundary crossing close dates must retain the exact cutoff row without repeating IDs.
#[test]
fn keyset_crosses_close_dates_and_includes_cutoff() {
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    conn.execute_batch("CREATE TABLE orders_rep (core_uid INTEGER, newrecid INTEGER, coin TEXT, buydate INTEGER, closedate INTEGER);
        WITH RECURSIVE ids(n) AS (SELECT 1 UNION ALL SELECT n+1 FROM ids WHERE n < 1005)
        INSERT INTO orders_rep SELECT 1, n, 'SYN', 10, CASE WHEN n > 7 THEN 21 ELSE 20 END FROM ids;
        INSERT INTO orders_rep VALUES (1,2000,'SYN',10,19);").unwrap();
    let sql = catchup_sql(&conn).unwrap();
    let first = catchup_page(&conn, &sql, 1, 20, None).unwrap();
    let last = first.last().unwrap();
    assert_eq!((last.closedate, last.rec_id), (20, 6));
    let second = catchup_page(&conn, &sql, 1, 20, Some((last.closedate, last.rec_id))).unwrap();
    assert_eq!(
        second
            .iter()
            .map(|row| (row.closedate, row.rec_id))
            .collect::<Vec<_>>(),
        vec![(20, 5), (20, 4), (20, 3), (20, 2), (20, 1)]
    );
}

/// Running the listing inline would block session drain; concurrent triggers must share one guard.
#[test]
fn listing_is_background_and_single_per_core() {
    let caller = std::thread::current().id();
    let running = Arc::new(AtomicBool::new(false));
    let (started, rx) = std::sync::mpsc::channel();
    let (release, wait) = std::sync::mpsc::channel();
    assert!(spawn_catchup(running.clone(), move || {
        started.send(std::thread::current().id()).unwrap();
        wait.recv().unwrap();
    }));
    assert_ne!(rx.recv_timeout(Duration::from_secs(2)).unwrap(), caller);
    assert!(!spawn_catchup(running.clone(), || panic!(
        "duplicate listing"
    )));
    release.send(()).unwrap();
}
/// Unattributable rows do not own a market window and cannot indefinitely veto maintenance.
#[test]
fn permanently_unattributable_rows_do_not_block_the_listing() {
    let mut listing = ListingProgress::default();
    // One unknown coin and one invalid stamp among otherwise resolved rows.
    listing.resolved_page(1000, 998, 2);
    listing.resolved_page(40, 40, 0);
    assert!(listing.complete());
    listing.resolved_page(1, 0, 0);
    assert!(
        !listing.complete(),
        "a transient catalog gap still protects tape"
    );
}

/// The protecting query reaches straddlers older than a year without listing all old rows.
#[test]
fn protecting_listing_reads_only_retention_straddlers() {
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    conn.execute_batch("CREATE TABLE orders_rep (core_uid INTEGER, newrecid INTEGER, coin TEXT, buydate INTEGER, closedate INTEGER);
        INSERT INTO orders_rep VALUES (1,1,'XALPHA',10,30),(1,2,'XALPHA',21,30),(1,3,'XALPHA',1,19);").unwrap();
    let rows = catchup_page(&conn, &listing_sql(&conn, true).unwrap(), 1, 20, None).unwrap();
    assert_eq!(
        rows.iter().map(|row| row.rec_id).collect::<Vec<_>>(),
        vec![1]
    );
}

/// A listed row is tuner-usable only when BOTH stamps came from the millisecond columns, the
/// same gate the tuner's deal read applies. Treating a seconds fallback as usable would keep
/// fetching tape for the pre-millisecond history the switch exists to skip.
#[test]
fn tuner_can_replay_needs_both_millisecond_stamps() {
    let row = |buy, close| CatchupRow {
        rec_id: 4,
        coin: "SYN".into(),
        buy,
        close,
        closedate: 2,
        emulator: false,
    };
    assert!(tuner_can_replay(&row(
        ReportStamp::Millis(1_000),
        ReportStamp::Millis(2_000)
    )));
    assert!(!tuner_can_replay(&row(
        ReportStamp::Seconds(1),
        ReportStamp::Millis(2_000)
    )));
    assert!(!tuner_can_replay(&row(
        ReportStamp::Millis(1_000),
        ReportStamp::Seconds(2)
    )));
}
