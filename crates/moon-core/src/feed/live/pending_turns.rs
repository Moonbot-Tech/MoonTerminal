//! Order event turns captured between two order publications.
//!
//! A turn holds the rows one loop pass's order events carried. The next publication replays the
//! turns ahead of its snapshot, so a state that left the snapshot in between still reaches the
//! line store.
//!
//! Publications normally follow each other within 250 ms and this holds a handful of rows. While
//! the consumer does not take the previous batch the feed thread publishes nothing (the feed's
//! `order_gate`), and the turns would grow for as long as the stall lasts. Past [`FOLD_ROWS`]
//! the earlier turns therefore collapse to the latest row of each order.

use std::collections::HashSet;

use crate::feed::OrderRow;

#[cfg(test)]
mod tests;

/// Pending rows above which the earlier turns are folded.
///
/// Far above what one publication period captures, so an undisturbed feed never folds: 512 rows
/// inside 250 ms is two thousand order events a second from a single core.
const FOLD_ROWS: usize = 512;

/// The captured turns in arrival order, bounded by the orders they touch rather than by time.
#[derive(Debug)]
pub(super) struct PendingTurns {
    turns: Vec<Vec<OrderRow>>,
    /// Rows across every turn, an empty turn counting as one.
    rows: usize,
    /// Row count that triggers the next fold. Raised after a fold so a set of orders that is
    /// itself above [`FOLD_ROWS`] does not fold again on every push.
    fold_at: usize,
}

impl Default for PendingTurns {
    /// Starts empty, with the first fold at [`FOLD_ROWS`].
    fn default() -> Self {
        Self {
            turns: Vec::new(),
            rows: 0,
            fold_at: FOLD_ROWS,
        }
    }
}

impl PendingTurns {
    /// Appends one turn's rows. An empty turn is kept: as the last turn it tells the publication
    /// that a snapshot-only update followed the captured rows.
    pub(super) fn push(&mut self, turn: Vec<OrderRow>) {
        // An empty turn weighs one row. The events that carry no row (a trace point, a corridor
        // change, a resynchronisation) keep arriving through a stall, and unweighed their turns
        // would never reach the fold.
        self.rows += turn.len().max(1);
        self.turns.push(turn);
        if self.rows > self.fold_at {
            self.fold_earlier();
        }
    }

    /// Hands every turn to a publication and starts over.
    pub(super) fn take(&mut self) -> Vec<Vec<OrderRow>> {
        std::mem::take(self).turns
    }

    /// Collapses every turn but the last into one turn holding each order's latest row.
    ///
    /// The last turn stays as it is. The publication overlays it on the snapshot, and an empty
    /// one there means "overlay nothing"; merging older rows into it would lay rows from before
    /// a resynchronisation over the snapshot that replaced them.
    ///
    /// What is lost is the intermediate rows of an order that changed several times: its line
    /// keeps the final state and the trace the core sent with it, and the clock-skew estimate
    /// sees one sample for it instead of several.
    fn fold_earlier(&mut self) {
        let Some(last) = self.turns.pop() else {
            return;
        };
        let mut seen = HashSet::new();
        let mut folded: Vec<OrderRow> = self
            .turns
            .drain(..)
            .rev()
            .flat_map(|turn| turn.into_iter().rev())
            .filter(|row| seen.insert(row.uid))
            .collect();
        // Back to arrival order, each order at the place of its latest row.
        folded.reverse();
        self.rows = folded.len() + last.len().max(1);
        if !folded.is_empty() {
            self.turns.push(folded);
        }
        self.turns.push(last);
        self.fold_at = FOLD_ROWS.max(self.rows * 2);
    }
}
