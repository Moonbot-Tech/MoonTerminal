//! select items for report reads.

use super::*;

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
        // Replication detail BEHIND the seconds columns, not facts of their own: the chart's trade
        // query names them directly, so hiding them here costs it nothing, while leaving them out
        // would surface three raw epoch numbers in the column menu and in the all-column CSV the
        // moment any core supplies them.
        "buydatems",
        "sellsetdatems",
        "closedatems",
    ];
    let mut have = rep::table_cols_res(conn)?;
    let legacy = table_columns_res(conn)?;
    if legacy.contains("db_id") {
        have.insert("id".to_string());
    }
    have.extend(legacy);
    // A synthetic column is offered on the REPORT schema alone, never on `valuation::is_attached`:
    // gating it on the derived cache would churn the column set — and with it every saved width and
    // visibility keyed to it — each time that cache detaches. An unattached cache renders empty
    // cells instead.
    let mut out: Vec<String> = DISPLAY_COLUMNS
        .iter()
        .filter(|c| {
            have.contains(**c)
                || synthetic(c)
                    .is_some_and(|entry| entry.inputs.iter().all(|name| have.contains(*name)))
        })
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

/// Build one synthetic column's SQL against one physical source.
///
/// The single definition behind both the projection and the `ORDER BY`, so the two cannot disagree
/// about what a column means or about whether this source can produce it.
///
/// Args:
///     entry: Synthetic column being built.
///     src: Physical source whose schema decides availability.
///     valuation: Derived-cache fragments, absent when that cache is not joined.
///
/// Returns:
///     The expression, or `None` when this source cannot compute the column.
pub(super) fn synthetic_expression(
    entry: &Synthetic,
    src: &ReadSource,
    valuation: Option<&crate::db::valuation::CoverageSql>,
) -> Option<String> {
    if !entry.inputs.iter().all(|name| src.cols.contains(*name)) {
        return None;
    }
    Some(match entry.name {
        PROFIT_PERCENT_COLUMN => {
            // Both legs are the SETTLED amounts, so a COIN-M liquidation divides like for like.
            let profit = crate::db::quote::settled_amount_expr("r", &src.cols, "profitbtc");
            let spent = crate::db::quote::settled_amount_expr("r", &src.cols, "spentbtc");
            format!("CASE WHEN {spent} > 0 THEN {profit} / {spent} * 100.0 END")
        }
        // A cache-free retry has no `v` or `ra` aliases, so the expression must vanish with the
        // joins that back it.
        VALUATION_PROFIT_COLUMN => valuation?.profit_usdt.clone(),
        VALUATION_RATE_COLUMN => valuation?.per_row.rate.clone(),
        VALUATION_SOURCE_COLUMN => valuation?.per_row.source.clone(),
        _ => return None,
    })
}

/// Project a source onto the shared `cols`: preserve its own columns, map legacy
/// `db_id` to `id`, and emit NULL for absent columns.
///
/// Every reference is qualified with the source alias because the valuation joins bring their own
/// `closedate`, `core_uid` and `status` columns into scope; an unqualified name would be ambiguous.
///
/// Args:
///     src: Physical report source and its discovered schema.
///     cols: Shared runtime display columns to project in order.
///     valuation: Derived-cache fragments when that cache is joined.
///     core_names: Current configured core names for the `core_name` column.
///
/// Returns:
///     Comma-separated SQL projection for the aliased source.
pub(super) fn source_select(
    src: &ReadSource,
    cols: &[String],
    valuation: Option<&crate::db::valuation::CoverageSql>,
    core_names: &crate::db::CoreNames,
) -> String {
    cols.iter()
        .map(|c| {
            if c == MINI_ENTRY_VOLUME_RATE_COLUMN {
                let volume = traded_volume_sql(src, valuation.map(|v| v.per_row.rate.as_str()));
                format!(
                    "CASE WHEN {} THEN ({}) END AS \"{c}\"",
                    volume.reconstructed, volume.rate
                )
            } else if let Some(sql) = notify_column_expression(src, c) {
                format!("{sql} AS \"{c}\"")
            } else if let Some(entry) = synthetic(c) {
                let sql = synthetic_expression(entry, src, valuation)
                    .unwrap_or_else(|| "NULL".to_string());
                format!("{sql} AS \"{c}\"")
            } else if src.legacy && c == "id" && src.cols.contains("db_id") {
                "r.\"db_id\" AS \"id\"".to_string()
            } else if let Some(sql) = corrected_column_expression(src, c, core_names) {
                format!("{sql} AS \"{c}\"")
            } else if src.cols.contains(c) {
                format!("r.\"{c}\"")
            } else {
                format!("NULL AS \"{c}\"")
            }
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// Return the corrected SQL for a stored column the reader must not serve raw, if `col` is one.
///
/// Both the projection and the `ORDER BY` go through here for the same reason synthetic columns do:
/// every source is truncated by `LIMIT` before the Rust merge, so a column projected corrected and
/// sorted raw would order the rows by one value and then show another — silently returning the
/// wrong top rows when the user sorts by that column.
///
/// Args:
///     src: Physical source whose schema decides availability.
///     col: Runtime Report column key.
///     core_names: Current configured core names, resolving `core_name`.
///
/// Returns:
///     The expression, or `None` for every column this reader serves as stored.
pub(super) fn corrected_column_expression(
    src: &ReadSource,
    col: &str,
    core_names: &crate::db::CoreNames,
) -> Option<String> {
    if col == "core_name" && src.cols.contains(col) && src.cols.contains("core_uid") {
        // The stored name is a copy from download time; a renamed core must read one name on
        // every trade, and sort by that same name.
        let sql = core_names.sql("r");
        return (sql != format!("r.\"{col}\"")).then_some(sql);
    }
    if col == "basecurrency" && src.cols.contains(col) {
        // The displayed ticker must name the currency the row's own profit column is in, or the
        // table would print USDT beside a total the footer counted as BTC.
        return Some(crate::db::quote::effective_ordinal_expr("r", &src.cols));
    }
    // The money columns for the same reason: the footer, the percent column and the USDT valuation
    // all read the settled amount, so a grid serving the stored one would show 0.00001826 in the
    // row, 0.01120 in the total, and sort by the dust.
    if !matches!(col, "profitbtc" | "spentbtc") || !src.cols.contains(col) {
        return None;
    }
    let settled = crate::db::quote::settled_amount_expr("r", &src.cols, col);
    // A source that cannot evidence a liquidation gets the plain column back. Reporting that as a
    // "correction" would be a lie the sort/projection contract test rightly refuses.
    (settled != format!("r.\"{col}\"")).then_some(settled)
}

/// Compare values while merging sorted sources: numbers as `f64`, text
/// lexicographically. The caller handles NULL and always places it last.
pub(super) fn cmp_values(a: &Value, b: &Value) -> std::cmp::Ordering {
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
pub(super) fn sort_column(cols: &[String], key: &str) -> String {
    if let Some(c) = cols.iter().find(|c| c.as_str() == key) {
        return c.clone();
    }
    if cols.iter().any(|c| c == "closedate") {
        "closedate".to_string()
    } else {
        "newrecid".to_string()
    }
}

/// Return the source-local SQL expression for a validated report sort column.
///
/// Synthetic columns need their defining expression here because every source is truncated before
/// the Rust merge. Sorting only after that truncation would return the wrong global top rows.
///
/// Args:
///     src: Physical source whose schema determines sort availability.
///     col: Validated runtime sort-column key.
///     valuation: Derived-cache fragments when that cache is joined.
///     core_names: Current configured core names for the `core_name` column.
///
/// Returns:
///     SQL expression when the source can sort by the column, otherwise `None`.
pub(super) fn source_sort_expression(
    src: &ReadSource,
    col: &str,
    valuation: Option<&crate::db::valuation::CoverageSql>,
    core_names: &crate::db::CoreNames,
) -> Option<String> {
    if let Some(entry) = synthetic(col) {
        return synthetic_expression(entry, src, valuation);
    }
    if let Some(sql) = corrected_column_expression(src, col, core_names) {
        return Some(sql);
    }
    if src.cols.contains(col) {
        Some(format!("r.\"{col}\""))
    } else if src.legacy && col == "id" && src.cols.contains("db_id") {
        Some("r.\"db_id\"".to_string())
    } else {
        None
    }
}
