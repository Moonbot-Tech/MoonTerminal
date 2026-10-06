use super::*;

/// One durable report-side change after its source mutation committed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct OutboxEvent {
    /// Monotonic report-database sequence.
    pub seq: i64,
    /// Typed or legacy source partition.
    pub source: TradeSource,
    /// Runtime core identity.
    pub core_uid: i64,
    /// Row identity for row/delete events; zero for core-wide events.
    pub row_id: i64,
    /// Work required by the valuation worker.
    pub action: OutboxAction,
}

/// Durable valuation work staged by the report writer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum OutboxAction {
    /// Re-read and prepare one current committed report row.
    Row,
    /// Remove one prepared value after a hard report delete.
    Delete,
    /// Drop a typed core partition before scanning its recreated database.
    RescanCore,
    /// Drop a legacy core partition after typed synchronization purges it.
    PurgeLegacy,
}

impl OutboxAction {
    /// Stable integer persisted in the report outbox.
    ///
    /// Returns:
    ///     Database representation of this action.
    const fn code(self) -> i64 {
        match self {
            Self::Row => 0,
            Self::Delete => 1,
            Self::RescanCore => 2,
            Self::PurgeLegacy => 3,
        }
    }

    /// Decode one persisted outbox action.
    ///
    /// Args:
    ///     value: Integer stored in the report outbox.
    ///
    /// Returns:
    ///     Known action, or a SQLite conversion error for corrupted data.
    fn from_code(value: i64) -> rusqlite::Result<Self> {
        match value {
            0 => Ok(Self::Row),
            1 => Ok(Self::Delete),
            2 => Ok(Self::RescanCore),
            3 => Ok(Self::PurgeLegacy),
            value => Err(rusqlite::Error::IntegralValueOutOfRange(4, value)),
        }
    }
}

/// Physical report source owning one stable row identity.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TradeSource {
    /// Typed replica row keyed by `(core_uid, newrecid)`.
    Typed,
    /// Legacy report row keyed by `(core_uid, db_id)`.
    Legacy,
}

impl TradeSource {
    /// Stable integer stored in `valuation.sqlite` and the report outbox.
    ///
    /// Returns:
    ///     Zero for typed rows and one for legacy rows.
    pub(crate) const fn code(self) -> i64 {
        match self {
            Self::Typed => 0,
            Self::Legacy => 1,
        }
    }

    /// Decode one persisted source partition.
    ///
    /// Args:
    ///     value: Integer stored in the report outbox or valuation table.
    ///
    /// Returns:
    ///     Typed or legacy source, or a SQLite conversion error for corrupted data.
    fn from_code(value: i64) -> rusqlite::Result<Self> {
        match value {
            0 => Ok(Self::Typed),
            1 => Ok(Self::Legacy),
            value => Err(rusqlite::Error::IntegralValueOutOfRange(1, value)),
        }
    }
}

/// Create the durable report-side valuation outbox.
///
/// Args:
///     conn: Sole report-writer connection during schema initialization.
///
/// `seq` is the table's rowid, so it needs no separate index; an older build's redundant one is
/// dropped here.
///
/// Returns:
///     SQLite success after the table exists.
pub(in crate::db) fn init_report_outbox(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(&format!(
        "CREATE TABLE IF NOT EXISTS {OUTBOX_TABLE} (
             seq INTEGER PRIMARY KEY AUTOINCREMENT,
             source_kind INTEGER NOT NULL,
             core_uid INTEGER NOT NULL,
             row_id INTEGER NOT NULL,
             action INTEGER NOT NULL
         );
         DROP INDEX IF EXISTS idx_valuation_outbox_seq;"
    ))
}

/// Stage one committed row for input-complete valuation.
///
/// Args:
///     conn: Active report-writer transaction.
///     source: Typed or legacy source partition.
///     core_uid: Runtime core identity.
///     row_id: Physical source row identity.
///
/// Returns:
///     SQLite success after the event is durable in the same transaction.
pub(in crate::db) fn stage_row(
    conn: &Connection,
    source: TradeSource,
    core_uid: u64,
    row_id: i64,
) -> rusqlite::Result<()> {
    stage_outbox(conn, source, core_uid, row_id, OutboxAction::Row)
}

/// Stage one hard report-row delete.
///
/// Args:
///     conn: Active report-writer transaction.
///     source: Typed or legacy source partition.
///     core_uid: Runtime core identity.
///     row_id: Physical source row identity.
///
/// Returns:
///     SQLite success after the event is durable in the same transaction.
pub(in crate::db) fn stage_delete(
    conn: &Connection,
    source: TradeSource,
    core_uid: u64,
    row_id: i64,
) -> rusqlite::Result<()> {
    stage_outbox(conn, source, core_uid, row_id, OutboxAction::Delete)
}

/// Stage invalidation of one recreated typed report database.
///
/// Args:
///     conn: Active report-writer transaction.
///     core_uid: Runtime core identity whose typed rows were replaced.
///
/// Returns:
///     SQLite success after the core-wide event is durable.
pub(in crate::db) fn stage_rescan_core(conn: &Connection, core_uid: u64) -> rusqlite::Result<()> {
    stage_outbox(
        conn,
        TradeSource::Typed,
        core_uid,
        0,
        OutboxAction::RescanCore,
    )
}

/// Stage removal of one legacy partition after typed synchronization purges it.
///
/// Args:
///     conn: Active report-writer transaction.
///     core_uid: Runtime core identity whose legacy rows were purged.
///
/// Returns:
///     SQLite success after the core-wide event is durable.
pub(in crate::db) fn stage_legacy_purge(conn: &Connection, core_uid: u64) -> rusqlite::Result<()> {
    stage_outbox(
        conn,
        TradeSource::Legacy,
        core_uid,
        0,
        OutboxAction::PurgeLegacy,
    )
}

/// Insert one durable outbox event inside the current report transaction.
///
/// Args:
///     conn: Active report-writer transaction.
///     source: Typed or legacy source partition.
///     core_uid: Runtime core identity.
///     row_id: Physical row identity or zero for a partition event.
///     action: Required worker operation.
///
/// Returns:
///     SQLite success after the event is inserted.
fn stage_outbox(
    conn: &Connection,
    source: TradeSource,
    core_uid: u64,
    row_id: i64,
    action: OutboxAction,
) -> rusqlite::Result<()> {
    // Every process that writes reports runs the worker that drains this: the terminal and both
    // station profiles. Rows a light station replicated before it valued reports were staged
    // nowhere; the worker's startup reconciliation walk values those.
    conn.execute(
        &format!(
            "INSERT INTO {OUTBOX_TABLE}(source_kind, core_uid, row_id, action)
             VALUES (?1,?2,?3,?4)"
        ),
        params![source.code(), core_uid as i64, row_id, action.code()],
    )?;
    Ok(())
}

/// Delete one contiguous acknowledged outbox prefix through the sole report writer.
///
/// Args:
///     conn: Active report-writer transaction.
///     through_seq: Highest sequence safely reflected in `valuation.sqlite`.
///
/// Returns:
///     SQLite success after old events are removed.
pub(in crate::db) fn ack_outbox(conn: &Connection, through_seq: i64) -> rusqlite::Result<()> {
    conn.execute(
        &format!("DELETE FROM {OUTBOX_TABLE} WHERE seq <= ?1"),
        [through_seq],
    )?;
    Ok(())
}

/// Read one ordered durable outbox batch from a report reader.
///
/// Args:
///     conn: Open report reader or pinned snapshot.
///     limit: Maximum number of events to return.
///
/// Returns:
///     Ordered events, or a classified report-read failure.
pub(crate) fn read_outbox(conn: &Connection, limit: usize) -> ReadResult<Vec<OutboxEvent>> {
    let sql = format!(
        "SELECT seq, source_kind, core_uid, row_id, action
         FROM {OUTBOX_TABLE} ORDER BY seq LIMIT ?1"
    );
    let mut stmt = conn
        .prepare(&sql)
        .map_err(|error| read_fail::read_fail("valuation: read outbox", error))?;
    let rows = stmt
        .query_map([limit as i64], |row| {
            Ok(OutboxEvent {
                seq: row.get(0)?,
                source: TradeSource::from_code(row.get(1)?)?,
                core_uid: row.get(2)?,
                row_id: row.get(3)?,
                action: OutboxAction::from_code(row.get(4)?)?,
            })
        })
        .map_err(|error| read_fail::read_fail("valuation: query outbox", error))?;
    let mut events = Vec::new();
    for row in rows {
        events.push(row.map_err(|error| read_fail::read_fail("valuation: outbox row", error))?);
    }
    Ok(events)
}
