//! Report-replica start-state and alive-map reconciliation regression tests.

use super::*;
use crate::db::init_db;
use moonproto::{ReportSyncComplete, ReportSyncTicket};
use rusqlite::Connection;

/// Open an in-memory replica carrying `deleted`, plus the shared start-state map.
///
/// The column is created BEFORE `init` so the schema cache (`st.cols`) carries it — the state
/// `apply_schema` leaves once a core has declared the column.
fn replica_with_deleted() -> (Connection, RepState, Arc<Mutex<HashMap<u64, ReportStart>>>) {
    let conn = Connection::open_in_memory().unwrap();
    init_db(&conn).unwrap();
    conn.execute_batch(
        "CREATE TABLE orders_rep (core_uid INTEGER NOT NULL, core_name TEXT NOT NULL,
            newrecid INTEGER NOT NULL, deleted INTEGER, PRIMARY KEY (core_uid, newrecid));",
    )
    .unwrap();
    let starts = Arc::new(Mutex::new(HashMap::new()));
    let open_rows = Arc::new(Mutex::new(HashMap::new()));
    let st = init(&conn, starts.clone(), open_rows).unwrap();
    (conn, st, starts)
}

/// Insert `newrecid = 1..=n` for `core_uid`, all visible.
fn seed_rows(conn: &Connection, core_uid: i64, n: i64) {
    let mut stmt = conn
        .prepare(
            "INSERT INTO orders_rep (core_uid, core_name, newrecid, deleted)
             VALUES (?1, 'Rep', ?2, 0)",
        )
        .unwrap();
    for rec in 1..=n {
        stmt.execute(rusqlite::params![core_uid, rec]).unwrap();
    }
}

/// Read one row's `deleted` flag, or `None` when the row is gone.
fn deleted_of(conn: &Connection, core_uid: i64, rec_id: i64) -> Option<i64> {
    conn.query_row(
        "SELECT COALESCE(deleted,0) FROM orders_rep WHERE core_uid=?1 AND newrecid=?2",
        rusqlite::params![core_uid, rec_id],
        |r| r.get(0),
    )
    .ok()
}

/// Build a completion describing a catch-up over `1..=max_rec_id` of `epoch`.
fn completion(epoch: i32, max_rec_id: i64) -> ReportSyncComplete {
    ReportSyncComplete {
        ticket: ReportSyncTicket { sync_id: 1 },
        page_count: 1,
        total_rows: 1,
        epoch,
        max_rec_id,
        next_from_rec_id: max_rec_id + 1,
    }
}

/// A committed `SyncComplete` must NOT store the durable checkpoint.
///
/// Breaks on: `db/rep.rs:apply_sync_complete` (or `commit_sync_complete`) gaining a
/// `store_checkpoint(conn, core_uid, done.checkpoint())` call — the "obvious" simplification that
/// removes the alive-map round trip. Catch-up advances by `newRecID` and cannot observe an offline
/// soft-delete, restore or retention removal of an older row, so a checkpoint stored there records
/// a repair that never ran: every following session resumes past those rows and the terminal keeps
/// showing trades the core deleted, permanently.
#[test]
fn the_checkpoint_is_stored_only_with_the_alive_map() {
    let (conn, mut st, starts) = replica_with_deleted();
    seed_rows(&conn, 2, 4);

    apply_sync_complete(&conn, &mut st, 2, &completion(91, 4)).unwrap();
    commit_sync_complete(2, &completion(91, 4));

    assert_eq!(load_checkpoint(&conn, 2), None);
    assert!(starts.lock().unwrap().get(&2).is_none());

    // The alive map for the SAME completion is what stores it.
    let done = completion(91, 4);
    let applied = apply_alive_map(
        &conn,
        &st,
        2,
        done.max_rec_id,
        |_| Some(true),
        done.checkpoint(),
    )
    .unwrap()
    .expect("a replica with `deleted` applies the map");
    commit_alive_map(&st, 2, done.checkpoint(), applied);

    assert_eq!(load_checkpoint(&conn, 2), Some(done.checkpoint()));
    assert_eq!(
        starts.lock().unwrap().get(&2).copied(),
        Some(ReportStart::Checkpoint(done.checkpoint()))
    );
}

/// A clear bit must HIDE a row, never remove it.
///
/// Breaks on: `db/rep.rs:apply_alive_map` replacing its `UPDATE ... SET deleted=1` with a
/// `DELETE FROM orders_rep` — a tempting reading of "physically absent on the core". The map
/// cannot distinguish a soft-delete from a retention removal, so deleting locally would destroy
/// history that a later core-side restore is supposed to bring back, and "show deleted" would
/// have nothing left to show.
#[test]
fn the_alive_map_hides_rows_without_deleting_them() {
    let (conn, st, _starts) = replica_with_deleted();
    seed_rows(&conn, 2, 10);
    // A decoy on another core sharing an affected newrecid: the map is scoped to one core.
    conn.execute(
        "INSERT INTO orders_rep (core_uid, core_name, newrecid, deleted) VALUES (3, 'Other', 3, 0)",
        [],
    )
    .unwrap();

    let done = completion(91, 10);
    let applied = apply_alive_map(
        &conn,
        &st,
        2,
        done.max_rec_id,
        |rec_id| Some(rec_id != 3 && rec_id != 7),
        done.checkpoint(),
    )
    .unwrap()
    .expect("a replica with `deleted` applies the map");

    let rows: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM orders_rep WHERE core_uid=2",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(rows, 10, "hidden rows must survive");
    assert_eq!(applied.hidden, 2);
    assert_eq!(applied.revealed, 0);
    for rec in 1..=10 {
        let want = i64::from(rec == 3 || rec == 7);
        assert_eq!(deleted_of(&conn, 2, rec), Some(want), "newrecid {rec}");
    }
    assert_eq!(
        deleted_of(&conn, 3, 3),
        Some(0),
        "another core is untouched"
    );

    // A later map that calls row 3 alive again reveals it, which is what "hide, do not delete"
    // buys: the row is still there to reveal.
    let applied = apply_alive_map(
        &conn,
        &st,
        2,
        done.max_rec_id,
        |rec_id| Some(rec_id != 7),
        done.checkpoint(),
    )
    .unwrap()
    .expect("a replica with `deleted` applies the map");
    assert_eq!(applied.revealed, 1);
    assert_eq!(deleted_of(&conn, 2, 3), Some(0));
}

/// A stored checkpoint must be used as-is, never raised to the local maximum `newrecid`.
///
/// Breaks on: `db/rep.rs:startup_start` returning `max(stored.next_from_rec_id, max_local + 1)`,
/// which looks like a harmless optimization because the local maximum is normally the larger of
/// the two. Live upserts land above the checkpoint between two catch-ups, so taking the maximum
/// silently skips every page in between — the exact hole the checkpoint exists to prevent.
#[test]
fn a_stored_checkpoint_is_not_raised_to_max_newrecid() {
    let (conn, _st, _starts) = replica_with_deleted();
    seed_rows(&conn, 2, 150);
    let checkpoint = ReportSyncCheckpoint {
        epoch: 7,
        next_from_rec_id: 100,
    };
    store_checkpoint(&conn, 2, checkpoint).unwrap();

    assert_eq!(startup_start(&conn, 2), ReportStart::Checkpoint(checkpoint));
}

/// A replica with rows but no stored epoch must RESUME, not re-download everything.
///
/// Breaks on: `db/rep.rs:startup_start` folding the no-checkpoint case into `ReportStart::Fresh`
/// — the tidy-up that makes the match two arms instead of three. That path would re-download the
/// whole report history (hundreds of MB per replica) even though resuming once and reconciling the
/// resulting alive map establishes the required checkpoint.
#[test]
fn an_existing_replica_without_an_epoch_resumes_instead_of_resyncing() {
    let (conn, _st, _starts) = replica_with_deleted();
    seed_rows(&conn, 2, 40);

    assert_eq!(startup_start(&conn, 2), ReportStart::Resume(41));

    // An empty replica is the genuinely fresh case, checkpoint or not.
    store_checkpoint(
        &conn,
        5,
        ReportSyncCheckpoint {
            epoch: 7,
            next_from_rec_id: 900,
        },
    )
    .unwrap();
    assert_eq!(startup_start(&conn, 5), ReportStart::Fresh);
}

/// The visibility scan covers the local rows the map speaks for, and only those.
///
/// Two ends, because the bound is wrong in two opposite ways:
///
/// Breaks on: `db/rep.rs:apply_alive_map` dropping the `newrecid<=?2` bound from its scan. Rows
/// ABOVE the coverage are ones the map says nothing about — live trades that arrived after the
/// catch-up — and `is_alive` returns `None` for them, so an unbounded scan would walk rows the
/// core never described and any later reading of `None` as "not alive" would hide them.
///
/// Breaks on: that same scan being replaced by a loop over `1..=covered_up_to` probing each id —
/// the straightforward reading of "authoritative for 1..=covered_up_to". `covered_up_to` is a
/// core-side high-water in the millions, so that loop would block the sole report writer for
/// minutes on every reconnect while the feed thread backs up behind it. The second half of this
/// test is what catches it: a coverage far above any local row, where an id loop would report
/// (and cost) a million inspections instead of eleven.
#[test]
fn the_visibility_scan_is_bounded_by_the_maps_coverage() {
    let (conn, st, _starts) = replica_with_deleted();
    seed_rows(&conn, 2, 10);
    // Row 11 arrived live, above the coverage of the catch-up this map answers.
    conn.execute(
        "INSERT INTO orders_rep (core_uid, core_name, newrecid, deleted) VALUES (2, 'Rep', 11, 0)",
        [],
    )
    .unwrap();

    let checkpoint = ReportSyncCheckpoint {
        epoch: 91,
        next_from_rec_id: 11,
    };
    // Everything the map covers is dead; the uncovered row must not be judged by it.
    let applied = apply_alive_map(
        &conn,
        &st,
        2,
        10,
        |rec_id| (rec_id <= 10).then_some(false),
        checkpoint,
    )
    .unwrap()
    .expect("a replica with `deleted` applies the map");

    assert_eq!(applied.inspected, 10, "only the covered rows are scanned");
    assert_eq!(applied.hidden, 10);
    assert_eq!(
        deleted_of(&conn, 2, 11),
        Some(0),
        "uncovered row is untouched"
    );

    // Coverage far above every local row: the work is bounded by the 11 rows that exist, not by
    // the core's high-water. An id-probing loop would inspect a million ids to reach the same
    // eleven rows.
    let wide = apply_alive_map(
        &conn,
        &st,
        2,
        1_000_000,
        |_| Some(true),
        ReportSyncCheckpoint {
            epoch: 91,
            next_from_rec_id: 1_000_001,
        },
    )
    .unwrap()
    .expect("a replica with `deleted` applies the map");
    assert_eq!(wide.inspected, 11, "the scan is bounded by the local rows");
}

/// Without the `deleted` column the checkpoint must NOT advance.
///
/// Breaks on: `db/rep.rs:apply_alive_map` storing the checkpoint before, or regardless of, its
/// column guard. A core whose schema omits `deleted` has nowhere to record visibility, so
/// recording the map as applied would mark the repair done while every clear bit went nowhere —
/// and no later session would ever retry it.
#[test]
fn a_replica_without_the_deleted_column_stores_no_checkpoint() {
    let conn = Connection::open_in_memory().unwrap();
    init_db(&conn).unwrap();
    conn.execute_batch(
        "CREATE TABLE orders_rep (core_uid INTEGER NOT NULL, core_name TEXT NOT NULL,
            newrecid INTEGER NOT NULL, PRIMARY KEY (core_uid, newrecid));",
    )
    .unwrap();
    let starts = Arc::new(Mutex::new(HashMap::new()));
    let st = init(&conn, starts.clone(), Arc::new(Mutex::new(HashMap::new()))).unwrap();
    seed_rows_without_deleted(&conn, 2, 4);

    let done = completion(91, 4);
    let applied = apply_alive_map(
        &conn,
        &st,
        2,
        done.max_rec_id,
        |_| Some(false),
        done.checkpoint(),
    )
    .unwrap();

    assert!(
        applied.is_none(),
        "no `deleted` column means no application"
    );
    assert_eq!(load_checkpoint(&conn, 2), None);
    assert!(starts.lock().unwrap().get(&2).is_none());
}

/// Insert rows into a replica that has no `deleted` column.
fn seed_rows_without_deleted(conn: &Connection, core_uid: i64, n: i64) {
    let mut stmt = conn
        .prepare("INSERT INTO orders_rep (core_uid, core_name, newrecid) VALUES (?1, 'Rep', ?2)")
        .unwrap();
    for rec in 1..=n {
        stmt.execute(rusqlite::params![core_uid, rec]).unwrap();
    }
}

/// A replica reset must drop the durable checkpoint with the rows.
///
/// Breaks on: `db/rep.rs:reset_replica` losing its `clear_checkpoint` call and retaining only the
/// `DELETE FROM orders_rep`. The rows would go while the checkpoint stayed, so the next start
/// would resume with a stale epoch against the new database, detect recreation again, wipe the
/// partly rebuilt replica, and loop — a full re-download every restart, with nothing failing.
///
/// What this does NOT cover: that `apply_page`'s `database_recreated` branch and
/// `apply_replica_recreated` both ROUTE through `reset_replica`. Driving the page path needs a
/// `ReportSyncPage`, which moonproto keeps unconstructible outside its own crate (`wire_row_count`
/// is private), so a caller open-coding its own `DELETE` stays green here — that one is guarded by
/// review, not by this test.
#[test]
fn a_recreated_database_clears_the_checkpoint() {
    let (conn, _st, _starts) = replica_with_deleted();
    seed_rows(&conn, 2, 6);
    seed_rows(&conn, 3, 6);
    store_checkpoint(
        &conn,
        2,
        ReportSyncCheckpoint {
            epoch: 7,
            next_from_rec_id: 7,
        },
    )
    .unwrap();
    store_checkpoint(
        &conn,
        3,
        ReportSyncCheckpoint {
            epoch: 8,
            next_from_rec_id: 7,
        },
    )
    .unwrap();

    reset_replica(&conn, 2).unwrap();

    assert_eq!(load_checkpoint(&conn, 2), None);
    assert_eq!(startup_start(&conn, 2), ReportStart::Fresh);
    // Scoped to one core: the other core keeps both its rows and its checkpoint.
    assert!(load_checkpoint(&conn, 3).is_some());
    assert_eq!(deleted_of(&conn, 3, 1), Some(0));
}

/// An agreeing row must break a disagreement range.
///
/// Breaks on: `db/rep.rs:DisagreementRanges::push` merging by adjacency alone — dropping the
/// `running` guard so ids 1 and 3 coalesce into `1..=3`. Row 2 would then be flipped although the
/// map agrees with the replica about it, hiding a live trade (or revealing a deleted one) that the
/// core never mentioned.
#[test]
fn coalescing_breaks_a_range_at_an_agreeing_row() {
    let mut ranges = DisagreementRanges::default();
    // 1 and 3 must be hidden; 2 already agrees with the map.
    ranges.push(1, false, Some(false));
    ranges.push(2, false, Some(true));
    ranges.push(3, false, Some(false));
    let (hide, reveal) = ranges.finish();

    assert_eq!(hide, vec![(1, 1), (3, 3)]);
    assert!(reveal.is_empty());

    // A physical gap with no local row in it does NOT break a range: nothing lives there to flip.
    let mut ranges = DisagreementRanges::default();
    ranges.push(1, false, Some(false));
    ranges.push(9, false, Some(false));
    assert_eq!(ranges.finish().0, vec![(1, 9)]);

    // Ids outside the map's coverage say nothing and must break the range rather than extend it.
    let mut ranges = DisagreementRanges::default();
    ranges.push(1, false, Some(false));
    ranges.push(2, false, None);
    ranges.push(3, false, Some(false));
    assert_eq!(ranges.finish().0, vec![(1, 1), (3, 3)]);
}
