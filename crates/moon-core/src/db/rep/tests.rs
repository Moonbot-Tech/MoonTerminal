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
    let st = init(&conn, starts.clone()).unwrap();
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

/// An interrupted catch-up must resume at its frontier, not above the live rows (#665).
///
/// Breaks on: `db/rep.rs:startup_start` going back to `Resume(max + 1)` when a frontier is
/// stored. Live rows land above catch-up from the first second of a connection, so the local
/// maximum overstates what arrived and the span between the frontier and the first live row is
/// never requested again, in any later session.
#[test]
fn an_interrupted_catch_up_resumes_at_its_frontier() {
    let (conn, _st, _starts) = replica_with_deleted();
    // Catch-up delivered 1..=40, then live rows 900 and 901 arrived, then the terminal closed.
    seed_rows(&conn, 2, 40);
    for rec in [900, 901] {
        conn.execute(
            "INSERT INTO orders_rep (core_uid, core_name, newrecid, deleted) VALUES (2, 'Rep', ?1, 0)",
            [rec],
        )
        .unwrap();
    }
    record_frontier(&conn, 2, 40).unwrap();

    assert_eq!(startup_start(&conn, 2), ReportStart::Resume(41));
}

/// The frontier only rises, and an empty page moves nothing.
///
/// Breaks on: `db/rep.rs:record_frontier` writing `last_rec_id` unconditionally — an empty page
/// reports `last_rec_id = 0` and would send the next start back to the beginning of history, and a
/// page replayed after a retry would lower it.
#[test]
fn the_frontier_never_moves_down() {
    let (conn, _st, _starts) = replica_with_deleted();
    record_frontier(&conn, 2, 500).unwrap();
    record_frontier(&conn, 2, 0).unwrap();
    record_frontier(&conn, 2, 120).unwrap();
    assert_eq!(load_frontier(&conn, 2), Some(500));
    record_frontier(&conn, 2, 700).unwrap();
    assert_eq!(load_frontier(&conn, 2), Some(700));
}

/// Live rows of a core replicating from zero pin frontier 0, so an interruption before the first
/// page resumes from the beginning.
///
/// Breaks on: `db/rep.rs:note_live_row` being dropped or writing for every core. Without it a
/// fresh core whose first rows were live ones restarts at `max + 1` — the whole history under them
/// is lost. Writing it for a core already past its first page would reset a real frontier.
#[test]
fn live_rows_before_the_first_page_resume_from_the_beginning() {
    let (conn, st, starts) = replica_with_deleted();
    // Core 2 is fresh (absent from the start map), core 3 already resumes.
    starts.lock().unwrap().insert(3, ReportStart::Resume(51));
    record_frontier(&conn, 3, 50).unwrap();

    seed_rows(&conn, 2, 1);
    note_live_row(&conn, &st, 2).unwrap();
    note_live_row(&conn, &st, 3).unwrap();

    assert_eq!(load_frontier(&conn, 2), Some(0));
    assert_eq!(startup_start(&conn, 2), ReportStart::Resume(1));
    assert_eq!(
        load_frontier(&conn, 3),
        Some(50),
        "a resuming core keeps its frontier"
    );
}

/// A checkpoint retires the frontier, and a replica reset puts it back to zero.
///
/// Breaks on: `db/rep.rs:store_checkpoint` leaving the frontier behind, or `reset_replica` keeping
/// the old one or dropping it. A stale frontier would make the rebuilt replica resume mid-history
/// on its first interruption; no frontier would make a live row that lands before the first page
/// read as an old replica and resume above it.
#[test]
fn checkpoint_and_reset_retire_the_frontier() {
    let (conn, _st, _starts) = replica_with_deleted();
    seed_rows(&conn, 2, 10);
    record_frontier(&conn, 2, 10).unwrap();
    store_checkpoint(
        &conn,
        2,
        ReportSyncCheckpoint {
            epoch: 7,
            next_from_rec_id: 11,
        },
    )
    .unwrap();
    assert_eq!(load_frontier(&conn, 2), None);

    record_frontier(&conn, 2, 10).unwrap();
    reset_replica(&conn, 2).unwrap();
    assert_eq!(
        load_frontier(&conn, 2),
        Some(0),
        "a wiped replica restarts from the beginning, whatever lands before the first page"
    );
}

/// Opening a replica written before the frontier existed pins its old resume point, so live rows
/// of this session cannot raise it.
///
/// Breaks on: `db/rep.rs:init` no longer pinning the frontier for a `Resume` core. The next start
/// would read the maximum raised by this session's live rows.
#[test]
fn opening_an_old_replica_pins_its_resume_point() {
    let conn = Connection::open_in_memory().unwrap();
    init_db(&conn).unwrap();
    conn.execute_batch(
        "CREATE TABLE orders_rep (core_uid INTEGER NOT NULL, core_name TEXT NOT NULL,
            newrecid INTEGER NOT NULL, deleted INTEGER, PRIMARY KEY (core_uid, newrecid));",
    )
    .unwrap();
    seed_rows(&conn, 2, 40);
    let starts = Arc::new(Mutex::new(HashMap::new()));
    init(&conn, starts.clone()).unwrap();
    assert_eq!(
        starts.lock().unwrap().get(&2),
        Some(&ReportStart::Resume(41))
    );

    // A live row far above arrives; the next start still resumes at 41.
    conn.execute(
        "INSERT INTO orders_rep (core_uid, core_name, newrecid, deleted) VALUES (2, 'Rep', 900, 0)",
        [],
    )
    .unwrap();
    assert_eq!(startup_start(&conn, 2), ReportStart::Resume(41));
}

/// A committed page moves the in-memory start of a fresh or resuming core, never a checkpoint.
///
/// Breaks on: `db/rep.rs:commit_page` overwriting a checkpoint (the epoch is lost, so a recreated
/// core database goes undetected) or leaving a fresh core fresh (a reconnect in the same session
/// downloads the whole history again).
#[test]
fn a_committed_page_moves_the_session_start() {
    let (_conn, st, starts) = replica_with_deleted();
    let checkpoint = ReportSyncCheckpoint {
        epoch: 7,
        next_from_rec_id: 11,
    };
    starts
        .lock()
        .unwrap()
        .insert(4, ReportStart::Checkpoint(checkpoint));

    commit_page(&st, 2, 300);
    commit_page(&st, 2, 0);
    commit_page(&st, 4, 300);

    let m = starts.lock().unwrap();
    assert_eq!(m.get(&2), Some(&ReportStart::Resume(301)));
    assert_eq!(m.get(&4), Some(&ReportStart::Checkpoint(checkpoint)));
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
    let st = init(&conn, starts.clone()).unwrap();
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

/// Open an in-memory replica with `closedate` and `deleted`, both known to the column cache.
fn replica_with_close_dates() -> (Connection, RepState) {
    let conn = Connection::open_in_memory().unwrap();
    init_db(&conn).unwrap();
    conn.execute_batch(
        "CREATE TABLE orders_rep (core_uid INTEGER NOT NULL, core_name TEXT NOT NULL,
            newrecid INTEGER NOT NULL, closedate INTEGER, deleted INTEGER,
            PRIMARY KEY (core_uid, newrecid));",
    )
    .unwrap();
    let st = init(&conn, Arc::new(Mutex::new(HashMap::new()))).unwrap();
    (conn, st)
}

/// Insert one row of core 3 with the given close date and `deleted` flag.
fn put_row(conn: &Connection, rec_id: i64, closedate: Option<i64>, deleted: i64) {
    conn.execute(
        "INSERT INTO orders_rep VALUES (3, 'Rep', ?1, ?2, ?3)",
        rusqlite::params![rec_id, closedate, deleted],
    )
    .unwrap();
}

/// The open-row set is read from the replica when it is asked for, never from database open.
///
/// Breaks on: `db/rep.rs:open_row_ids` being computed once in `init` again, or losing its
/// `deleted` filter. The first loses a close that a reconnect dropped for a trade opened during
/// the session — it stays "open" until a restart (#742). The second spends the core's 100-id
/// budget re-checking deleted rows on every connect: 54 of 55 ids on one measured core.
#[test]
fn the_open_row_set_is_the_replica_now_without_deleted_or_closed_rows() {
    let (conn, st) = replica_with_close_dates();
    put_row(&conn, 1, Some(0), 1);
    put_row(&conn, 2, Some(0), 0);
    put_row(&conn, 4, Some(1_750_000_000), 0);
    // Opened after the replica was opened, as a trade during the session is.
    put_row(&conn, 3, None, 0);

    assert_eq!(
        apply_recheck_open_rows(&conn, &st, 3, None).unwrap(),
        vec![3, 2]
    );
    // Another core's rows never leak into this core's set.
    assert!(
        apply_recheck_open_rows(&conn, &st, 4, None)
            .unwrap()
            .is_empty()
    );
}

/// More open rows than one check carries are walked page by page, strictly downwards.
///
/// Breaks on: the page cap drifting from [`OPEN_ROWS_PAGE`], the step below a page being
/// inclusive — it would re-send its boundary row and, with one row per page, never finish — or an
/// empty `after` falling through to the newest page, which restarts the walk forever.
#[test]
fn open_rows_beyond_one_check_are_walked_in_pages() {
    let (conn, st) = replica_with_close_dates();
    for rec in 1..=250 {
        put_row(&conn, rec, Some(0), 0);
    }

    let first = apply_recheck_open_rows(&conn, &st, 3, None).unwrap();
    assert_eq!(first.len(), OPEN_ROWS_PAGE);
    assert_eq!((first[0], first[OPEN_ROWS_PAGE - 1]), (250, 151));
    let second = apply_recheck_open_rows(&conn, &st, 3, Some(&first)).unwrap();
    assert_eq!((second[0], second.len()), (150, OPEN_ROWS_PAGE));
    let last = apply_recheck_open_rows(&conn, &st, 3, Some(&second)).unwrap();
    assert_eq!((last[0], last.len()), (50, 50));
    assert!(
        apply_recheck_open_rows(&conn, &st, 3, Some(&last))
            .unwrap()
            .is_empty()
    );
    assert!(
        apply_recheck_open_rows(&conn, &st, 3, Some(&[]))
            .unwrap()
            .is_empty()
    );
}

/// A continuation of a walk that a newer walk replaced is dropped, not sent.
///
/// Breaks on: `db/rep.rs:OpenRowsWalk::admit` accepting a continuation without comparing it with
/// the last registered page. `check_open_rows` replaces the library's set, so a reconnect's
/// newest page followed by the old walk's next step would leave the newest page — the one that
/// holds a close lost in that reconnect — unchecked until the next reconnect (#742 again).
#[test]
fn a_stale_walk_step_cannot_replace_a_newer_walk() {
    let mut walk = OpenRowsWalk::default();
    let page_one = [300, 250, 200];
    let page_two = [150, 100];

    assert!(walk.admit(3, None, &page_one));
    // The completion names the page in the library's ascending order, not the writer's.
    assert!(walk.admit(3, Some(&[200, 250, 300]), &page_two));
    // A reconnect restarts the walk at the newest page...
    assert!(walk.admit(3, None, &page_one));
    // ...so the old walk's completion of page two, arriving after it, is stale.
    assert!(!walk.admit(3, Some(&page_two), &[50]));
    // The restarted walk still continues, and ends quietly on an empty page.
    assert!(walk.admit(3, Some(&page_one), &page_two));
    assert!(!walk.admit(3, Some(&page_two), &[]));
    assert!(!walk.admit(3, Some(&page_two), &[50]));

    // A new walk always goes out, even empty: that clears the library's retained set.
    assert!(walk.admit(3, None, &[]));
    // Walks are per core, and a wipe forgets the core's walk.
    assert!(walk.admit(4, None, &page_one));
    walk.forget(4);
    assert!(!walk.admit(4, Some(&page_one), &page_two));
}
