//! Read layer for the Reports window: filters, source projection, sort/merge, and aggregates.

use rusqlite::types::Value;
use rusqlite::Connection;

use super::read_fail::read_fail;
use super::rep;
use super::{read_sources_res, table_columns_res, ReadResult, ReadSource};

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
    /// `newrecid` (the replica replication key) for each row, parallel to `rows`.
    ///
    /// Also a hidden service column. It is the id the soft-delete protocol addresses, so the
    /// Report panel actions read it to build `set_report_rows_deleted`. Legacy rows, which have no
    /// `newrecid` and cannot be soft-deleted, carry `0` — never a real rec id.
    pub rec_ids: Vec<i64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SideFilter {
    #[default]
    All,
    Long,
    Short,
}

/// Which quantity every profit figure in the Analytics window is measured in.
///
/// `Usdt` sums the raw `profitbtc` (absolute money in the pair quote currency, as before).
/// `Percent` measures each trade as `profitbtc / spentbtc * 100` — the exact formula of the
/// MoonBot report's `Profit` column: return on the capital spent, independent of order size.
/// The choice is a per-`Query` lens, applied once in the source projection (see
/// `analytics::unified_from`), so every aggregation and the tuner sweep read the same metric.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ProfitMetric {
    /// Absolute money — the historical behaviour and the `ProfitUSDT` report column.
    #[default]
    Usdt,
    /// Return on spent capital in percent — the report's `Profit` column.
    Percent,
}

/// Complete filter shared by Report rows, totals, export, and strategy discovery.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
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
    /// Require a positive close timestamp, matching Analytics' closed-trade universe.
    pub closed_only: bool,
    /// Exact strategy identities; `None` selects all strategies, while `Some` remains constrained.
    ///
    /// The core is part of every key because strategy ids repeat across cores. An explicit empty
    /// collection intentionally matches no rows so a lost/stale selection cannot broaden a query.
    pub strategies: Option<Vec<ReportStrategyKey>>,
}

/// Exact report strategy identity across all connected cores.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ReportStrategyKey {
    /// Runtime core identity stored by the report replica.
    pub core_uid: u64,
    /// Delphi-signed strategy id stored in reports and `strategies.sqlite`.
    pub strategy_id: i64,
}

/// Strategy option shown by the Report filter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReportStrategy {
    /// Exact database identity used by the filter.
    pub key: ReportStrategyKey,
    /// Strategy name from `strategies.sqlite`, or the numeric id when metadata is unavailable.
    pub name: String,
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

/// Apply report predicates to one aliased source.
///
/// Before the core schema arrives, the replica may lack `closedate`, `coin`,
/// `isshort`, or `emulator`; filtering on an absent column would fail the entire
/// SELECT. An exact strategy is different: a source without either identity column
/// cannot prove a match and therefore contributes zero rows.
///
/// Args:
///     f: Complete Report filter.
///     cols: Columns available on this source.
///     has_strategy_names: Whether liquidation attribution metadata is readable.
///
/// Returns:
///     Parameterized SQL suffix and its ordered bound values.
fn build_where(
    f: &ReportFilter,
    cols: &std::collections::HashSet<String>,
    has_strategy_names: bool,
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
        sql.push_str(&format!(" AND r.core_uid IN ({ids})"));
    }
    append_strategy_filter(&mut sql, f, cols, has_strategy_names);
    if f.closed_only {
        if has("closedate") {
            sql.push_str(" AND r.closedate > 0");
        } else {
            sql.push_str(" AND 1=0");
        }
    }
    if has("closedate") {
        if let Some(from) = f.date_from {
            sql.push_str(" AND r.closedate IS NOT NULL AND r.closedate >= ?");
            params.push(Box::new(from));
        }
        if let Some(to) = f.date_to {
            sql.push_str(" AND r.closedate IS NOT NULL AND r.closedate <= ?");
            params.push(Box::new(to));
        }
    }
    let coin = f.coin.trim();
    if !coin.is_empty() && has("coin") {
        sql.push_str(" AND r.coin LIKE ?");
        params.push(Box::new(format!("%{}%", coin.to_uppercase())));
    }
    if has("isshort") {
        match f.side {
            SideFilter::All => {}
            SideFilter::Long => sql.push_str(" AND r.isshort = 0"),
            SideFilter::Short => sql.push_str(" AND r.isshort = 1"),
        }
    }
    if has("emulator") {
        match f.emulator {
            None => {}
            Some(true) => sql.push_str(" AND COALESCE(r.emulator, 0) = 1"),
            Some(false) => sql.push_str(" AND COALESCE(r.emulator, 0) = 0"),
        }
    }
    // Deleted-mode semantics live on `ReportFilter::deleted_only`; the `1=0` arm makes
    // a column-less source contribute nothing when only deleted rows are wanted.
    if has("deleted") {
        sql.push_str(if f.deleted_only {
            " AND COALESCE(r.deleted, 0) <> 0"
        } else {
            " AND COALESCE(r.deleted, 0) = 0"
        });
    } else if f.deleted_only {
        sql.push_str(" AND 1=0");
    }
    (sql, params)
}

/// Append the exact multi-strategy predicate without consuming SQLite bind-variable capacity.
///
/// Strategy and core ids are typed integers, so grouping their numeric literals by core is safe and
/// keeps a very large checkbox selection below SQLite's parameter limit. An explicit empty set or
/// a source without strategy identity remains a no-match constraint.
///
/// Args:
///     sql: Mutable WHERE clause receiving the strategy predicate.
///     filter: Complete Report filter containing the optional exact-key collection.
///     columns: Columns available on the current report source.
///     has_strategy_names: Whether liquidation attribution metadata is readable.
///
/// Returns:
///     Nothing; `sql` is unchanged only when the strategy filter is implicit All.
fn append_strategy_filter(
    sql: &mut String,
    filter: &ReportFilter,
    columns: &std::collections::HashSet<String>,
    has_strategy_names: bool,
) {
    let Some(strategies) = &filter.strategies else {
        return;
    };
    if strategies.is_empty() || !columns.contains("core_uid") || !columns.contains("strategyid") {
        sql.push_str(" AND 1=0");
        return;
    }

    let mut by_core: std::collections::BTreeMap<u64, std::collections::BTreeSet<i64>> =
        std::collections::BTreeMap::new();
    for strategy in strategies {
        by_core
            .entry(strategy.core_uid)
            .or_default()
            .insert(strategy.strategy_id);
    }
    let sid = super::analytics::effective_sid_expr("r", columns, has_strategy_names);
    let groups = by_core
        .into_iter()
        .map(|(core_uid, strategy_ids)| {
            let ids = strategy_ids
                .into_iter()
                .map(|strategy_id| strategy_id.to_string())
                .collect::<Vec<_>>()
                .join(",");
            format!(
                "(r.core_uid = {} AND COALESCE({sid}, 0) IN ({ids}))",
                core_uid as i64
            )
        })
        .collect::<Vec<_>>()
        .join(" OR ");
    sql.push_str(&format!(" AND ({groups})"));
}

/// Aggregate profit and order count over the complete filter, not only the top N.
///
/// Returns `Failed` when source discovery or any aggregate query fails; only a
/// successful empty result returns `(0.0, 0)`. The open connection means this
/// function cannot return `NotReady`.
///
/// Args:
///     conn: Open report reader or snapshot.
///     f: Complete Report filter.
///
/// Returns:
///     Exact profit sum and row count across all sources.
///
/// Errors:
///     Returns `Failed` for source, SQL, or row conversion errors.
pub fn query_totals(conn: &Connection, f: &ReportFilter) -> ReadResult<(f64, i64)> {
    const CTX: &str = "отчёты: query_totals";
    let (mut sum, mut count) = (0.0f64, 0i64);
    let has_strategy_names = f
        .strategies
        .as_ref()
        .is_some_and(|strategies| !strategies.is_empty())
        && super::analytics::strategies_attached(conn);
    for src in read_sources_res(conn)? {
        let (where_sql, params) = build_where(f, &src.cols, has_strategy_names);
        let profit = if src.cols.contains("profitbtc") {
            "COALESCE(SUM(r.profitbtc),0.0)"
        } else {
            "0.0"
        };
        let sql = format!("SELECT {profit}, COUNT(*) FROM {} r{where_sql}", src.table);
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
///
/// Args:
///     conn: Open report reader or snapshot.
///     f: Complete Report filter.
///     sort_key: Requested runtime column name.
///     desc: Whether to sort descending.
///     limit: Maximum merged rows.
///
/// Returns:
///     Runtime columns and the top matching report rows.
///
/// Errors:
///     Returns `Failed` for source, schema, SQL, or row conversion errors.
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
    let has_strategy_names = f
        .strategies
        .as_ref()
        .is_some_and(|strategies| !strategies.is_empty())
        && super::analytics::strategies_attached(conn);

    // Query the top N from EACH source separately so indexes work, then merge below.
    // Each entry is `(core_uid, rec_id, data)`; `rec_id` is the replica `newrecid` or 0 for a
    // legacy row that has none.
    let mut merged: Vec<(u64, i64, Vec<Value>)> = Vec::new();
    for src in read_sources_res(conn)? {
        let (where_sql, mut params) = build_where(f, &src.cols, has_strategy_names);
        let select = source_select(&src, &cols);
        // `newrecid` is a real column only on the typed replica; a legacy source projects 0, which
        // marks its rows as not soft-deletable (0 is never a real rec id).
        let rec_id_select = if src.cols.contains("newrecid") {
            "newrecid"
        } else {
            "0"
        };
        // Sort in SQL only if the source has the column. The legacy `id` alias is
        // visible to ORDER BY; otherwise source order is irrelevant because the merge reorders it.
        let sortable = src.cols.contains(&col) || (src.legacy && col == "id");
        let order = if sortable {
            format!("\"{col}\" IS NULL, \"{col}\" {dir}")
        } else {
            "1".to_string()
        };
        let sql = format!(
            "SELECT r.core_uid, {rec_id_select}, {select} FROM {} r{where_sql} ORDER BY {order} LIMIT ?",
            src.table
        );
        params.push(Box::new(limit as i64));
        let refs: Vec<&dyn rusqlite::types::ToSql> = params.iter().map(|b| b.as_ref()).collect();
        let mut stmt = conn.prepare(&sql).map_err(|e| read_fail(CTX, e))?;
        let n = cols.len();
        let mapped = stmt
            .query_map(refs.as_slice(), |r| {
                let core_uid = r.get::<_, i64>(0)? as u64;
                let rec_id = r.get::<_, i64>(1)?;
                let mut v = Vec::with_capacity(n);
                for i in 0..n {
                    v.push(r.get::<_, Value>(i + 2)?);
                }
                Ok((core_uid, rec_id, v))
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
        let va = sort_ix.and_then(|i| a.2.get(i)).unwrap_or(&Value::Null);
        let vb = sort_ix.and_then(|i| b.2.get(i)).unwrap_or(&Value::Null);
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
    let mut rec_ids = Vec::with_capacity(merged.len());
    for (uid, rec_id, row) in merged {
        core_uids.push(uid);
        rec_ids.push(rec_id);
        rows.push(row);
    }
    Ok(ReportTable {
        cols,
        rows,
        core_uids,
        rec_ids,
    })
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
        // MAX over the leading PK column is an index seek, and one seek is all this needs —
        // `distinct_cores` below walks every core because it must NAME them all.
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
        let table = src.table;
        // Within each source, cores come out newest-first (sources are then concatenated, so
        // every replica core still precedes a legacy-only one). HOW that order is computed
        // depends on whether the source's recency key is index-ordered, and the two answers are
        // not interchangeable — each statement is the fast one for its table and the slow one
        // for the other.
        let sql = if src.legacy {
            // The legacy table's key (`updated_ms`) sits in no index, so asking per core for
            // its newest row would sort that core's rows, once per core: measured 317 ms
            // against 150 ms for this single grouped pass on a 300k-row legacy table. This
            // read-only, transitional table therefore keeps the statement it always had —
            // including its reliance on SQLite's bare-column rule to pick the name.
            format!(
                "SELECT core_uid, core_name FROM {table}
                 GROUP BY core_uid ORDER BY MAX(COALESCE(updated_ms, 0)) DESC"
            )
        } else {
            // The replica's key IS index-ordered (`newrecid` is the second column of the
            // primary key), so walk the distinct `core_uid` values by seeking past each one — a
            // LOOSE INDEX SCAN — instead of reading every row in the table to answer a question
            // about 19 values. Measured on a 458 MB replica (529 862 rows): 250 ms for the
            // grouped pass against under a millisecond here. Both panels now throttle the core
            // list to once a minute, so it is no longer paid per reload — but it is still paid
            // on every panel construction, and `report/state.rs` pays it synchronously on the
            // UI thread. It is what the app's own "медленный query 298ms" warning was reporting.
            //
            // The rows are IDENTICAL, order included (verified one by one against the old
            // statement on that replica): the name comes from the core's newest row, which is
            // the row SQLite's bare-column rule for a lone min/max aggregate was already
            // picking — implicitly, and only while that query keeps exactly one such aggregate.
            format!(
                "WITH RECURSIVE cores(uid) AS (
                     SELECT MIN(core_uid) FROM {table}
                     UNION ALL
                     SELECT (SELECT MIN(core_uid) FROM {table} WHERE core_uid > cores.uid)
                     FROM cores WHERE cores.uid IS NOT NULL
                 )
                 SELECT uid,
                        (SELECT core_name FROM {table}
                         WHERE core_uid = cores.uid ORDER BY newrecid DESC LIMIT 1),
                        (SELECT MAX(newrecid) FROM {table} WHERE core_uid = cores.uid)
                 FROM cores WHERE uid IS NOT NULL
                 ORDER BY 3 DESC"
            )
        };
        let mut stmt = conn.prepare(&sql).map_err(|e| read_fail(CTX, e))?;
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

/// Load exact strategy identities present in report sources within the active Report scope.
///
/// Identity uses the same liquidation attribution and NULL-to-Manual semantics as
/// the exact filter, so every offered option can return the rows it represents.
/// Legacy sources that cannot identify strategies are skipped. Names come from the
/// attached strategy database when available and otherwise use the signed numeric id.
/// The strategy predicate itself is deliberately removed so an active checkbox does not hide
/// alternative strategies that match every other Report filter.
///
/// Args:
///     conn: Open report reader or snapshot with optional strategy attachment.
///     filter: Active Report filter; every predicate except `strategies` scopes discovery.
///
/// Returns:
///     Sorted exact strategy choices present in report sources.
///
/// Errors:
///     Returns `Failed` for source, strategy metadata, SQL, or row conversion errors.
pub fn distinct_strategies(
    conn: &Connection,
    filter: &ReportFilter,
) -> ReadResult<Vec<ReportStrategy>> {
    const CTX: &str = "reports: distinct_strategies";
    let mut scope = filter.clone();
    scope.strategies = None;
    let has_strategy_names = super::analytics::strategies_attached(conn);
    let mut names = std::collections::HashMap::new();
    if has_strategy_names {
        let mut stmt = conn
            .prepare("SELECT core_uid, strategy_id, name FROM strat.strategies")
            .map_err(|e| read_fail(CTX, e))?;
        let rows = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, i64>(0)? as u64,
                    row.get::<_, i64>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })
            .map_err(|e| read_fail(CTX, e))?;
        for row in rows {
            let (core_uid, strategy_id, name) = row.map_err(|e| read_fail(CTX, e))?;
            names.insert((core_uid, strategy_id), name);
        }
    }

    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for src in read_sources_res(conn)? {
        if !src.cols.contains("core_uid") || !src.cols.contains("strategyid") {
            continue;
        }
        let strategy_id = super::analytics::effective_sid_expr("r", &src.cols, has_strategy_names);
        let (where_sql, params) = build_where(&scope, &src.cols, has_strategy_names);
        let sql = format!(
            "SELECT DISTINCT r.core_uid, COALESCE({strategy_id}, 0) FROM {} r{where_sql}",
            src.table,
        );
        let refs: Vec<&dyn rusqlite::types::ToSql> =
            params.iter().map(|value| value.as_ref()).collect();
        let mut stmt = conn.prepare(&sql).map_err(|e| read_fail(CTX, e))?;
        let rows = stmt
            .query_map(refs.as_slice(), |row| {
                Ok((row.get::<_, i64>(0)? as u64, row.get::<_, i64>(1)?))
            })
            .map_err(|e| read_fail(CTX, e))?;
        for row in rows {
            let (core_uid, strategy_id) = row.map_err(|e| read_fail(CTX, e))?;
            if seen.insert((core_uid, strategy_id)) {
                out.push(ReportStrategy {
                    key: ReportStrategyKey {
                        core_uid,
                        strategy_id,
                    },
                    name: names
                        .get(&(core_uid, strategy_id))
                        .cloned()
                        .unwrap_or_else(|| strategy_id.to_string()),
                });
            }
        }
    }
    out.sort_by_cached_key(|strategy| {
        (
            strategy.name.to_lowercase(),
            strategy.key.core_uid,
            strategy.key.strategy_id,
        )
    });
    Ok(out)
}

#[cfg(test)]
mod tests;
