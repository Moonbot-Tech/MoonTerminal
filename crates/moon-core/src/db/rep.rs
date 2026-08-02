//! Typed replica of the Orders report database (`moonproto::Event::Report`) in
//! the `orders_rep` table.
//!
//! The core supplies an append-only schema through `ReportEvent::Schema`.
//! Column names are stored lowercase to match the legacy names consumed by the
//! Report panel; SQLite itself is case-insensitive. The replication key is
//! `(core_uid, newrecid)`. The legacy `closed_sell_reports` table uses a different
//! `db_id`, is read-only, and is purged per core after the first complete typed sync.
//!
//! Under the protocol-v4 replication contract, a core resumes at the local
//! `max(newRecID) + 1`; zero requests a fresh sync. Open rows below that cursor
//! are reconciled separately through `check_open_rows`.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::SyncSender;
use std::sync::{Arc, Mutex};

use moonproto::{
    MoonReports, ReportRow as RepRow, ReportRowsDeleted, ReportSchema, ReportSyncComplete,
    ReportSyncPage, ReportValue,
};
use rusqlite::types::Value;
use rusqlite::Connection;

pub(super) const TABLE: &str = "orders_rep";

/// Message for the SQLite writer, the sole owner of the write connection.
pub enum DbMsg {
    /// Append-only core replica schema used to create or extend columns.
    Schema {
        core_uid: u64,
        schema: Arc<ReportSchema>,
    },
    /// Idempotently upsert a live typed row by `(core_uid, newrecid)`.
    Upsert {
        core_uid: u64,
        core_name: String,
        row: RepRow,
    },
    Delete {
        core_uid: u64,
        rec_id: i64,
    },
    /// Bulk soft-delete or restore broadcast by the core: set the `deleted` flag on every
    /// replica row of this core whose `newrecid` is in one of the ranges or the singles.
    ///
    /// Distinct from [`DbMsg::Delete`], which hard-removes one row — this only flips the flag
    /// the Report and Analytics filters read, so `deleted = false` restores the rows rather
    /// than dropping them.
    SetDeleted {
        core_uid: u64,
        change: ReportRowsDeleted,
    },
    /// Catch-up page under the report-replication flow-control contract.
    ///
    /// The writer applies rows and sends `page_applied` AFTER committing the
    /// transaction. Until then, the library requests no next page, keeping one
    /// page in flight and providing backpressure by design.
    Page {
        core_uid: u64,
        core_name: String,
        page: Arc<ReportSyncPage>,
        ack: MoonReports,
    },
    /// Complete catch-up after the final-page acknowledgement, commit the cursor,
    /// and purge legacy rows.
    SyncComplete {
        core_uid: u64,
        done: ReportSyncComplete,
    },
    /// Acknowledge valuation outbox rows after their derived values committed.
    ///
    /// This internal message returns through the sole report writer so the valuation worker never
    /// opens a second write connection to `reports.sqlite`.
    ValuationAck {
        /// Highest contiguous outbox sequence safely reflected in `valuation.sqlite`.
        through_seq: i64,
    },
}

/// Value exposed to the feed thread as `ReportTx`: a writer channel and start cursors.
///
/// The writer computes the cursors when opening the database, and each feed takes
/// its own cursor when starting sync. The channel is BOUNDED by
/// [`super::REPORT_QUEUE_CAP`]: a large-history catch-up emits batches faster than
/// the writer inserts them, and an unbounded queue once consumed ALL machine memory
/// (88 GB virtual commit measured before the system froze). When full, `send` blocks
/// the core feed thread to apply backpressure, which affects almost only catch-up.
#[derive(Clone)]
pub struct ReportSink {
    pub(super) tx: SyncSender<DbMsg>,
    pub(super) cursors: Arc<Mutex<HashMap<u64, i64>>>,
    /// Open rows per core at database open: `newrecid`, newest first, at most 100.
    ///
    /// The feed registers them with `check_open_rows` because an open trade may
    /// have closed or been deleted offline BELOW the cursor. Results arrive as
    /// ordinary `RowUpsert` or `RowDelete` messages.
    pub(super) open_rows: Arc<Mutex<HashMap<u64, Vec<i64>>>>,
    /// Coalesces the terminal channel-closed diagnostic if the writer thread panics.
    pub(super) send_failed: Arc<AtomicBool>,
}

impl ReportSink {
    /// Send one replica event, blocking when the bounded writer queue applies backpressure.
    pub fn send(&self, msg: DbMsg) {
        if self.tx.send(msg).is_err() && !self.send_failed.swap(true, Ordering::AcqRel) {
            log::error!("reports: writer channel closed; replica event was not persisted");
        }
    }

    /// Cursor for the core's next sync request: zero starts fresh and positive values resume.
    ///
    /// Under the report-replication flow-control contract, the cursor is always the local replica's
    /// `max(newRecID) + 1`. Sequential page acknowledgements prevent a crash from
    /// skipping successfully applied tail rows. Any row failure aborts and retries its
    /// whole writer batch before the cursor can advance.
    pub fn next_cursor(&self, core_uid: u64) -> i64 {
        self.cursors
            .lock()
            .ok()
            .and_then(|m| m.get(&core_uid).copied())
            .unwrap_or(0)
    }

    /// Core's open rows for `check_open_rows`; empty means there is nothing to check.
    pub fn open_rows(&self, core_uid: u64) -> Vec<i64> {
        self.open_rows
            .lock()
            .ok()
            .and_then(|m| m.get(&core_uid).cloned())
            .unwrap_or_default()
    }
}

/// Typed-replica state owned by the writer thread.
#[derive(Clone)]
pub(super) struct RepState {
    /// Per-core schema mapping field indexes to names for row conversion.
    schemas: HashMap<u64, Arc<ReportSchema>>,
    /// Cache of lowercase table-column names to avoid PRAGMA calls for every row.
    cols: HashSet<String>,
    cursors: Arc<Mutex<HashMap<u64, i64>>>,
    /// Whether the read-only legacy table still exists; completed typed syncs
    /// progressively purge it by core.
    pub(super) legacy_exists: bool,
    /// Cores with a persisted completed-sync marker (`rep_synced_*=1`).
    /// Initialization uses this set to purge legacy rows left by terminal versions
    /// that still wrote close-SQL reports; protocol v4 has no such write path.
    pub(super) synced: HashSet<u64>,
    /// The legacy table was dropped, so the writer must VACUUM after committing
    /// the batch; VACUUM is forbidden inside the transaction, and without it the
    /// database retains the dropped table's allocated space.
    pub(super) vacuum_pending: bool,
    /// Whether every replica index is guaranteed to exist, avoiding CREATE calls
    /// for every core schema.
    indexes_done: bool,
}

/// Every index the replica's read paths need, as `(name, columns)`.
///
/// ONE list, so "which indexes exist" has a single answer: [`ensure_indexes`] creates each
/// entry whose columns the replica already has, its guard is derived from those same columns,
/// and the index test iterates this table instead of repeating the names.
///
/// - `idx_rep_closedate` — the Report window's default period filter, like the legacy
///   `idx_csr_closedate`.
/// - `idx_rep_core_close` — period AND core filter TOGETHER. Analytics and the Report window
///   both narrow by `core_uid IN (...)` plus a `closedate` range, and neither other index can
///   serve that pair: without this one the planner falls back to a `core_uid`-leading index
///   (`idx_rep_strat` on the real replica, the primary key on a small one) and then reads the
///   whole history of every selected core out of the table just to test `closedate` row by
///   row. Measured on a 458 MB replica (529 862 rows, 19 cores) with 13 cores selected and the
///   period "today": every Analytics statement cost 0.7-1.0 s REGARDLESS of the period — one
///   day cost exactly as much as all history — and one Summary load ran ~10 s; with this index
///   the same load is ~50 ms. The trailing `closedate` is the whole point: it keeps the period
///   range inside the index, so a core's off-period rows are never touched.
///
///   The 2026-07-17 index audit deferred exactly this index as "not hurting yet", over the
///   writer paying for it on every catch-up row. Re-measured on disk under WAL in the shape
///   `apply_upsert` writes (52 columns, batches of 512): 100 000 upserts cost 1.44 s without
///   it and 1.79 s with it when close dates grow with time, as a catch-up stream's do (+63% in
///   the worst case of dates arriving unsorted). That is a one-off second per 100k caught-up
///   rows, against ten seconds on every filter change.
/// - `idx_rep_strat` — strategy-version analytics (`strat_db`): select one strategy's trades
///   and range-join `buydate` against the version's `valid_from` and `valid_to`.
pub(super) const REP_INDEXES: &[(&str, &[&str])] = &[
    ("idx_rep_closedate", &["closedate"]),
    ("idx_rep_core_close", &["core_uid", "closedate"]),
    ("idx_rep_strat", &["core_uid", "strategyid", "buydate"]),
];

/// Create every [`REP_INDEXES`] entry whose columns the replica already has.
///
/// Columns arrive from the core schema at unpredictable times, so check both at
/// startup, when an old database may already have them, and after every schema.
/// Creating an index only after a successful ALTER lost it PERMANENTLY when its
/// column predated the index code or CREATE failed once. Returns `true` once all
/// indexes definitely exist and no further calls are needed; a failing CREATE
/// propagates, because a replica that cannot be indexed is a reason to refuse the
/// writer rather than to run without indexes.
///
/// On an existing replica the first pass is a one-off full-table read per missing index, and it
/// happens here — on the startup thread, before the first window exists. Measured at 0.4 s over
/// 529 862 rows (458 MB) with `idx_rep_core_close` the only one missing; a cold or bigger
/// replica costs more. Nothing else reports that pause, so time the pass and log it when it
/// actually cost something: a one-off delay with no line in the log is indistinguishable from a
/// hang.
fn ensure_indexes(conn: &Connection, cols: &HashSet<String>) -> rusqlite::Result<bool> {
    let started = std::time::Instant::now();
    let mut done = true;
    for (name, index_cols) in REP_INDEXES {
        // A column the core schema has not sent yet: the index waits for a later schema rather
        // than being created without it, and `done` stays false so the caller keeps retrying.
        if !index_cols.iter().all(|c| cols.contains(*c)) {
            done = false;
            continue;
        }
        // Name and columns are the literals above, never data, so the interpolation is safe.
        conn.execute(
            &format!(
                "CREATE INDEX IF NOT EXISTS {name} ON {TABLE}({})",
                index_cols.join(", ")
            ),
            [],
        )?;
    }
    let took = started.elapsed();
    if took >= std::time::Duration::from_millis(200) {
        log::info!("reports(rep): replica indexes built in {took:?} (one-off)");
    }
    Ok(done)
}

/// Initialize the replica table, per-core cursors, and open-row sets.
///
/// Table-creation, required-schema, and core-scan setup errors return `Err`.
/// In particular, an unreadable column schema makes the caller disable the
/// writer for this session rather than acknowledge incompletely written pages.
pub(super) fn init(
    conn: &Connection,
    cursors: Arc<Mutex<HashMap<u64, i64>>>,
    open_rows: Arc<Mutex<HashMap<u64, Vec<i64>>>>,
) -> rusqlite::Result<RepState> {
    conn.execute(
        &format!(
            "CREATE TABLE IF NOT EXISTS {TABLE} (core_uid INTEGER NOT NULL, \
             core_name TEXT NOT NULL, newrecid INTEGER NOT NULL, \
             PRIMARY KEY (core_uid, newrecid))"
        ),
        [],
    )?;
    // Fatal on purpose: this cache decides which fields every incoming page may
    // write. Continuing with an empty cache would omit fields while still ACKing
    // the page, and ACKed pages are not sent again.
    //
    // `spawn_writer` therefore disables report recording for this session and
    // logs the failure. The server-side cursor remains unchanged, so restarting
    // with a readable replica can resync the page instead of losing its fields.
    let cols = table_cols_for_init(conn)?;
    {
        let mut map = cursors.lock().unwrap_or_else(|e| e.into_inner());
        let mut open_map = open_rows.lock().unwrap_or_else(|e| e.into_inner());
        map.clear();
        open_map.clear();
        let mut stmt = conn.prepare(&format!("SELECT DISTINCT core_uid FROM {TABLE}"))?;
        let uids: Vec<i64> = stmt.query_map([], |r| r.get(0))?.flatten().collect();
        for uid in uids {
            map.insert(uid as u64, startup_cursor(conn, uid));
            open_map.insert(uid as u64, open_row_ids(conn, &cols, uid));
        }
    }
    let legacy_exists = table_exists(conn, "closed_sell_reports");
    // Load already synchronized cores from metadata persisted across restarts.
    let mut synced: HashSet<u64> = HashSet::new();
    if let Ok(mut stmt) =
        conn.prepare("SELECT key FROM app_meta WHERE key LIKE 'rep_synced_%' AND value='1'")
    {
        if let Ok(rows) = stmt.query_map([], |r| r.get::<_, String>(0)) {
            synced.extend(rows.flatten().filter_map(|k| {
                k.strip_prefix("rep_synced_")
                    .and_then(|s| s.parse::<u64>().ok())
            }));
        }
    }
    let indexes_done = ensure_indexes(conn, &cols)?;
    let mut st = RepState {
        schemas: HashMap::new(),
        cols,
        cursors,
        legacy_exists,
        synced,
        vacuum_pending: false,
        indexes_done,
    };
    // Purge rows left after SyncComplete by older terminal versions that still
    // consumed the legacy close-SQL stream; otherwise the merged reader sees duplicates.
    for uid in st.synced.clone() {
        purge_legacy(conn, &mut st, uid)?;
    }
    Ok(st)
}

/// Purge one core's legacy rows and permanently drop the empty legacy table.
///
/// The `legacy_dropped` marker prevents `init_db` from recreating it. This is the
/// shared path for `SyncComplete` and initialization cleanup.
fn purge_legacy(conn: &Connection, st: &mut RepState, core_uid: u64) -> rusqlite::Result<()> {
    if !st.legacy_exists {
        return Ok(());
    }
    conn.execute(
        "DELETE FROM closed_sell_reports WHERE core_uid=?1",
        [core_uid as i64],
    )?;
    let left: i64 = conn.query_row("SELECT COUNT(*) FROM closed_sell_reports", [], |r| r.get(0))?;
    if left == 0 {
        conn.execute("DROP TABLE closed_sell_reports", [])?;
        st.legacy_exists = false;
        st.vacuum_pending = true;
        meta_set_i64(conn, "legacy_dropped", 1)?;
        log::info!(
            "отчёты(rep): легаси-таблица closed_sell_reports снесена — все ядра на typed-реплике"
        );
    }
    Ok(())
}

/// Whether a lowercase key is a safe SQLite identifier (guards against injection
/// where the name is interpolated into `ALTER TABLE` without quoting):
/// `[a-z_][a-z0-9_]*`.
fn valid_ident(name: &str) -> bool {
    let b = name.as_bytes();
    !b.is_empty()
        && (b[0] == b'_' || b[0].is_ascii_lowercase())
        && b.iter()
            .all(|&c| c == b'_' || c.is_ascii_lowercase() || c.is_ascii_digit())
}

/// Extend the table with columns missing from the append-only core schema.
///
/// Names are lowercase. `sql_spec` comes from the authenticated core with the same
/// trust as moonproto's own `sqlite_add_column_sql`; the name is additionally
/// validated as an identifier.
pub(super) fn apply_schema(
    conn: &Connection,
    st: &mut RepState,
    core_uid: u64,
    schema: Arc<ReportSchema>,
) -> rusqlite::Result<()> {
    for f in schema.fields() {
        let name = f.name.to_ascii_lowercase();
        if name == "newrecid" || st.cols.contains(&name) {
            continue;
        }
        if !valid_ident(&name) {
            log::warn!(
                "reports(rep): rejecting invalid schema identifier '{}'",
                f.name
            );
            return Err(rusqlite::Error::InvalidParameterName(name));
        }
        conn.execute(
            &format!("ALTER TABLE {TABLE} ADD COLUMN \"{name}\" {}", f.sql_spec),
            [],
        )?;
        log::info!(
            "reports(rep): added column '{name}' {} for core schema {core_uid}",
            f.sql_spec
        );
        st.cols.insert(name);
    }
    // Index columns arrive automatically from the core schema, so finish creating
    // indexes here once columns definitely exist. IF NOT EXISTS makes repeats cheap.
    if !st.indexes_done {
        st.indexes_done = ensure_indexes(conn, &st.cols)?;
    }
    st.schemas.insert(core_uid, schema);
    Ok(())
}

/// Idempotently upsert a row by `(core_uid, newrecid)`.
///
/// Write only fields present in the row because live updates may be partial;
/// preserve all other values.
pub(super) fn apply_upsert(
    conn: &Connection,
    st: &RepState,
    core_uid: u64,
    core_name: &str,
    row: &RepRow,
) -> rusqlite::Result<()> {
    let Some(schema) = st.schemas.get(&core_uid) else {
        log::warn!(
            "reports(rep): rejecting rec_id={} for core {core_uid} before its schema",
            row.rec_id
        );
        return Err(rusqlite::Error::InvalidQuery);
    };
    let mut names: Vec<String> = Vec::with_capacity(row.fields.len());
    let mut vals: Vec<Value> = Vec::with_capacity(row.fields.len());
    for fv in &row.fields {
        let Some(f) = schema.field(fv.field_index) else {
            continue;
        };
        let name = f.name.to_ascii_lowercase();
        if name == "newrecid" || !st.cols.contains(&name) {
            continue;
        }
        vals.push(match &fv.value {
            ReportValue::Integer(i) => Value::Integer(*i),
            ReportValue::Float(x) => Value::Real(*x),
            ReportValue::Text(s) => Value::Text(s.clone()),
        });
        names.push(name);
    }
    let mut cols_sql = String::from("core_uid, core_name, newrecid");
    let mut ph = String::from("?, ?, ?");
    let mut set_sql = String::from("core_name=excluded.core_name");
    for n in &names {
        cols_sql.push_str(&format!(", \"{n}\""));
        ph.push_str(", ?");
        set_sql.push_str(&format!(", \"{n}\"=excluded.\"{n}\""));
    }
    let sql = format!(
        "INSERT INTO {TABLE} ({cols_sql}) VALUES ({ph}) \
         ON CONFLICT(core_uid, newrecid) DO UPDATE SET {set_sql}"
    );
    let uid = core_uid as i64;
    let mut params: Vec<&dyn rusqlite::types::ToSql> = Vec::with_capacity(vals.len() + 3);
    params.push(&uid);
    params.push(&core_name);
    params.push(&row.rec_id);
    for v in &vals {
        params.push(v);
    }
    // Catch-up batches have a stable field set, so `prepare_cached` can reuse the
    // same SQL. Compiling every row used to bottleneck the writer and grow the queue to OOM.
    let mut stmt = conn.prepare_cached(&sql)?;
    stmt.execute(params.as_slice())?;
    Ok(())
}

pub(super) fn apply_delete(conn: &Connection, core_uid: u64, rec_id: i64) -> rusqlite::Result<()> {
    conn.execute(
        &format!("DELETE FROM {TABLE} WHERE core_uid=?1 AND newrecid=?2"),
        rusqlite::params![core_uid as i64, rec_id],
    )?;
    Ok(())
}

/// Apply a core-broadcast bulk soft-delete/restore ([`moonproto::ReportEvent::RowsDeleted`]):
/// set the `deleted` flag to the operation's value on every replica row of this core whose
/// `newrecid` is in one of the inclusive ranges or the singles list.
///
/// Guarded on the `deleted` column existing in the schema cache, mirroring the read side's
/// `has("deleted")`: moonproto broadcasts `RowsDeleted` without a schema gate and treats the
/// field as possibly-absent, so a core whose report schema omits `deleted` is a clean no-op
/// here instead of a "no such column" error per event. Ranges and singles run as separate
/// statements, but the caller invokes this inside the writer's batch transaction (see the
/// writer loop in `db::mod`), so the whole operation commits atomically with the rest of the
/// batch. Each `UPDATE` shape is `prepare_cached` and reused across its loop, as in
/// [`apply_upsert`].
pub(super) fn apply_set_deleted(
    conn: &Connection,
    st: &RepState,
    core_uid: u64,
    change: &ReportRowsDeleted,
) -> rusqlite::Result<()> {
    if change.is_empty() || !st.cols.contains("deleted") {
        return Ok(());
    }
    // `deleted` is stored as 0/1, matching the `COALESCE(deleted, 0)` the read filters use.
    let flag = i64::from(change.deleted);
    let uid = core_uid as i64;
    {
        let mut stmt = conn.prepare_cached(&format!(
            "UPDATE {TABLE} SET deleted=?1 WHERE core_uid=?2 AND newrecid BETWEEN ?3 AND ?4"
        ))?;
        for range in change.ranges.iter() {
            stmt.execute(rusqlite::params![
                flag,
                uid,
                range.from_rec_id,
                range.to_rec_id
            ])?;
        }
    }
    {
        let mut stmt = conn.prepare_cached(&format!(
            "UPDATE {TABLE} SET deleted=?1 WHERE core_uid=?2 AND newrecid=?3"
        ))?;
        for &rec_id in change.singles.iter() {
            stmt.execute(rusqlite::params![flag, uid, rec_id])?;
        }
    }
    Ok(())
}

/// Apply a catch-up page, resetting the core replica when `database_recreated` is set.
///
/// When reset, the library restarts from zero after the acknowledgement; this page's
/// rows are upserted idempotently in the meantime. Any row failure aborts the
/// writer transaction so the page cannot be acknowledged with a hole.
pub(super) fn apply_page(
    conn: &Connection,
    st: &mut RepState,
    core_uid: u64,
    core_name: &str,
    page: &ReportSyncPage,
) -> rusqlite::Result<()> {
    if page.database_recreated {
        conn.execute(
            &format!("DELETE FROM {TABLE} WHERE core_uid=?1"),
            [core_uid as i64],
        )?;
        log::warn!(
            "отчёты(rep): ядро {core_uid} пересоздало БД — реплика сброшена, lib рестартует sync"
        );
    }
    for row in page.rows.iter() {
        apply_upsert(conn, st, core_uid, core_name, row)?;
    }
    Ok(())
}

/// Stage `SyncComplete` database effects while keeping the in-memory cursor unchanged.
///
/// The legacy migration completes in the transaction; cursor publication happens only
/// after the caller commits that transaction successfully.
pub(super) fn apply_sync_complete(
    conn: &Connection,
    st: &mut RepState,
    core_uid: u64,
) -> rusqlite::Result<()> {
    meta_set_i64(conn, &format!("rep_synced_{core_uid}"), 1)?;
    st.synced.insert(core_uid);
    purge_legacy(conn, st, core_uid)
}

/// Publish the in-memory cursor side effect of a committed `SyncComplete`.
///
/// Keeping this outside [`apply_sync_complete`] prevents a rolled-back batch
/// from advancing reconnect beyond rows that never reached SQLite.
pub(super) fn commit_sync_complete(st: &RepState, core_uid: u64, done: &ReportSyncComplete) {
    if let Ok(mut m) = st.cursors.lock() {
        m.insert(core_uid, done.next_from_rec_id.max(1));
    }
    log::info!(
        "отчёты(rep): sync ядра {core_uid} завершён: страниц={} строк={} max_rec_id={} курсор={}",
        done.page_count,
        done.total_rows,
        done.max_rec_id,
        done.next_from_rec_id,
    );
}

/// Derive a core cursor from the local database under the report-replication flow-control contract.
///
/// The cursor is ALWAYS `max(newRecID) + 1`. Sequential page acknowledgements prevent
/// a crash or row failure from skipping tail rows because a failed page is never acknowledged.
/// Zero means the core replica is empty and requests a fresh sync. `check_open_rows` reconciles
/// offline changes to open rows below the cursor, so the old minimum-open rule no longer applies.
fn startup_cursor(conn: &Connection, uid: i64) -> i64 {
    conn.query_row(
        &format!("SELECT MAX(newrecid) FROM {TABLE} WHERE core_uid=?1"),
        [uid],
        |r| r.get::<_, Option<i64>>(0),
    )
    .ok()
    .flatten()
    .map(|m| m + 1)
    .unwrap_or(0)
}

/// Load at most 100 open core rows, with NULL or non-positive `closedate`, newest first.
///
/// The set feeds `check_open_rows`. The library also sorts and caps it, but avoiding
/// excess channel traffic here is cheaper.
fn open_row_ids(conn: &Connection, cols: &HashSet<String>, uid: i64) -> Vec<i64> {
    if !cols.contains("closedate") {
        return Vec::new();
    }
    let mut out = Vec::new();
    if let Ok(mut stmt) = conn.prepare(&format!(
        "SELECT newrecid FROM {TABLE} \
         WHERE core_uid=?1 AND newrecid>0 AND (closedate IS NULL OR closedate<=0) \
         ORDER BY newrecid DESC LIMIT 100"
    )) {
        if let Ok(rows) = stmt.query_map([uid], |r| r.get::<_, i64>(0)) {
            out.extend(rows.flatten());
        }
    }
    out
}

/// Probe replica columns, surfacing SQLite's own error.
///
/// The writer needs the raw error to refuse startup; readers wrap it through
/// [`table_cols_res`].
fn table_cols_raw(conn: &Connection) -> rusqlite::Result<HashSet<String>> {
    let mut out = HashSet::new();
    let mut stmt = conn.prepare(&format!("PRAGMA table_info({TABLE})"))?;
    let rows = stmt.query_map([], |r| r.get::<_, String>(1))?;
    for n in rows {
        out.insert(n?.to_ascii_lowercase());
    }
    Ok(out)
}

/// Probe replica columns for [`init`], retrying a transient lock first.
///
/// The caller turns a failure into "no writer for the entire session", which is
/// far too high a price for the 3 s `busy_timeout` expiring under a checkpoint
/// or a second process. Only a failure that is NOT mere contention is worth
/// failing closed on — which is exactly the distinction
/// [`super::read_fail::classify`] exists to make.
fn table_cols_for_init(conn: &Connection) -> rusqlite::Result<HashSet<String>> {
    const ATTEMPTS: u32 = 3;
    let mut last = None;
    for attempt in 1..=ATTEMPTS {
        match table_cols_raw(conn) {
            Ok(cols) => return Ok(cols),
            Err(e) if super::read_fail::classify(&e) == super::FailKind::Busy => {
                log::warn!(
                    "отчёты(rep): PRAGMA table_info занят ({e}) — попытка {attempt} из {ATTEMPTS}"
                );
                last = Some(e);
                std::thread::sleep(std::time::Duration::from_millis(250 * u64::from(attempt)));
            }
            Err(e) => return Err(e),
        }
    }
    Err(last.expect("цикл выходит сюда только после ошибки занятости"))
}

/// Probe replica columns while distinguishing an absent schema from PRAGMA failure.
pub(super) fn table_cols_res(conn: &Connection) -> super::ReadResult<HashSet<String>> {
    use super::read_fail::read_fail;
    // Const, not `format!`: this runs on the healthy path of every schema probe
    // (2-3 per analytics query plus the writer init).
    const CTX: &str = "отчёты(rep): PRAGMA table_info(orders_rep)";
    table_cols_raw(conn).map_err(|e| read_fail(CTX, e))
}

fn table_exists(conn: &Connection, name: &str) -> bool {
    conn.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?1",
        [name],
        |r| r.get::<_, i64>(0),
    )
    .map(|n| n > 0)
    .unwrap_or(false)
}

fn meta_set_i64(conn: &Connection, key: &str, val: i64) -> rusqlite::Result<()> {
    super::meta_set(conn, key, &val.to_string())
}
