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
    /// Report panel's deletion mode reads it to build `set_report_rows_deleted`. Legacy rows,
    /// which have no `newrecid` and cannot be soft-deleted, carry `0` — never a real rec id.
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
    // Each entry is `(core_uid, rec_id, data)`; `rec_id` is the replica `newrecid` or 0 for a
    // legacy row that has none.
    let mut merged: Vec<(u64, i64, Vec<Value>)> = Vec::new();
    for src in read_sources_res(conn)? {
        let (where_sql, mut params) = build_where(f, &src.cols);
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
            "SELECT core_uid, {rec_id_select}, {select} FROM {}{where_sql} ORDER BY {order} LIMIT ?",
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
