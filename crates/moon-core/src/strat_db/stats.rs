//! Lazy per-strategy-version statistics: profit and trades during the version's effective
//! window (`orders_rep` by `buydate`, with entries attributed to the version's parameters).
//!
//! A frozen aggregate cannot be taken when a version closes because of open trades,
//! replication lag, and backdated RowUpsert events. Therefore, the `version_stats` cache in
//! strategies.sqlite is populated lazily on reads and invalidated by a freshness marker:
//! `as_of` is the maximum `last_update_at` among the strategy's replica rows at calculation time.
//! Only a newer `last_update_at` advances the marker; partial upserts and removals do not guarantee
//! invalidation. A strategy with no further replica updates keeps the same marker. Versions with
//! `open_left > 0` (open trades remain in the window) and the current version are recalculated on
//! each read while the reports replica is attached; otherwise cached values or zeros are returned.
//!
//! Call only from a background executor because this module queries the SQLite replica.

use rusqlite::{Connection, OptionalExtension};

use crate::config::paths;
use crate::util::now_unix_ms_i64 as now_ms;

/// Strategy version and its statistics for the version list in the Strategies window.
#[derive(Clone, Debug)]
pub struct VersionInfo {
    /// Start of the effective period in Unix milliseconds (the version key).
    pub valid_from: i64,
    /// End of the effective period in Unix milliseconds; `None` means the current version.
    pub valid_to: Option<i64>,
    pub change_kind: String,
    pub origin: Option<String>,
    pub n_changed: i64,
    /// Trades entering the version window by `buydate` and their total profit in the quote
    /// currency from `profitbtc`. Open trades without a `closedate` do not contribute to profit.
    pub trades: i64,
    pub profit: f64,
    pub open_left: i64,
}

fn open_rw() -> Option<Connection> {
    let path = paths::strategies_db_path();
    if !path.exists() {
        return None;
    }
    let conn = Connection::open(&path).ok()?;
    let _ = conn.busy_timeout(std::time::Duration::from_secs(3));
    Some(conn)
}

/// Attach the reports replica only after its recovery preflight authorized this process.
///
/// A read-only attachment is unnecessary because writes target only `version_stats` in the main
/// file. Without authorized replica access, uncached statistics remain zero and cached values are
/// reused.
///
/// Args:
///     conn: Open strategies connection that will own the attachment.
///
/// Returns:
///     `true` after a successful attachment, otherwise `false`.
fn attach_reports(conn: &Connection) -> bool {
    if crate::db::report_recovery::ensure_access().is_err() {
        return false;
    }
    let rep = paths::reports_db_path();
    if !rep.exists() {
        return false;
    }
    let sql = format!(
        "ATTACH DATABASE '{}' AS rep",
        rep.to_string_lossy().replace('\'', "''")
    );
    match conn.execute(&sql, []) {
        Ok(_) => true,
        Err(e) => {
            log::warn!("стратегии(stats): ATTACH реплики не удался: {e}");
            false
        }
    }
}

/// Load the attached replica's offset segments for ONE core, plus the axis generation they belong to.
///
/// A version window's boundaries are stamped by THIS machine in true-UTC milliseconds, while
/// `buydate` is the CORE's own wall clock. Comparing them raw attributes every trade near a
/// boundary to the wrong strategy version — by the width of the core's offset, so a UTC-4 core
/// mis-attributes four hours of trades at every version change.
///
/// Reads through the `rep` attachment because that is where the replica lives from this database's
/// point of view. An absent or unreadable table yields the identity axis, which reproduces exactly
/// the behaviour this function had before offsets existed; the generation returned alongside is
/// what makes a later measurement invalidate the cache written under it.
///
/// Args:
///     conn: Strategies connection with the reports replica already attached as `rep`.
///     core_uid: The core whose version windows are being converted.
///
/// Returns:
///     The axis carrying only that core's segments, and the replica's current axis generation.
fn core_axis(conn: &Connection, core_uid: u64) -> (crate::db::ReportAxis, i64) {
    let generation: i64 = conn
        .query_row(
            "SELECT CAST(value AS INTEGER) FROM rep.app_meta WHERE key=?1",
            [crate::db::AXIS_GENERATION_KEY],
            |row| row.get(0),
        )
        .unwrap_or(0);
    let mut segments = Vec::new();
    if let Ok(mut stmt) = conn.prepare(
        "SELECT from_utc, offset_secs FROM rep.core_time_offset
         WHERE core_uid=?1 ORDER BY from_utc",
    ) {
        if let Ok(rows) = stmt.query_map([core_uid as i64], |row| {
            Ok(crate::db::OffsetSegment {
                from_utc: row.get(0)?,
                offset_secs: row.get::<_, i64>(1)? as i32,
            })
        }) {
            segments.extend(rows.flatten());
        }
    }
    if segments.is_empty() {
        return (crate::db::ReportAxis::default(), generation);
    }
    let measured = std::collections::HashMap::from([(core_uid, segments)]);
    (
        crate::db::ReportAxis::from_measured(measured, chrono_tz::UTC),
        generation,
    )
}

/// Freshness marker: the maximum `last_update_at` among the strategy's replica rows, or 0 when
/// rows or the column are unavailable. It changes only when that maximum advances; partial upserts
/// and removals do not guarantee a change.
fn max_last_update(conn: &Connection, core_uid: i64, sid: i64) -> i64 {
    conn.query_row(
        "SELECT COALESCE(MAX(last_update_at),0) FROM rep.orders_rep
         WHERE core_uid=?1 AND strategyid=?2",
        rusqlite::params![core_uid, sid],
        |r| r.get(0),
    )
    .unwrap_or(0)
}

/// Returns strategy versions newest-first with statistics, recalculating and updating stale cache entries.
/// An empty list means the database or strategy is unavailable.
pub fn versions_with_stats(core_uid: u64, strategy_id: i64) -> Vec<VersionInfo> {
    let Some(conn) = open_rw() else {
        return Vec::new();
    };
    let uid = core_uid as i64;
    let has_rep = attach_reports(&conn);
    let _ = conn.execute(
        "CREATE TABLE IF NOT EXISTS version_stats (
            core_uid    INTEGER NOT NULL,
            strategy_id INTEGER NOT NULL,
            valid_from  INTEGER NOT NULL,
            trades      INTEGER NOT NULL DEFAULT 0,
            profit      REAL NOT NULL DEFAULT 0,
            open_left   INTEGER NOT NULL DEFAULT 0,
            as_of       INTEGER NOT NULL DEFAULT 0,
            computed_ms INTEGER NOT NULL DEFAULT 0,
            axis_gen    INTEGER NOT NULL DEFAULT 0,
            PRIMARY KEY (core_uid, strategy_id, valid_from))",
        [],
    );
    // A cache written before offsets existed has no `axis_gen` column at all, and
    // `CREATE TABLE IF NOT EXISTS` above will not add one. The default of 0 is what makes the
    // migration self-correcting: every pre-existing row reads as generation 0, so the first read
    // after any measurement finds it stale and recomputes. A duplicate-column error here means the
    // column is already present, which is the other correct outcome.
    let _ = conn.execute(
        "ALTER TABLE version_stats ADD COLUMN axis_gen INTEGER NOT NULL DEFAULT 0",
        [],
    );
    let (axis, axis_gen) = if has_rep {
        core_axis(&conn, core_uid)
    } else {
        (crate::db::ReportAxis::default(), 0)
    };

    let mut out: Vec<VersionInfo> = Vec::new();
    {
        let Ok(mut stmt) = conn.prepare(
            "SELECT v.valid_from, v.valid_to, v.change_kind, v.origin, v.n_changed,
                    s.trades, s.profit, s.open_left, s.as_of, s.axis_gen
             FROM strategy_versions v
             LEFT JOIN version_stats s
               ON s.core_uid=v.core_uid AND s.strategy_id=v.strategy_id
              AND s.valid_from=v.valid_from
             WHERE v.core_uid=?1 AND v.strategy_id=?2
             ORDER BY v.valid_from DESC",
        ) else {
            return Vec::new();
        };
        let rows = stmt.query_map(rusqlite::params![uid, strategy_id], |r| {
            Ok((
                VersionInfo {
                    valid_from: r.get(0)?,
                    valid_to: r.get(1)?,
                    change_kind: r.get(2)?,
                    origin: r.get(3)?,
                    n_changed: r.get(4)?,
                    trades: r.get::<_, Option<i64>>(5)?.unwrap_or(0),
                    profit: r.get::<_, Option<f64>>(6)?.unwrap_or(0.0),
                    open_left: r.get::<_, Option<i64>>(7)?.unwrap_or(0),
                },
                r.get::<_, Option<i64>>(8)?, // Cached as_of value (None means no cache entry).
                r.get::<_, Option<i64>>(9)?, // Axis generation the cached figures were computed under.
            ))
        });
        let Ok(rows) = rows else { return Vec::new() };

        let max_lu = if has_rep {
            max_last_update(&conn, uid, strategy_id)
        } else {
            0
        };
        for row in rows.flatten() {
            let (mut info, as_of, cached_axis_gen) = row;
            // An adopted offset changes no report row and no `last_update_at`, so without the
            // generation in this test a CLOSED version's cached figures would stay "fresh" forever
            // and the corrected attribution below would never run for it.
            let stale = as_of != Some(max_lu)
                || info.open_left > 0
                || info.valid_to.is_none()
                || cached_axis_gen != Some(axis_gen);
            if stale && has_rep {
                // Replica buydate values use Unix seconds, while version boundaries use milliseconds.
                // The BOUNDARIES move onto the core's clock, never the column: `buydate` stays
                // bare so the replica's index over it is still usable, and this query is already
                // scoped to one core, so the conversion is a single offset rather than a group.
                let from_s = axis.from_utc(info.valid_from / 1000, core_uid);
                let to_s = info.valid_to.map(|t| axis.from_utc(t / 1000, core_uid));
                let agg: Option<(i64, f64, i64)> = conn
                    .query_row(
                        "SELECT COUNT(*),
                                COALESCE(SUM(CASE WHEN closedate>0 THEN profitbtc END),0),
                                COALESCE(SUM(closedate IS NULL OR closedate<=0),0)
                         FROM rep.orders_rep
                         WHERE core_uid=?1 AND strategyid=?2
                           AND buydate>=?3 AND (?4 IS NULL OR buydate<?4)",
                        rusqlite::params![uid, strategy_id, from_s, to_s],
                        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                    )
                    .optional()
                    .unwrap_or(None);
                if let Some((trades, profit, open_left)) = agg {
                    info.trades = trades;
                    info.profit = profit;
                    info.open_left = open_left;
                    let _ = conn.execute(
                        "INSERT INTO version_stats
                            (core_uid, strategy_id, valid_from, trades, profit,
                             open_left, as_of, computed_ms, axis_gen)
                         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)
                         ON CONFLICT (core_uid, strategy_id, valid_from) DO UPDATE SET
                            trades=excluded.trades, profit=excluded.profit,
                            open_left=excluded.open_left, as_of=excluded.as_of,
                            computed_ms=excluded.computed_ms, axis_gen=excluded.axis_gen",
                        rusqlite::params![
                            uid,
                            strategy_id,
                            info.valid_from,
                            info.trades,
                            info.profit,
                            info.open_left,
                            max_lu,
                            now_ms(),
                            axis_gen,
                        ],
                    );
                }
            }
            out.push(info);
        }
    }
    out
}

/// Version contents for panels in the Strategies window: displayable fields and the diff from
/// the previous version.
pub struct VersionView {
    /// Fields from the version's `raw_json`, excluding `__` metadata, as display strings formatted
    /// like live strategies' `fmt_field` values (Yes/No and compact numbers).
    pub fields: Vec<(String, String)>,
    /// Fields changed in this version, mapping each name to its old display value. An empty value
    /// can mean the old field was missing, null, or an empty string. Empty for creation events or
    /// versions without a diff.
    pub changed: Vec<(String, String)>,
}

/// Returns a version's fields and diff, or `None` if it is absent, unavailable, unreadable, or invalid.
pub fn version_view(core_uid: u64, strategy_id: i64, valid_from: i64) -> Option<VersionView> {
    let conn = open_rw()?;
    let (raw, changed_raw): (String, Option<String>) = conn
        .query_row(
            "SELECT raw_json, changed_json FROM strategy_versions
             WHERE core_uid=?1 AND strategy_id=?2 AND valid_from=?3",
            rusqlite::params![core_uid as i64, strategy_id, valid_from],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()
        .ok()??;
    let map: serde_json::Map<String, serde_json::Value> = serde_json::from_str(&raw).ok()?;
    let fields = map
        .into_iter()
        .filter(|(k, _)| !k.starts_with("__"))
        .map(|(k, v)| (k, json_display(&v)))
        .collect();
    // Convert {"Field":{"old":…,"new":…}} diffs into (name, old display value) pairs.
    let mut changed = Vec::new();
    if let Some(cj) = changed_raw {
        if let Ok(serde_json::Value::Object(m)) = serde_json::from_str(&cj) {
            for (name, pair) in m {
                let old = pair
                    .get("old")
                    .filter(|v| !v.is_null())
                    .map(json_display)
                    .unwrap_or_default();
                changed.push((name, old));
            }
        }
    }
    Some(VersionView { fields, changed })
}

/// Converts a field's JSON value to a display string, mirroring `feed::strategies::fmt_field`.
fn json_display(v: &serde_json::Value) -> String {
    use serde_json::Value as J;
    match v {
        J::Bool(b) => if *b { "Yes" } else { "No" }.to_string(),
        J::Number(n) => {
            if let Some(i) = n.as_i64() {
                i.to_string()
            } else if let Some(u) = n.as_u64() {
                u.to_string()
            } else {
                crate::util::fmt::compact(n.as_f64().unwrap_or(0.0), 6)
            }
        }
        J::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// Strategy head row from strategies.sqlite, used to display deleted strategies that are no
/// longer present in the core's live store.
#[derive(Clone, Debug, PartialEq)]
pub struct HeadRow {
    pub core_uid: u64,
    pub strategy_id: i64,
    pub name: String,
    pub kind: String,
    pub kind_ordinal: u8,
    pub folder_path: String,
    pub is_short: bool,
}

fn head_from_row(r: &rusqlite::Row) -> rusqlite::Result<HeadRow> {
    Ok(HeadRow {
        core_uid: r.get::<_, i64>(0)? as u64,
        strategy_id: r.get(1)?,
        name: r.get(2)?,
        kind: r.get(3)?,
        kind_ordinal: r.get::<_, i64>(4)? as u8,
        folder_path: r.get(5)?,
        is_short: r.get::<_, i64>(6)? != 0,
    })
}

const HEAD_COLS: &str = "core_uid, strategy_id, name, kind, kind_ordinal, folder_path, is_short";

/// Returns all strategies deleted on their servers (`head.deleted=1`) for the Deleted folder in
/// the Strategies window tree. Their version history remains available.
pub fn deleted_heads() -> Vec<HeadRow> {
    let Some(conn) = super::open_reader() else {
        return Vec::new();
    };
    let Ok(mut stmt) = conn.prepare(&format!(
        "SELECT {HEAD_COLS} FROM strategies WHERE deleted=1 ORDER BY core_uid, name"
    )) else {
        return Vec::new();
    };
    stmt.query_map([], |r| head_from_row(r))
        .map(|rows| rows.flatten().collect())
        .unwrap_or_default()
}

/// Returns display strings for the latest strategy version's fields, used to restore a deleted
/// strategy with `RestoreStrategy`, or `None` if no version exists or its data is unavailable,
/// unreadable, or invalid.
pub fn latest_version_fields(core_uid: u64, strategy_id: i64) -> Option<Vec<(String, String)>> {
    let conn = open_rw()?;
    let vf: i64 = conn
        .query_row(
            "SELECT MAX(valid_from) FROM strategy_versions
             WHERE core_uid=?1 AND strategy_id=?2",
            rusqlite::params![core_uid as i64, strategy_id],
            |r| r.get::<_, Option<i64>>(0),
        )
        .ok()
        .flatten()?;
    version_view(core_uid, strategy_id, vf).map(|v| v.fields)
}

/// Returns the head of one live or deleted strategy as the basis for a synthetic row when the
/// strategy is absent from the live store.
pub fn head_row(core_uid: u64, strategy_id: i64) -> Option<HeadRow> {
    let conn = super::open_reader()?;
    conn.query_row(
        &format!("SELECT {HEAD_COLS} FROM strategies WHERE core_uid=?1 AND strategy_id=?2"),
        rusqlite::params![core_uid as i64, strategy_id],
        |r| head_from_row(r),
    )
    .optional()
    .ok()
    .flatten()
}

/// Format Unix milliseconds as `DD.MM` in a selected display zone.
///
/// Args:
///     ms: Instant to format as Unix milliseconds.
///     zone: IANA display zone selected by the terminal clock.
///
/// Returns:
///     Compact civil date, or an empty string for an invalid timestamp.
pub fn short_date(ms: i64, zone: chrono_tz::Tz) -> String {
    crate::util::display_time::at_millis(ms, zone)
        .map(|dt| dt.format("%d.%m").to_string())
        .unwrap_or_default()
}
