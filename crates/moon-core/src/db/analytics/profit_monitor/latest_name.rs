//! SQLite aggregate retaining the latest nonblank report name without a second row scan.

use rusqlite::functions::{Aggregate, Context};

/// Non-NULL SQLite dates retain their storage class and exact BINARY comparison bytes.
enum CloseDate {
    Integer(i64),
    Real(f64),
    Text(Vec<u8>),
    Blob(Vec<u8>),
}

impl CloseDate {
    /// Compare integers and reals as SQLite does, without rounding integer timestamps.
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        match (self, other) {
            (Self::Integer(left), Self::Integer(right)) => left.cmp(right),
            (Self::Integer(left), Self::Real(right)) => integer_real_cmp(*left, *right),
            (Self::Real(left), Self::Integer(right)) => integer_real_cmp(*right, *left).reverse(),
            (Self::Real(left), Self::Real(right)) => {
                if left < right {
                    std::cmp::Ordering::Less
                } else if left > right {
                    std::cmp::Ordering::Greater
                } else {
                    std::cmp::Ordering::Equal
                }
            }
            (Self::Text(left), Self::Text(right)) | (Self::Blob(left), Self::Blob(right)) => {
                left.cmp(right)
            }
            (Self::Blob(_), _) | (Self::Text(_), Self::Integer(_) | Self::Real(_)) => {
                std::cmp::Ordering::Greater
            }
            (_, Self::Blob(_)) | (Self::Integer(_) | Self::Real(_), Self::Text(_)) => {
                std::cmp::Ordering::Less
            }
        }
    }
}

/// Mirror SQLite's exact integer/real comparison, including the two i64 limits.
fn integer_real_cmp(integer: i64, real: f64) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    if real < i64::MIN as f64 {
        return Ordering::Greater;
    }
    if real >= -(i64::MIN as f64) {
        return Ordering::Less;
    }
    let whole = real as i64;
    let order = integer.cmp(&whole);
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

/// Latest nonblank name, with the first row winning equal-date ties like SQLite MAX.
#[derive(Default)]
pub(super) struct LatestName {
    close: Option<CloseDate>,
    name: Option<rusqlite::types::Value>,
}

/// Stateless registration for the per-group name accumulator.
pub(super) struct LatestNameAggregate;

impl Aggregate<LatestName, Option<rusqlite::types::Value>> for LatestNameAggregate {
    /// Start a group with no nonblank name.
    fn init(&self, _ctx: &mut Context<'_>) -> rusqlite::Result<LatestName> {
        Ok(LatestName::default())
    }

    /// Retain MAX's first non-NULL tie, or its last visited row if every date is NULL.
    fn step(&self, ctx: &mut Context<'_>, acc: &mut LatestName) -> rusqlite::Result<()> {
        let close = match ctx.get_raw(0) {
            rusqlite::types::ValueRef::Null => {
                if acc.close.is_none() {
                    acc.name = Some(ctx.get(1)?);
                }
                return Ok(());
            }
            rusqlite::types::ValueRef::Integer(value) => CloseDate::Integer(value),
            rusqlite::types::ValueRef::Real(value) => CloseDate::Real(value),
            rusqlite::types::ValueRef::Text(value) => CloseDate::Text(value.to_vec()),
            rusqlite::types::ValueRef::Blob(value) => CloseDate::Blob(value.to_vec()),
        };
        if acc
            .close
            .as_ref()
            .is_none_or(|previous| close.cmp(previous).is_gt())
        {
            acc.close = Some(close);
            acc.name = Some(ctx.get(1)?);
        }
        Ok(())
    }

    /// Yield the selected raw name; absent/blank groups are displayed as an empty name.
    fn finalize(
        &self,
        _ctx: &mut Context<'_>,
        acc: Option<LatestName>,
    ) -> rusqlite::Result<Option<rusqlite::types::Value>> {
        Ok(acc.and_then(|acc| acc.name))
    }
}
