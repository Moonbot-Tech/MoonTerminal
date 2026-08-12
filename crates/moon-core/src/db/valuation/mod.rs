//! Persistent historical quote-to-USDT valuation for report rows.
//!
//! `valuation.sqlite` is a derived cache, separate from the recoverable report replica. It stores
//! immutable closed-minute spot rates and prepared trade values. Readers still compare every
//! stored input with the current report row, so an upsert under the same report identity cannot
//! publish a stale conversion while the worker is catching up.

mod current;
mod health;
mod provider;
mod worker;

#[cfg(test)]
mod tests;

// `FaultCause` stays crate-private: it is the worker's builder, and a public constructor would let
// anything mint a fault the worker never reported — the opposite of "health is published, not
// derived". `FailureKind` is public only because `ValuationFault.kind` is a public field, so a
// consumer could otherwise read the class without being able to name its type.
pub(crate) use current::current_rate_sql;
use current::publish_current_rates;
pub use current::{pin_current_rates, RatePin, ValuationMode};
pub(crate) use current::{CurrentRate, CurrentRates, FRESHNESS_MS};
pub(crate) use health::FaultCause;
pub use health::{FailureKind, StageHealth, ValuationFault, ValuationStage, ValuationStatus};
pub(crate) use provider::{HttpSpotRateSource, SpotRateSource};
pub use worker::{spawn_worker, ValuationHandle};

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::RwLock;
use std::time::Duration;

use rusqlite::{params, Connection, OpenFlags, OptionalExtension};

use super::read_fail::{self, ReadResult};

/// Schema and routing version included in every cached rate and prepared valuation.
///
/// Incrementing this value invalidates old rows without destructive migration when provider
/// routing or conversion semantics change.
pub const ALGORITHM_VERSION: i64 = 1;

/// Attached SQLite schema name used by report and Analytics queries.
pub(crate) const SCHEMA: &str = "valuation";

/// Durable report-side change queue consumed by the valuation worker.
const OUTBOX_TABLE: &str = "valuation_outbox";

/// The canonical derived cache is available to new report readers.
const HEALTHY: u8 = 1;

/// The canonical derived cache must not be attached until recovery succeeds.
const UNHEALTHY: u8 = 2;

/// Process-wide health of the one canonical valuation cache.
static CACHE_HEALTH: AtomicU8 = AtomicU8::new(HEALTHY);

/// Serializes file-family replacement against attachment proof and validation.
static CACHE_LIFECYCLE: RwLock<()> = RwLock::new(());

/// Process-wide serialization for tests that change canonical cache health.
#[cfg(test)]
static TEST_HEALTH_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Test guard that restores cache health after a fixture changes global state.
#[cfg(test)]
pub(super) struct TestHealthGuard {
    /// Held solely to serialize global cache-health fixtures.
    _lock: std::sync::MutexGuard<'static, ()>,
}

#[cfg(test)]
impl Drop for TestHealthGuard {
    /// Restore the healthy default before another parallel test can attach its fixture.
    fn drop(&mut self) {
        CACHE_HEALTH.store(HEALTHY, Ordering::Release);
    }
}

/// Serialize one fixture that attaches valuation storage and reset its health baseline.
///
/// Returns:
///     Guard holding the test-only global health lock until fixture teardown.
#[cfg(test)]
pub(super) fn test_health_guard() -> TestHealthGuard {
    let guard = TEST_HEALTH_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    CACHE_HEALTH.store(HEALTHY, Ordering::Release);
    TestHealthGuard { _lock: guard }
}

/// SQL fragments used only by the per-row Report reader.
///
/// Aggregates never need these: they sum [`CoverageSql::profit_usdt`] instead. A row-level reader
/// wants the applied rate and its provenance beside the converted profit, so a user can check one
/// trade rather than trust a total.
pub(crate) struct PerRowSql {
    /// The `v` prepared-value join followed by the `ra` ready-rate provenance join.
    ///
    /// The row reader needs provenance but never counts permanent misses, so it omits the `mr`
    /// join used by [`CoverageSql::joins`]. The joins are complete and dependency-ordered because
    /// `ra` reads `v.rate_minute_utc`.
    pub joins: String,
    /// USDT paid for one quote unit, `1.0` on an identity row.
    pub rate: String,
    /// Human-readable provenance of that rate, NULL while the row is uncovered.
    pub source: String,
}

/// SQL fragments for aggregate valuation coverage and per-row Report values.
pub(crate) struct CoverageSql {
    /// The aggregate reader's `v` prepared-value and `mr` permanent-miss joins.
    ///
    /// The `mr` alias supplies the `unavailable` count; rate provenance is unused here and remains
    /// exclusive to [`PerRowSql::joins`].
    pub joins: String,
    /// One for every row carrying a known quote identity.
    pub eligible: String,
    /// One only for identity-USDT or an input-matching prepared value.
    pub valued: String,
    /// One only for a cached permanent route/candle miss without a prepared value.
    pub unavailable: String,
    /// Complete-row USDT profit expression, NULL while uncovered.
    ///
    /// Also the row-level converted profit: ONE expression, so a row and the total it belongs to
    /// can never disagree about the same number.
    pub profit_usdt: String,
    /// Complete-row USDT spend expression, NULL when absent or uncovered.
    pub spent_usdt: String,
    /// Row-level projections and the joins that back them.
    pub per_row: PerRowSql,
}

impl CoverageSql {
    /// Build the six grouped aggregate columns shared by Report and Analytics totals.
    ///
    /// Returns:
    ///     SQL expressions ordered for [`CoverageAggregate::add_row`].
    pub(crate) fn aggregate_columns(&self) -> String {
        format!(
            "COALESCE(SUM(CASE WHEN {eligible} THEN 1 ELSE 0 END),0),
             COALESCE(SUM(CASE WHEN {valued} THEN 1 ELSE 0 END),0),
             COALESCE(SUM(CASE WHEN {unavailable} THEN 1 ELSE 0 END),0),
             COALESCE(SUM({profit_usdt}),0.0),
             COALESCE(SUM({spent_usdt}),0.0),
             COALESCE(SUM(CASE WHEN {valued} AND ({spent_usdt}) IS NOT NULL
                               THEN 1 ELSE 0 END),0)",
            eligible = self.eligible,
            valued = self.valued,
            unavailable = self.unavailable,
            profit_usdt = self.profit_usdt,
            spent_usdt = self.spent_usdt,
        )
    }
}

/// Mutable aggregation of coverage rows across typed and legacy report sources.
#[derive(Default)]
pub(crate) struct CoverageAggregate {
    eligible: i64,
    valued: i64,
    unavailable: i64,
    profit_usdt: f64,
    spent_usdt: f64,
    spent_rows: i64,
}

impl CoverageAggregate {
    /// Add one physical source's coverage aggregate.
    ///
    /// Args:
    ///     eligible: Known-quote rows.
    ///     valued: Identity or prepared rows.
    ///     unavailable: Cached permanent misses.
    ///     profit_usdt: Sum over valued rows only.
    ///     spent_usdt: Sum over valued rows carrying numeric spend.
    ///     spent_rows: Valued rows carrying numeric spend.
    pub(crate) fn add(
        &mut self,
        eligible: i64,
        valued: i64,
        unavailable: i64,
        profit_usdt: f64,
        spent_usdt: f64,
        spent_rows: i64,
    ) {
        self.eligible += eligible;
        self.valued += valued;
        self.unavailable += unavailable;
        self.profit_usdt += profit_usdt;
        self.spent_usdt += spent_usdt;
        self.spent_rows += spent_rows;
    }

    /// Decode and add the shared six-column coverage suffix from one grouped SQLite row.
    ///
    /// Args:
    ///     row: Current grouped aggregate row.
    ///     offset: Index of the first coverage column.
    ///
    /// Returns:
    ///     SQLite success after every typed aggregate was decoded.
    pub(crate) fn add_row(&mut self, row: &rusqlite::Row, offset: usize) -> rusqlite::Result<()> {
        self.add(
            row.get(offset)?,
            row.get(offset + 1)?,
            row.get(offset + 2)?,
            row.get(offset + 3)?,
            row.get(offset + 4)?,
            row.get(offset + 5)?,
        );
        Ok(())
    }

    /// Finish complete-only public coverage without exposing a partial monetary sum.
    ///
    /// Returns:
    ///     Coverage whose USDT total exists only when every eligible row is valued.
    pub(crate) fn finish(self) -> crate::db::ValuationCoverage {
        let complete = self.eligible == self.valued && self.unavailable == 0;
        crate::db::ValuationCoverage {
            eligible_orders: self.eligible,
            valued_orders: self.valued,
            unavailable_orders: self.unavailable,
            usdt: complete.then_some(crate::db::UsdtTotal {
                profit: self.profit_usdt,
                spent: (self.spent_rows == self.valued).then_some(self.spent_usdt),
            }),
        }
    }
}

/// The row-level guards both conversions share.
pub(super) struct SourcePredicates {
    /// One for a row whose quote currency is a trusted persisted ordinal.
    pub quote_known: String,
    /// One for a row whose profit is numeric.
    pub numeric_profit: String,
    /// The row's spend, or NULL when absent or non-numeric.
    pub spent_value: String,
}

/// Build the guards that decide which rows either conversion may touch at all.
///
/// Shared by both SQL builders on purpose. The trusted-ordinal contract and the storage-class
/// checks are what "eligible" MEANS, and stating them twice is how the two modes would come to
/// disagree about which rows count — silently, with both still compiling and both still passing.
///
/// Args:
///     alias: Qualified report-row alias used by the caller.
///     columns: Discovered physical source columns.
///
/// Returns:
///     Predicates that name only columns the source actually has.
pub(super) fn source_predicates(
    alias: &str,
    columns: &std::collections::HashSet<String>,
) -> SourcePredicates {
    // SQLite resolves every column reference at prepare time, unreachable arms included, so an
    // absent column collapses to a constant rather than being named.
    SourcePredicates {
        quote_known: if columns.contains("basecurrency") {
            // The effective expression already rejects every non-integer storage class, so the
            // range check alone decides whether the identity is one this build knows.
            format!(
                "({quote}) BETWEEN 0 AND 20",
                quote = super::quote::effective_ordinal_expr(alias, columns)
            )
        } else {
            "0".to_string()
        },
        numeric_profit: if columns.contains("profitbtc") {
            format!("typeof({alias}.profitbtc) IN ('integer','real')")
        } else {
            "0".to_string()
        },
        spent_value: if columns.contains("spentbtc") {
            format!(
                "CASE WHEN typeof({alias}.spentbtc) IN ('integer','real') THEN {alias}.spentbtc END"
            )
        } else {
            "NULL".to_string()
        },
    }
}

/// Build input-matching valuation SQL for one physical report source.
///
/// Args:
///     alias: Qualified report-row alias used by the caller.
///     columns: Discovered physical source columns.
///     source: Typed or legacy source partition.
///
/// Returns:
///     Coverage fragments; sources without stable identity retain identity-USDT coverage only.
pub(crate) fn coverage_sql(
    alias: &str,
    columns: &std::collections::HashSet<String>,
    source: TradeSource,
) -> CoverageSql {
    let id_column = match source {
        TradeSource::Typed if columns.contains("newrecid") => Some("newrecid"),
        TradeSource::Legacy if columns.contains("db_id") => Some("db_id"),
        TradeSource::Typed | TradeSource::Legacy => None,
    };
    let has_closedate = columns.contains("closedate");
    let has_quote = columns.contains("basecurrency");
    let has_profit = columns.contains("profitbtc");
    // Every quote reference below goes through this ONE expression: the cache is keyed by quote
    // ordinal, so a join that matched the raw column while the projection valued the effective one
    // would look up the rate of a currency the row is not denominated in.
    let quote = super::quote::effective_ordinal_expr(alias, columns);
    let SourcePredicates {
        quote_known,
        numeric_profit,
        spent_value,
    } = source_predicates(alias, columns);
    let date_valid = if has_closedate {
        format!("typeof({alias}.closedate)='integer' AND {alias}.closedate>0")
    } else {
        "0".to_string()
    };
    let input_match = if let Some(id_column) =
        id_column.filter(|_| columns.contains("core_uid") && has_required_trade_inputs(columns))
    {
        format!(
            "v.source_kind={source_kind}
             AND typeof({alias}.core_uid)='integer'
             AND typeof({alias}.{id_column})='integer'
             AND v.core_uid={alias}.core_uid AND v.row_id={alias}.{id_column}
             AND v.algorithm_version={algorithm_version}
             AND v.closedate={alias}.closedate AND v.quote_ordinal=({quote})
             AND v.profit_quote={alias}.profitbtc AND v.spent_quote IS {spent_value}",
            source_kind = source.code(),
            algorithm_version = ALGORITHM_VERSION,
        )
    } else {
        "0".to_string()
    };
    let rate_match = if has_closedate && has_quote {
        format!(
            "mr.algorithm_version={algorithm_version}
             AND mr.quote_ordinal=({quote})
             AND mr.minute_utc=({alias}.closedate/60)*60
             AND mr.status=1",
            algorithm_version = ALGORITHM_VERSION,
        )
    } else {
        "0".to_string()
    };
    let eligible = format!("({quote_known})");
    let identity = if has_quote {
        format!(
            "({eligible} AND {numeric_profit} AND ({quote})={usdt})",
            usdt = super::QuoteCurrency::usdt().ordinal()
        )
    } else {
        "0".to_string()
    };
    let prepared =
        format!("({eligible} AND {numeric_profit} AND {date_valid} AND v.row_id IS NOT NULL)");
    let valued = format!("({identity} OR {prepared})");
    let unavailable = format!(
        "({eligible} AND {numeric_profit} AND NOT {identity} AND {date_valid}
          AND v.row_id IS NULL AND mr.minute_utc IS NOT NULL)"
    );
    let profit_usdt = if has_profit {
        format!(
            "CASE WHEN {identity} THEN {alias}.profitbtc
                  WHEN {prepared} THEN v.profit_usdt END"
        )
    } else {
        "NULL".to_string()
    };
    let spent_usdt = format!(
        "CASE WHEN {identity} THEN {spent_value}
              WHEN {prepared} THEN v.spent_usdt END"
    );
    // An identity row stores no `rates` row at all — `resolve_rate_batch` synthesizes the USDT
    // identity in memory — so every per-row expression must answer for it before consulting the
    // cache. Treating a NULL provider as "not valued" would blank the column on most trades.
    let rate = format!("CASE WHEN {identity} THEN 1.0 WHEN {prepared} THEN v.rate_usdt END");
    // `status=0` is the ready-rate partition. The `mr` join above deliberately matches `status=1`,
    // the permanent-miss partition, so it can never supply provenance for a valued row. Without a
    // quote column there is nothing to join on, and the source expression must not name `ra` at
    // all: SQLite resolves every column reference at prepare time, unreachable arms included.
    let (rate_join, source) = if has_quote {
        (
            format!(
                " LEFT JOIN valuation.rates ra
                    ON ra.algorithm_version={algorithm_version}
                   AND ra.quote_ordinal=({quote})
                   AND ra.minute_utc=v.rate_minute_utc AND ra.status=0",
                algorithm_version = ALGORITHM_VERSION,
            ),
            format!(
                "CASE WHEN {identity} THEN 'identity'
                      WHEN {prepared} THEN COALESCE(
                          ra.provider || ' ' || ra.symbol
                          || CASE ra.orientation WHEN {inverse} THEN ' inv' ELSE '' END,
                          'cached') END",
                inverse = RateOrientation::Inverse.code(),
            ),
        )
    } else {
        (String::new(), "NULL".to_string())
    };
    let value_join = format!(" LEFT JOIN valuation.trade_values v ON {input_match}");
    // `mr` serves `unavailable` and nothing else, so a row query that never counts must not pay a
    // probe into it per matching row; `ra` serves provenance only, so an aggregate must not either.
    let miss_join = format!(" LEFT JOIN valuation.rates mr ON {rate_match}");
    CoverageSql {
        joins: format!("{value_join}{miss_join}"),
        eligible,
        valued,
        unavailable,
        per_row: PerRowSql {
            // `rate_join` resolves `v.rate_minute_utc`, so it can only follow the value join.
            joins: format!("{value_join}{rate_join}"),
            rate,
            source,
        },
        profit_usdt,
        spent_usdt,
    }
}

/// Build valuation SQL for one physical report source under the requested mode.
///
/// The one seam between the two conversions. Every reader goes through here, so a mode cannot
/// reach one query path and miss another — which would show a row and the total it belongs to
/// converted by different rules.
///
/// Only the historical mode can be withheld. Its rows live in the attached derived cache, so a
/// detached or corrupt cache leaves nothing to project; current rates live in memory and stay
/// available whatever the cache is doing, which is also why they need no DETACH-and-retry path.
///
/// Args:
///     mode: Historical per-trade rates, or the latest known rates.
///     attached: Whether the derived valuation cache may be joined.
///     alias: Qualified report-row alias used by the caller.
///     columns: Discovered physical source columns.
///     source: Typed or legacy source partition.
///
/// Returns:
///     Coverage fragments in the shape both modes share, or `None` when the historical cache is
///     unavailable and the caller must fall back to native money.
pub(in crate::db) fn projection(
    mode: ValuationMode,
    attached: bool,
    alias: &str,
    columns: &std::collections::HashSet<String>,
    source: TradeSource,
) -> Option<CoverageSql> {
    match mode {
        ValuationMode::Historical => attached.then(|| coverage_sql(alias, columns, source)),
        ValuationMode::Current => {
            // One snapshot AND one clock for every projection of this read batch — see `RatePin`.
            let (rates, now_ms) = current::current_rates_at();
            Some(current_rate_sql(alias, columns, &rates, now_ms))
        }
    }
}

/// Report columns a trade must carry before it can be valued at all.
///
/// Shared with the Report window's synthetic-column table, so the columns that OFFER a conversion
/// and the gate that decides a row can be converted cannot drift into disagreeing — the visible
/// symptom of which would be a column that is permanently blank rather than absent.
pub(crate) const REQUIRED_TRADE_INPUTS: &[&str] = &["closedate", "basecurrency", "profitbtc"];

/// Check whether one report source carries every mandatory valuation input column.
///
/// Args:
///     columns: Discovered physical source columns.
///
/// Returns:
///     Whether close time, quote identity, and native profit are available.
pub(crate) fn has_required_trade_inputs(columns: &std::collections::HashSet<String>) -> bool {
    REQUIRED_TRADE_INPUTS
        .iter()
        .all(|column| columns.contains(*column))
}

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
/// Returns:
///     SQLite success after the table and lookup index exist.
pub(super) fn init_report_outbox(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(&format!(
        "CREATE TABLE IF NOT EXISTS {OUTBOX_TABLE} (
             seq INTEGER PRIMARY KEY AUTOINCREMENT,
             source_kind INTEGER NOT NULL,
             core_uid INTEGER NOT NULL,
             row_id INTEGER NOT NULL,
             action INTEGER NOT NULL
         );
         CREATE INDEX IF NOT EXISTS idx_valuation_outbox_seq
             ON {OUTBOX_TABLE}(seq);"
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
pub(super) fn stage_row(
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
pub(super) fn stage_delete(
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
pub(super) fn stage_rescan_core(conn: &Connection, core_uid: u64) -> rusqlite::Result<()> {
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
pub(super) fn stage_legacy_purge(conn: &Connection, core_uid: u64) -> rusqlite::Result<()> {
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
pub(super) fn ack_outbox(conn: &Connection, through_seq: i64) -> rusqlite::Result<()> {
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

/// Direction used to turn one spot close into quote units per USDT.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RateOrientation {
    /// A `QUOTEUSDT` close is already USDT per quote unit.
    Direct,
    /// A `USDTQUOTE` close must be inverted.
    Inverse,
    /// USDT itself is exactly one and needs no market request.
    Identity,
}

impl RateOrientation {
    /// Stable integer persisted with rate provenance.
    ///
    /// Returns:
    ///     Database representation of the orientation.
    const fn code(self) -> i64 {
        match self {
            Self::Direct => 0,
            Self::Inverse => 1,
            Self::Identity => 2,
        }
    }
}

/// One validated closed-minute conversion result ready for persistence.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ResolvedRate {
    /// Persisted MoonBot quote ordinal.
    pub quote_ordinal: i64,
    /// UTC minute start in Unix seconds.
    pub minute_utc: i64,
    /// USDT received for one quote unit.
    pub rate_usdt: f64,
    /// Canonical provider identifier.
    pub provider: String,
    /// Spot market used by the provider.
    pub symbol: String,
    /// Direct, inverse, or identity conversion.
    pub orientation: RateOrientation,
    /// Provider candle open time in Unix milliseconds.
    pub candle_open_ms: i64,
    /// Provider candle close time in Unix milliseconds.
    pub candle_close_ms: i64,
}

/// Cached outcome for one algorithm-versioned quote minute.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum CachedRate {
    /// A finite positive conversion rate with complete provenance.
    Ready(ResolvedRate),
    /// Every canonical direct/inverse route proved permanently unavailable for the minute.
    PermanentMissing,
}

/// Current report inputs used to guard a prepared valuation against same-key upserts.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct TradeInput {
    /// Typed or legacy physical source.
    pub source: TradeSource,
    /// Stable runtime core identity.
    pub core_uid: i64,
    /// `newrecid` for typed rows or `db_id` for legacy rows.
    pub row_id: i64,
    /// Close timestamp in Unix seconds.
    pub closedate: i64,
    /// Persisted quote ordinal.
    pub quote_ordinal: i64,
    /// Native quote-currency profit.
    pub profit_quote: f64,
    /// Native quote-currency positive spend, when supplied by the core.
    pub spent_quote: Option<f64>,
}

/// Return whether one SQLite error proves database-page corruption.
///
/// Args:
///     error: SQLite operation failure.
///
/// Returns:
///     Whether SQLite classified the file as corrupt or not a database.
pub(crate) fn is_corruption(error: &rusqlite::Error) -> bool {
    matches!(
        error,
        rusqlite::Error::SqliteFailure(code, _)
            if matches!(
                code.code,
                rusqlite::ErrorCode::DatabaseCorrupt | rusqlite::ErrorCode::NotADatabase
            )
    )
}

/// Mark the canonical derived cache unavailable and wake its recovery worker.
///
/// Args:
///     reason: Diagnostic reason retained in the application log.
pub(crate) fn mark_unhealthy(reason: &dyn std::fmt::Display) {
    if CACHE_HEALTH.swap(UNHEALTHY, Ordering::AcqRel) != UNHEALTHY {
        log::warn!("valuation: derived cache disabled: {reason}");
    }
    worker::wake_for_recovery();
}

/// Publish a successfully opened canonical cache to new readers.
fn mark_healthy() {
    CACHE_HEALTH.store(HEALTHY, Ordering::Release);
}

/// Disable attachment while startup validation runs without emitting a false damage warning.
fn begin_store_validation() {
    CACHE_HEALTH.store(UNHEALTHY, Ordering::Release);
}

/// Return whether new readers may attach the canonical derived cache.
///
/// Returns:
///     `true` only after startup validation or recovery succeeded.
pub(crate) fn cache_is_healthy() -> bool {
    CACHE_HEALTH.load(Ordering::Acquire) == HEALTHY
}

/// Check one existing cache without creating or mutating it.
///
/// Args:
///     path: Existing valuation database path.
///
/// Returns:
///     `true` for a complete healthy schema, `false` for proven corruption or an invalid schema,
///     and an error description when the check itself could not run conclusively.
fn existing_store_is_healthy(path: &Path) -> Result<bool, String> {
    let uri = sqlite_read_only_uri(path);
    let flags = OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI;
    let conn = Connection::open_with_flags(uri, flags)
        .map_err(|error| format!("read-only open failed: {error}"))?;
    let check = conn.query_row("PRAGMA main.quick_check(1)", [], |row| {
        row.get::<_, String>(0)
    });
    match check {
        Ok(result) if result == "ok" => {}
        Ok(result) => {
            log::warn!("valuation: quick_check reported: {result}");
            return Ok(false);
        }
        Err(error) if is_corruption(&error) => return Ok(false),
        Err(error) => return Err(format!("quick_check failed: {error}")),
    }
    match validate_store_schema(&conn) {
        Ok(()) => Ok(true),
        Err(error) if is_corruption(&error) => Ok(false),
        Err(error) => {
            log::warn!("valuation: existing schema is unusable: {error}");
            Ok(false)
        }
    }
}

/// Validate the two reader-facing tables on a direct valuation connection.
///
/// Args:
///     conn: Direct read-only or read-write valuation connection.
///
/// Returns:
///     SQLite success after both schemas and their first reachable rows are readable.
fn validate_store_schema(conn: &Connection) -> rusqlite::Result<()> {
    validate_schema_with_prefix(conn, "")
}

/// Prove that the report connection's main schema passes SQLite's bounded consistency check.
///
/// Args:
///     conn: Report reader whose integrity must remain fail-closed.
///
/// Returns:
///     Whether `PRAGMA main.quick_check(1)` returned exactly `ok`.
fn main_is_healthy(conn: &Connection) -> bool {
    matches!(
        conn.query_row("PRAGMA main.quick_check(1)", [], |row| row.get::<_, String>(0)),
        Ok(result) if result == "ok"
    )
}

/// Prove corruption of a derived file that failed before SQLite could attach its schema.
///
/// Args:
///     conn: Healthy report-main candidate.
///     path: Explicit valuation file passed to `ATTACH`.
///     error: Corruption-class attachment failure.
///
/// Returns:
///     `true` only when report main is healthy and read-only derived validation proves damage.
fn prove_detached_store_corruption(
    conn: &Connection,
    path: &Path,
    error: &rusqlite::Error,
) -> bool {
    is_corruption(error)
        && main_is_healthy(conn)
        && matches!(existing_store_is_healthy(path), Ok(false))
}

/// Return whether filesystem metadata describes a directory without any link or reparse point.
///
/// Args:
///     metadata: Metadata obtained without following the final path component.
///
/// Returns:
///     `true` only for a plain directory safe to use as a recovery boundary.
fn is_plain_directory(metadata: &std::fs::Metadata) -> bool {
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return false;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;

        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0400;
        metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT == 0
    }
    #[cfg(not(windows))]
    {
        true
    }
}

/// Create or validate one recovery directory without following a substituted final component.
///
/// Args:
///     path: Damage root or persistent pending retirement directory.
///
/// Returns:
///     Success only when the resulting path is a plain directory.
fn ensure_plain_directory(path: &Path) -> std::io::Result<()> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if is_plain_directory(&metadata) => return Ok(()),
        Ok(_) => {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("recovery path is not a plain directory: {}", path.display()),
            ));
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    std::fs::create_dir_all(path)?;
    let metadata = std::fs::symlink_metadata(path)?;
    if is_plain_directory(&metadata) {
        Ok(())
    } else {
        Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("recovery path is not a plain directory: {}", path.display()),
        ))
    }
}

/// Move every remaining live cache member into the persistent pending directory.
///
/// WAL and SHM move before the main database. A crash therefore cannot permit replacement
/// creation while an old main file is still live, and the same pending directory is resumed on
/// the next launch.
///
/// Args:
///     files: Main, WAL, and SHM paths in that order.
///     pending: Stable retirement directory.
///
/// Returns:
///     Filesystem success only after no live cache member remains.
///
/// Errors:
///     Returns an I/O error when the recovery boundary is unsafe or any family member cannot be
///     inspected or moved.
fn retire_live_family(files: &[PathBuf; 3], pending: &Path) -> std::io::Result<()> {
    ensure_plain_directory(pending)?;
    for index in [1usize, 2, 0] {
        let source = &files[index];
        if !source.try_exists()? {
            continue;
        }
        let name = source.file_name().ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "valuation member has no name",
            )
        })?;
        let destination = pending.join(name);
        if destination.try_exists()? {
            return Err(std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                format!(
                    "pending valuation member already exists: {}",
                    destination.display()
                ),
            ));
        }
        std::fs::rename(source, destination)?;
    }
    for path in files {
        if path.try_exists()? {
            return Err(std::io::Error::new(
                std::io::ErrorKind::Other,
                format!("live valuation member remains: {}", path.display()),
            ));
        }
    }
    Ok(())
}

/// Publish one completed pending retirement under a unique immutable directory.
///
/// Args:
///     pending: Stable pending directory containing the complete retired family.
///     damage_root: Plain parent directory for the immutable archive.
///
/// Returns:
///     Published quarantine path, or a filesystem failure that leaves `pending` resumable.
///
/// Errors:
///     Returns an I/O error when either boundary is unsafe or the atomic publish rename fails.
fn finalize_pending_retirement(
    pending: &Path,
    damage_root: &Path,
) -> std::io::Result<std::path::PathBuf> {
    ensure_plain_directory(damage_root)?;
    ensure_plain_directory(pending)?;
    let mut timestamp = crate::util::now_unix_ms_i64();
    let process_id = std::process::id();
    loop {
        let archive =
            crate::config::paths::valuation_recovery_archive_in(damage_root, timestamp, process_id);
        if !archive.try_exists()? {
            std::fs::rename(pending, &archive)?;
            return Ok(archive);
        }
        timestamp = timestamp.saturating_add(1);
    }
}

/// Resume or begin crash-consistent retirement of the canonical derived cache.
///
/// Args:
///     files: Main, WAL, and SHM paths in that order.
///     damage_root: Plain parent directory for immutable quarantine archives.
///     pending: Stable directory used to resume an interrupted retirement.
///
/// Returns:
///     Published quarantine directory after every live member is retired.
///
/// Errors:
///     Returns a diagnostic string when directory validation, retirement, or publication fails.
fn retire_store_family(
    files: &[PathBuf; 3],
    damage_root: &Path,
    pending: &Path,
) -> Result<std::path::PathBuf, String> {
    ensure_plain_directory(damage_root)
        .map_err(|error| format!("create {} failed: {error}", damage_root.display()))?;
    retire_live_family(files, pending)
        .map_err(|error| format!("retire valuation family failed: {error}"))?;
    finalize_pending_retirement(pending, damage_root)
        .map_err(|error| format!("publish valuation retirement failed: {error}"))
}

/// Open one derived store after validating or retiring its complete SQLite family.
///
/// Args:
///     files: Main, WAL, and SHM paths in that order.
///     damage_root: Parent for immutable quarantine directories.
///     pending: Stable directory used to resume interrupted retirement.
///
/// Returns:
///     Healthy read-write cache connection, or a diagnostic failure before replacement.
fn open_recoverable_store(
    files: [PathBuf; 3],
    damage_root: &Path,
    pending: &Path,
) -> Result<Connection, String> {
    let path = &files[0];
    let pending_exists = pending
        .try_exists()
        .map_err(|error| format!("inspect {} failed: {error}", pending.display()))?;
    let main_exists = path
        .try_exists()
        .map_err(|error| format!("inspect {} failed: {error}", path.display()))?;
    let orphan_exists = files[1..]
        .iter()
        .try_fold(false, |found, member| {
            member.try_exists().map(|exists| found || exists)
        })
        .map_err(|error| format!("inspect valuation sidecar failed: {error}"))?;

    let needs_retirement = if pending_exists || (!main_exists && orphan_exists) {
        true
    } else if main_exists {
        !existing_store_is_healthy(path)?
    } else {
        false
    };
    if needs_retirement {
        let archive = retire_store_family(&files, damage_root, pending)?;
        log::warn!(
            "valuation: damaged derived cache retired to {}",
            archive.display()
        );
    }

    let conn = open_store(path).map_err(|error| format!("initialize fresh store: {error}"))?;
    validate_store_schema(&conn).map_err(|error| format!("validate fresh store: {error}"))?;
    Ok(conn)
}

/// Open the canonical store after validating or retiring every prior live member.
///
/// Existing storage is checked read-only before any journal-mode or schema mutation. A pending
/// retirement always finishes before a replacement may be created.
///
/// Returns:
///     Healthy read-write cache connection, or a diagnostic failure with cache attachment disabled.
pub(crate) fn open_canonical_store() -> Result<Connection, String> {
    let _lifecycle = CACHE_LIFECYCLE
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    begin_store_validation();
    let files = crate::config::paths::valuation_db_files();
    let damage_root = crate::config::paths::damaged_valuation_dir();
    let pending = crate::config::paths::valuation_recovery_pending_dir();
    let conn = open_recoverable_store(files, &damage_root, &pending)?;
    mark_healthy();
    Ok(conn)
}

/// Classify a direct cache operation failure and disable attachment on proven corruption.
///
/// Proven corruption is reported as [`FailureKind::CacheUnhealthy`] rather than a write failure
/// because the two need different responses: an ordinary write error is worth retrying against the
/// same file, while a damaged cache is only cleared by the recovery stage rebuilding it.
///
/// Args:
///     error: SQLite failure from the canonical valuation writer.
///
/// Returns:
///     Stage-less classified cause carrying the diagnostic text.
pub(crate) fn store_fault(error: rusqlite::Error) -> FaultCause {
    let kind = if is_corruption(&error) {
        mark_unhealthy(&error);
        FailureKind::CacheUnhealthy
    } else {
        FailureKind::CacheWrite
    };
    FaultCause::new(kind, error.to_string())
}

/// Open and initialize the historical valuation store.
///
/// Args:
///     path: Canonical or fixture SQLite path.
///
/// Returns:
///     WAL-mode connection with the current schema and indexes.
///
/// Errors:
///     Returns the underlying SQLite error when the file or schema cannot be initialized.
pub(crate) fn open_store(path: &Path) -> rusqlite::Result<Connection> {
    let conn = Connection::open(path)?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    // The cache has many small autocommits during backfill. A less frequent checkpoint reduces
    // checkpoint pressure while the fixed SQLite WAL implementation coordinates attached readers.
    conn.pragma_update(None, "wal_autocheckpoint", 8_192)?;
    conn.busy_timeout(Duration::from_secs(3))?;
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS rates (
             algorithm_version INTEGER NOT NULL,
             quote_ordinal INTEGER NOT NULL,
             minute_utc INTEGER NOT NULL,
             status INTEGER NOT NULL,
             rate_usdt REAL,
             provider TEXT,
             symbol TEXT,
             orientation INTEGER,
             candle_open_ms INTEGER,
             candle_close_ms INTEGER,
             fetched_at_ms INTEGER NOT NULL,
             PRIMARY KEY (algorithm_version, quote_ordinal, minute_utc)
         );
         CREATE TABLE IF NOT EXISTS trade_values (
             source_kind INTEGER NOT NULL,
             core_uid INTEGER NOT NULL,
             row_id INTEGER NOT NULL,
             algorithm_version INTEGER NOT NULL,
             closedate INTEGER NOT NULL,
             quote_ordinal INTEGER NOT NULL,
             profit_quote REAL NOT NULL,
             spent_quote REAL,
             rate_minute_utc INTEGER NOT NULL,
             rate_usdt REAL NOT NULL,
             profit_usdt REAL NOT NULL,
             spent_usdt REAL,
             valued_at_ms INTEGER NOT NULL,
             PRIMARY KEY (source_kind, core_uid, row_id)
         );
         CREATE INDEX IF NOT EXISTS idx_trade_values_inputs
             ON trade_values (algorithm_version, quote_ordinal, rate_minute_utc);",
    )?;
    Ok(conn)
}

/// Attach an existing valuation store to a report reader.
///
/// Missing derived storage is a normal `Ok(false)` while startup initializes it. An existing file
/// that cannot attach is a classified read failure rather than zero conversion coverage.
///
/// Args:
///     conn: Report connection before its read snapshot begins.
///
/// Returns:
///     Whether the valuation schema was attached.
pub(in crate::db) fn attach(conn: &Connection) -> ReadResult<bool> {
    if !cache_is_healthy() {
        return Ok(false);
    }
    let path = crate::config::paths::valuation_db_path();
    attach_store(conn, &path)
}

/// Attach one explicit valuation store after its filesystem and schema checks pass.
///
/// Args:
///     conn: Report connection before its read snapshot begins.
///     path: Canonical valuation path or isolated fixture equivalent.
///
/// Returns:
///     Whether the valuation schema was attached.
fn attach_store(conn: &Connection, path: &Path) -> ReadResult<bool> {
    let _lifecycle = CACHE_LIFECYCLE
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    match std::fs::metadata(&path) {
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => {
            return Err(read_fail::io_fail("valuation: database metadata", &error));
        }
    }
    let uri = sqlite_read_only_uri(path);
    let sql = format!("ATTACH DATABASE '{}' AS {SCHEMA}", uri.replace('\'', "''"));
    if let Err(error) = conn.execute(&sql, []) {
        if prove_detached_store_corruption(conn, path, &error) {
            mark_unhealthy(&error);
            return Ok(false);
        }
        return Err(read_fail::read_fail("valuation: attach", error));
    }
    match validate_attachment(conn) {
        Ok(()) => Ok(true),
        Err(error) if prove_derived_corruption(conn, &error) => {
            let _ = conn.execute(&format!("DETACH DATABASE {SCHEMA}"), []);
            Ok(false)
        }
        Err(error) => Err(read_fail::read_fail(
            "valuation: validate attachment",
            error,
        )),
    }
}

/// Encode one Windows path as a read-only SQLite file URI for reader attachment.
///
/// Args:
///     path: Existing valuation database path.
///
/// Returns:
///     SQLite URI with reserved path characters escaped and write access disabled.
fn sqlite_read_only_uri(path: &Path) -> String {
    let escaped = path
        .to_string_lossy()
        .replace('%', "%25")
        .replace('?', "%3F")
        .replace('#', "%23")
        .replace('\\', "/");
    format!("file:{escaped}?mode=ro")
}

/// Test whether a connection currently carries the validated valuation attachment.
///
/// Args:
///     conn: Report reader or read snapshot that may carry the attachment.
///
/// Returns:
///     `true` when startup attachment succeeded and the cache has not since been disabled.
pub(in crate::db) fn is_attached(conn: &Connection) -> bool {
    if !cache_is_healthy() {
        return false;
    }
    conn.query_row(
        "SELECT 1 FROM pragma_database_list WHERE name = ?1 LIMIT 1",
        [SCHEMA],
        |_| Ok(()),
    )
    .is_ok()
}

/// Execute the complete reader-facing valuation schema contract.
///
/// Args:
///     conn: Report reader with a candidate `valuation` attachment.
///
/// Returns:
///     Success for readable current tables, including empty ones.
fn validate_attachment(conn: &Connection) -> rusqlite::Result<()> {
    validate_schema_with_prefix(conn, "valuation.")
}

/// Execute the reader-facing valuation schema contract with one table-name prefix.
///
/// Args:
///     conn: Direct valuation connection or report reader carrying an attachment.
///     prefix: Empty for a direct connection or `valuation.` for an attachment.
///
/// Returns:
///     Success after both tables and their first reachable rows are readable.
fn validate_schema_with_prefix(conn: &Connection, prefix: &str) -> rusqlite::Result<()> {
    let probes = [
        format!(
            "SELECT algorithm_version, quote_ordinal, minute_utc, status, rate_usdt,
                    provider, symbol, orientation, candle_open_ms, candle_close_ms
             FROM {prefix}rates LIMIT 1"
        ),
        format!(
            "SELECT source_kind, core_uid, row_id, algorithm_version, closedate,
                    quote_ordinal, profit_quote, spent_quote, rate_minute_utc,
                    rate_usdt, profit_usdt, spent_usdt
             FROM {prefix}trade_values LIMIT 1"
        ),
    ];
    for sql in probes {
        match conn.query_row(&sql, [], |_| Ok(())) {
            Ok(()) | Err(rusqlite::Error::QueryReturnedNoRows) => {}
            Err(error) => return Err(error),
        }
    }
    validate_primary_key(
        conn,
        prefix,
        "rates",
        &["algorithm_version", "quote_ordinal", "minute_utc"],
    )?;
    validate_primary_key(
        conn,
        prefix,
        "trade_values",
        &["source_kind", "core_uid", "row_id"],
    )?;
    Ok(())
}

/// Require one table to retain the exact primary key assumed by valuation joins and upserts.
///
/// Args:
///     conn: Direct valuation connection or report reader carrying an attachment.
///     prefix: Empty for a direct connection or `valuation.` for an attachment.
///     table: Valuation table whose key contract is checked.
///     expected: Primary-key column names in key order.
///
/// Returns:
///     Success only when SQLite reports the complete expected primary key.
fn validate_primary_key(
    conn: &Connection,
    prefix: &str,
    table: &str,
    expected: &[&str],
) -> rusqlite::Result<()> {
    let schema = prefix.trim_end_matches('.');
    let sql = if schema.is_empty() {
        format!("PRAGMA table_info({table})")
    } else {
        format!("PRAGMA {schema}.table_info({table})")
    };
    let mut statement = conn.prepare(&sql)?;
    let mut columns = statement
        .query_map([], |row| {
            Ok((row.get::<_, String>(1)?, row.get::<_, i64>(5)?))
        })?
        .filter_map(|column| match column {
            Ok((name, position)) if position > 0 => Some(Ok((position, name))),
            Ok(_) => None,
            Err(error) => Some(Err(error)),
        })
        .collect::<rusqlite::Result<Vec<_>>>()?;
    columns.sort_by_key(|(position, _)| *position);
    if columns
        .iter()
        .map(|(_, name)| name.as_str())
        .eq(expected.iter().copied())
    {
        Ok(())
    } else {
        Err(rusqlite::Error::InvalidQuery)
    }
}

/// Prove that a corruption error came from the attached derived cache, not `main`.
///
/// Both schema checks are explicit. Any inability to prove a healthy report main database returns
/// `false`, preserving the existing fail-closed report integrity path.
///
/// Args:
///     conn: Report reader carrying the candidate valuation attachment.
///     error: Corruption-class failure from a statement that may reference valuation tables.
///
/// Returns:
///     `true` only when `main` checks healthy and `valuation` checks damaged.
pub(crate) fn prove_derived_corruption(conn: &Connection, error: &rusqlite::Error) -> bool {
    if !is_corruption(error) {
        return false;
    }
    if !main_is_healthy(conn) {
        return false;
    }
    let valuation = conn.query_row("PRAGMA valuation.quick_check(1)", [], |row| {
        row.get::<_, String>(0)
    });
    let damaged = match valuation {
        Ok(result) => result != "ok",
        Err(check_error) => is_corruption(&check_error),
    };
    if damaged {
        mark_unhealthy(error);
    }
    damaged
}

/// Read a cached rate outcome for one quote minute.
///
/// Args:
///     conn: Open valuation store.
///     quote_ordinal: Persisted MoonBot quote ordinal.
///     minute_utc: UTC minute start in Unix seconds.
///
/// Returns:
///     Ready, permanently unavailable, or absent cache state.
pub(crate) fn cached_rate(
    conn: &Connection,
    quote_ordinal: i64,
    minute_utc: i64,
) -> rusqlite::Result<Option<CachedRate>> {
    conn.query_row(
        "SELECT status, rate_usdt, provider, symbol, orientation,
                candle_open_ms, candle_close_ms
         FROM rates
         WHERE algorithm_version=?1 AND quote_ordinal=?2 AND minute_utc=?3",
        params![ALGORITHM_VERSION, quote_ordinal, minute_utc],
        |row| {
            let status: i64 = row.get(0)?;
            if status != 0 {
                return Ok(CachedRate::PermanentMissing);
            }
            let orientation = match row.get::<_, i64>(4)? {
                0 => RateOrientation::Direct,
                1 => RateOrientation::Inverse,
                2 => RateOrientation::Identity,
                value => {
                    return Err(rusqlite::Error::IntegralValueOutOfRange(4, value));
                }
            };
            Ok(CachedRate::Ready(ResolvedRate {
                quote_ordinal,
                minute_utc,
                rate_usdt: row.get(1)?,
                provider: row.get(2)?,
                symbol: row.get(3)?,
                orientation,
                candle_open_ms: row.get(5)?,
                candle_close_ms: row.get(6)?,
            }))
        },
    )
    .optional()
}

/// Persist one successful immutable closed-minute conversion.
///
/// Args:
///     conn: Open valuation store.
///     rate: Validated conversion and provenance.
///     fetched_at_ms: Local fetch time in Unix milliseconds.
///
/// Returns:
///     Number of inserted or replaced rows.
pub(crate) fn store_rate(
    conn: &Connection,
    rate: &ResolvedRate,
    fetched_at_ms: i64,
) -> rusqlite::Result<usize> {
    conn.execute(
        "INSERT INTO rates (
             algorithm_version, quote_ordinal, minute_utc, status, rate_usdt,
             provider, symbol, orientation, candle_open_ms, candle_close_ms, fetched_at_ms
         ) VALUES (?1,?2,?3,0,?4,?5,?6,?7,?8,?9,?10)
         ON CONFLICT (algorithm_version, quote_ordinal, minute_utc) DO UPDATE SET
             status=0, rate_usdt=excluded.rate_usdt, provider=excluded.provider,
             symbol=excluded.symbol, orientation=excluded.orientation,
             candle_open_ms=excluded.candle_open_ms,
             candle_close_ms=excluded.candle_close_ms, fetched_at_ms=excluded.fetched_at_ms",
        params![
            ALGORITHM_VERSION,
            rate.quote_ordinal,
            rate.minute_utc,
            rate.rate_usdt,
            rate.provider,
            rate.symbol,
            rate.orientation.code(),
            rate.candle_open_ms,
            rate.candle_close_ms,
            fetched_at_ms,
        ],
    )
}

/// Persist that every canonical route lacks one historical quote minute.
///
/// Args:
///     conn: Open valuation store.
///     quote_ordinal: Persisted MoonBot quote ordinal.
///     minute_utc: UTC minute start in Unix seconds.
///     fetched_at_ms: Local verification time in Unix milliseconds.
///
/// Returns:
///     Number of inserted or replaced rows.
pub(crate) fn store_permanent_missing(
    conn: &Connection,
    quote_ordinal: i64,
    minute_utc: i64,
    fetched_at_ms: i64,
) -> rusqlite::Result<usize> {
    conn.execute(
        "INSERT INTO rates (
             algorithm_version, quote_ordinal, minute_utc, status, fetched_at_ms
         ) VALUES (?1,?2,?3,1,?4)
         ON CONFLICT (algorithm_version, quote_ordinal, minute_utc) DO UPDATE SET
             status=1, rate_usdt=NULL, provider=NULL, symbol=NULL, orientation=NULL,
             candle_open_ms=NULL, candle_close_ms=NULL, fetched_at_ms=excluded.fetched_at_ms",
        params![ALGORITHM_VERSION, quote_ordinal, minute_utc, fetched_at_ms],
    )
}

/// Persist a prepared USDT valuation guarded by its complete source inputs.
///
/// Args:
///     conn: Open valuation store.
///     input: Current committed report values.
///     rate: Cached finite positive historical rate.
///     valued_at_ms: Local calculation time in Unix milliseconds.
///
/// Returns:
///     Number of inserted or replaced rows.
pub(crate) fn store_trade_value(
    conn: &Connection,
    input: &TradeInput,
    rate: &ResolvedRate,
    valued_at_ms: i64,
) -> rusqlite::Result<usize> {
    let profit_usdt = input.profit_quote * rate.rate_usdt;
    let spent_usdt = input.spent_quote.map(|spent| spent * rate.rate_usdt);
    conn.execute(
        "INSERT INTO trade_values (
             source_kind, core_uid, row_id, algorithm_version, closedate, quote_ordinal,
             profit_quote, spent_quote, rate_minute_utc, rate_usdt, profit_usdt,
             spent_usdt, valued_at_ms
         ) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13)
         ON CONFLICT (source_kind, core_uid, row_id) DO UPDATE SET
             algorithm_version=excluded.algorithm_version, closedate=excluded.closedate,
             quote_ordinal=excluded.quote_ordinal, profit_quote=excluded.profit_quote,
             spent_quote=excluded.spent_quote, rate_minute_utc=excluded.rate_minute_utc,
             rate_usdt=excluded.rate_usdt, profit_usdt=excluded.profit_usdt,
             spent_usdt=excluded.spent_usdt, valued_at_ms=excluded.valued_at_ms
         WHERE trade_values.algorithm_version IS NOT excluded.algorithm_version
            OR trade_values.closedate IS NOT excluded.closedate
            OR trade_values.quote_ordinal IS NOT excluded.quote_ordinal
            OR trade_values.profit_quote IS NOT excluded.profit_quote
            OR trade_values.spent_quote IS NOT excluded.spent_quote
            OR trade_values.rate_minute_utc IS NOT excluded.rate_minute_utc
            OR trade_values.rate_usdt IS NOT excluded.rate_usdt
            OR trade_values.profit_usdt IS NOT excluded.profit_usdt
            OR trade_values.spent_usdt IS NOT excluded.spent_usdt",
        params![
            input.source.code(),
            input.core_uid,
            input.row_id,
            ALGORITHM_VERSION,
            input.closedate,
            input.quote_ordinal,
            input.profit_quote,
            input.spent_quote,
            rate.minute_utc,
            rate.rate_usdt,
            profit_usdt,
            spent_usdt,
            valued_at_ms,
        ],
    )
}
