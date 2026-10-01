//! The SQL face of [`crate::strategy_query`]: the `mt_strategy_name_match` scalar function.
//!
//! Its own module because two unrelated readers offer a strategy-NAME mask — the Report's paged
//! filter and the Analytics toolbar — and both must select exactly what the Strategies tree shows
//! for the same text. The function delegates to the shared Rust parser, so the grammar has one
//! implementation; placing it beside either consumer would make the other reach sideways into a
//! feature module for a primitive that belongs to neither.

use rusqlite::Connection;
use rusqlite::functions::FunctionFlags;
use rusqlite::types::ValueRef;

use crate::strategy_query::StrategyQuery;

/// Install `mt_strategy_name_match(name, query)`, the SQL face of [`StrategyQuery`].
///
/// SQLite's built-in `lower` and `LIKE` fold ASCII only, while strategy names are not restricted to
/// ASCII; delegating to the Rust parser keeps the SQL masks and the Strategies tree on one grammar.
/// The query argument is parsed once per statement and cached as SQLite auxiliary data.
///
/// Value-free on purpose: the Report and Analytics build different SQL around the same function,
/// so each caller decides when it is needed. Re-registering the same name and arity on one
/// connection replaces the previous definition, so installing it twice is harmless.
///
/// Args:
///     conn: Open report reader or snapshot receiving the deterministic scalar function.
///
/// Returns:
///     Success once the function is installed. In SQL it yields 1 on a match, 0 otherwise; a NULL
///     name never matches and a NULL query matches everything.
///
/// Errors:
///     Returns SQLite's registration error when the function cannot be installed.
pub(in crate::db) fn install_strategy_name_match(conn: &Connection) -> rusqlite::Result<()> {
    let flags = FunctionFlags::SQLITE_UTF8 | FunctionFlags::SQLITE_DETERMINISTIC;
    conn.create_scalar_function("mt_strategy_name_match", 2, flags, |ctx| {
        let name = match ctx.get_raw(0) {
            ValueRef::Null => return Ok(0i64),
            value => value
                .as_str()
                .map_err(|error| rusqlite::Error::UserFunctionError(Box::new(error)))?,
        };
        if ctx.get_raw(1).data_type() == rusqlite::types::Type::Null {
            return Ok(1i64);
        }
        let query = ctx.get_or_create_aux(1, |value| {
            value
                .as_str()
                .map(StrategyQuery::parse)
                .map_err(|error| Box::new(error) as Box<dyn std::error::Error + Send + Sync>)
        })?;
        Ok(i64::from(query.matches(name)))
    })
}
