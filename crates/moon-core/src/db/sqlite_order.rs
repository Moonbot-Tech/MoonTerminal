//! Exact SQLite storage-class and numeric ordering for Rust-side row merges.

use std::cmp::Ordering;

use rusqlite::types::Value;

/// Compare two SQLite values as NULL < numbers < text < blob, retaining integer precision.
///
/// Args:
///     left: First value to compare.
///     right: Second value to compare.
///
/// Returns:
///     SQLite's ascending ordering, with integer/real pairs compared numerically rather than
///     through an `f64` conversion of the integer.
pub(crate) fn compare(left: &Value, right: &Value) -> Ordering {
    rank(left)
        .cmp(&rank(right))
        .then_with(|| match (left, right) {
            (Value::Integer(left), Value::Integer(right)) => left.cmp(right),
            (Value::Integer(left), Value::Real(right)) => integer_real_cmp(*left, *right),
            (Value::Real(left), Value::Integer(right)) => integer_real_cmp(*right, *left).reverse(),
            (Value::Real(left), Value::Real(right)) => {
                if left < right {
                    Ordering::Less
                } else if left > right {
                    Ordering::Greater
                } else {
                    Ordering::Equal
                }
            }
            (Value::Text(left), Value::Text(right)) => left.cmp(right),
            (Value::Blob(left), Value::Blob(right)) => left.cmp(right),
            _ => Ordering::Equal,
        })
}

/// Return SQLite's storage-class rank for one value.
fn rank(value: &Value) -> u8 {
    match value {
        Value::Null => 0,
        Value::Integer(_) | Value::Real(_) => 1,
        Value::Text(_) => 2,
        Value::Blob(_) => 3,
    }
}

/// Compare an integer with a real exactly, including fractions and both `i64` boundaries.
pub(crate) fn integer_real_cmp(integer: i64, real: f64) -> Ordering {
    if real < i64::MIN as f64 {
        return Ordering::Greater;
    }
    if real >= -(i64::MIN as f64) {
        return Ordering::Less;
    }
    let order = integer.cmp(&(real as i64));
    if !order.is_eq() {
        return order;
    }
    if (integer as f64) < real {
        Ordering::Less
    } else if (integer as f64) > real {
        Ordering::Greater
    } else {
        Ordering::Equal
    }
}
