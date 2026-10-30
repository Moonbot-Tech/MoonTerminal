//! Single-pass aggregate selecting a core's latest trade by close, milliseconds, and identity.

use rusqlite::functions::{Aggregate, Context};
use rusqlite::types::Value;

use super::latest_name::CloseDate;

/// Latest row selected by the complete `(closedate, close_ms, newrecid)` key.
#[derive(Default)]
pub(super) struct LatestTrade {
    close: Option<CloseDate>,
    close_ms: Option<CloseDate>,
    identity: Option<CloseDate>,
    profit: Option<Value>,
}

impl LatestTrade {
    /// Replace the retained row only when the candidate's complete key is newer.
    fn offer(
        &mut self,
        close: CloseDate,
        close_ms: Option<CloseDate>,
        identity: Option<CloseDate>,
        profit: Value,
    ) {
        let replace = self.close.as_ref().is_none_or(|previous| {
            close
                .cmp(previous)
                .then_with(|| compare_optional(&close_ms, &self.close_ms))
                .then_with(|| compare_optional(&identity, &self.identity))
                .is_gt()
        });
        if replace {
            self.close = Some(close);
            self.close_ms = close_ms;
            self.identity = identity;
            self.profit = Some(profit);
        }
    }
}

/// Compare optional key parts with SQLite NULL below every non-NULL value.
fn compare_optional(left: &Option<CloseDate>, right: &Option<CloseDate>) -> std::cmp::Ordering {
    match (left, right) {
        (Some(left), Some(right)) => left.cmp(right),
        (None, None) => std::cmp::Ordering::Equal,
        (None, Some(_)) => std::cmp::Ordering::Less,
        (Some(_), None) => std::cmp::Ordering::Greater,
    }
}

/// Convert a SQLite value to the exact non-NULL comparator used by the latest-name aggregate.
fn close_date(value: Value) -> Option<CloseDate> {
    match value {
        Value::Null => None,
        Value::Integer(value) => Some(CloseDate::Integer(value)),
        Value::Real(value) => Some(CloseDate::Real(value)),
        Value::Text(value) => Some(CloseDate::Text(value.into_bytes())),
        Value::Blob(value) => Some(CloseDate::Blob(value)),
    }
}

/// Stateless registration for the latest-trade accumulator.
pub(super) struct LatestTradeAggregate;

impl Aggregate<LatestTrade, Option<Value>> for LatestTradeAggregate {
    /// Start a group with no retained row.
    fn init(&self, _ctx: &mut Context<'_>) -> rusqlite::Result<LatestTrade> {
        Ok(LatestTrade::default())
    }

    /// Offer one row's close second, millisecond fallback, recorder identity, and projected profit.
    fn step(&self, ctx: &mut Context<'_>, acc: &mut LatestTrade) -> rusqlite::Result<()> {
        let close = close_date(ctx.get(0)?);
        let Some(close) = close else {
            if acc.close.is_none() {
                // Match SQLite's all-NULL bare-column fallback: the last visited row wins.
                acc.profit = Some(ctx.get(3)?);
            }
            return Ok(());
        };
        let close_ms = close_date(ctx.get(1)?);
        let identity = close_date(ctx.get(2)?);
        acc.offer(close, close_ms, identity, ctx.get(3)?);
        Ok(())
    }

    /// Return the selected row's projected profit, preserving SQL NULL for an unknown amount.
    fn finalize(
        &self,
        _ctx: &mut Context<'_>,
        acc: Option<LatestTrade>,
    ) -> rusqlite::Result<Option<Value>> {
        Ok(acc.and_then(|acc| acc.profit))
    }
}
