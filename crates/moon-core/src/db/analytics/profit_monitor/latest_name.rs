//! SQLite aggregate retaining the latest nonblank report name without a second row scan.

use rusqlite::functions::{Aggregate, Context};

/// Numeric SQLite close dates retain integers exactly, including values above 2^53.
#[derive(Clone, Copy)]
enum CloseDate {
    Integer(i64),
    Real(f64),
}

impl CloseDate {
    /// Compare integers and reals as SQLite does, without rounding integer timestamps.
    fn cmp(self, other: Self) -> std::cmp::Ordering {
        match (self, other) {
            (Self::Integer(left), Self::Integer(right)) => left.cmp(&right),
            (Self::Integer(left), Self::Real(right)) => integer_real_cmp(left, right),
            (Self::Real(left), Self::Integer(right)) => integer_real_cmp(right, left).reverse(),
            (Self::Real(left), Self::Real(right)) => {
                if left < right {
                    std::cmp::Ordering::Less
                } else if left > right {
                    std::cmp::Ordering::Greater
                } else {
                    std::cmp::Ordering::Equal
                }
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

    /// Copy a name only when its date advances the group's latest named trade.
    fn step(&self, ctx: &mut Context<'_>, acc: &mut LatestName) -> rusqlite::Result<()> {
        let close = match ctx.get_raw(0) {
            rusqlite::types::ValueRef::Null => return Ok(()),
            rusqlite::types::ValueRef::Integer(value) => CloseDate::Integer(value),
            rusqlite::types::ValueRef::Real(value) => CloseDate::Real(value),
            other => {
                return Err(rusqlite::Error::InvalidFunctionParameterType(
                    0,
                    other.data_type(),
                ));
            }
        };
        if acc.close.is_none_or(|previous| close.cmp(previous).is_gt()) {
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
