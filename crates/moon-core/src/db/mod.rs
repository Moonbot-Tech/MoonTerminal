//! Local SQLite database for Orders reports.
//!
//! The PRIMARY path is a typed replica of the server database (moonproto
//! `Event::Report`, the `orders_rep` table; see [`rep`]): the core supplies the
//! schema, rows are upserted or deleted by `newRecID`, offline changes catch up
//! from a durable checkpoint, and the alive map that follows each `SyncComplete`
//! reconciles the visibility of older rows. Analytical fields
//! (deltas, pump, signaltype, and others) arrive with their real values.
//!
//! LEGACY table `closed_sell_reports` (PK `(core_uid, db_id)`) is READ-ONLY: the
//! terminal does not consume `Event::ClosedSellOrderReport`, so no new legacy
//! rows are written. The report window still UNION-s its existing rows while any
//! core lingers on it, and [`rep`]'s per-core purge drops the table once every
//! core is on typed replication (marker `legacy_dropped`).
//!
//! ONE writer thread performs writes; the Reports window reads through a separate
//! connection (WAL).

pub mod analytics;
pub mod coin_lists;
mod core_names;
mod dates;
pub mod integrity;
pub mod maint;
pub mod metrics;
pub mod order_traces;
mod quote;
mod read_cancel;
pub(crate) mod read_fail;
mod reader_budget;
mod rep;
pub mod report_axis;
mod report_read;
pub mod report_recovery;
mod sql_sum;
mod strategy_name_match;
pub mod tape_owners;
#[cfg(test)]
mod test_support;
pub mod trace;
mod trade_meta;
pub mod tuner;
pub mod valuation;
pub(crate) mod wal;

pub use core_names::CoreNames;
pub use dates::{fmt_unix, fmt_unix_date, fmt_unix_secs, parse_ymd};
pub use quote::{
    AverageOrderReturn, EntrySpend, OpenPositions, ProfitScope, ProfitUnit, QuoteBreakdown,
    QuoteCurrency, QuoteScope, QuoteSpend, QuoteTotal, QuoteVolume, TradedVolume, UsdtTotal,
    ValuationCoverage,
};
pub use read_cancel::{ReadCancellation, current_is_cancelled, with_read_cancellation};
pub use read_fail::{FailCode, FailKind, ReadFail, ReadResult};
pub use reader_budget::ReportReader;
pub(crate) use rep::core_offset::{latest_segment, latest_source as latest_offset_source};
pub use rep::{DbMsg, ReportSink};
pub(crate) use rep::{OPEN_ROWS_PAGE, ReportStart};
pub use report_axis::{MAX_OFFSET_SECS, MIN_OFFSET_SECS, OffsetSegment, ReportAxis, ReportStamp};
pub use report_read::{
    CHART_TRADE_HISTORY_ATTACH, COLUMNS_ADDED_SINCE_V2, ChartTradeHistory, ChartTradeRecord,
    DISPLAY_COLUMNS, MINI_ENTRY_VOLUME_RATE_COLUMN, NOTIFY_ENTRY_VOLUME_NATIVE_COLUMN,
    NOTIFY_PROFIT_NATIVE_COLUMN, NOTIFY_QUOTE_COLUMN, PROFIT_PERCENT_COLUMN, PeriodBasis,
    ProfitMetric, ReportFilter, ReportStrategy, ReportStrategyKey, ReportTable, ReportTotals,
    RowScope, SideFilter, StrategyPurgeRows, TotalsSlice, VALUATION_PROFIT_COLUMN,
    VALUATION_RATE_COLUMN, VALUATION_SOURCE_COLUMN, display_columns, distinct_cores,
    distinct_strategies, max_core_uid, open_rows_for_bound, query_chart_trade_history,
    query_chart_trade_history_for_cores, query_mini_trades, query_notify_trades, query_reports,
    query_totals, query_totals_sliced, report_coin_is_exact, rows_by_core, strategy_purge_rows,
};
pub(crate) use report_read::{count_by_core, max_core_uid_in};
pub use trade_meta::{TradeMeta, query_trade_meta};

use read_fail::read_fail;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::Receiver;
use std::time::Duration;

use moonproto::{MoonReports, ReportSyncCheckpoint, ReportSyncComplete};
use rusqlite::Connection;

use crate::config::paths;

/// Feed-to-writer sink for typed replication messages plus per-core replication start states.
pub type ReportTx = ReportSink;

/// Database handle containing the writer channel and post-commit publication state.
///
/// The generation is the source of truth; bounded dirty edges distinguish immediate live changes
/// from coalesced historical catch-up after successful commits.
pub struct ReportsHandle {
    /// Typed replication sink used by core feed sessions.
    pub tx: ReportTx,
    /// Monotonic committed report generation.
    pub generation: Arc<AtomicU64>,
    /// Coalescing edge for live report changes that must refresh UI immediately.
    pub immediate_commit_dirty: Arc<AtomicBool>,
    /// Coalescing edge for historical catch-up changes that may refresh UI in bulk.
    pub background_commit_dirty: Arc<AtomicBool>,
}

/// Initialize shared report metadata and any still-present read-only legacy table.
///
/// A fresh database creates no `closed_sell_reports` table. Existing compatible
/// tables receive only the indexes needed by report reads; the obsolete
/// `server_id` schema is dropped because no protocol-v4 writer can repopulate it.
fn init_db(conn: &Connection) -> rusqlite::Result<()> {
    wal::enable(conn)?;
    let _ = conn.busy_timeout(Duration::from_secs(3));

    conn.execute(
        "CREATE TABLE IF NOT EXISTS app_meta (key TEXT PRIMARY KEY, value TEXT NOT NULL)",
        [],
    )?;
    valuation::init_report_outbox(conn)?;

    // The legacy table was already dropped after the full typed-replica migration.
    // Do not recreate it: CREATE IF NOT EXISTS would bring the dead table back.
    let legacy_dropped: bool = conn
        .query_row(
            "SELECT value FROM app_meta WHERE key='legacy_dropped'",
            [],
            |r| r.get::<_, String>(0),
        )
        .map(|v| v == "1")
        .unwrap_or(false);
    if legacy_dropped {
        return Ok(());
    }

    // Very old runtime-`server_id` schema is incompatible with the reader (which
    // selects `core_uid`), so drop it — same as before. We no longer RECREATE the
    // table: legacy rows are not written any more, and the reader gates its query
    // on the table's existence.
    let has_old: bool = conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('closed_sell_reports') WHERE name='server_id'",
            [],
            |r| r.get::<_, i64>(0),
        )
        .map(|n| n > 0)
        .unwrap_or(false);
    if has_old {
        conn.execute("DROP TABLE IF EXISTS closed_sell_reports", [])?;
        log::warn!("отчёты: старая схема (server_id) — таблица снесена");
    }

    // Report-window indexes on the legacy table — only when it still exists (cores
    // on the transition period). A fresh install has no legacy table, so there is
    // nothing to index; `CREATE INDEX` on a missing table would error.
    let legacy_exists: bool = conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('closed_sell_reports')",
            [],
            |r| r.get::<_, i64>(0),
        )
        .map(|n| n > 0)
        .unwrap_or(false);
    if legacy_exists {
        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_csr_closedate ON closed_sell_reports(closedate)",
            [],
        )?;
        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_csr_core ON closed_sell_reports(core_uid)",
            [],
        )?;
        let strategy_close_columns: i64 = conn.query_row(
            "SELECT COUNT(*) FROM pragma_table_info('closed_sell_reports')
             WHERE name IN ('core_uid', 'strategyid', 'closedate')",
            [],
            |row| row.get(0),
        )?;
        if strategy_close_columns == 3 {
            conn.execute(
                "CREATE INDEX IF NOT EXISTS idx_csr_strategy_close
                 ON closed_sell_reports(core_uid, strategyid, closedate)",
                [],
            )?;
        }
    }
    Ok(())
}

/// Probe legacy-table columns (the reader maps legacy rows onto display columns).
///
/// Opening a database does not validate its schema b-tree. An absent table
/// therefore yields `Ok(empty)`, while any SQLite error remains `Err`.
fn table_columns_res(conn: &Connection) -> ReadResult<std::collections::HashSet<String>> {
    const CTX: &str = "отчёты: PRAGMA table_info(closed_sell_reports)";
    let mut out = std::collections::HashSet::new();
    let mut stmt = conn
        .prepare("PRAGMA table_info(closed_sell_reports)")
        .map_err(|e| read_fail(CTX, e))?;
    let rows = stmt
        .query_map([], |r| r.get::<_, String>(1))
        .map_err(|e| read_fail(CTX, e))?;
    for n in rows {
        out.insert(n.map_err(|e| read_fail(CTX, e))?);
    }
    Ok(out)
}

/// Count rows currently in the typed replica for the Storage settings tab.
///
/// Returns `NotReady` when the replica file is absent and `Failed` when opening
/// it or reading the count fails. Only a successful query may return zero.
pub fn report_row_count() -> ReadResult<i64> {
    const CTX: &str = "отчёты: число строк реплики";
    let conn = open_reader()?;
    conn.query_row(&format!("SELECT COUNT(*) FROM {}", rep::TABLE), [], |r| {
        r.get(0)
    })
    .map_err(|e| read_fail(CTX, e))
}

/// Report rows per core with each core's newest name, for the Storage tab.
pub fn report_rows_by_core() -> ReadResult<Vec<(u64, String, u64)>> {
    let conn = open_reader()?;
    rows_by_core(&conn)
}

mod report_writer;
#[cfg(test)]
use report_writer::*;
pub use report_writer::{read_state_revision, spawn_writer};

mod reader_open;
use reader_open::*;
pub use reader_open::{AttachSet, open_reader, open_reader_with, read_snapshot};

mod report_meta;
use report_meta::*;
pub(crate) use report_meta::{AXIS_GENERATION_KEY, bump_axis_generation};
pub use report_meta::{
    load_comment_pane, load_sort, load_visible, save_comment_pane, save_sort, save_visible,
};

/// Report read source: a table, its columns, and a legacy flag.
///
/// Query the typed replica and the legacy table, while it exists, SEPARATELY so
/// each gets its own WHERE, ORDER, and LIMIT clauses and can use its indexes, then
/// merge in Rust. SQLite does not flatten a UNION ALL subquery with NULL projection,
/// so its filter was not pushed into the branches and every refresh fully scanned
/// the hundreds-of-MB legacy database (measured at about 400 ms for "Today").
/// During typed catch-up, a core can temporarily have rows in both tables until
/// `SyncComplete` purges its legacy rows. Readers merge both sources as-is, so any
/// overlap can temporarily contribute both copies to results.
struct ReadSource {
    table: &'static str,
    cols: std::collections::HashSet<String>,
    legacy: bool,
}

/// Discover report sources while preserving failures from either schema probe.
///
/// An absent legacy table is omitted. Schema probe errors map to `Failed`; this
/// function receives an open connection and therefore cannot return `NotReady`.
fn read_sources_res(conn: &Connection) -> ReadResult<Vec<ReadSource>> {
    let mut out = vec![ReadSource {
        table: rep::TABLE,
        cols: rep::table_cols_res(conn)?,
        legacy: false,
    }];
    let legacy_cols = table_columns_res(conn)?;
    // An empty probe means the legacy table is absent. Omitting that source keeps
    // `query_reports` from generating a SELECT against a missing table.
    if !legacy_cols.is_empty() {
        out.push(ReadSource {
            table: "closed_sell_reports",
            cols: legacy_cols,
            legacy: true,
        });
    }
    // Every money reader passes through here, and this is the last point that still holds BOTH a
    // connection and the sources. The quote module's SQL builders are pure functions of the
    // columns they are handed, so the one fact they cannot derive — which cores own COIN-M rows —
    // is learned here instead of being threaded through every builder signature.
    quote::learn_coin_m_cores(conn, &out);
    Ok(out)
}

/// Open the replica strictly read-only, for a probe that must not own the file.
///
/// Not budgeted: this runs before the writer exists as a single pre-window probe.
///
/// [`open_reader`] opens read-WRITE despite its name, which is harmless for its own callers
/// because the writer connection is alive by then. A probe that runs BEFORE `spawn_writer` would
/// instead be the only connection, and closing the last read-write connection makes SQLite run
/// its final WAL checkpoint and delete `-wal`/`-shm` — a WAL left by a killed run reaches
/// hundreds of MB, so that would land as a synchronous copy on the pre-window startup path. A
/// read-only last connection does not perform that close-time checkpoint or cleanup.
pub fn open_readonly() -> ReadResult<Connection> {
    let path = paths::reports_db_path();
    // Same reasoning as `open_reader`: only a genuine absence may report `NotReady`.
    metadata_gate(&path, "отчёты(ro): доступ к файлу")?;
    let conn = Connection::open_with_flags(
        &path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|e| read_fail::read_fail_at("отчёты(ro)", &path, e))?;
    trace::install_on(&conn);
    Ok(conn)
}

#[cfg(test)]
mod tests;
