//! inventory items for report reads.

use super::*;

/// Convert one generic Report value to an integer without accepting lossy non-integral reals.
///
/// Args:
///     value: SQLite value returned by the dynamic Report projection.
///
/// Returns:
///     Parsed integer, or `None` for NULL, blobs, malformed text, and non-integral reals.
pub(in crate::db) fn report_value_i64(value: &Value) -> Option<i64> {
    match value {
        Value::Integer(value) => Some(*value),
        Value::Real(value) if value.is_finite() && value.fract() == 0.0 => Some(*value as i64),
        Value::Text(value) => value.parse().ok(),
        Value::Null | Value::Blob(_) | Value::Real(_) => None,
    }
}

/// Convert one generic Report value to a finite floating-point number.
///
/// Args:
///     value: SQLite value returned by the dynamic Report projection.
///
/// Returns:
///     Parsed finite number, or `None` for NULL, blobs, malformed text, and non-finite values.
pub(super) fn report_value_f64(value: &Value) -> Option<f64> {
    let value = match value {
        Value::Integer(value) => *value as f64,
        Value::Real(value) => *value,
        Value::Text(value) => value.parse().ok()?,
        Value::Null | Value::Blob(_) => return None,
    };
    value.is_finite().then_some(value)
}

/// Convert one generic Report value to its stored text identity.
///
/// Args:
///     value: SQLite value returned by the dynamic Report projection.
///
/// Returns:
///     Cloned text, or `None` for every non-text storage class.
pub(in crate::db) fn report_value_text(value: &Value) -> Option<String> {
    match value {
        Value::Text(value) => Some(value.clone()),
        Value::Null | Value::Integer(_) | Value::Real(_) | Value::Blob(_) => None,
    }
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

/// Row count per `core_uid` in one table, or an empty map when the table does not exist.
///
/// One grouped pass over the `core_uid`-leading key: 16 ms on a 608k-row replica (29 cores),
/// 1-2 ms on the strategy and trace stores. Shared by every store the Storage tab counts.
pub(crate) fn count_by_core(
    conn: &Connection,
    table: &str,
    ctx: &'static str,
) -> ReadResult<HashMap<u64, u64>> {
    let present: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?1",
            [table],
            |r| r.get(0),
        )
        .map_err(|e| read_fail(ctx, e))?;
    if present == 0 {
        return Ok(HashMap::new());
    }
    let mut stmt = conn
        .prepare(&format!(
            "SELECT core_uid, COUNT(*) FROM {table} GROUP BY core_uid"
        ))
        .map_err(|e| read_fail(ctx, e))?;
    let rows = stmt
        .query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)))
        .map_err(|e| read_fail(ctx, e))?;
    let mut out = HashMap::new();
    for row in rows {
        let (uid, n) = row.map_err(|e| read_fail(ctx, e))?;
        if let (Ok(uid), Ok(n)) = (u64::try_from(uid), u64::try_from(n)) {
            out.insert(uid, n);
        }
    }
    Ok(out)
}

/// Every core with report rows, newest first, with its newest name and its row count across both
/// schemas — the Storage tab's list of whose data the replica holds.
pub fn rows_by_core(conn: &Connection) -> ReadResult<Vec<(u64, String, u64)>> {
    const CTX: &str = "отчёты: rows_by_core";
    let mut counts: HashMap<u64, u64> = HashMap::new();
    for src in read_sources_res(conn)? {
        if !src.cols.contains("core_uid") {
            continue;
        }
        for (uid, n) in count_by_core(conn, src.table, CTX)? {
            *counts.entry(uid).or_default() += n;
        }
    }
    Ok(distinct_cores(conn)?
        .into_iter()
        .map(|(uid, name)| (uid, name, counts.get(&uid).copied().unwrap_or(0)))
        .collect())
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
/// attached strategy database when available and otherwise use the signed numeric id. Both
/// strategy predicates are deliberately removed so an active checkbox or name mask does not hide
/// alternative strategies that match every other Report filter.
///
/// Split into a normal-strategy arm and a liquidation arm when the two differ. What the split
/// buys is NOT an index-only scan — `deleted` sits in no index, so both arms still read table
/// rows to apply the deletion predicate. It buys keeping the liquidation-attribution CASE and
/// its two correlated subqueries off the rows that can never satisfy it, which on the 600k-row
/// measurement fixture is 599 696 of 600 000. Measured there: 2057 ms as one statement,
/// 1236 ms as two arms.
///
/// Args:
///     conn: Open report reader or snapshot with optional strategy attachment.
///     filter: Active Report filter; every non-strategy predicate scopes discovery.
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
    scope.strategy_name_mask.clear();
    let has_strategy_names = crate::db::analytics::strategies_attached(conn);
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
        let strategy_id =
            crate::db::analytics::effective_sid_expr("r", &src.cols, has_strategy_names);
        let (where_sql, params) = build_where(
            &scope,
            &src.cols,
            &StrategyMeta::without_mask(has_strategy_names),
        );
        // `COALESCE(r."strategyid",0)` never yields NULL, so `<> 0` / `= 0` partition every row
        // with no third case, and identity is set equality (this function sorts its whole output
        // in Rust below, so SQL row order never matters). When `strategy_id` is already the plain
        // column, both arms would be identical and the split would buy nothing — keep the single
        // statement in that case.
        //
        // `UNION`, not `UNION ALL`, and this is MEASURED rather than reasoned. The arms are
        // disjoint by construction and the caller below dedups every pair through `seen`, so
        // `UNION ALL` looks like the free choice — it is the opposite. `UNION` lets SQLite
        // dedup the compound ONCE, and its left arm then needs no `DISTINCT` of its own;
        // `UNION ALL` makes each arm materialise its own `USE TEMP B-TREE FOR DISTINCT`.
        // On the 600k-row measurement fixture: 2057 ms before this split, 1236 ms with `UNION`,
        // 6599 ms with `UNION ALL` — three times WORSE than doing nothing at all.
        //
        // ONE `build_where`, its parameters bound TWICE. Calling it a second time would re-read
        // the wall clock — `append_row_scope` resolves "does this window still reach the
        // present" against `now`, and its own doc says that must be ONE reading for the whole
        // predicate — so two calls can straddle a second boundary and scope the two arms to
        // genuinely different row states. Binding the same vector twice also keeps the bound
        // parameter count where it was instead of doubling it.
        let two_arms = strategy_id != "r.\"strategyid\"";
        let sql = if two_arms {
            format!(
                "SELECT DISTINCT r.core_uid, COALESCE(r.\"strategyid\",0) FROM {table} r{where_sql} AND COALESCE(r.\"strategyid\",0) <> 0 \
                 UNION \
                 SELECT DISTINCT r.core_uid, COALESCE({strategy_id}, 0) FROM {table} r{where_sql} AND COALESCE(r.\"strategyid\",0) = 0",
                table = src.table,
            )
        } else {
            format!(
                "SELECT DISTINCT r.core_uid, COALESCE({strategy_id}, 0) FROM {} r{where_sql}",
                src.table,
            )
        };
        let mut refs: Vec<&dyn rusqlite::types::ToSql> =
            params.iter().map(|value| value.as_ref()).collect();
        if two_arms {
            let second: Vec<&dyn rusqlite::types::ToSql> =
                params.iter().map(|value| value.as_ref()).collect();
            refs.extend(second);
        }
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
