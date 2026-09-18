use super::*;

/// A strategies database with the two tables the placement queries read, in memory.
fn db() -> Connection {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch(
        "CREATE TABLE strategies (
            core_uid INTEGER NOT NULL, strategy_id INTEGER NOT NULL,
            name TEXT NOT NULL DEFAULT '', kind TEXT NOT NULL DEFAULT '',
            kind_ordinal INTEGER NOT NULL DEFAULT 0, folder_path TEXT NOT NULL DEFAULT '',
            is_short INTEGER NOT NULL DEFAULT 0, deleted INTEGER NOT NULL DEFAULT 0,
            PRIMARY KEY (core_uid, strategy_id));
         CREATE TABLE strategy_versions (
            id INTEGER PRIMARY KEY, core_uid INTEGER NOT NULL, strategy_id INTEGER NOT NULL,
            valid_from INTEGER NOT NULL, valid_to INTEGER, change_kind TEXT NOT NULL DEFAULT '',
            raw_json TEXT NOT NULL DEFAULT '{}');",
    )
    .unwrap();
    conn
}

fn version(conn: &Connection, from: i64, to: Option<i64>) {
    conn.execute(
        "INSERT INTO strategy_versions (core_uid, strategy_id, valid_from, valid_to)
         VALUES (7, 42, ?1, ?2)",
        rusqlite::params![from, to],
    )
    .unwrap();
}

#[test]
fn instant_inside_a_closed_window_names_that_version() {
    let conn = db();
    version(&conn, 1_000, Some(2_000));
    version(&conn, 2_000, Some(3_000));
    version(&conn, 3_000, None);
    assert_eq!(
        version_at_on(&conn, 7, 42, 2_500),
        VersionAt::Known {
            valid_from: 2_000,
            current: false
        }
    );
}

/// The boundary belongs to the version that STARTS there, as the stats range join reads it
/// (`buydate >= from AND buydate < to`).
#[test]
fn instant_on_a_boundary_belongs_to_the_newer_version() {
    let conn = db();
    version(&conn, 1_000, Some(2_000));
    version(&conn, 2_000, None);
    assert_eq!(
        version_at_on(&conn, 7, 42, 2_000),
        VersionAt::Known {
            valid_from: 2_000,
            current: true
        }
    );
}

#[test]
fn instant_after_the_last_open_version_is_current() {
    let conn = db();
    version(&conn, 1_000, Some(2_000));
    version(&conn, 2_000, None);
    assert_eq!(
        version_at_on(&conn, 7, 42, 9_999),
        VersionAt::Known {
            valid_from: 2_000,
            current: true
        }
    );
}

/// A deleted strategy closes its last version; a trade that entered before the deletion still
/// belongs to that version, not to "unknown".
#[test]
fn instant_after_a_closed_final_version_still_names_it() {
    let conn = db();
    version(&conn, 1_000, Some(2_000));
    assert_eq!(
        version_at_on(&conn, 7, 42, 5_000),
        VersionAt::Known {
            valid_from: 1_000,
            current: false
        }
    );
}

#[test]
fn instant_before_the_first_snapshot_is_before_history() {
    let conn = db();
    version(&conn, 1_000, None);
    assert_eq!(version_at_on(&conn, 7, 42, 999), VersionAt::BeforeHistory);
}

#[test]
fn a_strategy_without_versions_has_no_history() {
    let conn = db();
    version(&conn, 1_000, None); // Another strategy's version must not leak in.
    assert_eq!(version_at_on(&conn, 7, 43, 5_000), VersionAt::NoHistory);
    assert_eq!(version_at_on(&conn, 8, 42, 5_000), VersionAt::NoHistory);
}

#[test]
fn head_status_reports_the_deleted_flag() {
    let conn = db();
    conn.execute(
        "INSERT INTO strategies (core_uid, strategy_id, name, kind, deleted)
         VALUES (7, 42, 'Hook', 'Hook', 1), (7, 43, 'Live', 'Drop', 0)",
        [],
    )
    .unwrap();
    let gone = head_status_on(&conn, 7, 42).unwrap();
    assert!(gone.deleted);
    assert_eq!(gone.head.name, "Hook");
    let live = head_status_on(&conn, 7, 43).unwrap();
    assert!(!live.deleted);
    assert_eq!(head_status_on(&conn, 7, 44), None);
}
