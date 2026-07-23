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
pub mod integrity;
pub mod maint;
pub(crate) mod read_fail;
mod rep;
#[cfg(test)]
mod test_support;
pub mod tuner;
pub mod tuner_smart;

pub use read_fail::{FailKind, ReadFail, ReadResult};
pub use rep::{DbMsg, ReportSink};

use read_fail::read_fail;

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::Receiver;
use std::sync::Arc;
use std::time::Duration;

use rusqlite::types::Value;
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

/// Columns and ordering displayed in the Reports window; the window owns titles and widths.
///
/// `core_uid` and `newrecid` are hidden service columns. `id` is the server row id
/// formerly represented by the legacy `db_id`.
pub const DISPLAY_COLUMNS: &[&str] = &[
    "buydate",
    "closedate",
    "sellsetdate",
    "core_name",
    "id",
    "taskid",
    "exorderid",
    "coin",
    "isshort",
    "quantity",
    "boughtq",
    "buyprice",
    "sellprice",
    "spentbtc",
    "gainedbtc",
    "profitbtc",
    "lev",
    "strategyid",
    "source",
    "channel",
    "channelname",
    "signaltype",
    "fname",
    "basecurrency",
    "emulator",
    "status",
    "sellreason",
    "comment",
    "btc1hdelta",
    "exchange1hdelta",
    "btc24hdelta",
    "exchange24hdelta",
    "btc5mdelta",
    "bvsvratio",
    "pump1h",
    "dump1h",
    "d24h",
    "d3h",
    "d1h",
    "d15m",
    "d5m",
    "d1m",
    "dbtc1m",
    "vd1m",
    "pricebug",
    "hvol",
    "hvolf",
    "dvol",
    "takeprofitlag",
    "last_update_at",
];

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

/// Query result containing column names and generic value rows for every column.
///
/// `cols` is a RUNTIME list from `PRAGMA table_info`, so core-added fields appear
/// without code changes: known columns keep canonical order and new ones follow.
pub struct ReportTable {
    pub cols: Vec<String>,
    pub rows: Vec<Vec<Value>>,
    /// `core_uid` for each row, parallel to `rows`.
    ///
    /// This service column is absent from `cols` and `DISPLAY_COLUMNS`, but lets a
    /// report coin click open the chart ON THE CORE that made the trade
    /// (`core_uid` equals the runtime `CoreId`).
    pub core_uids: Vec<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SideFilter {
    #[default]
    All,
    Long,
    Short,
}

#[derive(Debug, Clone, Default)]
pub struct ReportFilter {
    /// Selected cores for the multi-select filter; empty means all cores.
    pub core_uids: Vec<u64>,
    pub date_from: Option<i64>,
    pub date_to: Option<i64>,
    pub coin: String,
    pub side: SideFilter,
    /// Emulator orders: `None` selects all, `Some(false)` only real orders, and
    /// `Some(true)` only emulator orders. A NULL column value counts as real.
    pub emulator: Option<bool>,
    /// Soft-deleted trades (the core-supplied `deleted` column): `false` hides them,
    /// `true` shows ONLY them. A NULL column value counts as not deleted, matching
    /// the analytics filter; a source without the column holds no soft-deleted rows.
    pub deleted_only: bool,
}

/// Open a reader while distinguishing an absent replica from a SQLite failure.
///
/// A genuinely absent file maps to `NotReady`; metadata and SQLite open errors
/// map to `Failed` so callers cannot present them as an empty period.
pub fn open_reader() -> ReadResult<Connection> {
    let path = paths::reports_db_path();
    // NOT `Path::exists`: it reports false for a permission or metadata error
    // too, which would take the silent `NotReady` branch and tell the user the
    // replica is merely not synced yet. Only a genuine absence may say that.
    // The check still has to happen because `Connection::open` would otherwise
    // CREATE an empty database in place of the missing one.
    match std::fs::metadata(&path) {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Err(ReadFail::NotReady),
        Err(e) => return Err(read_fail::io_fail("отчёты(reader): доступ к файлу", &e)),
    }
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

pub fn save_sort(conn: &Connection, key: &str, desc: bool) {
    let _ = conn.execute(
        "INSERT INTO app_meta(key,value) VALUES('sort_key',?1)
         ON CONFLICT(key) DO UPDATE SET value=excluded.value",
        rusqlite::params![key],
    );
    let _ = conn.execute(
        "INSERT INTO app_meta(key,value) VALUES('sort_desc',?1)
         ON CONFLICT(key) DO UPDATE SET value=excluded.value",
        rusqlite::params![if desc { "1" } else { "0" }],
    );
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
    let csv = cols.join(",");
    let _ = conn.execute(
        "INSERT INTO app_meta(key,value) VALUES('report_visible',?1)
         ON CONFLICT(key) DO UPDATE SET value=excluded.value",
        rusqlite::params![csv],
    );
}

/// Build the runtime display-column list from the typed and legacy schemas.
///
/// Known columns follow `DISPLAY_COLUMNS`; extra columns follow alphabetically,
/// service columns are omitted, and legacy `db_id` is exposed as `id`. Schema
/// probe errors map to `Failed`; an absent table contributes no columns. This
/// function receives an open connection and therefore cannot return `NotReady`.
pub fn display_columns(conn: &Connection) -> ReadResult<Vec<String>> {
    const SERVICE: &[&str] = &[
        "core_uid",
        "newrecid",
        "db_id",
        "sql",
        "created_ms",
        "updated_ms",
    ];
    let mut have = rep::table_cols_res(conn)?;
    let legacy = table_columns_res(conn)?;
    if legacy.contains("db_id") {
        have.insert("id".to_string());
    }
    have.extend(legacy);
    let mut out: Vec<String> = DISPLAY_COLUMNS
        .iter()
        .filter(|c| have.contains(**c))
        .map(|c| (*c).to_string())
        .collect();
    let mut extra: Vec<String> = have
        .iter()
        .filter(|h| !SERVICE.contains(&h.as_str()) && !DISPLAY_COLUMNS.contains(&h.as_str()))
        .cloned()
        .collect();
    extra.sort();
    out.extend(extra);
    Ok(out)
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

/// Project a source onto the shared `cols`: preserve its own columns, map legacy
/// `db_id` to `id`, and emit NULL for absent columns.
fn source_select(src: &ReadSource, cols: &[String]) -> String {
    cols.iter()
        .map(|c| {
            if src.legacy && c == "id" && src.cols.contains("db_id") {
                "db_id AS \"id\"".to_string()
            } else if src.cols.contains(c) {
                format!("\"{c}\"")
            } else {
                format!("NULL AS \"{c}\"")
            }
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// Compare values while merging sorted sources: numbers as `f64`, text
/// lexicographically. The caller handles NULL and always places it last.
fn cmp_values(a: &Value, b: &Value) -> std::cmp::Ordering {
    fn num(v: &Value) -> Option<f64> {
        match v {
            Value::Integer(i) => Some(*i as f64),
            Value::Real(r) => Some(*r),
            _ => None,
        }
    }
    match (num(a), num(b)) {
        (Some(x), Some(y)) => x.partial_cmp(&y).unwrap_or(std::cmp::Ordering::Equal),
        _ => match (a, b) {
            (Value::Text(x), Value::Text(y)) => x.cmp(y),
            _ => std::cmp::Ordering::Equal,
        },
    }
}

/// Validate the sort key against runtime columns to prevent injection.
///
/// Fall back to `closedate`, or to the always-present `newrecid` before the core
/// schema supplies `closedate`.
fn sort_column(cols: &[String], key: &str) -> String {
    if let Some(c) = cols.iter().find(|c| c.as_str() == key) {
        return c.clone();
    }
    if cols.iter().any(|c| c == "closedate") {
        "closedate".to_string()
    } else {
        "newrecid".to_string()
    }
}

/// Apply column predicates only to columns that EXIST in the source.
///
/// Before the core schema arrives, the replica may lack `closedate`, `coin`,
/// `isshort`, or `emulator`; filtering on an absent column would fail the entire
/// SELECT, while an absent column also means there is no data for that condition.
fn build_where(
    f: &ReportFilter,
    cols: &std::collections::HashSet<String>,
) -> (String, Vec<Box<dyn rusqlite::types::ToSql>>) {
    let has = |n: &str| cols.contains(n);
    let mut sql = String::from(" WHERE 1=1");
    let mut params: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
    if !f.core_uids.is_empty() {
        // Inline numeric values safely; IN supports the multi-core selector.
        let ids = f
            .core_uids
            .iter()
            .map(|u| (*u as i64).to_string())
            .collect::<Vec<_>>()
            .join(",");
        sql.push_str(&format!(" AND core_uid IN ({ids})"));
    }
    if has("closedate") {
        if let Some(from) = f.date_from {
            sql.push_str(" AND closedate IS NOT NULL AND closedate >= ?");
            params.push(Box::new(from));
        }
        if let Some(to) = f.date_to {
            sql.push_str(" AND closedate IS NOT NULL AND closedate <= ?");
            params.push(Box::new(to));
        }
    }
    let coin = f.coin.trim();
    if !coin.is_empty() && has("coin") {
        sql.push_str(" AND coin LIKE ?");
        params.push(Box::new(format!("%{}%", coin.to_uppercase())));
    }
    if has("isshort") {
        match f.side {
            SideFilter::All => {}
            SideFilter::Long => sql.push_str(" AND isshort = 0"),
            SideFilter::Short => sql.push_str(" AND isshort = 1"),
        }
    }
    if has("emulator") {
        match f.emulator {
            None => {}
            Some(true) => sql.push_str(" AND COALESCE(emulator, 0) = 1"),
            Some(false) => sql.push_str(" AND COALESCE(emulator, 0) = 0"),
        }
    }
    // Deleted-mode semantics live on `ReportFilter::deleted_only`; the `1=0` arm makes
    // a column-less source contribute nothing when only deleted rows are wanted.
    if has("deleted") {
        sql.push_str(if f.deleted_only {
            " AND COALESCE(deleted, 0) <> 0"
        } else {
            " AND COALESCE(deleted, 0) = 0"
        });
    } else if f.deleted_only {
        sql.push_str(" AND 1=0");
    }
    (sql, params)
}

/// Aggregate profit and order count over the complete filter, not only the top N.
///
/// Returns `Failed` when source discovery or any aggregate query fails; only a
/// successful empty result returns `(0.0, 0)`. The open connection means this
/// function cannot return `NotReady`.
pub fn query_totals(conn: &Connection, f: &ReportFilter) -> ReadResult<(f64, i64)> {
    const CTX: &str = "отчёты: query_totals";
    let (mut sum, mut count) = (0.0f64, 0i64);
    for src in read_sources_res(conn)? {
        let (where_sql, params) = build_where(f, &src.cols);
        let profit = if src.cols.contains("profitbtc") {
            "COALESCE(SUM(profitbtc),0.0)"
        } else {
            "0.0"
        };
        let sql = format!("SELECT {profit}, COUNT(*) FROM {}{where_sql}", src.table);
        let refs: Vec<&dyn rusqlite::types::ToSql> = params.iter().map(|b| b.as_ref()).collect();
        let (s, c) = conn
            .query_row(&sql, refs.as_slice(), |r| {
                Ok((r.get::<_, f64>(0)?, r.get::<_, i64>(1)?))
            })
            .map_err(|e| read_fail(CTX, e))?;
        sum += s;
        count += c;
    }
    Ok((sum, count))
}

/// Return the top `limit` reports for the filter and sort using all display columns.
///
/// Source, schema, query, and row-conversion errors map to `Failed`; no partial
/// table is returned, which also keeps exports from writing incomplete data.
/// The open connection means this function cannot return `NotReady`.
pub fn query_reports(
    conn: &Connection,
    f: &ReportFilter,
    sort_key: &str,
    desc: bool,
    limit: usize,
) -> ReadResult<ReportTable> {
    const CTX: &str = "отчёты: query_reports";
    let cols = display_columns(conn)?;
    let col = sort_column(&cols, sort_key);
    let sort_ix = cols.iter().position(|c| *c == col);
    let dir = if desc { "DESC" } else { "ASC" };

    // Query the top N from EACH source separately so indexes work, then merge below.
    let mut merged: Vec<(u64, Vec<Value>)> = Vec::new();
    for src in read_sources_res(conn)? {
        let (where_sql, mut params) = build_where(f, &src.cols);
        let select = source_select(&src, &cols);
        // Sort in SQL only if the source has the column. The legacy `id` alias is
        // visible to ORDER BY; otherwise source order is irrelevant because the merge reorders it.
        let sortable = src.cols.contains(&col) || (src.legacy && col == "id");
        let order = if sortable {
            format!("\"{col}\" IS NULL, \"{col}\" {dir}")
        } else {
            "1".to_string()
        };
        let sql = format!(
            "SELECT core_uid, {select} FROM {}{where_sql} ORDER BY {order} LIMIT ?",
            src.table
        );
        params.push(Box::new(limit as i64));
        let refs: Vec<&dyn rusqlite::types::ToSql> = params.iter().map(|b| b.as_ref()).collect();
        let mut stmt = conn.prepare(&sql).map_err(|e| read_fail(CTX, e))?;
        let n = cols.len();
        let mapped = stmt
            .query_map(refs.as_slice(), |r| {
                let core_uid = r.get::<_, i64>(0)? as u64;
                let mut v = Vec::with_capacity(n);
                for i in 0..n {
                    v.push(r.get::<_, Value>(i + 1)?);
                }
                Ok((core_uid, v))
            })
            .map_err(|e| read_fail(CTX, e))?;
        // Every row is a trade the user is entitled to see, and the same rows
        // are what the export writes — so no row error is skippable here.
        for row in mapped {
            merged.push(row.map_err(|e| read_fail(CTX, e))?);
        }
    }

    // Merge with NULL always last, like `{col} IS NULL` in SQL, then apply direction.
    merged.sort_by(|a, b| {
        let va = sort_ix.and_then(|i| a.1.get(i)).unwrap_or(&Value::Null);
        let vb = sort_ix.and_then(|i| b.1.get(i)).unwrap_or(&Value::Null);
        match (matches!(va, Value::Null), matches!(vb, Value::Null)) {
            (true, true) => std::cmp::Ordering::Equal,
            (true, false) => std::cmp::Ordering::Greater,
            (false, true) => std::cmp::Ordering::Less,
            _ => {
                let o = cmp_values(va, vb);
                if desc {
                    o.reverse()
                } else {
                    o
                }
            }
        }
    });
    merged.truncate(limit);

    let mut rows = Vec::with_capacity(merged.len());
    let mut core_uids = Vec::with_capacity(merged.len());
    for (uid, row) in merged {
        core_uids.push(uid);
        rows.push(row);
    }
    Ok(ReportTable {
        cols,
        rows,
        core_uids,
    })
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
    match std::fs::metadata(&path) {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Err(ReadFail::NotReady),
        Err(e) => return Err(read_fail::io_fail("отчёты(ro): доступ к файлу", &e)),
    }
    Connection::open_with_flags(
        &path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|e| read_fail("отчёты(ro)", e))
}

/// Highest `core_uid` in one table, or `Ok(None)` when it is absent or holds no rows.
///
/// Shared by both stores keyed on `core_uid`, so the negative-value drop and the
/// absent-versus-failed distinction have one definition rather than one per caller.
pub(crate) fn max_core_uid_in(
    conn: &Connection,
    table: &str,
    ctx: &'static str,
) -> ReadResult<Option<u64>> {
    let present: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?1",
            [table],
            |r| r.get(0),
        )
        .map_err(|e| read_fail(ctx, e))?;
    if present == 0 {
        return Ok(None);
    }
    let found: Option<i64> = conn
        .query_row(&format!("SELECT MAX(core_uid) FROM {table}"), [], |r| {
            r.get::<_, Option<i64>>(0)
        })
        .map_err(|e| read_fail(ctx, e))?;
    // A negative value cannot be a uid; dropping it beats wrapping into a huge `u64`.
    Ok(found.and_then(|v| u64::try_from(v).ok()))
}

/// Highest `core_uid` any report row has ever carried, across both schemas.
///
/// Feeds the durable uid high-water mark: rows here outlive the server that wrote them, so a
/// uid still present in this replica must never be handed to a new core. `Ok(None)` means the
/// read succeeded and found no rows — the caller must keep that distinct from a failure, since
/// only the former is safe to treat as "this store contributes nothing".
///
/// A source whose schema lacks `core_uid` is skipped rather than queried: `read_sources_res`
/// always reports the modern table, which does not exist until `rep::init` has run. Negative
/// values cannot be uids and are dropped instead of wrapping into a huge `u64`.
pub fn max_core_uid(conn: &Connection) -> ReadResult<Option<u64>> {
    const CTX: &str = "отчёты: max_core_uid";
    let mut max: Option<u64> = None;
    for src in read_sources_res(conn)? {
        if !src.cols.contains("core_uid") {
            continue;
        }
        // MAX over the leading PK column is an index seek. The selector-shaped GROUP BY in
        // `distinct_cores` would instead scan every row of a replica that reaches hundreds of MB.
        max = max.max(max_core_uid_in(conn, src.table, CTX)?);
    }
    Ok(max)
}

/// Load cores for the filter selector.
///
/// Source, query, and row-conversion errors map to `Failed`; only a successful
/// query may return an empty list. The open connection means this function
/// cannot return `NotReady`.
pub fn distinct_cores(conn: &Connection) -> ReadResult<Vec<(u64, String)>> {
    const CTX: &str = "отчёты: distinct_cores";
    let mut out: Vec<(u64, String)> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for src in read_sources_res(conn)? {
        let order = if src.legacy {
            "MAX(COALESCE(updated_ms, 0))"
        } else {
            "MAX(newrecid)"
        };
        let mut stmt = conn
            .prepare(&format!(
                "SELECT core_uid, core_name FROM {} GROUP BY core_uid ORDER BY {order} DESC",
                src.table
            ))
            .map_err(|e| read_fail(CTX, e))?;
        let rows = stmt
            .query_map([], |r| {
                Ok((r.get::<_, i64>(0)? as u64, r.get::<_, String>(1)?))
            })
            .map_err(|e| read_fail(CTX, e))?;
        for row in rows {
            // core_uid keys the whole selector, so a conversion miss here is
            // never a skippable "dirty label".
            let (uid, name) = row.map_err(|e| read_fail(CTX, e))?;
            if seen.insert(uid) {
                out.push((uid, name));
            }
        }
    }
    Ok(out)
}

// ============================================================================
//  Date and time without external crates (cross-platform)
// ============================================================================

/// Convert Unix seconds to `YYYY-MM-DD HH:MM` in UTC; return empty for values <= 0.
pub fn fmt_unix(secs: i64) -> String {
    if secs <= 0 {
        return String::new();
    }
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let (h, mi) = (rem / 3600, (rem % 3600) / 60);
    let (y, m, d) = crate::util::time::civil_from_days(days);
    format!("{y:04}-{m:02}-{d:02} {h:02}:{mi:02}")
}

/// unix-seconds → "YYYY-MM-DD" in UTC. EMPTY for <= 0 — callers rendering a date column
/// must turn that into their own "not known" marker rather than a blank cell.
///
/// A function of its own rather than a substring of [`fmt_unix`]: the time is part of that
/// one's contract, and a column slicing it off would drift silently on any format change.
pub fn fmt_unix_date(secs: i64) -> String {
    if secs <= 0 {
        return String::new();
    }
    let (y, m, d) = crate::util::time::civil_from_days(secs.div_euclid(86_400));
    format!("{y:04}-{m:02}-{d:02}")
}

/// Convert Unix seconds to `YYYY-MM-DD HH:MM:SS` in UTC for the command log.
pub fn fmt_unix_secs(secs: i64) -> String {
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let (h, mi, s) = (rem / 3600, (rem % 3600) / 60, rem % 60);
    let (y, m, d) = crate::util::time::civil_from_days(days);
    format!("{y:04}-{m:02}-{d:02} {h:02}:{mi:02}:{s:02}")
}

/// Convert `YYYY-MM-DD` to Unix seconds at the start of that UTC day.
pub fn parse_ymd(s: &str) -> Option<i64> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    let mut it = s.split('-');
    let y: i64 = it.next()?.trim().parse().ok()?;
    let m: i64 = it.next()?.trim().parse().ok()?;
    let d: i64 = it.next()?.trim().parse().ok()?;
    if !(1..=12).contains(&m) || !(1..=31).contains(&d) {
        return None;
    }
    Some(days_from_civil(y, m, d) * 86_400)
}

fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

#[cfg(test)]
mod tests;
