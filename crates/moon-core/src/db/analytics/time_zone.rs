//! SQLite scalar functions that project UTC report timestamps onto one display-time axis.

use chrono_tz::Tz;
use rusqlite::functions::FunctionFlags;
use rusqlite::Connection;

/// Install deterministic civil-time helpers for one Analytics query snapshot.
///
/// Args:
///     conn: SQLite connection that will prepare the analytical statements.
///     zone: User-selected display time zone captured for this read.
///
/// Returns:
///     Success after all helpers are registered.
///
/// Errors:
///     Returns SQLite's registration error if any scalar function cannot be installed.
pub(crate) fn install(conn: &Connection, zone: Tz) -> rusqlite::Result<()> {
    let flags = FunctionFlags::SQLITE_UTF8 | FunctionFlags::SQLITE_DETERMINISTIC;
    conn.create_scalar_function("mt_local_bucket", 2, flags, move |ctx| {
        let secs = ctx.get::<i64>(0)?;
        let bucket = ctx.get::<i64>(1)?;
        Ok(crate::util::display_time::bucket_start(secs, bucket, zone).unwrap_or(secs))
    })?;
    conn.create_scalar_function("mt_minute_of_day", 1, flags, move |ctx| {
        Ok(crate::util::display_time::minute_of_day(ctx.get::<i64>(0)?, zone).unwrap_or(0) as i64)
    })?;
    conn.create_scalar_function("mt_minute_of_week", 1, flags, move |ctx| {
        Ok(crate::util::display_time::minute_of_week(ctx.get::<i64>(0)?, zone).unwrap_or(0) as i64)
    })?;
    Ok(())
}
