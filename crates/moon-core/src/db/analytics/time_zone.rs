//! SQLite scalar functions that project a replicated report timestamp onto one display-time axis.
//!
//! Every one of these takes the row's `core_uid` as its last argument, because the stored value is
//! the CORE's own wall clock rather than UTC: it must reach true UTC through
//! [`crate::db::ReportAxis`] before any civil-time question can be asked of it. See that module
//! for why the correction happens on read and what it must never be applied to.
//!
//! These helpers belong in `SELECT` and `GROUP BY` position ONLY. Wrapping the column in a `WHERE`
//! clause with one of them makes the replica's `closedate` indexes unusable and turns the report's
//! period filter into a full table scan; a window bound is converted on the BOUND side instead,
//! per core group, through [`crate::db::ReportAxis::from_utc`] and
//! [`crate::db::ReportAxis::groups`].

use rusqlite::functions::FunctionFlags;
use rusqlite::Connection;

use crate::db::ReportAxis;

/// Install deterministic civil-time helpers for one Analytics query snapshot.
///
/// Args:
///     conn: SQLite connection that will prepare the analytical statements.
///     axis: Per-core time axis captured for this read, carrying both the core-clock correction
///         and the user-selected display zone.
///
/// Returns:
///     Success after all helpers are registered.
///
/// Errors:
///     Returns SQLite's registration error if any scalar function cannot be installed.
pub(crate) fn install(conn: &Connection, axis: &ReportAxis) -> rusqlite::Result<()> {
    let flags = FunctionFlags::SQLITE_UTF8 | FunctionFlags::SQLITE_DETERMINISTIC;
    let zone = axis.zone();

    let bucket_axis = axis.clone();
    conn.create_scalar_function("mt_local_bucket", 3, flags, move |ctx| {
        let secs = ctx.get::<i64>(0)?;
        let bucket = ctx.get::<i64>(1)?;
        let core_uid = core_of(ctx, 2)?;
        let utc = bucket_axis.to_utc(secs, core_uid);
        Ok(crate::util::display_time::bucket_start(utc, bucket, zone).unwrap_or(utc))
    })?;

    let day_axis = axis.clone();
    conn.create_scalar_function("mt_minute_of_day", 2, flags, move |ctx| {
        let secs = ctx.get::<i64>(0)?;
        let core_uid = core_of(ctx, 1)?;
        let utc = day_axis.to_utc(secs, core_uid);
        Ok(crate::util::display_time::minute_of_day(utc, zone).unwrap_or(0) as i64)
    })?;

    let week_axis = axis.clone();
    conn.create_scalar_function("mt_minute_of_week", 2, flags, move |ctx| {
        let secs = ctx.get::<i64>(0)?;
        let core_uid = core_of(ctx, 1)?;
        let utc = week_axis.to_utc(secs, core_uid);
        Ok(crate::util::display_time::minute_of_week(utc, zone).unwrap_or(0) as i64)
    })?;

    Ok(())
}

/// Read the row's owning core from one scalar-function argument.
///
/// The unified report source always PROJECTS a `core_uid` column but a legacy physical source has
/// none, in which case the projection emits `NULL AS "core_uid"`. A NULL therefore means "this row
/// has no identifiable core", which is exactly the case that must convert as the identity — the
/// same degradation an unmeasured core gets. Reading it as a plain `i64` would instead fail the
/// whole statement, and a failed read must never reach a caller as an empty result.
///
/// Args:
///     ctx: Scalar-function invocation context.
///     index: Zero-based argument position holding the core uid.
///
/// Every other reader of this column in `db` guards it with `typeof(...) = 'integer'` before
/// trusting it, because the replica stores whatever storage class the core sent regardless of the
/// declared affinity. That guard is a SQL predicate, and putting one here would mean editing eight
/// query strings for a value that is only ever read in `SELECT` position. Reading the raw value
/// instead gives the same protection with none of that reach: a non-integer is treated exactly
/// like a NULL rather than raising, so one odd row can never fail a statement covering thousands.
///
/// Returns:
///     The row's core uid, or `0` when the source cannot name one — including when the column
///     holds NULL, or any storage class an integer uid cannot come from.
fn core_of(ctx: &rusqlite::functions::Context<'_>, index: usize) -> rusqlite::Result<u64> {
    Ok(match ctx.get_raw(index) {
        rusqlite::types::ValueRef::Integer(uid) => uid as u64,
        _ => 0,
    })
}
