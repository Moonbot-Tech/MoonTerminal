//! Version-letter and order publication decisions and the retained-table throttle.

use super::*;

/// Publication period for the retained order table, about 4 Hz.
///
/// Held by the table's `CoalescedDeadline`, which answers both "may I publish" and "how long until
/// I may" from this one interval. The two answers used to be two expressions in two places, each
/// free to be edited without the other.
pub(super) const ORDERS_TABLE_PERIOD: Duration = Duration::from_millis(250);

/// Leading-edge cadence for chart order lines during bursts.
pub(super) const ORDER_LINES_PERIOD: Duration = Duration::from_millis(50);

/// What one orders turn publishes.
///
/// Only the two things a turn can actually send: "nothing" is the absence of one, spelled `None` by
/// [`Self::decide`], so no site downstream has to carry a branch for a case the decision excluded.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum OrdersPublish {
    /// The whole retained table, on the [`ORDERS_TABLE_PERIOD`] throttle.
    Table,
    /// Only the chart's order lines, on the leading-edge 50 ms throttle.
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
    ///     lines_due: Whether queued line work has reached its leading-edge deadline.
    ///
    /// Returns:
    ///     The publication, or `None` when this turn owes nothing.
    pub(super) fn decide(table_due: bool, lines_due: bool) -> Option<Self> {
        if table_due {
            Some(Self::Table)
        } else if lines_due {
            Some(Self::Lines)
        } else {
            None
        }
    }

    /// The message this publication puts on the channel.
    pub(super) fn message(self, rows: crate::feed::OrderRows) -> FeedMsg {
        match self {
            Self::Table => FeedMsg::Orders(rows),
            Self::Lines => FeedMsg::OrderLines(rows),
        }
    }
}

/// What one orders turn's events put in debt: the table, the chart's line copies, or both.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct OrderDebts {
    /// The retained table owes a publication.
    pub(super) table: bool,
    /// The chart's line copies owe a publication.
    pub(super) lines: bool,
}

impl OrderDebts {
    /// The debts a turn with these events creates.
    ///
    /// A station has no chart, so it publishes no line copies: a line event puts the table in
    /// debt instead, and the turns captured for the copies ride the next table, so an order
    /// removed between two tables still reaches the store. Its order-line store — the Mini App's
    /// Panic Sell state, the clock-skew estimator — then moves at the table's 250 ms instead of
    /// the copies' 50 ms. Measured on a 27-core station with the Mini App on (#1081): the copies
    /// were twice the tables' count, two thirds of the rows built, and 4 of its 13 CPU seconds a
    /// minute; the table count rose by 2 % without them.
    ///
    /// Args:
    ///     station: Whether this feed serves a headless station.
    ///     table_event: The turn holds an event the table shows.
    ///     line_event: The turn holds an event the chart's lines show — every table event, and
    ///         trace points besides.
    ///
    /// Returns:
    ///     Which publications the turn owes.
    pub(super) fn of_turn(station: bool, table_event: bool, line_event: bool) -> Self {
        Self {
            table: table_event || (station && line_event),
            lines: line_event && !station,
        }
    }
}

/// Whether `info_addr` is still the `ServerInfo` allocation observed at `ServerRestart`.
///
/// The caller holds the `Arc<MoonStateSnapshot>` captured at the restart (`PinnedRestartInfo`).
/// That snapshot owns a clone of the inner `Arc<ServerInfo>`, so this address stays allocated
/// and cannot be reused for the replacement process's BaseCheck while the mark is set. MoonProto
/// can refresh before the restart event is drained, so the pin can also capture the new process.
/// Withhold only its letter, never its number; the respawned client's run has no pin and supplies
/// the fresh letter, which update judging reads only in the fresh epoch. A same-process reconnect
/// passes `None` and is never stale, so it still republishes the retained snapshot.
///
/// Args:
///     info_addr: Address of the `ServerInfo` inside the current snapshot. Compared, never
///         dereferenced.
///     restart_addr: Address captured when `ServerRestart` was observed, or `None` when this
///         connection has not seen one since the last publication. The allocation behind
///         this address is pinned by the snapshot the caller still holds.
///
/// Returns:
///     `true` when the snapshot is the allocation observed at the restart.
pub(super) fn server_info_predates_restart(info_addr: usize, restart_addr: Option<usize>) -> bool {
    restart_addr == Some(info_addr)
}

/// Withhold an unconfirmed restart letter while preserving ordinary reported letters.
///
/// Args:
///     suffix: Letter paired with the published number, including an explicit empty release.
///     predates_restart: Whether the snapshot matches the allocation pinned at restart.
///
/// Returns:
///     Unknown for the pinned allocation; otherwise the reported suffix unchanged.
pub(super) fn letter_to_publish(suffix: Option<String>, predates_restart: bool) -> Option<String> {
    if predates_restart { None } else { suffix }
}

#[cfg(test)]
mod tests;
