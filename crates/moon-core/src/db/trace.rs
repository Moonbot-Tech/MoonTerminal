//! Opt-in statement profiler shared by every read connection this crate opens.
//!
//! This is a measurement instrument, not a feature: with no hook installed it costs one
//! `OnceLock::get` per connection birth and changes no query, no schema, and no result. It must
//! never gate behaviour — a caller that wants to branch on "is profiling on" is using this
//! module wrong.
//!
//! `trace_v2` accepts only a plain `fn` pointer, not a closure, so the hook itself carries no
//! state; a caller that needs to accumulate results supplies a `fn` that writes into its own
//! static or thread-local storage (see `crates/moon-core/examples/db_read_timing.rs` for the
//! harness that does exactly this).
//!
//! `trace_v2` is CONNECTION-LOCAL: this crate opens report/strategy/valuation/kline read
//! connections from ten call sites, and each one must call [`install_on`] itself. There is no
//! single choke point that sees every connection.

use std::sync::OnceLock;
use std::time::Duration;

use rusqlite::Connection;
use rusqlite::trace::{TraceEvent, TraceEventCodes};

/// One profiled SQLite statement, captured from a `trace_v2` PROFILE event.
#[derive(Clone, Debug)]
pub struct ProfiledStatement {
    /// The statement text. Expanded (bound values substituted) when [`Self::expanded`] is
    /// true; otherwise the raw text with `?N` placeholders.
    pub sql: String,
    /// Whether [`Self::sql`] is expanded. A placeholder-only capture cannot be replayed with
    /// `EXPLAIN QUERY PLAN` against real literals and must never be ranked as if it could.
    pub expanded: bool,
    /// Wall time SQLite itself reports for running this statement.
    pub duration: Duration,
}

/// Process-wide profiler hook. `None` until [`install_read_profiler`] sets it.
static HOOK: OnceLock<fn(ProfiledStatement)> = OnceLock::new();

/// Install the process-wide read profiler.
///
/// There can be only one; a later call never replaces an earlier one; see the memory ordering
/// of [`OnceLock::set`].
///
/// Args:
///     hook: Called once per profiled statement, on the thread that ran it.
///
/// Returns:
///     Whether this call actually installed the hook (`false` when one was already set).
pub fn install_read_profiler(hook: fn(ProfiledStatement)) -> bool {
    HOOK.set(hook).is_ok()
}

/// Attach the profiler to one freshly opened connection, if a hook is installed.
///
/// No-op when [`install_read_profiler`] was never called: one `OnceLock::get`, no query
/// touched, no behaviour changed. Call this from every connection-owning call site in this
/// crate — there is no single place that sees every connection (see the module docs).
///
/// Args:
///     conn: Freshly opened connection, before it is handed to any caller.
pub(crate) fn install_on(conn: &Connection) {
    if HOOK.get().is_some() {
        conn.trace_v2(
            TraceEventCodes::SQLITE_TRACE_PROFILE,
            Some(on_profile_event),
        );
    }
}

/// `trace_v2` callback: build a [`ProfiledStatement`] from a PROFILE event and forward it to
/// the installed hook. A plain `fn`, per `trace_v2`'s signature — it carries no state of its
/// own and reads the current hook from [`HOOK`] on every call.
fn on_profile_event(event: TraceEvent<'_>) {
    let TraceEvent::Profile(stmt, duration) = event else {
        return;
    };
    let Some(hook) = HOOK.get() else {
        return;
    };
    let (sql, expanded) = match stmt.expanded_sql() {
        Some(sql) => (sql, true),
        None => (stmt.sql().into_owned(), false),
    };
    hook(ProfiledStatement {
        sql,
        expanded,
        duration,
    });
}
