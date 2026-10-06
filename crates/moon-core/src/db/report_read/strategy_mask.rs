//! strategy mask items for report reads.

use super::*;

/// Return whether one filter needs the attached strategy-name metadata.
///
/// Exact keys need it only when attribution may change a physical strategy id. A non-empty name
/// mask always needs it because the report source stores ids rather than strategy names.
///
/// Args:
///     filter: Complete Report filter.
///
/// Returns:
///     Whether rows and totals should enable the shared strategy metadata path.
fn strategy_metadata_required(filter: &ReportFilter) -> bool {
    filter
        .strategies
        .as_ref()
        .is_some_and(|strategies| !strategies.is_empty())
        || strategy_name_query(filter).is_some()
}

/// Parse the Report's strategy-name mask — the ONE emptiness decision for it.
///
/// Args:
///     filter: Complete Report filter.
///
/// Returns:
///     The parsed query, or `None` when it selects everything and no predicate is needed.
fn strategy_name_query(filter: &ReportFilter) -> Option<StrategyQuery> {
    let query = StrategyQuery::parse(&filter.strategy_name_mask);
    (!query.is_empty()).then_some(query)
}

/// Strategy metadata of one rows/totals read, resolved on the connection the read runs on.
///
/// Names are readable only when the filter needs them; the name mask is resolved here once, so
/// a valuation retry reuses the same pairs.
///
/// Args:
///     conn: Open report reader or snapshot.
///     f: Complete Report filter.
///
/// Errors:
///     Returns the SQLite error of the strategy-name lookup.
pub(super) fn report_strategy_meta(
    conn: &Connection,
    f: &ReportFilter,
) -> rusqlite::Result<StrategyMeta> {
    let names = strategy_metadata_required(f) && crate::db::analytics::strategies_attached(conn);
    Ok(StrategyMeta {
        names,
        name_mask: resolve_strategy_name_mask(conn, f, names)?,
    })
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
pub(super) fn append_strategy_filter(
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

    let mut by_core: BTreeMap<i64, BTreeSet<i64>> = BTreeMap::new();
    for strategy in strategies {
        by_core
            .entry(strategy.core_uid as i64)
            .or_default()
            .insert(strategy.strategy_id);
    }
    let sid = crate::db::analytics::effective_sid_expr("r", columns, has_strategy_names);
    let groups = core_sid_groups_sql(&sid, &by_core);
    sql.push_str(&format!(" AND ({groups})"));
}

/// Render `(core, strategy ids)` groups as literal SQL, one per core, joined by `OR`.
///
/// Both keys are integers, so inlining them as literals is safe and spends no bind variable; the
/// expression depth grows with the number of cores, not strategies.
///
/// Args:
///     sid: SQL expression of the row's effective strategy id.
///     by_core: Strategy ids grouped by the stored (`i64`) core uid.
///
/// Returns:
///     The groups joined by ` OR `, without outer parentheses.
fn core_sid_groups_sql(sid: &str, by_core: &BTreeMap<i64, BTreeSet<i64>>) -> String {
    by_core
        .iter()
        .map(|(core_uid, strategy_ids)| {
            let ids = strategy_ids
                .iter()
                .map(|strategy_id| strategy_id.to_string())
                .collect::<Vec<_>>()
                .join(",");
            format!("(r.core_uid = {core_uid} AND COALESCE({sid}, 0) IN ({ids}))")
        })
        .collect::<Vec<_>>()
        .join(" OR ")
}

/// Render exclusion pairs as CASE: 60 ms versus 1706 ms for per-row OR groups at 200k rows / 100 cores.
///
/// Args:
///     sid: SQL expression of the row's effective strategy id.
///     by_core: Strategy ids grouped by the stored (`i64`) core uid.
///
/// Returns:
///     A CASE expression selecting one core's IN list, or zero for an unlisted or NULL core.
fn core_sid_case_sql(sid: &str, by_core: &BTreeMap<i64, BTreeSet<i64>>) -> String {
    let mut sql = String::from("CASE r.core_uid");
    for (core_uid, strategy_ids) in by_core {
        let ids = strategy_ids
            .iter()
            .map(|strategy_id| strategy_id.to_string())
            .collect::<Vec<_>>()
            .join(",");
        sql.push_str(&format!(
            " WHEN {core_uid} THEN COALESCE({sid}, 0) IN ({ids})"
        ));
    }
    sql.push_str(" ELSE 0 END");
    sql
}

/// The `(core, strategy)` pairs a strategy-name mask selects, resolved once per read.
#[derive(Clone, Debug)]
struct NameMaskSet {
    /// Whether the mask has a positive term: keep these pairs, rather than drop them.
    positive: bool,
    /// Strategy ids keyed by the stored (`i64`) core uid.
    pairs: BTreeMap<i64, BTreeSet<i64>>,
}

/// Strategy metadata one Report read shares across its statements and its valuation retry.
#[derive(Clone, Debug)]
pub(super) struct StrategyMeta {
    /// Whether liquidation attribution metadata is readable.
    pub(super) names: bool,
    /// The resolved name mask, `None` when the filter has none or it cannot be resolved.
    name_mask: Option<NameMaskSet>,
}

impl StrategyMeta {
    /// Metadata for a read whose filter carries no strategy-name mask.
    ///
    /// Args:
    ///     names: Whether liquidation attribution metadata is readable.
    pub(super) fn without_mask(names: bool) -> Self {
        Self {
            names,
            name_mask: None,
        }
    }
}

/// Resolve the strategy-name mask to the `(core, strategy)` pairs it selects.
///
/// Positive: the pairs with at least one named row the query matches, as the old per-row
/// `EXISTS (.. match = 1)`. Exclusion-only: the pairs with at least one named row the query
/// rejects, as the old `NOT EXISTS (.. name IS NOT NULL AND match = 0)`. Like that subquery, deleted
/// strategies stay in. Rows whose core uid is not an integer, whose strategy id is not integral,
/// or whose name is not valid text are skipped.
///
/// Args:
///     conn: Open report reader or snapshot, with `strat` attached when `has_strategy_names`.
///     f: Complete Report filter.
///     has_strategy_names: Whether the attached strategy metadata is readable.
///
/// Returns:
///     `None` when the filter has no mask or the metadata is unreadable; the caller fails closed.
///
/// Errors:
///     Returns the SQLite error of the lookup.
fn resolve_strategy_name_mask(
    conn: &Connection,
    f: &ReportFilter,
    has_strategy_names: bool,
) -> rusqlite::Result<Option<NameMaskSet>> {
    let Some(query) = strategy_name_query(f) else {
        return Ok(None);
    };
    if !has_strategy_names {
        return Ok(None);
    }
    let positive = query.has_positive();
    let mut pairs: BTreeMap<i64, BTreeSet<i64>> = BTreeMap::new();
    let mut stmt = conn.prepare(
        "SELECT core_uid, strategy_id, name FROM strat.strategies WHERE name IS NOT NULL",
    )?;
    let mut rows = stmt.query([])?;
    while let Some(row) = rows.next()? {
        // The old SQL join compared numerically, so an integral REAL id matched too.
        let Some(core_uid) = integral_id(row.get_ref(0)?) else {
            continue;
        };
        let Some(strategy_id) = integral_id(row.get_ref(1)?) else {
            continue;
        };
        let ValueRef::Text(bytes) = row.get_ref(2)? else {
            continue;
        };
        let Ok(name) = std::str::from_utf8(bytes) else {
            continue;
        };
        if query.matches(name) == positive {
            pairs.entry(core_uid).or_default().insert(strategy_id);
        }
    }
    Ok(Some(NameMaskSet { positive, pairs }))
}

/// An id stored as INTEGER, or as an integral in-range REAL; anything else is no id.
fn integral_id(v: ValueRef<'_>) -> Option<i64> {
    match v {
        ValueRef::Integer(i) => Some(i),
        ValueRef::Real(r) if r.fract() == 0.0 && r >= i64::MIN as f64 && r < i64::MAX as f64 => {
            Some(r as i64)
        }
        _ => None,
    }
}

/// Append the strategy-name predicate in the shared `moon_core::strategy_query` syntax.
///
/// The mask is resolved ONCE per read ([`resolve_strategy_name_mask`]) to the `(core, strategy)`
/// pairs it selects, and rows are filtered by those pairs as integer literals, joined by the same
/// effective strategy id as the exact selector so liquidation attribution and physical strategy
/// ids agree. That selects exactly what the per-row subquery it replaces did: a pair is listed
/// exactly when that subquery would have found a matching (positive) or rejecting (exclusion)
/// named row. Positive `OR` depth is bounded by the number of cores, as in [`append_strategy_filter`];
/// exclusions use `CASE` to select one core's strategy ids per row.
/// A TEXT `strategyid` compares by affinity differently from the old join; the exact selector
/// already compares the same way, so both filters agree.
///
/// Two shapes. A query with a positive term keeps rows whose strategy name matches, so rows
/// without a named strategy drop out. An exclusion-only query means "everything except", so it
/// drops only rows whose named strategy fails it: manual, sid-0, unknown-strategy and NULL-core
/// rows survive (`CASE` defaults to zero and `COALESCE` keeps a NULL result from dropping the row).
///
/// Args:
///     sql: Mutable WHERE clause receiving the name predicate.
///     filter: Complete Report filter containing the optional name mask.
///     columns: Columns available on the current report source.
///     meta: Strategy metadata of this read, carrying the resolved mask.
///
/// Returns:
///     Nothing; a non-empty mask of either shape fails closed when its identity or name metadata
///     is unavailable, since not even an exclusion can be proven then.
pub(super) fn append_strategy_name_mask(
    sql: &mut String,
    filter: &ReportFilter,
    columns: &std::collections::HashSet<String>,
    meta: &StrategyMeta,
) {
    if strategy_name_query(filter).is_none() {
        return;
    }
    let mask = match &meta.name_mask {
        Some(mask)
            if meta.names && columns.contains("core_uid") && columns.contains("strategyid") =>
        {
            mask
        }
        _ => {
            sql.push_str(" AND 1=0");
            return;
        }
    };

    let sid = crate::db::analytics::effective_sid_expr("r", columns, meta.names);
    if mask.pairs.is_empty() {
        if mask.positive {
            sql.push_str(" AND 1=0");
        }
        return;
    }
    if mask.positive {
        let groups = core_sid_groups_sql(&sid, &mask.pairs);
        sql.push_str(&format!(" AND ({groups})"));
    } else {
        let case = core_sid_case_sql(&sid, &mask.pairs);
        sql.push_str(&format!(" AND NOT COALESCE({case}, 0)"));
    }
}
