use super::*;

#[test]
/// Each failure keeps its own granularity, so the UI can give guidance that
/// is true of THAT failure rather than one blanket "retry".
fn failures_keep_their_kind_and_not_ready_has_none() {
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
}

/// Repeated instances of the same failure are suppressed within the window.
#[test]
fn repeated_failures_are_throttled() {
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
