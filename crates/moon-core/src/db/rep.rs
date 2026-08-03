//! Typed replica of the Orders report database (`moonproto::Event::Report`) in
//! the `orders_rep` table.
//!
//! The core supplies an append-only schema through `ReportEvent::Schema`.
//! Column names are stored lowercase to match the legacy names consumed by the
//! Report panel; SQLite itself is case-insensitive. The replication key is
//! `(core_uid, newrecid)`. The legacy `closed_sell_reports` table uses a different
//! `db_id`, is read-only, and is purged per core after the first complete typed sync.
//!
//! A core resumes from a durable checkpoint — `{epoch, next_from_rec_id}` — held in `app_meta`
//! under `rep_epoch_{core_uid}` / `rep_next_{core_uid}`. The checkpoint is written ONLY by the
//! transaction that applies an alive map, never by `SyncComplete`: catch-up advances by
//! `newRecID` and therefore cannot observe a soft-delete, restore, or retention removal of an
//! older row that happened while the terminal was offline. The alive map is what repairs that,
//! so a checkpoint stored before it would record the repair as done. Open rows below the
//! checkpoint are reconciled separately through `check_open_rows`.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::SyncSender;
use std::sync::{Arc, Mutex};

use moonproto::{
    MoonReports, ReportAliveMapComplete, ReportRow as RepRow, ReportRowsDeleted, ReportSchema,
    ReportSyncCheckpoint, ReportSyncComplete, ReportSyncPage, ReportValue,
};
use rusqlite::types::Value;
use rusqlite::Connection;

#[cfg(test)]
mod tests;

pub(super) const TABLE: &str = "orders_rep";

/// The ONE ranged visibility update. Both visibility paths use this exact text so they share one
/// `prepare_cached` entry on the writer connection.
fn set_deleted_range_sql() -> String {
    format!("UPDATE {TABLE} SET deleted=?1 WHERE core_uid=?2 AND newrecid BETWEEN ?3 AND ?4")
}

/// Report field whose non-zero value hides a row from the default Report and Analytics filters.
const DELETED_COL: &str = "deleted";

/// Where an upserted row came from, which decides how an ABSENT `deleted` field is read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RowSource {
    /// A live `RowUpsert`: partial by nature, and a missing `deleted` means the row is alive.
    Live,
    /// A catch-up page row: carries the core's whole column set.
    CatchUpPage,
}

/// Where a core's report replication starts when a feed connects.
///
/// The states stay separate because a replica with rows but no checkpoint must resume without a
/// full-history download, while `sync_from` requires an epoch-bearing checkpoint.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ReportStart {
    /// No local rows: request the core's full retained history.
    Fresh,
    /// Local rows but no stored epoch: resume at the next local id until an alive map commits the
    /// first epoch-bearing checkpoint.
    Resume(i64),
    /// A checkpoint committed together with an applied alive map.
    Checkpoint(ReportSyncCheckpoint),
}

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
    /// Complete catch-up after the final-page acknowledgement and purge legacy rows.
    ///
    /// This does NOT advance the replication start state: the alive map that follows does,
    /// together with the visibility it repairs.
    SyncComplete {
        core_uid: u64,
        done: ReportSyncComplete,
    },
    /// Authoritative row visibility for `newrecid = 1..=covered_up_to`, committed in the same
    /// transaction as the durable checkpoint of the catch-up it answers.
    ///
    /// A clear bit means the row is soft-deleted OR physically absent on the core (retention).
    /// Both hide the local row rather than dropping it, so a later restore or upsert revives it.
    /// The library overlays live `RowUpsert`/`RowDelete`/`RowsDeleted` onto the bitmap before
    /// emitting completion, and anything arriving afterwards sits behind this message in the same
    /// FIFO — so the sole writer needs no race-recovery layer of its own.
    AliveMap {
        core_uid: u64,
        map: ReportAliveMapComplete,
        /// Taken from the `ReportSyncComplete` this map answers, never from the map itself.
        checkpoint: ReportSyncCheckpoint,
    },
    /// The core serves another report database: wipe this core's replica and sync it from zero.
    ///
    /// Carries the reports handle for the same reason [`DbMsg::Page`] carries `ack`: only the
    /// writer knows the wipe committed, so it re-issues the fresh sync itself once it has.
    ReplicaRecreated {
        core_uid: u64,
        reports: MoonReports,
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

/// Value exposed to the feed thread as `ReportTx`: a writer channel and per-core start states.
///
/// The writer computes the start states when opening the database, and each feed takes
/// its own when starting sync. The channel is BOUNDED by
/// [`super::REPORT_QUEUE_CAP`]: a large-history catch-up emits batches faster than
/// the writer inserts them, and an unbounded queue once consumed ALL machine memory
/// (88 GB virtual commit measured before the system froze). When full, `send` blocks
/// the core feed thread to apply backpressure, which affects almost only catch-up.
#[derive(Clone)]
pub struct ReportSink {
    pub(super) tx: SyncSender<DbMsg>,
    pub(super) starts: Arc<Mutex<HashMap<u64, ReportStart>>>,
    /// Open rows per core at database open: `newrecid`, newest first, at most 100.
    ///
    /// The feed registers them with `check_open_rows` because an open trade may
    /// have closed or been deleted offline BELOW the checkpoint. Results arrive as
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

    /// Where this core's replication starts, defaulting to [`ReportStart::Fresh`].
    ///
    /// A core absent from the map has no local rows, and a poisoned lock has no trustworthy
    /// answer; both resolve to a full re-sync rather than to a guessed position.
    pub(crate) fn next_start(&self, core_uid: u64) -> ReportStart {
        self.starts
            .lock()
            .ok()
            .and_then(|m| m.get(&core_uid).copied())
            .unwrap_or(ReportStart::Fresh)
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
    starts: Arc<Mutex<HashMap<u64, ReportStart>>>,
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

/// Initialize the replica table, per-core start states, and open-row sets.
///
/// Table-creation, required-schema, and core-scan setup errors return `Err`.
/// In particular, an unreadable column schema makes the caller disable the
/// writer for this session rather than acknowledge incompletely written pages.
pub(super) fn init(
    conn: &Connection,
    starts: Arc<Mutex<HashMap<u64, ReportStart>>>,
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
        let mut map = starts.lock().unwrap_or_else(|e| e.into_inner());
        let mut open_map = open_rows.lock().unwrap_or_else(|e| e.into_inner());
        map.clear();
        open_map.clear();
        let mut stmt = conn.prepare(&format!("SELECT DISTINCT core_uid FROM {TABLE}"))?;
        let uids: Vec<i64> = stmt.query_map([], |r| r.get(0))?.flatten().collect();
        for uid in uids {
            map.insert(uid as u64, startup_start(conn, uid));
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
        starts,
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
///
/// [`RowSource::Live`] additionally treats an ABSENT `deleted` field as "alive": that is how
/// moonproto itself reads a live row when it overlays one onto an in-flight alive map
/// (`state/report.rs`, `unwrap_or(0) == 0`). Preserving the stored flag instead would leave a row
/// the alive map hid hidden forever, since the core has no reason to resend a field that did not
/// change — and the map's own contract is that a hidden row can be revived. Page rows carry the
/// whole column set, so [`RowSource::CatchUpPage`] keeps the plain preserve-what-is-absent rule.
pub(super) fn apply_upsert(
    conn: &Connection,
    st: &RepState,
    core_uid: u64,
    core_name: &str,
    row: &RepRow,
    source: RowSource,
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
    let mut carries_deleted = false;
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
        carries_deleted |= name == DELETED_COL;
        names.push(name);
    }
    if source == RowSource::Live && !carries_deleted && st.cols.contains(DELETED_COL) {
        vals.push(Value::Integer(0));
        names.push(DELETED_COL.to_owned());
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
    // same SQL. Compiling it per row would bottleneck the writer and let the queue grow to OOM.
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
    if change.is_empty() || !st.cols.contains(DELETED_COL) {
        return Ok(());
    }
    // `deleted` is stored as 0/1, matching the `COALESCE(deleted, 0)` the read filters use.
    let flag = i64::from(change.deleted);
    let uid = core_uid as i64;
    {
        let mut stmt = conn.prepare_cached(&set_deleted_range_sql())?;
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

/// Drop one core's replica rows and its durable checkpoint.
///
/// The ONE reset used by both recreation signals — the `database_recreated` page and the alive
/// map's [`moonproto::ReportAliveMapOutcome::DatabaseRecreated`]. Splitting them would leave the
/// page path clearing rows while the checkpoint survived, and the next start would then resume
/// with a stale epoch, wipe the partly rebuilt replica again, and re-download everything on a
/// loop. `rep_synced_{uid}` is deliberately kept: it only gates the legacy purge, which is a
/// property of this terminal's migration, not of the core's database identity.
fn reset_replica(conn: &Connection, core_uid: u64) -> rusqlite::Result<()> {
    conn.execute(
        &format!("DELETE FROM {TABLE} WHERE core_uid=?1"),
        [core_uid as i64],
    )?;
    clear_checkpoint(conn, core_uid)
}

/// Publish the in-memory half of a committed [`reset_replica`].
///
/// The open-row set goes with the rows: checking `newrecid`s from the superseded database
/// reconciles nothing. Both recreation signals — the page and the alive map — publish
/// through here, so neither can leave the other's in-memory state behind.
pub(super) fn commit_replica_reset(
    st: &RepState,
    core_uid: u64,
    open_rows: &Mutex<HashMap<u64, Vec<i64>>>,
) {
    if let Ok(mut m) = st.starts.lock() {
        m.insert(core_uid, ReportStart::Fresh);
    }
    if let Ok(mut m) = open_rows.lock() {
        m.remove(&core_uid);
    }
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
        reset_replica(conn, core_uid)?;
        log::warn!(
            "отчёты(rep): ядро {core_uid} пересоздало БД — реплика сброшена, lib рестартует sync"
        );
    }
    for row in page.rows.iter() {
        apply_upsert(conn, st, core_uid, core_name, row, RowSource::CatchUpPage)?;
    }
    Ok(())
}

/// Wipe a core's replica because the core serves another report database.
pub(super) fn apply_replica_recreated(conn: &Connection, core_uid: u64) -> rusqlite::Result<()> {
    reset_replica(conn, core_uid)?;
    log::warn!(
        "отчёты(rep): карта живых строк ядра {core_uid} сообщила о другой БД — реплика сброшена"
    );
    Ok(())
}

/// Stage `SyncComplete` database effects.
///
/// The legacy migration completes in the transaction. `done` is taken but its checkpoint is
/// deliberately NOT stored: catch-up advances by `newRecID` and so cannot see an offline
/// soft-delete, restore or retention removal of an older row. Storing it here would record that
/// repair as done before the alive map ran it, and no later session would retry — the whole point
/// of [`apply_alive_map`] owning the write.
pub(super) fn apply_sync_complete(
    conn: &Connection,
    st: &mut RepState,
    core_uid: u64,
    done: &ReportSyncComplete,
) -> rusqlite::Result<()> {
    meta_set_i64(conn, &format!("rep_synced_{core_uid}"), 1)?;
    st.synced.insert(core_uid);
    log::debug!(
        "отчёты(rep): sync ядра {core_uid} закрыт на epoch={} (чекпойнт ждёт карту живых строк)",
        done.epoch,
    );
    purge_legacy(conn, st, core_uid)
}

/// Log a committed `SyncComplete`. Visibility and the checkpoint follow with the alive map.
pub(super) fn commit_sync_complete(core_uid: u64, done: &ReportSyncComplete) {
    log::info!(
        "отчёты(rep): sync ядра {core_uid} завершён: страниц={} строк={} epoch={} max_rec_id={} \
         курсор={}",
        done.page_count,
        done.total_rows,
        done.epoch,
        done.max_rec_id,
        done.next_from_rec_id,
    );
}

/// How many rows one alive map inspected and how many changed visibility.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(super) struct AliveMapApplied {
    /// Local rows of this core at or below the map's coverage.
    pub inspected: i64,
    /// Rows the map hid.
    pub hidden: i64,
    /// Rows the map revealed.
    pub revealed: i64,
}

impl AliveMapApplied {
    /// Whether any row's visibility changed, i.e. whether open surfaces must requery.
    pub(super) fn changed(self) -> bool {
        self.hidden > 0 || self.revealed > 0
    }
}

/// Accumulates the `newrecid` ranges an alive map must update, fed one scanned row at a time.
///
/// A range may span physical gaps — those hold no local rows of this core, so widening across
/// them updates nothing extra — but an AGREEING row must break it, or the update would flip a row
/// the core did not ask about. Feeding row by row is what keeps the scan streaming: only the
/// coalesced ranges are retained, never the rows.
#[derive(Debug, Default)]
struct DisagreementRanges {
    hide: Vec<(i64, i64)>,
    reveal: Vec<(i64, i64)>,
    /// Which list the immediately preceding row extended, so an agreeing row breaks both.
    running: Option<bool>,
}

impl DisagreementRanges {
    /// Record one scanned row: its id, whether the replica currently hides it, and what the map
    /// says. `alive == None` covers ids outside the map, which the map says nothing about.
    fn push(&mut self, rec_id: i64, deleted: bool, alive: Option<bool>) {
        // A row disagrees exactly when the map's verdict equals its current `deleted` flag:
        // alive-but-hidden must be revealed, dead-but-visible must be hidden. `None` — an id
        // outside the map — says nothing, so it breaks any running range.
        let Some(want_alive) = alive.filter(|&alive| alive == deleted) else {
            self.running = None;
            return;
        };
        let list = if want_alive {
            &mut self.reveal
        } else {
            &mut self.hide
        };
        match list.last_mut() {
            Some(last) if self.running == Some(want_alive) => last.1 = rec_id,
            _ => list.push((rec_id, rec_id)),
        }
        self.running = Some(want_alive);
    }

    /// The collected `(hide, reveal)` ranges, inclusive on both ends.
    fn finish(self) -> (Vec<(i64, i64)>, Vec<(i64, i64)>) {
        (self.hide, self.reveal)
    }
}

/// Apply an alive map's visibility and write its checkpoint alongside the row changes.
///
/// The caller's surrounding transaction commits both atomically.
///
/// `is_alive` is the map's bit lookup, taken abstractly because `ReportAliveMapComplete` has no
/// public constructor and could not otherwise be exercised by a test.
///
/// The scan is bounded by this core's LOCAL rows, never by `covered_up_to`, which is a core-side
/// high-water in the millions: the primary key `(core_uid, newrecid)` serves both the filter and
/// the ordering, so no sort, no temporary b-tree, and no additional index is needed — and
/// [`REP_INDEXES`] documents why another index is not free. Only disagreements are collected, so
/// the steady state after the first pass costs one index scan and no writes.
///
/// Without the `deleted` column there is nowhere to record visibility, so the checkpoint is NOT
/// stored: recording an alive map as applied while its clear bits went nowhere would leave rows
/// the core has dropped visible forever. Such a core stays on its previous start state and
/// reconciles again next session.
///
/// Nothing is staged into the valuation outbox, matching [`apply_set_deleted`] — hiding a row
/// changes no trade's value, and the valuation worker does not read the flag.
pub(super) fn apply_alive_map(
    conn: &Connection,
    st: &RepState,
    core_uid: u64,
    covered_up_to: i64,
    is_alive: impl Fn(i64) -> Option<bool>,
    checkpoint: ReportSyncCheckpoint,
) -> rusqlite::Result<Option<AliveMapApplied>> {
    if !st.cols.contains(DELETED_COL) {
        log::warn!(
            "отчёты(rep): у ядра {core_uid} нет колонки `deleted` — карта живых строк не \
             применена, чекпойнт не сохранён"
        );
        return Ok(None);
    }
    let mut applied = AliveMapApplied::default();
    let uid = core_uid as i64;
    let (hide, reveal) = {
        let mut stmt = conn.prepare_cached(&format!(
            "SELECT newrecid, COALESCE(deleted,0) FROM {TABLE} \
             WHERE core_uid=?1 AND newrecid>0 AND newrecid<=?2 ORDER BY newrecid"
        ))?;
        let mut rows = stmt.query(rusqlite::params![uid, covered_up_to])?;
        let mut ranges = DisagreementRanges::default();
        while let Some(row) = rows.next()? {
            let rec_id: i64 = row.get(0)?;
            let deleted: i64 = row.get(1)?;
            applied.inspected += 1;
            ranges.push(rec_id, deleted != 0, is_alive(rec_id));
        }
        ranges.finish()
    };
    // The scan statement is dropped above: an UPDATE must not interleave with an open cursor
    // over the same table on this connection.
    let mut stmt = conn.prepare_cached(&set_deleted_range_sql())?;
    for (flag, ranges, counter) in [
        (1i64, &hide, &mut applied.hidden),
        (0i64, &reveal, &mut applied.revealed),
    ] {
        for &(from, to) in ranges {
            *counter += stmt.execute(rusqlite::params![flag, uid, from, to])? as i64;
        }
    }
    store_checkpoint(conn, core_uid, checkpoint)?;
    Ok(Some(applied))
}

/// Publish the in-memory half of a committed alive map.
///
/// Outside [`apply_alive_map`] for the same reason [`commit_sync_complete`] is outside
/// [`apply_sync_complete`]: a rolled-back batch must not move the next connection's start state.
pub(super) fn commit_alive_map(
    st: &RepState,
    core_uid: u64,
    checkpoint: ReportSyncCheckpoint,
    applied: AliveMapApplied,
) {
    if let Ok(mut m) = st.starts.lock() {
        m.insert(core_uid, ReportStart::Checkpoint(checkpoint));
    }
    log::info!(
        "отчёты(rep): карта живых строк ядра {core_uid} применена: просмотрено={} скрыто={} \
         возвращено={} чекпойнт={{epoch={}, next={}}}",
        applied.inspected,
        applied.hidden,
        applied.revealed,
        checkpoint.epoch,
        checkpoint.next_from_rec_id,
    );
}

/// `app_meta` key holding a core's checkpoint epoch.
fn epoch_key(core_uid: u64) -> String {
    format!("rep_epoch_{core_uid}")
}

/// `app_meta` key holding a core's checkpoint cursor.
fn next_key(core_uid: u64) -> String {
    format!("rep_next_{core_uid}")
}

/// Read a core's durable checkpoint, or `None` when it is absent or unusable.
///
/// The validity rule mirrors moonproto's `ReportSyncCheckpoint::is_valid`, which is crate-private
/// there: a zero epoch identifies no database, and a negative cursor addresses no row. Either one
/// would be rejected by `sync_from`, so an unusable pair is treated as no checkpoint at all.
fn load_checkpoint(conn: &Connection, core_uid: u64) -> Option<ReportSyncCheckpoint> {
    let epoch = super::meta_get_i64(conn, &epoch_key(core_uid))?;
    let next_from_rec_id = super::meta_get_i64(conn, &next_key(core_uid))?;
    let epoch = i32::try_from(epoch).ok()?;
    (epoch != 0 && next_from_rec_id >= 0).then_some(ReportSyncCheckpoint {
        epoch,
        next_from_rec_id,
    })
}

/// Write a core's durable checkpoint. Only [`apply_alive_map`] calls this.
fn store_checkpoint(
    conn: &Connection,
    core_uid: u64,
    checkpoint: ReportSyncCheckpoint,
) -> rusqlite::Result<()> {
    meta_set_i64(conn, &epoch_key(core_uid), i64::from(checkpoint.epoch))?;
    meta_set_i64(conn, &next_key(core_uid), checkpoint.next_from_rec_id)
}

/// Drop a core's durable checkpoint, so its next sync starts from zero.
fn clear_checkpoint(conn: &Connection, core_uid: u64) -> rusqlite::Result<()> {
    super::meta_delete(conn, &epoch_key(core_uid))?;
    super::meta_delete(conn, &next_key(core_uid))
}

/// Decide where a core's replication starts, from the durable checkpoint and the local rows.
///
/// Two rules that both look like defects until stated:
///
/// - the stored `next_from_rec_id` is NEVER raised to `max(newrecid) + 1`. Live upserts
///   legitimately land ABOVE the checkpoint between two catch-ups, so `max(newrecid) + 1` is
///   normally the larger of the two. Resuming at the checkpoint re-fetches those few rows, which
///   upsert idempotently by `(core_uid, newrecid)`; resuming at the maximum would skip the pages
///   between them, which is the hole the checkpoint exists to prevent.
/// - a checkpoint with no local rows is DISCARDED. Otherwise a core whose rows were all removed
///   would resume at a high cursor and never re-download the history it just lost.
///
/// The inverse — rows lost while `app_meta` survives — cannot arise from any path here:
/// `maint::compact_db` and `backup_db` delete no rows, and `report_recovery` moves whole database
/// files, so rows and metadata are always recovered as one unit. A future partial-restore path
/// must clear the checkpoint keys itself.
///
/// ACCEPTED RESIDUAL RISK whenever replication starts from [`ReportStart::Resume`]. Without a
/// stored epoch the library has no identity to compare, so it falls back to its
/// high-water heuristic; a core that recreated its report database while this terminal was offline
/// goes undetected once the replacement has already grown past the old maximum `newrecid`. Rows
/// whose ids exist in BOTH databases then keep the dead database's field values, and the alive map
/// that follows cannot repair them — it carries visibility, not content. Ids the replacement does
/// not have are still hidden by that map. Once an alive map commits a checkpoint, later
/// recreations are detected by epoch. A full-history download avoids the risk at the cost of
/// downloading the entire retained replica; a user who suspects stale content can clear the
/// replica to force that sync.
fn startup_start(conn: &Connection, uid: i64) -> ReportStart {
    let max_local: Option<i64> = conn
        .query_row(
            &format!("SELECT MAX(newrecid) FROM {TABLE} WHERE core_uid=?1"),
            [uid],
            |r| r.get::<_, Option<i64>>(0),
        )
        .ok()
        .flatten();
    match (max_local, load_checkpoint(conn, uid as u64)) {
        (None, _) => ReportStart::Fresh,
        (Some(_), Some(checkpoint)) => ReportStart::Checkpoint(checkpoint),
        (Some(max), None) => ReportStart::Resume(max + 1),
    }
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
