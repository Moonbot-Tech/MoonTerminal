use super::*;

/// `load_all` -- an ABSENT `core_time_offset` table is a fresh install with nothing measured yet,
/// not a failure. Distinguishing this from a genuinely unreadable table (below) is the whole
/// reason `load_all` returns a [`ReadResult`] instead of quietly defaulting to empty on any
/// problem.
#[test]
fn load_all_on_an_absent_table_is_ok_and_empty() {
    let conn = rusqlite::Connection::open_in_memory().expect("open absent-table fixture");

    let loaded = load_all(&conn).expect("an absent table must not be a read failure");
    assert!(
        loaded.is_empty(),
        "a fresh install with no measurements yet must load as empty, not fail"
    );
}

/// `load_all` -- a healthy table round-trips exactly what [`store_segment`] wrote, sorted
/// ascending by `from_utc` as [`crate::db::report_axis::ReportAxis::from_measured`] expects.
#[test]
fn load_all_round_trips_stored_segments_sorted_by_from_utc() {
    let conn = rusqlite::Connection::open_in_memory().expect("open healthy fixture");
    ensure_table(&conn).expect("create the offset table");
    // Stored out of order on purpose -- the load, not the write order, must produce the sort.
    store_segment(&conn, 9, 500, 3_600, 1_000, "log").expect("store the later segment");
    store_segment(&conn, 9, 100, -3_600, 900, "log").expect("store the earlier segment");

    let loaded = load_all(&conn).expect("load a healthy single-core table");
    assert_eq!(
        loaded.get(&9),
        Some(&vec![
            OffsetSegment {
                from_utc: 100,
                offset_secs: -3_600,
            },
            OffsetSegment {
                from_utc: 500,
                offset_secs: 3_600,
            },
        ]),
        "segments for one core must load ascending by from_utc regardless of write order"
    );
}

/// `db/rep/core_offset.rs:latest_offset` -- changing `ORDER BY from_utc DESC` to `ASC` would
/// compare later observations against the first-ever offset. A re-confirmation of the current
/// clock offset would then repeatedly rescan the core's USDT valuation cache after every restart.
#[test]
fn latest_offset_reads_the_newest_segment_even_when_inserted_first() {
    let conn = rusqlite::Connection::open_in_memory().expect("open newest-segment fixture");
    ensure_table(&conn).expect("create the offset table");
    // Insert newest first so SQLite row order cannot accidentally satisfy the time-order oracle.
    store_segment(&conn, 4, 9_000, 3_600, 9_000_000, "newest")
        .expect("store the newer offset first");
    store_segment(&conn, 4, 1_000, -3_600, 1_000_000, "oldest")
        .expect("store the older offset second");

    assert_eq!(
        latest_offset(&conn, 4),
        Some(3_600),
        "the active offset is selected by the greatest from_utc, not insertion order"
    );
}

/// `load_all` -- collapsing a self-inconsistent table's error into `Ok(empty)` instead of
/// [`ReadFail::Failed`] would make a skewed core silently read as "never measured", which is the
/// WRONG-MONEY axis: the identity conversion is applied to a core whose measured offset the
/// replica can no longer be trusted to state correctly.
///
/// This case: an `offset_secs` outside the plausible real-time-zone band
/// ([`crate::db::report_axis::MIN_OFFSET_SECS`]..=[`crate::db::report_axis::MAX_OFFSET_SECS`]),
/// written directly (bypassing [`store_segment`]'s caller-side discipline, which never validates
/// the range itself) to prove `load_all` is the seam that actually enforces it on read.
#[test]
fn load_all_fails_closed_on_an_offset_outside_the_plausible_band() {
    let conn = rusqlite::Connection::open_in_memory().expect("open out-of-band fixture");
    ensure_table(&conn).expect("create the offset table");
    store_segment(
        &conn,
        3,
        0,
        crate::db::report_axis::MAX_OFFSET_SECS + 1,
        0,
        "log",
    )
    .expect("store an out-of-band segment directly, bypassing caller-side validation");

    match load_all(&conn) {
        Err(ReadFail::Failed { kind, .. }) => {
            assert_eq!(
                kind,
                FailKind::Corrupt,
                "must classify as Corrupt, never NotReady"
            )
        }
        other => panic!(
            "an offset outside the plausible band must fail closed as ReadFail::Failed, got \
             {other:?}"
        ),
    }
}

/// `load_all` -- a duplicate or non-ascending `from_utc` for one core cannot arise from a healthy
/// `PRIMARY KEY (core_uid, from_utc)` table, so seeing one is itself a corruption signal. The
/// fixture table is created WITHOUT that primary key specifically to let two rows share one
/// `(core_uid, from_utc)` pair, reproducing what a damaged index page could otherwise serve.
#[test]
fn load_all_fails_closed_on_a_duplicate_from_utc_for_one_core() {
    let conn = rusqlite::Connection::open_in_memory().expect("open duplicate-key fixture");
    conn.execute_batch(
        "CREATE TABLE core_time_offset (
            core_uid    INTEGER NOT NULL,
            from_utc    INTEGER NOT NULL,
            offset_secs INTEGER NOT NULL,
            observed_at INTEGER NOT NULL,
            source      TEXT    NOT NULL
        )",
    )
    .expect("seed a core_time_offset table with no primary key");
    conn.execute(
        "INSERT INTO core_time_offset VALUES (11, 100, 3600, 0, 'log')",
        [],
    )
    .expect("insert the first row of the duplicate pair");
    conn.execute(
        "INSERT INTO core_time_offset VALUES (11, 100, -3600, 0, 'log')",
        [],
    )
    .expect("insert the duplicate (core_uid, from_utc) row");

    match load_all(&conn) {
        Err(ReadFail::Failed { kind, .. }) => {
            assert_eq!(
                kind,
                FailKind::Corrupt,
                "must classify as Corrupt, never NotReady"
            )
        }
        other => panic!(
            "a duplicate from_utc for one core must fail closed as ReadFail::Failed, got {other:?}"
        ),
    }
}

/// `load_all` -- a wrong `typeof` on a column NEITHER `core_uid`, `from_utc` NOR `offset_secs`
/// decode as (here `source`, which the row-decode path never reads as a typed value at all)
/// means the replica no longer matches its declared schema and must not be trusted, even though
/// nothing about the columns `load_all` actually decodes would itself raise a conversion error.
///
/// The fixture table declares `source` with NO column type, so SQLite gives it BLOB affinity and
/// stores an inserted INTEGER as-is instead of coercing it to TEXT the way the real
/// `TEXT`-affinity column in [`ensure_table`] would -- reproducing a row whose `source` column
/// silently stopped being text.
#[test]
fn load_all_fails_closed_on_a_typeof_mismatch_in_an_undecoded_column() {
    let conn = rusqlite::Connection::open_in_memory().expect("open typeof-mismatch fixture");
    conn.execute_batch(
        "CREATE TABLE core_time_offset (
            core_uid    INTEGER NOT NULL,
            from_utc    INTEGER NOT NULL,
            offset_secs INTEGER NOT NULL,
            observed_at INTEGER NOT NULL,
            source
        )",
    )
    .expect("seed a core_time_offset table whose source column has no declared affinity");
    conn.execute(
        "INSERT INTO core_time_offset VALUES (5, 0, 3600, 0, 42)",
        [],
    )
    .expect("insert a row whose source column holds an INTEGER, not TEXT");

    match load_all(&conn) {
        Err(ReadFail::Failed { kind, .. }) => {
            assert_eq!(
                kind,
                FailKind::Corrupt,
                "must classify as Corrupt, never NotReady"
            )
        }
        other => {
            panic!("a column typeof mismatch must fail closed as ReadFail::Failed, got {other:?}")
        }
    }
}
