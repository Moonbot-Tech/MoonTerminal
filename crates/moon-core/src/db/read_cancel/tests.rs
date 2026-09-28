use rusqlite::ErrorCode;

use super::*;

/// A superseded analytical scan must stop inside SQLite instead of running to completion.
///
/// Removing `read_cancel.rs:install_current` or making its callback always return false lets the
/// recursive workload complete successfully, so an obsolete filter request keeps occupying a DB
/// worker after the user has moved to a new scope.
#[test]
fn cancelled_scope_interrupts_sqlite_work() {
    let cancellation = ReadCancellation::new();
    let error = with_read_cancellation(cancellation.clone(), || {
        let connection = Connection::open_in_memory().expect("in-memory database");
        install_current(&connection).expect("install cancellation callback");
        cancellation.cancel();
        connection
            .query_row(
                "WITH RECURSIVE n(value) AS (
                     SELECT 1 UNION ALL SELECT value + 1 FROM n WHERE value < 1000000
                 )
                 SELECT SUM(value) FROM n",
                [],
                |row| row.get::<_, i64>(0),
            )
            .expect_err("cancelled query must be interrupted")
    });

    assert!(matches!(
        error,
        rusqlite::Error::SqliteFailure(ref failure, _)
            if failure.code == ErrorCode::OperationInterrupted
    ));
}
