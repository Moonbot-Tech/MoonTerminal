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

/// A SQLite failure keeps the replica path, the `ctx` operation and both result codes.
///
/// Formatting only `{error}` drops every field the user needs to tell a lock from I/O.
#[test]
fn sqlite_failure_carries_path_operation_and_codes() {
    let _state = super::super::integrity::test_state_guard();
    super::super::integrity::reset_test_state();
    let failure = read_fail(
        "reports(reader)",
        rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error {
                code: ErrorCode::DatabaseBusy,
                extended_code: 5,
            },
            Some("database is locked".into()),
        ),
    );
    let expected_path = crate::config::paths::reports_db_path()
        .to_string_lossy()
        .into_owned();
    assert_eq!(failure.kind(), Some(FailKind::Busy));
    assert_eq!(failure.operation(), Some("reports(reader)"));
    assert_eq!(failure.path(), Some(expected_path.as_str()));
    assert_eq!(
        failure.code(),
        Some(FailCode::Sqlite {
            primary: 5,
            extended: 5
        })
    );
    assert_eq!(failure.to_string(), "database is locked");
    super::super::integrity::reset_test_state();
}

/// Corruption arriving as `SQLITE_IOERR_READ` (extended 266) stays `Other` and still
/// exposes 10/266 — the raw pair is the only remaining clue once classify maps it away
/// from `Corrupt`.
#[test]
fn ioerr_extended_code_stays_on_the_failure() {
    let failure = read_fail(
        "reports: snapshot",
        rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error {
                code: ErrorCode::SystemIoFailure,
                extended_code: 266,
            },
            Some("disk I/O error".into()),
        ),
    );
    assert_eq!(failure.kind(), Some(FailKind::Other));
    assert_eq!(
        failure.code(),
        Some(FailCode::Sqlite {
            primary: 10,
            extended: 266
        })
    );
}

/// A filesystem refusal keeps the file it could not stat and the OS error number.
///
/// `std::fs::metadata` does not put the path on its own error, which is why Access Denied
/// used to render as `Access is denied. (os error 5)` with no file.
#[test]
fn filesystem_failure_carries_path_and_os_code() {
    let path = std::path::Path::new("C:/data/reports.sqlite");
    let failure = io_fail(
        "reports(reader): file access",
        path,
        &std::io::Error::from_raw_os_error(5),
    );
    assert_eq!(failure.kind(), Some(FailKind::Other));
    assert_eq!(failure.operation(), Some("reports(reader): file access"));
    assert_eq!(failure.path(), Some(path.to_string_lossy().as_ref()));
    assert_eq!(failure.code(), Some(FailCode::Os(5)));
    let line = warn_line(
        "reports(reader): file access",
        &failure,
        path.to_string_lossy().as_ref(),
        FailCode::Os(5),
        0,
    );
    assert!(
        line.contains("path=") && line.contains("code=os 5"),
        "log line must carry path and OS code, got {line}"
    );
    assert!(
        line.contains("reports(reader): file access"),
        "log line must keep the operation, got {line}"
    );
}

/// `read_fail/mod.rs::classify` must map `SQLITE_CANTOPEN` to `FailKind::Exhausted`.
///
/// Folding `ErrorCode::CannotOpen` into `_ => Other` makes Analytics treat CANTOPEN as
/// `Settled` again, so the descriptor-exhaustion retry from #667 never fires.
#[test]
fn cannot_open_is_exhausted() {
    let error = rusqlite::Error::SqliteFailure(
        rusqlite::ffi::Error {
            code: ErrorCode::CannotOpen,
            extended_code: 14,
        },
        None,
    );
    assert_eq!(classify(&error), FailKind::Exhausted);
    assert_eq!(
        read_fail("cantopen-classify", error).kind(),
        Some(FailKind::Exhausted)
    );
}

/// `read_fail/mod.rs::descriptor_suffix` must stay empty off Exhausted and mark a saturated
/// probe with `+`.
///
/// Dropping the `kind != Exhausted` early return probes every logged failure; collapsing the
/// saturated branch prints a capped count as exact.
#[test]
fn descriptor_suffix_only_marks_exhausted_and_saturation() {
    let exact = crate::metrics::DescriptorCount {
        count: 12,
        saturated: false,
    };
    let floor = crate::metrics::DescriptorCount {
        count: 65536,
        saturated: true,
    };
    assert_eq!(descriptor_suffix(FailKind::Busy, Some(exact)), "");
    assert_eq!(descriptor_suffix(FailKind::Other, Some(floor)), "");
    assert_eq!(
        descriptor_suffix(FailKind::Exhausted, Some(exact)),
        " descriptors=12"
    );
    assert_eq!(
        descriptor_suffix(FailKind::Exhausted, Some(floor)),
        " descriptors=65536+"
    );
    assert_eq!(
        descriptor_suffix(FailKind::Exhausted, None),
        " descriptors=unknown"
    );
}
