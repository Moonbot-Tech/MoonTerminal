//! Forever-persistence of warning episodes in a dedicated SQLite database.
//!
//! Closed episodes are appended as they resolve; the store is queried by server and time window when
//! the chart-badge phase draws warning marks. The `core_warning_series` table (the per-badge ±1 min
//! graph slices) is created now but populated later, when the engine captures those slices.
//!
//! Kept separate from `reports.sqlite` on purpose: warning history is the only copy and must survive
//! a report-replica reset (see `paths::core_warnings_db_path`).

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::net::IpAddr;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use moon_core::session::CoreId;
use rusqlite::types::Value;
use rusqlite::{Connection, OptionalExtension, Row, params, params_from_iter};

use super::{WarnAxis, WarnEpisode, WarnSnapshot};

mod maintenance;
use maintenance::{IndexMaintenance, SERIES_INDEX_SQL, WarnWrite, rebuild_series_index};

/// Schema: closed episodes, plus the (initially empty) per-badge series-slice table.
///
/// `synchronous = NORMAL`: warning history is a recoverable log, not critical data, so a full
/// fsync per commit is not worth the occasional stall on a cloud-synced data dir — losing at most
/// the last episode on a hard crash is acceptable.
const SCHEMA: &str = "
PRAGMA synchronous = NORMAL;
CREATE TABLE IF NOT EXISTS core_warnings (
    id           INTEGER PRIMARY KEY,
    axis         TEXT    NOT NULL,
    server_ip    TEXT,
    core_id      INTEGER,
    start_ms     INTEGER NOT NULL,
    end_ms       INTEGER,
    peak         INTEGER NOT NULL,
    sys_cpu      INTEGER NOT NULL DEFAULT 0,
    occ_mem      INTEGER NOT NULL DEFAULT 0,
    free_mb      INTEGER NOT NULL DEFAULT 0,
    used_mb      INTEGER NOT NULL DEFAULT 0,
    logical_cpus INTEGER NOT NULL DEFAULT 0,
    rtt_ms       INTEGER NOT NULL DEFAULT 0,
    order_ms     INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS ix_warn_server_time ON core_warnings(server_ip, start_ms);
CREATE TABLE IF NOT EXISTS core_warning_series (
    episode_id INTEGER NOT NULL,
    badge      INTEGER NOT NULL,
    subject    TEXT    NOT NULL,
    blob       BLOB    NOT NULL
);
CREATE INDEX IF NOT EXISTS ix_warn_series_ep ON core_warning_series(episode_id);
";

/// Optional read accelerators: existing history is indexed only by the maintenance thread.
const RECENT_INDEX_SCHEMA: &str = "
CREATE INDEX IF NOT EXISTS ix_warn_core_time ON core_warnings(core_id, start_ms);
CREATE INDEX IF NOT EXISTS ix_warn_server_null_time ON core_warnings(server_ip, start_ms) WHERE core_id IS NULL;
";

/// Index-ordered top-N start read for one core. A separate statement per core avoids SQLite's
/// IN/ORDER BY temporary sort.
const RECENT_CORE_EPISODES_SQL: &str =
    "SELECT start_ms FROM core_warnings WHERE core_id = ?1 ORDER BY start_ms DESC LIMIT ?2";
/// Index-ordered top-N start read for server-wide episodes at one address.
const RECENT_SERVER_EPISODES_SQL: &str = "SELECT start_ms FROM core_warnings WHERE server_ip = ?1 AND core_id IS NULL ORDER BY start_ms DESC LIMIT ?2";
/// Boundary-inclusive candidate read for one core, including every row tied at the cutoff.
const RECENT_CORE_TIES_SQL: &str =
    "SELECT id FROM core_warnings WHERE core_id = ?1 AND start_ms >= ?2 ORDER BY start_ms DESC";
/// Boundary-inclusive candidate read for server-wide episodes at one address.
const RECENT_SERVER_TIES_SQL: &str = "SELECT id FROM core_warnings WHERE server_ip = ?1 AND core_id IS NULL AND start_ms >= ?2 ORDER BY start_ms DESC";
/// Connection-local candidate stores. The full candidate table uses the episode id as its rowid so a
/// scan preserves the original table's tie input order.
const RECENT_CANDIDATE_SCHEMA: &str = "
CREATE TEMP TABLE IF NOT EXISTS recent_warn_candidate_ids (id INTEGER PRIMARY KEY);
CREATE TEMP TABLE IF NOT EXISTS recent_warn_candidates (
    id           INTEGER PRIMARY KEY,
    axis         TEXT    NOT NULL,
    server_ip    TEXT,
    core_id      INTEGER,
    start_ms     INTEGER NOT NULL,
    end_ms       INTEGER,
    peak         INTEGER NOT NULL,
    sys_cpu      INTEGER NOT NULL,
    occ_mem      INTEGER NOT NULL,
    free_mb      INTEGER NOT NULL,
    used_mb      INTEGER NOT NULL,
    logical_cpus INTEGER NOT NULL,
    rtt_ms       INTEGER NOT NULL,
    order_ms     INTEGER NOT NULL
);
";

/// SQLite-backed episode log.
pub(crate) struct WarnStore {
    /// Open connection to the warnings database.
    conn: Connection,
    /// Episode-log mutation counter used by presentation caches.
    revision: Arc<AtomicU64>,
    /// Highest id allocated by this writer, including episodes queued ahead of their captures.
    episode_id: Cell<i64>,
    /// One-time index work; episodes, captures and startup pruning queue on its worker.
    index_maintenance: RefCell<Option<IndexMaintenance>>,
}

impl WarnStore {
    /// Open the warning log, scheduling expensive read-index creation off the startup thread.
    ///
    /// Args:
    ///     path: Filesystem path to the SQLite database.
    ///
    /// Returns:
    ///     A ready store, or a SQLite error if the file or schema could not be prepared.
    pub(crate) fn open(path: &Path) -> rusqlite::Result<Self> {
        let store = Self::from_connection(Connection::open(path)?)?;
        let rebuild_series = !store.series_index_is_current()?;
        let build_recent = store.needs_recent_indexes()?;
        if rebuild_series || build_recent {
            store
                .index_maintenance
                .replace(Some(IndexMaintenance::start(
                    path.to_owned(),
                    rebuild_series,
                    build_recent,
                    Arc::clone(&store.revision),
                )));
        }
        Ok(store)
    }

    /// Apply tables and existing indexes; build missing read/series indexes inline only when empty.
    fn from_connection(conn: Connection) -> rusqlite::Result<Self> {
        moon_core::db::wal::enable(&conn)?;
        conn.execute_batch(SCHEMA)?;
        // Add the detection-snapshot columns to a pre-existing database; a fresh one already has
        // them from the schema, so a "duplicate column" error here is expected and ignored.
        for column in [
            "sys_cpu",
            "occ_mem",
            "free_mb",
            "used_mb",
            "logical_cpus",
            "rtt_ms",
            "order_ms",
        ] {
            let _ = conn.execute(
                &format!(
                    "ALTER TABLE core_warnings ADD COLUMN {column} INTEGER NOT NULL DEFAULT 0"
                ),
                [],
            );
        }
        let has_history: bool = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM core_warnings LIMIT 1)",
            [],
            |row| row.get(0),
        )?;
        if !has_history {
            conn.execute_batch(RECENT_INDEX_SCHEMA)?;
        }
        let episode_id = conn.query_row(
            "SELECT COALESCE(MAX(id), 0) FROM core_warnings",
            [],
            |row| row.get(0),
        )?;
        let store = Self {
            conn,
            revision: Arc::new(AtomicU64::new(0)),
            episode_id: Cell::new(episode_id),
            index_maintenance: RefCell::new(None),
        };
        let has_series: bool = store.conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM core_warning_series LIMIT 1)",
            [],
            |row| row.get(0),
        )?;
        if !has_series && !store.series_index_is_current()? {
            rebuild_series_index(&store.conn)?;
        }
        Ok(store)
    }

    /// Compare stored SQL so ordinary opens never drop and rebuild an already-current index.
    fn series_index_is_current(&self) -> rusqlite::Result<bool> {
        let sql: Option<String> = self.conn.query_row(
            "SELECT sql FROM sqlite_master WHERE type = 'index' AND name = 'ix_warn_series_unique'",
            [], |row| row.get(0),
        ).optional()?;
        Ok(sql.as_deref() == Some(SERIES_INDEX_SQL))
    }

    /// Complete ordered warning writes before an explicit Storage deletion preserves its row count.
    fn finish_index_maintenance(&self) -> rusqlite::Result<()> {
        let mut state = self.index_maintenance.borrow_mut();
        if let Some(migration) = state.as_mut() {
            migration.finish()?;
        }
        *state = None;
        Ok(())
    }

    /// Queue warning mutations during index maintenance, or execute after its queue closes.
    /// Queued mutations return zero immediately; the worker publishes committed episode revisions.
    fn write(&self, write: WarnWrite) -> rusqlite::Result<usize> {
        let write = {
            let state = self.index_maintenance.borrow();
            match state.as_ref() {
                Some(migration) => match migration.submit(write)? {
                    None => return Ok(0),
                    Some(write) => write,
                },
                None => return write.execute(&self.conn, &self.revision),
            }
        };
        write.execute(&self.conn, &self.revision)
    }

    /// Detect missing read accelerators using schema metadata rather than scanning the log.
    fn needs_recent_indexes(&self) -> rusqlite::Result<bool> {
        self.conn.query_row(
            "SELECT COUNT(*) != 2 FROM sqlite_master WHERE type = 'index' \
             AND name IN ('ix_warn_core_time', 'ix_warn_server_null_time')",
            [],
            |row| row.get(0),
        )
    }

    /// Return the episode-log mutation counter. Series-only writes do not advance it.
    pub(crate) fn revision(&self) -> u64 {
        self.revision.load(Ordering::Acquire)
    }

    /// Append one closed episode and return its database row id.
    ///
    /// The row id keys this episode's persisted history slice (`core_warning_series.episode_id`), so
    /// the caller keeps it to capture that slice.
    ///
    /// Args:
    ///     episode: A resolved episode (its `end_ms` is normally set).
    ///
    /// Returns:
    ///     A stable row id after insertion or maintenance-queue acceptance, or a SQLite error.
    ///     Queued episodes publish their revision only after the worker commits the row.
    pub(crate) fn insert_episode(&self, episode: &WarnEpisode) -> rusqlite::Result<i64> {
        let id = self.episode_id.get().checked_add(1).ok_or_else(|| {
            rusqlite::Error::SqliteFailure(
                rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_FULL),
                Some("warning episode ids exhausted".into()),
            )
        })?;
        self.write(WarnWrite::Episode {
            id,
            episode: episode.clone(),
        })?;
        self.episode_id.set(id);
        Ok(id)
    }

    /// Return a server's episodes that started within `[from_ms, to_ms]`, oldest first.
    ///
    /// Args:
    ///     ip: Server endpoint address to filter by.
    ///     from_ms: Inclusive lower bound on `start_ms`.
    ///     to_ms: Inclusive upper bound on `start_ms`.
    ///
    /// Returns:
    ///     Matching episodes, or a SQLite error.
    pub(crate) fn episodes_for_server(
        &self,
        ip: IpAddr,
        from_ms: i64,
        to_ms: i64,
    ) -> rusqlite::Result<Vec<WarnEpisode>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, axis, server_ip, core_id, start_ms, end_ms, peak, sys_cpu, occ_mem, free_mb, \
             used_mb, logical_cpus, rtt_ms, order_ms FROM core_warnings \
             WHERE server_ip = ?1 AND start_ms BETWEEN ?2 AND ?3 ORDER BY start_ms",
        )?;
        let rows = stmt.query_map(params![ip.to_string(), from_ms, to_ms], row_to_episode)?;
        rows.collect()
    }

    /// Return the most recent episodes across all servers, newest first, for the Warnings list.
    ///
    /// Args:
    ///     limit: Maximum rows to return.
    ///
    /// Returns:
    ///     Episodes ordered by `start_ms` descending, or a SQLite error.
    #[cfg(test)]
    pub(crate) fn recent_episodes(&self, limit: usize) -> rusqlite::Result<Vec<WarnEpisode>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, axis, server_ip, core_id, start_ms, end_ms, peak, sys_cpu, occ_mem, free_mb, \
             used_mb, logical_cpus, rtt_ms, order_ms FROM core_warnings \
             ORDER BY start_ms DESC LIMIT ?1",
        )?;
        let rows = stmt.query_map(params![limit as i64], row_to_episode)?;
        rows.collect()
    }

    /// Return recent persisted episodes inside one effective core/server scope.
    ///
    /// Each core and server gets an index-ordered top-`limit` read, avoiding SQLite's temporary
    /// sort for `IN (...) ORDER BY start_ms DESC`. Every episode tied at an identity's cutoff joins
    /// the candidates, so the bound cannot discard a row the old full scan would keep. The original
    /// OR query runs only over a rowid-ordered temporary table of those candidates, preserving its
    /// exact row and tie semantics without scanning or sorting the retained history again.
    ///
    /// Args:
    ///     core_ids: Effective core identities accepted by the panel.
    ///     server_ips: Effective server identities accepted for server-wide warning axes.
    ///     limit: Maximum rows to return.
    ///
    /// Returns:
    ///     In-scope episodes ordered by `start_ms` descending, or a SQLite error.
    pub(crate) fn recent_episodes_for_scope(
        &self,
        core_ids: &HashSet<CoreId>,
        server_ips: &HashSet<IpAddr>,
        limit: usize,
    ) -> rusqlite::Result<Vec<WarnEpisode>> {
        if limit == 0 || (core_ids.is_empty() && server_ips.is_empty()) {
            return Ok(Vec::new());
        }
        let limit = i64::try_from(limit).unwrap_or(i64::MAX);
        let mut candidates = Vec::new();
        let mut ids: Vec<_> = core_ids.iter().copied().collect();
        ids.sort_unstable();
        for core in ids {
            self.collect_recent_candidates(
                RECENT_CORE_EPISODES_SQL,
                RECENT_CORE_TIES_SQL,
                Value::Integer(core as i64),
                limit,
                &mut candidates,
            )?;
        }
        let mut ips: Vec<_> = server_ips.iter().copied().collect();
        ips.sort_unstable();
        for ip in ips {
            self.collect_recent_candidates(
                RECENT_SERVER_EPISODES_SQL,
                RECENT_SERVER_TIES_SQL,
                Value::Text(ip.to_string()),
                limit,
                &mut candidates,
            )?;
        }
        self.merge_recent_candidates(core_ids, server_ips, limit, &candidates)
    }

    /// Collect one identity's top-`limit` candidates and every episode tied at its cutoff.
    ///
    /// Args:
    ///     recent_sql: Index-ordered statement returning the identity's cutoff start times.
    ///     ties_sql: Index-ordered statement returning every candidate at or above that cutoff.
    ///     identity: Core id or server address bound to both statements.
    ///     limit: Maximum rows that can survive the final merge.
    ///     candidates: Destination for candidate database ids.
    ///
    /// Returns:
    ///     Unit, or the SQLite error from either read.
    fn collect_recent_candidates(
        &self,
        recent_sql: &str,
        ties_sql: &str,
        identity: Value,
        limit: i64,
        candidates: &mut Vec<i64>,
    ) -> rusqlite::Result<()> {
        let starts = self
            .conn
            .prepare(recent_sql)?
            .query_map(params![&identity, limit], |row| row.get::<_, i64>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let Some(cutoff) = starts.last().copied() else {
            return Ok(());
        };
        let mut statement = self.conn.prepare(ties_sql)?;
        let rows = statement.query_map(params![&identity, cutoff], |row| row.get::<_, i64>(0))?;
        candidates.extend(rows.collect::<rusqlite::Result<Vec<_>>>()?);
        Ok(())
    }

    /// Order candidates with the pre-optimization OR query, never with the retained history table.
    ///
    /// Candidate ids are staged in bounded batches because a large fleet can exceed SQLite's
    /// positional-parameter limit. Both temporary tables are connection-local and reset every read.
    ///
    /// Args:
    ///     core_ids: Effective core identities accepted by the original OR arm.
    ///     server_ips: Effective server identities accepted by the original OR arm.
    ///     limit: Maximum merged rows to return.
    ///     candidates: Top rows and boundary ties collected from the indexes.
    ///
    /// Returns:
    ///     Exactly the old query's rows over the candidate set, or a SQLite error.
    fn merge_recent_candidates(
        &self,
        core_ids: &HashSet<CoreId>,
        server_ips: &HashSet<IpAddr>,
        limit: i64,
        candidates: &[i64],
    ) -> rusqlite::Result<Vec<WarnEpisode>> {
        if candidates.is_empty() {
            return Ok(Vec::new());
        }
        self.conn.execute_batch(RECENT_CANDIDATE_SCHEMA)?;
        let tx = self.conn.unchecked_transaction()?;
        tx.execute("DELETE FROM temp.recent_warn_candidate_ids", [])?;
        tx.execute("DELETE FROM temp.recent_warn_candidates", [])?;
        for chunk in candidates.chunks(500) {
            let placeholders = std::iter::repeat_n("(?)", chunk.len())
                .collect::<Vec<_>>()
                .join(", ");
            tx.execute(
                &format!(
                    "INSERT OR IGNORE INTO temp.recent_warn_candidate_ids (id) VALUES {placeholders}"
                ),
                params_from_iter(chunk.iter()),
            )?;
        }
        tx.execute(
            "INSERT INTO temp.recent_warn_candidates \
             SELECT id, axis, server_ip, core_id, start_ms, end_ms, peak, sys_cpu, occ_mem, free_mb, \
             used_mb, logical_cpus, rtt_ms, order_ms FROM core_warnings \
             WHERE id IN (SELECT id FROM temp.recent_warn_candidate_ids)",
            [],
        )?;
        tx.commit()?;

        let mut clauses = Vec::with_capacity(2);
        let mut values = Vec::with_capacity(core_ids.len() + server_ips.len() + 1);
        if !core_ids.is_empty() {
            clauses.push(format!(
                "core_id IN ({})",
                std::iter::repeat_n("?", core_ids.len())
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
            values.extend(core_ids.iter().map(|core| Value::Integer(*core as i64)));
        }
        if !server_ips.is_empty() {
            clauses.push(format!(
                "(core_id IS NULL AND server_ip IN ({}))",
                std::iter::repeat_n("?", server_ips.len())
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
            values.extend(server_ips.iter().map(|ip| Value::Text(ip.to_string())));
        }
        values.push(Value::Integer(limit));
        let sql = format!(
            "SELECT id, axis, server_ip, core_id, start_ms, end_ms, peak, sys_cpu, occ_mem, free_mb, \
             used_mb, logical_cpus, rtt_ms, order_ms FROM temp.recent_warn_candidates \
             WHERE ({}) ORDER BY start_ms DESC LIMIT ?",
            clauses.join(" OR ")
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(params_from_iter(values.iter()), row_to_episode)?;
        rows.collect()
    }

    /// Persist one episode's ±1 min history slice so its card can draw a graph long after the live
    /// ring has rolled past it.
    ///
    /// `INSERT OR REPLACE` keyed by the `(episode_id, badge, subject)` unique index: capturing the
    /// same episode twice (a durable partial at close, then the complete window once the forward tail
    /// has filled) overwrites rather than duplicating.
    ///
    /// Args:
    ///     episode_id: The `core_warnings` row id this slice belongs to.
    ///     badge: Slice owner — `0` for the whole-server graph, otherwise the core id.
    ///     subject: Which series the samples are (`"server"` for the server graph, `"core"` per core).
    ///     base_ms: Unix ms of the window's first sample, stored for future timestamp alignment.
    ///     samples: 1 Hz `(cpu %, mem %)` pairs across the window.
    ///
    /// Returns:
    ///     Unit after persistence or ordered migration-queue acceptance, or a SQLite error.
    pub(crate) fn insert_series(
        &self,
        episode_id: i64,
        badge: i64,
        subject: &str,
        base_ms: i64,
        samples: &[(u8, u8)],
    ) -> rusqlite::Result<()> {
        let blob = encode_series(base_ms, samples);
        self.write(WarnWrite::Slice {
            episode_id,
            badge,
            subject: subject.to_owned(),
            blob,
        })?;
        Ok(())
    }

    /// Read back one episode's persisted history slice, if it was captured.
    ///
    /// Args:
    ///     episode_id: The `core_warnings` row id.
    ///     subject: Which series to read (`"server"`).
    ///
    /// Returns:
    ///     The stored `(cpu %, mem %)` pairs, `None` if no slice was captured, or a SQLite error.
    pub(crate) fn series_for_episode(
        &self,
        episode_id: i64,
        badge: i64,
        subject: &str,
    ) -> rusqlite::Result<Option<Vec<(u8, u8)>>> {
        let mut stmt = self.conn.prepare(
            "SELECT blob FROM core_warning_series \
             WHERE episode_id = ?1 AND badge = ?2 AND subject = ?3 LIMIT 1",
        )?;
        let mut rows = stmt.query(params![episode_id, badge, subject])?;
        match rows.next()? {
            Some(row) => {
                let blob: Vec<u8> = row.get(0)?;
                Ok(decode_series(&blob))
            }
            None => Ok(None),
        }
    }

    /// Persist one episode's ±1 min ping slice (a `u16`-per-sample blob, as a round-trip exceeds a
    /// `u8`). Idempotent via the same `(episode_id, badge, subject)` unique index.
    ///
    /// Args:
    ///     episode_id: The `core_warnings` row id.
    ///     badge: Slice owner — `0` for the server's worst-ping line, otherwise the core id.
    ///     subject: `"ping"`/`"exch"` for the server lines, `"core_ping"`/`"core_exch"` per core.
    ///     base_ms: Unix ms of the window's first sample.
    ///     samples: 1 Hz round-trip samples (ms) across the window.
    ///
    /// Returns:
    ///     Unit after persistence or ordered migration-queue acceptance, or a SQLite error.
    pub(crate) fn insert_ping_series(
        &self,
        episode_id: i64,
        badge: i64,
        subject: &str,
        base_ms: i64,
        samples: &[u16],
    ) -> rusqlite::Result<()> {
        let blob = encode_u16_series(base_ms, samples);
        self.write(WarnWrite::Slice {
            episode_id,
            badge,
            subject: subject.to_owned(),
            blob,
        })?;
        Ok(())
    }

    /// Read back one episode's persisted ping slice under `subject`, if captured.
    ///
    /// Args:
    ///     episode_id: The `core_warnings` row id.
    ///     subject: Which ping series to read (`"ping"` = client↔core, `"exch"` = core→exchange).
    ///
    /// Returns:
    ///     The stored round-trip samples (ms), `None` if none was captured, or a SQLite error.
    pub(crate) fn ping_series_for_episode(
        &self,
        episode_id: i64,
        badge: i64,
        subject: &str,
    ) -> rusqlite::Result<Option<Vec<u16>>> {
        let mut stmt = self.conn.prepare(
            "SELECT blob FROM core_warning_series \
             WHERE episode_id = ?1 AND badge = ?2 AND subject = ?3 LIMIT 1",
        )?;
        let mut rows = stmt.query(params![episode_id, badge, subject])?;
        match rows.next()? {
            Some(row) => {
                let blob: Vec<u8> = row.get(0)?;
                Ok(decode_u16_series(&blob))
            }
            None => Ok(None),
        }
    }

    /// Delete the persisted graph SLICES of episodes older than `cutoff_ms` (by `start_ms`), keeping
    /// the episode rows themselves. The per-core slices scale with the core count, so this bounds the
    /// file while the episode log stays forever.
    ///
    /// Args:
    ///     cutoff_ms: Episodes with `start_ms` below this lose their slices.
    ///
    /// Returns:
    ///     Rows deleted immediately, zero when queued behind migration, or a SQLite error.
    pub(crate) fn prune_slices(&self, cutoff_ms: i64) -> rusqlite::Result<usize> {
        self.write(WarnWrite::Prune { cutoff_ms })
    }

    /// Delete one core's warning history (Settings → Storage).
    ///
    /// Its own episodes go with their slices, and so do the slices it contributed to server-wide
    /// episodes (`badge` = the core id). The server-wide episodes themselves stay: they describe
    /// the machine, which other cores may share, and carry no core of their own to delete by.
    ///
    /// Args:
    ///     core: The core's uid.
    ///
    /// Returns:
    ///     How many of the core's own episodes were deleted.
    pub(crate) fn forget_core(&self, core: CoreId) -> rusqlite::Result<usize> {
        if let Err(error) = self.finish_index_maintenance() {
            // Deletion needs no series uniqueness constraint; an index failure must not prevent
            // cleanup after all queued writes have completed (or individually reported failure).
            log::warn!("core warnings maintenance before deletion failed: {error}");
        }
        let core = core as i64;
        let tx = self.conn.unchecked_transaction()?;
        tx.execute(
            "DELETE FROM core_warning_series WHERE badge = ?1 OR episode_id IN              (SELECT id FROM core_warnings WHERE core_id = ?1)",
            params![core],
        )?;
        let episodes = tx.execute(
            "DELETE FROM core_warnings WHERE core_id = ?1",
            params![core],
        )?;
        tx.commit()?;
        if episodes > 0 {
            self.revision.fetch_add(1, Ordering::Release);
        }
        Ok(episodes)
    }
}

/// Write an episode with its reserved id on either the UI connection or the maintenance worker.
fn insert_episode_row(
    conn: &Connection,
    id: i64,
    episode: &WarnEpisode,
) -> rusqlite::Result<usize> {
    conn.execute(
        "INSERT INTO core_warnings \
         (id, axis, server_ip, core_id, start_ms, end_ms, peak, sys_cpu, occ_mem, free_mb, used_mb, logical_cpus, rtt_ms, order_ms) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
        params![
            id, axis_str(episode.axis), episode.server_ip.map(|ip| ip.to_string()),
            episode.core_id.map(|id| id as i64), episode.start_ms, episode.end_ms,
            i64::from(episode.peak), i64::from(episode.snap.sys_cpu), i64::from(episode.snap.occ_mem),
            i64::from(episode.snap.free_mb), i64::from(episode.snap.used_mb), i64::from(episode.snap.logical_cpus),
            i64::from(episode.snap.round_trip_ms), i64::from(episode.snap.order_api_latency_ms),
        ],
    )
}

/// Read recent episodes at `path` on an independent read-only connection.
///
/// Args:
///     path: Warning database already initialized by the backend writer.
///     core_ids: Effective core identities accepted by the panel.
///     server_ips: Effective server identities accepted for server-wide warnings.
///     limit: Maximum merged rows to return.
///
/// Returns:
///     The same scoped list as the writer-side reader, an empty list for a missing file, or a SQLite
///     error. The connection is confined to the calling background worker.
pub(crate) fn read_recent_episodes_for_scope(
    path: &Path,
    core_ids: &HashSet<CoreId>,
    server_ips: &HashSet<IpAddr>,
    limit: usize,
) -> rusqlite::Result<Vec<WarnEpisode>> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    let conn = Connection::open_with_flags(
        path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    let _ = conn.busy_timeout(std::time::Duration::from_secs(3));
    let store = WarnStore {
        conn,
        revision: Arc::new(AtomicU64::new(0)),
        episode_id: Cell::new(0),
        index_maintenance: RefCell::new(None),
    };
    store.recent_episodes_for_scope(core_ids, server_ips, limit)
}

/// Episodes per core in the warnings database at `path`, read on a read-only connection so the
/// Storage tab can count off the UI thread. A missing file is an empty map; server-wide episodes
/// belong to no core and are not counted.
pub(crate) fn episodes_by_core(path: &Path) -> rusqlite::Result<HashMap<CoreId, u64>> {
    if !path.exists() {
        return Ok(HashMap::new());
    }
    let conn = Connection::open_with_flags(
        path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    let _ = conn.busy_timeout(std::time::Duration::from_secs(3));
    let mut stmt = conn.prepare(
        "SELECT core_id, COUNT(*) FROM core_warnings WHERE core_id IS NOT NULL GROUP BY core_id",
    )?;
    let rows = stmt.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)))?;
    let mut out = HashMap::new();
    for row in rows {
        let (core, n) = row?;
        if let (Ok(core), Ok(n)) = (u64::try_from(core), u64::try_from(n)) {
            out.insert(core, n);
        }
    }
    Ok(out)
}

/// Encode a scalar `u16` history slice as `[base_ms: i64 LE][n: u16 LE][n×(value u16 LE)]`.
fn encode_u16_series(base_ms: i64, samples: &[u16]) -> Vec<u8> {
    let n = samples.len().min(u16::MAX as usize);
    let mut out = Vec::with_capacity(10 + n * 2);
    out.extend_from_slice(&base_ms.to_le_bytes());
    out.extend_from_slice(&(n as u16).to_le_bytes());
    for &value in samples.iter().take(n) {
        out.extend_from_slice(&value.to_le_bytes());
    }
    out
}

/// Decode a slice written by [`encode_u16_series`]; `None` if the blob is truncated or malformed.
fn decode_u16_series(blob: &[u8]) -> Option<Vec<u16>> {
    if blob.len() < 10 {
        return None;
    }
    let n = u16::from_le_bytes([blob[8], blob[9]]) as usize;
    if blob.len() < 10 + n * 2 {
        return None;
    }
    Some(
        (0..n)
            .map(|i| u16::from_le_bytes([blob[10 + i * 2], blob[11 + i * 2]]))
            .collect(),
    )
}

/// Encode a history slice as `[base_ms: i64 LE][n: u16 LE][n×(cpu u8, mem u8)]`.
fn encode_series(base_ms: i64, samples: &[(u8, u8)]) -> Vec<u8> {
    let n = samples.len().min(u16::MAX as usize);
    let mut out = Vec::with_capacity(10 + n * 2);
    out.extend_from_slice(&base_ms.to_le_bytes());
    out.extend_from_slice(&(n as u16).to_le_bytes());
    for &(cpu, mem) in samples.iter().take(n) {
        out.push(cpu);
        out.push(mem);
    }
    out
}

/// Decode a slice written by [`encode_series`]; `None` if the blob is truncated or malformed.
fn decode_series(blob: &[u8]) -> Option<Vec<(u8, u8)>> {
    if blob.len() < 10 {
        return None;
    }
    let n = u16::from_le_bytes([blob[8], blob[9]]) as usize;
    if blob.len() < 10 + n * 2 {
        return None;
    }
    Some(
        (0..n)
            .map(|i| (blob[10 + i * 2], blob[11 + i * 2]))
            .collect(),
    )
}

/// Stable wire name for a warning axis.
fn axis_str(axis: WarnAxis) -> &'static str {
    match axis {
        WarnAxis::SysCpu => "sys_cpu",
        WarnAxis::MemGrowth => "mem_growth",
        WarnAxis::Unreachable => "connectivity",
        WarnAxis::Ping => "ping",
        WarnAxis::ExchPing => "exch_ping",
        WarnAxis::ApiExpiry => "api_expiry",
        WarnAxis::ApiQuota => "api_quota",
    }
}

/// Parse a stored axis name, defaulting unknown values to `SysCpu` rather than failing a whole row.
fn axis_from(name: &str) -> WarnAxis {
    match name {
        "mem_growth" => WarnAxis::MemGrowth,
        "connectivity" => WarnAxis::Unreachable,
        "ping" => WarnAxis::Ping,
        "exch_ping" => WarnAxis::ExchPing,
        "api_expiry" => WarnAxis::ApiExpiry,
        "api_quota" => WarnAxis::ApiQuota,
        _ => WarnAxis::SysCpu,
    }
}

/// Reconstruct an episode from one `core_warnings` row (column order matches the SELECT).
fn row_to_episode(row: &Row) -> rusqlite::Result<WarnEpisode> {
    let axis: String = row.get(1)?;
    let server_ip: Option<String> = row.get(2)?;
    let core_id: Option<i64> = row.get(3)?;
    Ok(WarnEpisode {
        id: row.get::<_, i64>(0)? as u64,
        axis: axis_from(&axis),
        server_ip: server_ip.and_then(|text| text.parse().ok()),
        core_id: core_id.map(|value| value as CoreId),
        start_ms: row.get(4)?,
        end_ms: row.get(5)?,
        peak: row.get::<_, i64>(6)? as u16,
        snap: WarnSnapshot {
            sys_cpu: row.get::<_, i64>(7)? as u8,
            occ_mem: row.get::<_, i64>(8)? as u8,
            free_mb: row.get::<_, i64>(9)? as u16,
            used_mb: row.get::<_, i64>(10)? as u32,
            logical_cpus: row.get::<_, i64>(11)? as u8,
            round_trip_ms: row.get::<_, i64>(12)? as u16,
            order_api_latency_ms: row.get::<_, i64>(13)? as u16,
        },
    })
}

#[cfg(test)]
mod tests;
