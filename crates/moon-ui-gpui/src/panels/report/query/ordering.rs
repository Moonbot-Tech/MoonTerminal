//! Header ordering within the Report's reserved open-position block.

use std::cmp::Ordering;

use moon_core::db::{ReportTable, RowScope};
use rusqlite::types::Value;

/// Reorder only the returned open prefix, keeping row identities and the closed ledger intact.
///
/// The database owns membership and the 500-row reservation. This display-only permutation runs
/// before selection keys and width identities are derived; file exports keep database ordering.
pub(super) fn sort_open_prefix(table: &mut ReportTable, scope: RowScope, key: &str, desc: bool) {
    if scope == RowScope::Closed {
        return;
    }
    let Some(close) = table.cols.iter().position(|col| col == "closedate") else {
        return;
    };
    let Some(sort) = table.cols.iter().position(|col| col == key) else {
        return;
    };
    // Match the reader's numeric-positive close test, including malformed legacy timestamps.
    let open_count = table
        .rows
        .iter()
        .take_while(|row| {
            !matches!(row.get(close), Some(Value::Integer(n)) if *n > 0)
                && !matches!(row.get(close), Some(Value::Real(n)) if *n > 0.0)
        })
        .count();
    let mut rows: Vec<_> = table
        .rows
        .drain(..)
        .zip(table.core_uids.drain(..))
        .zip(table.rec_ids.drain(..))
        .collect();
    rows[..open_count].sort_by(|a, b| {
        compare_cells(
            a.0.0.get(sort).unwrap_or(&Value::Null),
            b.0.0.get(sort).unwrap_or(&Value::Null),
            desc,
        )
    });
    for ((row, core), record) in rows {
        table.rows.push(row);
        table.core_uids.push(core);
        table.rec_ids.push(record);
    }
}

/// Compare raw projected cells in SQLite storage order, with NULL last in either direction.
fn compare_cells(left: &Value, right: &Value, desc: bool) -> Ordering {
    let order = match (left, right) {
        (Value::Null, Value::Null) => return Ordering::Equal,
        (Value::Null, _) => return Ordering::Greater,
        (_, Value::Null) => return Ordering::Less,
        (Value::Integer(a), Value::Integer(b)) => a.cmp(b),
        (Value::Integer(a), Value::Real(b)) => compare_integer_real(*a, *b),
        (Value::Real(a), Value::Integer(b)) => compare_integer_real(*b, *a).reverse(),
        (Value::Real(a), Value::Real(b)) => a.partial_cmp(b).unwrap_or(Ordering::Equal),
        (Value::Text(a), Value::Text(b)) => a.cmp(b),
        (Value::Blob(a), Value::Blob(b)) => a.cmp(b),
        (Value::Integer(_) | Value::Real(_), _) | (Value::Text(_), Value::Blob(_)) => {
            Ordering::Less
        }
        _ => Ordering::Greater,
    };
    if desc { order.reverse() } else { order }
}

/// Compare mixed numeric storage without rounding large integer identifiers through `f64`.
fn compare_integer_real(integer: i64, real: f64) -> Ordering {
    if real < i64::MIN as f64 {
        return Ordering::Greater;
    }
    if real >= -(i64::MIN as f64) {
        return Ordering::Less;
    }
    integer.cmp(&(real as i64)).then_with(|| {
        (integer as f64)
            .partial_cmp(&real)
            .unwrap_or(Ordering::Equal)
    })
}

#[cfg(test)]
mod tests;
