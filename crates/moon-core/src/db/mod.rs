//! Local SQLite database for Orders reports.
//!
//! The PRIMARY path is a typed replica of the server database (moonproto
//! `Event::Report`, the `orders_rep` table; see [`rep`]): the core supplies the
//! schema, rows are upserted or deleted by `newRecID`, offline changes catch up
//! from a cursor, and `SyncComplete` reconciles the replica. Analytical fields
//! (deltas, pump, signaltype, and others) arrive with their real values.
//!
//! LEGACY table `closed_sell_reports` (PK `(core_uid, db_id)`) is READ-ONLY: the
//! terminal no longer consumes `Event::ClosedSellOrderReport`, so no new legacy
//! rows are written. The report window still UNION-s its existing rows while any
//! core lingers on it, and [`rep`]'s per-core purge drops the table once every
//! core is on typed replication (marker `legacy_dropped`).
//!
//! ONE writer thread performs writes; the Reports window reads through a separate
//! connection (WAL).

pub mod analytics;
pub mod coin_lists;
mod dates;
pub mod integrity;
pub mod maint;
pub mod metrics;
pub(crate) mod read_fail;
mod rep;
mod report_read;
#[cfg(test)]
mod test_support;
pub mod tuner;
pub mod tuner_smart;

pub use dates::{fmt_unix, fmt_unix_date, fmt_unix_secs, parse_ymd};
pub use read_fail::{FailKind, ReadFail, ReadResult};
pub use rep::{DbMsg, ReportSink};
pub use report_read::{
    display_columns, distinct_cores, max_core_uid, query_reports, query_totals, DISPLAY_COLUMNS,
    ProfitMetric, ReportFilter, ReportTable, SideFilter,
};
pub(crate) use report_read::max_core_uid_in;

use read_fail::read_fail;

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::Receiver;
use std::sync::Arc;
use std::time::Duration;

use rusqlite::Connection;

use crate::config::paths;

/// Feed-to-writer sink for typed replication messages plus per-core start cursors.
pub type ReportTx = ReportSink;

/// Database handle containing the write channel and a generation counter.
///
/// The counter advances after writes so the Reports window can refresh without polling.
pub struct ReportsHandle {
    pub tx: ReportTx,
    pub generation: Arc<AtomicU64>,
}

/// Initialize shared report metadata and any still-present read-only legacy table.
///
/// A fresh database creates no `closed_sell_reports` table. Existing compatible
/// tables receive only the indexes needed by report reads; the obsolete
/// `server_id` schema is dropped because no protocol-v4 writer can repopulate it.
fn init_db(conn: &Connection) -> rusqlite::Result<()> {
    conn.pragma_update(None, "journal_mode", "WAL")?;
    let _ = conn.busy_timeout(Duration::from_secs(3));

    conn.execute(
        "CREATE TABLE IF NOT EXISTS app_meta (key TEXT PRIMARY KEY, value TEXT NOT NULL)",
        [],
    )?;

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

/// Writer-channel capacity: backpressure instead of OOM.
///
/// About 16k messages consume tens of MB at peak. When the channel is full, the
/// core feed thread waits for the writer, naturally throttling catch-up.
const REPORT_QUEUE_CAP: usize = 16_384;

pub fn spawn_writer() -> Option<ReportsHandle> {
    let (tx, rx): (std::sync::mpsc::SyncSender<DbMsg>, Receiver<DbMsg>) =
        std::sync::mpsc::sync_channel(REPORT_QUEUE_CAP);
    let path = paths::reports_db_path();
    let conn = match Connection::open(&path) {
        Ok(c) => c,
        Err(e) => {
            log::error!("отчёты: не удалось открыть {}: {e}", path.display());
            return None;
        }
    };
    if let Err(e) = init_db(&conn) {
        log::error!("отчёты: init схемы не удался: {e}");
        return None;
    }
    let cursors = Arc::new(std::sync::Mutex::new(std::collections::HashMap::new()));
    let open_rows = Arc::new(std::sync::Mutex::new(std::collections::HashMap::new()));
    let mut rep_state = match rep::init(&conn, cursors.clone(), open_rows.clone()) {
        Ok(st) => st,
        Err(e) => {
            log::error!("отчёты: init typed-реплики не удался: {e}");
            return None;
        }
    };
    let generation = Arc::new(AtomicU64::new(0));
    let gen_writer = generation.clone();
    if let Err(e) = std::thread::Builder::new()
        .name("reports-db".into())
        .spawn(move || {
            log::info!("отчёты: writer запущен ({})", path.display());
            // WAL path used by the lazy checkpoint based on the ACTUAL file size below.
            let wal_path = {
                let mut s = path.clone().into_os_string();
                s.push("-wal");
                std::path::PathBuf::from(s)
            };
            // Batch messages into one transaction: catch-up emits thousands of rows,
            // and per-row autocommit (fsync) would stretch catch-up into minutes.
            let mut thr_count: u64 = 0;
            let mut thr_started = std::time::Instant::now();
            // Backdate the value by 30 seconds so the first WAL size check becomes eligible
            // roughly 30 seconds after startup, on the next completed batch, rather than 60.
            let mut last_ckpt = std::time::Instant::now()
                .checked_sub(Duration::from_secs(30))
                .unwrap_or_else(std::time::Instant::now);
            loop {
                let first = match rx.recv() {
                    Ok(m) => m,
                    Err(_) => break,
                };
                let mut batch = vec![first];
                while batch.len() < 512 {
                    match rx.try_recv() {
                        Ok(m) => batch.push(m),
                        Err(_) => break,
                    }
                }
                thr_count += batch.len() as u64;
                let txn = match conn.unchecked_transaction() {
                    Ok(t) => t,
                    Err(e) => {
                        log::error!("отчёты: transaction не открылась: {e}");
                        continue;
                    }
                };
                // Acknowledge pages (`page_applied`) AFTER commit. Under the flow-control
                // contract, the library requests no next page until this one reaches disk.
                let mut acks = Vec::new();
                for msg in batch {
                    apply_msg(&txn, &mut rep_state, msg, &mut acks);
                }
                match txn.commit() {
                    Ok(()) => {
                        for (ack, page) in acks {
                            if let Err(e) = ack.page_applied(&page) {
                                log::warn!("отчёты(rep): page_applied не ушёл: {e:?}");
                            }
                        }
                    }
                    Err(e) => log::error!("отчёты: commit батча упал: {e}"),
                }
                // If this batch dropped the legacy table, run a one-off VACUUM outside the
                // transaction to reclaim its hundreds of MB. Only the writer is blocked.
                if rep_state.vacuum_pending {
                    rep_state.vacuum_pending = false;
                    let t = std::time::Instant::now();
                    match conn.execute("VACUUM", []) {
                        Ok(_) => log::info!(
                            "отчёты: VACUUM после сноса легаси — {}с",
                            t.elapsed().as_secs()
                        ),
                        Err(e) => log::warn!("отчёты: VACUUM не удался: {e}"),
                    }
                }
                gen_writer.fetch_add(1, Ordering::Relaxed);
                // LAZY WAL checkpoint. SQLite's PASSIVE auto-checkpoint normally keeps `-wal`
                // small on its own; forcing TRUNCATE every time would cause an unnecessary CPU
                // spike while the writer thread copies WAL into the main file. Call TRUNCATE
                // ONLY when `-wal` has actually grown because auto-checkpointing cannot keep up,
                // for example when a Reports reader prevents reset. This avoids normal-case
                // spikes without allowing disk usage to grow by hundreds of MB. Check the size
                // every 60 seconds; stat is cheap.
                if last_ckpt.elapsed() >= Duration::from_secs(60) {
                    last_ckpt = std::time::Instant::now();
                    let wal_big = std::fs::metadata(&wal_path)
                        .map(|m| m.len() > 32 * 1024 * 1024)
                        .unwrap_or(false);
                    if wal_big {
                        if let Err(e) =
                            conn.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(()))
                        {
                            log::debug!("отчёты: wal_checkpoint не удался: {e}");
                        }
                    }
                }
                // Throughput diagnostics for a dense catch-up stream show whether the writer
                // keeps pace with input. The bounded channel prevents OOM either way.
                let el = thr_started.elapsed();
                if el >= Duration::from_secs(10) {
                    if thr_count > 2_000 {
                        log::info!(
                            "отчёты: writer {} сообщений за {:.0}с (~{}/с)",
                            thr_count,
                            el.as_secs_f32(),
                            (thr_count as f32 / el.as_secs_f32()) as u64,
                        );
                    }
                    thr_count = 0;
                    thr_started = std::time::Instant::now();
                }
            }
            log::info!("отчёты: writer завершён");
        })
    {
        log::error!("отчёты: не удалось запустить writer thread: {e}");
        return None;
    }
    Some(ReportsHandle {
        tx: ReportSink {
            tx,
            cursors,
            open_rows,
        },
        generation,
    })
}

/// Apply one typed-replica writer message.
///
/// Successful pages append their acknowledgements to `acks`; the caller sends
/// `page_applied` only after the surrounding transaction commits.
fn apply_msg(
    conn: &Connection,
    rep_state: &mut rep::RepState,
    msg: DbMsg,
    acks: &mut Vec<(
        moonproto::MoonReports,
        std::sync::Arc<moonproto::ReportSyncPage>,
    )>,
) {
    match msg {
        DbMsg::Schema { core_uid, schema } => rep::apply_schema(conn, rep_state, core_uid, schema),
        DbMsg::Upsert {
            core_uid,
            core_name,
            row,
        } => {
            if let Err(e) = rep::apply_upsert(conn, rep_state, core_uid, &core_name, &row) {
                log::error!(
                    "отчёты(rep): upsert rec_id={} ядра {core_uid} упал: {e}",
                    row.rec_id
                );
            }
        }
        DbMsg::Delete { core_uid, rec_id } => {
            if let Err(e) = rep::apply_delete(conn, core_uid, rec_id) {
                log::error!("отчёты(rep): delete rec_id={rec_id} ядра {core_uid} упал: {e}");
            }
        }
        DbMsg::SetDeleted { core_uid, change } => {
            if let Err(e) = rep::apply_set_deleted(conn, rep_state, core_uid, &change) {
                log::error!("отчёты(rep): set-deleted ядра {core_uid} упал: {e}");
            }
        }
        DbMsg::Page {
            core_uid,
            core_name,
            page,
            ack,
        } => {
            if rep::apply_page(conn, rep_state, core_uid, &core_name, &page) {
                acks.push((ack, page));
            }
        }
        DbMsg::SyncComplete { core_uid, done } => {
            rep::apply_sync_complete(conn, rep_state, core_uid, &done);
        }
    }
}

// ============================================================================
//  Reads for the Reports window
// ============================================================================

/// Map a database path to a read outcome BEFORE opening it: a genuinely absent file becomes
/// `NotReady`, while any other metadata error becomes `Failed`.
///
/// NOT `Path::exists`: it reports false for a permission or metadata error too, which would
/// take the silent `NotReady` branch and tell the user the replica is merely not synced yet —
/// only a genuine absence may say that. The check still has to happen because
/// `Connection::open` would otherwise CREATE an empty database in place of the missing one.
/// `ctx` names the caller for the failure log. Shared by every path that opens a replica or
/// the strategy database read-only (the callers are this module and its descendants, so a
/// plain-private item reaches all of them without widening visibility to the crate root).
fn metadata_gate(path: &std::path::Path, ctx: &'static str) -> ReadResult<()> {
    match std::fs::metadata(path) {
        Ok(_) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Err(ReadFail::NotReady),
        Err(e) => Err(read_fail::io_fail(ctx, &e)),
    }
}

/// Open a reader while distinguishing an absent replica from a SQLite failure.
///
/// A genuinely absent file maps to `NotReady`; metadata and SQLite open errors
/// map to `Failed` so callers cannot present them as an empty period.
pub fn open_reader() -> ReadResult<Connection> {
    let path = paths::reports_db_path();
    metadata_gate(&path, "отчёты(reader): доступ к файлу")?;
    let conn = Connection::open(&path).map_err(|e| read_fail("отчёты(reader)", e))?;
    let _ = conn.busy_timeout(Duration::from_secs(3));
    // The strategy database rides along on EVERY reader.
    //
    // It used to be attached by `analytics::summary` alone, while `unified_from` — which is
    // what actually needs it — is reached from fourteen places (the whole tuner, the calendar,
    // the coin groups). Anything depending on it therefore worked on the summary and silently
    // did not elsewhere: the strategy list and that same strategy's KPI would have disagreed
    // about which trades belong to it. Attaching where the connection is born is the only
    // place that covers every reader, and it must happen BEFORE any transaction is opened —
    // ATTACH cannot run inside one.
    analytics::attach_strategies(&conn);
    Ok(conn)
}

/// Pin one WAL snapshot for a multi-statement read.
///
/// Separate autocommit statements each observe a newer snapshot, so a panel
/// could publish a row list and totals that disagree about which trades fall
/// inside the period. Read-only: dropping the transaction rolls back nothing.
/// ATTACH cannot run inside a transaction, so callers attach first. Transaction
/// creation errors map to `Failed`; this function cannot produce `NotReady`.
pub fn read_snapshot(conn: &Connection) -> ReadResult<rusqlite::Transaction<'_>> {
    conn.unchecked_transaction()
        .map_err(|e| read_fail("отчёты: снимок для чтения", e))
}

pub fn load_sort(conn: &Connection) -> Option<(String, bool)> {
    let key: String = conn
        .query_row("SELECT value FROM app_meta WHERE key='sort_key'", [], |r| {
            r.get(0)
        })
        .ok()?;
    let desc: String = conn
        .query_row(
            "SELECT value FROM app_meta WHERE key='sort_desc'",
            [],
            |r| r.get(0),
        )
        .unwrap_or_else(|_| "1".into());
    Some((key, desc != "0"))
}

/// Upsert one `app_meta` key/value pair. The single place the shared settings table is written.
/// Plain-private: every caller is this module or a descendant (`rep`).
fn meta_set(conn: &Connection, key: &str, value: &str) {
    let _ = conn.execute(
        "INSERT INTO app_meta(key,value) VALUES(?1,?2)
         ON CONFLICT(key) DO UPDATE SET value=excluded.value",
        rusqlite::params![key, value],
    );
}

pub fn save_sort(conn: &Connection, key: &str, desc: bool) {
    meta_set(conn, "sort_key", key);
    meta_set(conn, "sort_desc", if desc { "1" } else { "0" });
}

/// Load the saved comma-separated set of visible report columns, or `None` if never saved.
pub fn load_visible(conn: &Connection) -> Option<Vec<String>> {
    let csv: String = conn
        .query_row(
            "SELECT value FROM app_meta WHERE key='report_visible'",
            [],
            |r| r.get(0),
        )
        .ok()?;
    Some(
        csv.split(',')
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .collect(),
    )
}

/// Save the visible report-column names as a comma-separated set.
pub fn save_visible(conn: &Connection, cols: &[&str]) {
    meta_set(conn, "report_visible", &cols.join(","));
}

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
    Ok(out)
}

/// Open the replica strictly read-only, for a probe that must not own the file.
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
    Connection::open_with_flags(
        &path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|e| read_fail("отчёты(ro)", e))
}

#[cfg(test)]
mod tests;
