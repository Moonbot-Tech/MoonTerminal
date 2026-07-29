//! Forever-persistence of warning episodes in a dedicated SQLite database.
//!
//! Closed episodes are appended as they resolve; the store is queried by server and time window when
//! the chart-badge phase draws warning marks. The `core_warning_series` table (the per-badge ±1 min
//! graph slices) is created now but populated later, when the engine captures those slices.
//!
//! Kept separate from `reports.sqlite` on purpose: warning history is the only copy and must survive
//! a report-replica reset (see `paths::core_warnings_db_path`).

use std::net::IpAddr;
use std::path::Path;

use moon_core::session::CoreId;
use rusqlite::{Connection, Row, params};

use super::{WarnAxis, WarnEpisode, WarnSnapshot};

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
CREATE UNIQUE INDEX IF NOT EXISTS ix_warn_series_unique ON core_warning_series(episode_id, subject);
";

/// SQLite-backed episode log.
pub(crate) struct WarnStore {
    /// Open connection to the warnings database.
    conn: Connection,
}

impl WarnStore {
    /// Open (creating if needed) the warnings database at `path` and ensure its schema.
    ///
    /// Args:
    ///     path: Filesystem path to the SQLite database.
    ///
    /// Returns:
    ///     A ready store, or a SQLite error if the file or schema could not be prepared.
    pub(crate) fn open(path: &Path) -> rusqlite::Result<Self> {
        Self::from_connection(Connection::open(path)?)
    }

    /// Wrap a connection and apply the schema.
    fn from_connection(conn: Connection) -> rusqlite::Result<Self> {
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
        Ok(Self { conn })
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
    ///     The inserted row id, or the SQLite error if the insert failed.
    pub(crate) fn insert_episode(&self, episode: &WarnEpisode) -> rusqlite::Result<i64> {
        self.conn.execute(
            "INSERT INTO core_warnings \
             (axis, server_ip, core_id, start_ms, end_ms, peak, sys_cpu, occ_mem, free_mb, used_mb, logical_cpus, rtt_ms, order_ms) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
            params![
                axis_str(episode.axis),
                episode.server_ip.map(|ip| ip.to_string()),
                episode.core_id.map(|id| id as i64),
                episode.start_ms,
                episode.end_ms,
                i64::from(episode.peak),
                i64::from(episode.snap.sys_cpu),
                i64::from(episode.snap.occ_mem),
                i64::from(episode.snap.free_mb),
                i64::from(episode.snap.used_mb),
                i64::from(episode.snap.logical_cpus),
                i64::from(episode.snap.round_trip_ms),
                i64::from(episode.snap.order_api_latency_ms),
            ],
        )?;
        Ok(self.conn.last_insert_rowid())
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
    pub(crate) fn recent_episodes(&self, limit: usize) -> rusqlite::Result<Vec<WarnEpisode>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, axis, server_ip, core_id, start_ms, end_ms, peak, sys_cpu, occ_mem, free_mb, \
             used_mb, logical_cpus, rtt_ms, order_ms FROM core_warnings \
             ORDER BY start_ms DESC LIMIT ?1",
        )?;
        let rows = stmt.query_map(params![limit as i64], row_to_episode)?;
        rows.collect()
    }

    /// Persist one episode's ±1 min history slice so its card can draw a graph long after the live
    /// ring has rolled past it.
    ///
    /// `INSERT OR REPLACE` keyed by the `(episode_id, subject)` unique index: capturing the same
    /// episode twice (a durable partial at close, then the complete window once the forward tail has
    /// filled) overwrites rather than duplicating.
    ///
    /// Args:
    ///     episode_id: The `core_warnings` row id this slice belongs to.
    ///     subject: Which series the samples are (`"server"` today; `"core"` reserved).
    ///     base_ms: Unix ms of the window's first sample, stored for future timestamp alignment.
    ///     samples: 1 Hz `(cpu %, mem %)` pairs across the window.
    ///
    /// Returns:
    ///     Unit, or the SQLite error if the insert failed.
    pub(crate) fn insert_series(
        &self,
        episode_id: i64,
        subject: &str,
        base_ms: i64,
        samples: &[(u8, u8)],
    ) -> rusqlite::Result<()> {
        let blob = encode_series(base_ms, samples);
        self.conn.execute(
            "INSERT OR REPLACE INTO core_warning_series (episode_id, badge, subject, blob) \
             VALUES (?1, 0, ?2, ?3)",
            params![episode_id, subject, blob],
        )?;
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
        subject: &str,
    ) -> rusqlite::Result<Option<Vec<(u8, u8)>>> {
        let mut stmt = self.conn.prepare(
            "SELECT blob FROM core_warning_series WHERE episode_id = ?1 AND subject = ?2 LIMIT 1",
        )?;
        let mut rows = stmt.query(params![episode_id, subject])?;
        match rows.next()? {
            Some(row) => {
                let blob: Vec<u8> = row.get(0)?;
                Ok(decode_series(&blob))
            }
            None => Ok(None),
        }
    }

    /// Persist one episode's ±1 min ping slice under the `ping` subject (a `u16`-per-sample blob, as
    /// a round-trip exceeds a `u8`). Idempotent via the same `(episode_id, subject)` unique index.
    ///
    /// Args:
    ///     episode_id: The `core_warnings` row id.
    ///     base_ms: Unix ms of the window's first sample.
    ///     samples: 1 Hz round-trip samples (ms) across the window.
    ///
    /// Returns:
    ///     Unit, or the SQLite error if the insert failed.
    pub(crate) fn insert_ping_series(
        &self,
        episode_id: i64,
        subject: &str,
        base_ms: i64,
        samples: &[u16],
    ) -> rusqlite::Result<()> {
        let blob = encode_u16_series(base_ms, samples);
        self.conn.execute(
            "INSERT OR REPLACE INTO core_warning_series (episode_id, badge, subject, blob) \
             VALUES (?1, 0, ?2, ?3)",
            params![episode_id, subject, blob],
        )?;
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
        subject: &str,
    ) -> rusqlite::Result<Option<Vec<u16>>> {
        let mut stmt = self.conn.prepare(
            "SELECT blob FROM core_warning_series WHERE episode_id = ?1 AND subject = ?2 LIMIT 1",
        )?;
        let mut rows = stmt.query(params![episode_id, subject])?;
        match rows.next()? {
            Some(row) => {
                let blob: Vec<u8> = row.get(0)?;
                Ok(decode_u16_series(&blob))
            }
            None => Ok(None),
        }
    }
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
    }
}

/// Parse a stored axis name, defaulting unknown values to `SysCpu` rather than failing a whole row.
fn axis_from(name: &str) -> WarnAxis {
    match name {
        "mem_growth" => WarnAxis::MemGrowth,
        "connectivity" => WarnAxis::Unreachable,
        "ping" => WarnAxis::Ping,
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
