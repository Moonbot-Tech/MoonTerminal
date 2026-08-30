//! Caseless matching for strategy-NAME masks, shared by every reader that offers one.
//!
//! Its own module because two unrelated features match the same way — the Report's paged filter
//! and the Analytics toolbar mask — and the rule has to be ONE rule: a mask typed in one window
//! and the same mask typed in the other must select the same strategies. Placing it beside its
//! first consumer would make the second reach sideways into a feature module for a primitive that
//! belongs to neither.

use rusqlite::Connection;
use rusqlite::functions::FunctionFlags;
use unicode_casefold::UnicodeCaseFold;

/// Fold a strategy name for locale-independent Unicode caseless matching.
///
/// Full folding may expand one character into several, such as `ß` into `ss`, which ordinary
/// lowercasing does not do.
///
/// Args:
///     value: Strategy name or user mask to normalize.
///
/// Returns:
///     Full non-Turkic Unicode case-folded text.
pub(in crate::db) fn strategy_name_casefold(value: &str) -> String {
    value.case_fold().collect()
}

/// Install the Unicode case folding a strategy-name mask matches through.
///
/// SQLite's built-in `lower` handles ASCII only, while strategy names and the mask fields are not
/// restricted to ASCII. Full case folding also covers multi-character caseless equivalents.
///
/// Value-free on purpose: the Report and Analytics build different SQL around the same fold, so
/// the FUNCTION is shared while each caller decides when it is needed. Re-registering the same name
/// and arity on one connection replaces the previous definition, so a reader that installs it twice
/// — a Report read and an Analytics read on one connection — is harmless.
///
/// Args:
///     conn: Open report reader or snapshot receiving the deterministic scalar function.
///
/// Returns:
///     Success once the function is installed.
///
/// Errors:
///     Returns SQLite's registration error when the function cannot be installed.
pub(in crate::db) fn install_unicode_casefold(conn: &Connection) -> rusqlite::Result<()> {
    let flags = FunctionFlags::SQLITE_UTF8 | FunctionFlags::SQLITE_DETERMINISTIC;
    conn.create_scalar_function("mt_unicode_casefold", 1, flags, |ctx| {
        Ok(strategy_name_casefold(&ctx.get::<String>(0)?))
    })
}
