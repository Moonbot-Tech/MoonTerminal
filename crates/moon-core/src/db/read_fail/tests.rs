use super::*;

/// Each failure keeps its own granularity and read corruption disables writes.
///
/// Removing `integrity::record_corruption` from `read_fail` leaves the final writer-block
/// assertion false, so an Analytics read could prove the replica malformed while the writer keeps
/// retrying and acknowledging later batches.
#[test]
fn failures_keep_their_kind_and_not_ready_has_none() {
    let _state = super::super::integrity::test_state_guard();
    super::super::integrity::reset_test_state();
    let corrupt = read_fail(
        "test",
        rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error {
                code: ErrorCode::DatabaseCorrupt,
                extended_code: 11,
            },
            None,
        ),
    );
    let busy = read_fail(
        "test",
        rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error {
                code: ErrorCode::DatabaseBusy,
                extended_code: 5,
            },
            None,
        ),
    );
    assert_eq!(corrupt.kind(), Some(FailKind::Corrupt));
    assert_eq!(busy.kind(), Some(FailKind::Busy));
    assert_eq!(ReadFail::NotReady.kind(), None);
    assert!(matches!(
        busy,
        ReadFail::Failed {
            kind: FailKind::Busy,
            ..
        }
    ));
    assert!(super::super::integrity::writes_blocked());
    super::super::integrity::reset_test_state();
}

/// Weakening `read_fail_on` to suppress every corruption-class error would let genuine or
/// inconclusive report-main damage continue accepting writes when no damaged valuation schema can
/// be proven.
#[test]
fn unproven_corruption_remains_fail_closed() {
    let _state = super::super::integrity::test_state_guard();
    super::super::integrity::reset_test_state();
    let conn = rusqlite::Connection::open_in_memory().expect("open report fixture");
    let failure = read_fail_on(
        &conn,
        "test: unproven corruption",
        rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error {
                code: ErrorCode::DatabaseCorrupt,
                extended_code: 11,
            },
            None,
        ),
    );

    assert_eq!(failure.kind(), Some(FailKind::Corrupt));
    assert!(super::super::integrity::writes_blocked());
    super::super::integrity::reset_test_state();
}

/// A requested `SQLITE_INTERRUPT` is supersession, not a database-health warning.
///
/// Removing `read_fail/mod.rs:cancelled_read` inserts this unique context into `WARN_SEEN` and
/// would make rapid Analytics scope changes look like recurring database faults in the log.
#[test]
fn requested_interrupt_is_classified_without_logging() {
    let cancellation = super::super::read_cancel::ReadCancellation::new();
    let failure = super::super::read_cancel::with_read_cancellation(cancellation.clone(), || {
        let connection = rusqlite::Connection::open_in_memory().expect("in-memory database");
        super::super::read_cancel::install_current(&connection)
            .expect("install cancellation callback");
        cancellation.cancel();
        let error = connection
            .query_row(
                "WITH RECURSIVE n(value) AS (
                     SELECT 1 UNION ALL SELECT value + 1 FROM n WHERE value < 1000000
                 )
                 SELECT SUM(value) FROM n",
                [],
                |row| row.get::<_, i64>(0),
            )
            .expect_err("cancelled query must be interrupted");
        read_fail("requested-interrupt-test-unique-ctx", error)
    });

    assert_eq!(failure.kind(), Some(FailKind::Other));
    let logged = WARN_SEEN.get().is_some_and(|seen| {
        seen.lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .contains_key(&("requested-interrupt-test-unique-ctx", FailKind::Other))
    });
    assert!(
        !logged,
        "requested interruption must stay out of fault logs"
    );
}

/// Nested cancellation scopes classify the token captured by the interrupted connection.
///
/// Consulting only the innermost TLS token makes an outer connection's requested interrupt look
/// like a database fault when that connection is queried inside a nested database operation.
#[test]
fn nested_scope_classifies_the_connection_that_actually_interrupted() {
    let outer = super::super::read_cancel::ReadCancellation::new();
    let inner = super::super::read_cancel::ReadCancellation::new();
    let failure = super::super::read_cancel::with_read_cancellation(outer.clone(), || {
        let connection = rusqlite::Connection::open_in_memory().expect("in-memory database");
        super::super::read_cancel::install_current(&connection)
            .expect("install outer cancellation callback");
        outer.cancel();
        super::super::read_cancel::with_read_cancellation(inner, || {
            let error = connection
                .query_row(
                    "WITH RECURSIVE n(value) AS (
                         SELECT 1 UNION ALL SELECT value + 1 FROM n WHERE value < 1000000
                     )
                     SELECT SUM(value) FROM n",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .expect_err("outer connection must be interrupted");
            read_fail("nested-interrupt-test-unique-ctx", error)
        })
    });

    assert_eq!(failure.kind(), Some(FailKind::Other));
    let logged = WARN_SEEN.get().is_some_and(|seen| {
        seen.lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .contains_key(&("nested-interrupt-test-unique-ctx", FailKind::Other))
    });
    assert!(
        !logged,
        "nested requested interruption must stay out of fault logs"
    );
}

/// Repeated instances of the same failure are suppressed within the window.
#[test]
fn repeated_failures_are_throttled() {
    // `read_fail` on a corruption code reaches `integrity::record_corruption`, which sets the
    // process-global `WRITES_BLOCKED` latch. Without this guard those 51 calls can flip it while an
    // unrelated test asserts the latch is clear.
    let _state = super::super::integrity::test_state_guard();
    super::super::integrity::reset_test_state();
    let make = || {
        rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error {
                code: ErrorCode::DatabaseCorrupt,
                extended_code: 11,
            },
            None,
        )
    };
    let ctx = "throttle-test-unique-ctx";
    let _ = read_fail(ctx, make());
    for _ in 0..50 {
        let _ = read_fail(ctx, make());
    }
    let map = WARN_SEEN.get().expect("таблица подавления создана");
    let seen = map.lock().unwrap_or_else(|p| p.into_inner());
    let (_, suppressed) = seen
        .get(&(ctx, FailKind::Corrupt))
        .expect("ключ (ctx, kind) зарегистрирован");
    assert_eq!(*suppressed, 50, "все повторы в окне должны быть подавлены");
}
