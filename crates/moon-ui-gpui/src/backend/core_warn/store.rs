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

use super::{WarnAxis, WarnEpisode};

/// Schema: closed episodes, plus the (initially empty) per-badge series-slice table.
///
/// `synchronous = NORMAL`: warning history is a recoverable log, not critical data, so a full
/// fsync per commit is not worth the occasional stall on a cloud-synced data dir — losing at most
/// the last episode on a hard crash is acceptable.
const SCHEMA: &str = "
PRAGMA synchronous = NORMAL;
CREATE TABLE IF NOT EXISTS core_warnings (
    id         INTEGER PRIMARY KEY,
    axis       TEXT    NOT NULL,
    server_ip  TEXT,
    core_id    INTEGER,
    start_ms   INTEGER NOT NULL,
    end_ms     INTEGER,
    peak       INTEGER NOT NULL
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
        Ok(Self { conn })
    }

    /// Append one closed episode and return its database row id.
    ///
    /// The row id is what the upcoming per-badge series slices reference (`core_warning_series
    /// .episode_id`), so it is returned even though the current caller ignores it.
    ///
    /// Args:
    ///     episode: A resolved episode (its `end_ms` is normally set).
    ///
    /// Returns:
    ///     The inserted row id, or the SQLite error if the insert failed.
    pub(crate) fn insert_episode(&self, episode: &WarnEpisode) -> rusqlite::Result<i64> {
        self.conn.execute(
            "INSERT INTO core_warnings (axis, server_ip, core_id, start_ms, end_ms, peak) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                axis_str(episode.axis),
                episode.server_ip.map(|ip| ip.to_string()),
                episode.core_id.map(|id| id as i64),
                episode.start_ms,
                episode.end_ms,
                i64::from(episode.peak),
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
            "SELECT id, axis, server_ip, core_id, start_ms, end_ms, peak FROM core_warnings \
             WHERE server_ip = ?1 AND start_ms BETWEEN ?2 AND ?3 ORDER BY start_ms",
        )?;
        let rows = stmt.query_map(params![ip.to_string(), from_ms, to_ms], row_to_episode)?;
        rows.collect()
    }
}

/// Stable wire name for a warning axis.
fn axis_str(axis: WarnAxis) -> &'static str {
    match axis {
        WarnAxis::SysCpu => "sys_cpu",
        WarnAxis::MemGrowth => "mem_growth",
        WarnAxis::Unreachable => "connectivity",
    }
}

/// Parse a stored axis name, defaulting unknown values to `SysCpu` rather than failing a whole row.
fn axis_from(name: &str) -> WarnAxis {
    match name {
        "mem_growth" => WarnAxis::MemGrowth,
        "connectivity" => WarnAxis::Unreachable,
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
    })
}

#[cfg(test)]
mod tests;
