//! Order publication decisions and the retained-table throttle.

use super::*;

/// Publication period for the retained order table, about 4 Hz.
///
/// Held by the table's `CoalescedDeadline`, which answers both "may I publish" and "how long until
/// I may" from this one interval. The two answers used to be two expressions in two places, each
/// free to be edited without the other.
pub(super) const ORDERS_TABLE_PERIOD: Duration = Duration::from_millis(250);

/// What one orders turn publishes.
///
/// Only the two things a turn can actually send: "nothing" is the absence of one, spelled `None` by
/// [`Self::decide`], so no site downstream has to carry a branch for a case the decision excluded.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum OrdersPublish {
    /// The whole retained table, on the [`ORDERS_TABLE_PERIOD`] throttle.
    Table,
    /// Only the chart's order lines, immediately on an order event.
    Lines,
}

impl OrdersPublish {
    /// Decide what an orders turn owes, if anything.
    ///
    /// The table WINS over the lines when both are due: it carries the same rows, so sending both
    /// would put the identical set on the channel twice.
    ///
    /// Args:
    ///     table_due: Whether the table's throttle says it may publish now — see
    ///         `CoalescedDeadline::is_due`, which answers only for work actually queued.
    ///     has_line_event: Whether this batch carried an event the chart's order lines must see at
    ///         once.
    ///
    /// Returns:
    ///     The publication, or `None` when this turn owes nothing.
    pub(super) fn decide(table_due: bool, has_line_event: bool) -> Option<Self> {
        if table_due {
            Some(Self::Table)
        } else if has_line_event {
            Some(Self::Lines)
        } else {
            None
        }
    }

    /// The message this publication puts on the channel.
    pub(super) fn message(self, rows: Vec<crate::feed::OrderRow>) -> FeedMsg {
        match self {
            Self::Table => FeedMsg::Orders(rows),
            Self::Lines => FeedMsg::OrderLines(rows),
        }
    }
}
