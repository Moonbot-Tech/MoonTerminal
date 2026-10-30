//! What one `(exchange, market)` must have filed: the union of its trades' windows, shaped like
//! the tape's (`trade_replay::ReplayWindow::focus_spans`, `tape_recorder::plan`) so the prices
//! and the prints of a trade cover the same ground.
//!
//! - **open** — from `entry − margin`, and on for as long as the position might still turn out
//!   short: `entry + max(margin, long_position)`;
//! - **closed short** — `entry − margin` through `exit + tail`;
//! - **closed long** — the open part above, then `exit − margin` through `exit + tail`: the middle
//!   of a long position is not kept, as on the tape.
//!
//! Trades of one key from any core share the key: their windows are united, and the key is
//! done once the last of them is past its tail.

use std::collections::HashMap;

use crate::market::tape_recorder::TradeId;
use crate::market::trade_replay::Coverage;

/// An open trade whose exit never arrived is forgotten after this long.
const OPEN_HORIZON_MS: i64 = 24 * 3_600_000;

/// The tape window in force, read once per pass.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Window {
    /// Before the entry and on each side of a long position's exit.
    pub margin_ms: i64,
    /// A position held longer than this is filed as its two ends.
    pub long_ms: i64,
    /// After the exit.
    pub tail_ms: i64,
}

impl Window {
    /// Coverage one trade asks for under this window.
    fn need(self, open_ms: i64, close_ms: Option<i64>) -> Coverage {
        let margin = self.margin_ms.max(0);
        let before = open_ms.saturating_sub(margin);
        let open_end = open_ms.saturating_add(margin.max(self.long_ms));
        match close_ms {
            None => Coverage::one((before, open_end)),
            Some(close) if close.saturating_sub(open_ms) <= self.long_ms => {
                Coverage::one((before, close.saturating_add(self.tail_ms)))
            }
            Some(close) => Coverage::from_spans([
                (before, open_end),
                (
                    close.saturating_sub(margin),
                    close.saturating_add(self.tail_ms),
                ),
            ]),
        }
    }
}

/// One trade of a key.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Trade {
    open_ms: i64,
    close_ms: Option<i64>,
}

/// One key's trades and what of them is already handled.
#[derive(Default)]
pub(super) struct KeyPlan {
    trades: HashMap<TradeId, Trade>,
    /// Filed, or found lost: ground a later pass must not touch again.
    pub settled: Coverage,
}

impl KeyPlan {
    /// A new plan for a key whose previous plan already filed `settled`.
    pub(super) fn resuming(settled: Coverage) -> Self {
        Self {
            trades: HashMap::new(),
            settled,
        }
    }

    /// A trade opened. A close already known for it stays.
    pub(super) fn opened(&mut self, id: TradeId, open_ms: i64) {
        self.trades.entry(id).or_insert(Trade {
            open_ms,
            close_ms: None,
        });
    }

    /// A trade closed; one never seen open is taken up here.
    pub(super) fn closed(&mut self, id: TradeId, open_ms: i64, close_ms: i64) {
        self.trades.insert(
            id,
            Trade {
                open_ms,
                close_ms: Some(close_ms),
            },
        );
    }

    /// Forget open trades whose exit never came.
    pub(super) fn expire(&mut self, now_ms: i64) {
        self.trades.retain(|_, t| {
            t.close_ms.is_some() || now_ms.saturating_sub(t.open_ms) <= OPEN_HORIZON_MS
        });
    }

    /// The union of every trade's window.
    pub(super) fn need(&self, window: Window) -> Coverage {
        let mut need = Coverage::none();
        for t in self.trades.values() {
            for &span in window.need(t.open_ms, t.close_ms).spans() {
                need.add(span);
            }
        }
        need
    }

    /// Whether nothing of this key is left to file by `horizon_ms`: every trade closed and past
    /// its tail, or no trade at all.
    pub(super) fn done(&self, window: Window, horizon_ms: i64) -> bool {
        self.trades.values().all(|t| {
            t.close_ms
                .is_some_and(|close| close.saturating_add(window.tail_ms) <= horizon_ms)
        })
    }
}

#[cfg(test)]
mod tests;
