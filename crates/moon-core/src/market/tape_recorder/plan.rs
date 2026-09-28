//! One key's recording plan — which stretches its trades need, what one archive answer files, and
//! when the key asks next. Pure: no client, no clock, no disk, so every rule of the station's
//! schedule (`docs-internal/STATION.md` §4.3) is a unit test.
//!
//! A key is `(exchange_key, market)`: thirty cores trading one coin on one exchange are one key and
//! one stream of requests. Its trades each need a stretch of prints:
//!
//! - an OPEN trade needs its run-up and everything after the entry up to the long-position
//!   threshold — it may still close as a short position, whose whole body the tuner reads. Past
//!   the threshold it is known to be long, its entry side is settled, and the key stops asking
//!   until the exit arrives;
//! - a CLOSED trade needs its [`ReplayWindow::focus_spans`]: the whole body of a short position,
//!   the two neighbourhoods of a long one.
//!
//! An answer is the donor's retained ring after the archive merged into it. It is exhaustive from
//! its oldest print up to its newest less the last second, which the core is still coarsening
//! (moonproto keeps the same tail, `ARCHIVE_LIVE_TAIL_MS`). A market that has been silent for a
//! while answers with an old newest print; the silence up to shortly before the ask is then real
//! and filed as covered and empty — otherwise a quiet coin's exit could never be settled.
//! Whatever a trade needs before the oldest print is gone for good: the recorder names it lost
//! and asks twice as often.

use crate::market::trade_replay::{Coverage, ReplayWindow};

/// Shortest pause between two asks of one key.
pub(super) const MIN_STEP_MS: i64 = 3_000;
/// Longest pause between two asks of one key.
pub(super) const MAX_STEP_MS: i64 = 60_000;
/// The newest second of an answer is not filed: the core coarsens fresh prints late.
pub(super) const LIVE_TAIL_MS: i64 = 1_000;
/// How far behind the ask a silent market is filed as covered. Wider than the live tail: it has
/// to absorb the core's feed lag and the skew between this machine's clock and the venue's stamps,
/// since the ask is timed by the former and the prints by the latter.
pub(super) const QUIET_TAIL_MS: i64 = 10_000;
/// The last ask of a key comes this long after its deadline, so that even a silent market's
/// answer reaches past it ([`QUIET_TAIL_MS`]).
pub(super) const LAST_ASK_SLACK_MS: i64 = QUIET_TAIL_MS + 2_000;
/// Pause before asking again after a request that failed or never came back.
pub(super) const RETRY_MS: i64 = 6_000;
/// How far apart an open and a close stamp of the same trade may land. Both are lifted onto true
/// UTC through the core's measured clock offset, which may be re-measured between the two events.
const SAME_TRADE_MS: i64 = 5_000;

/// One trade of a key, with the settings in force when its window was last shaped.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Trade {
    open_ms: i64,
    close_ms: Option<i64>,
    margin_ms: i64,
    long_position_ms: i64,
}

impl Trade {
    /// The stretches this trade needs filed — see the module header.
    fn needed(self) -> Coverage {
        let margin = self.margin_ms.max(0);
        match self.close_ms {
            None => Coverage::one((
                self.open_ms.saturating_sub(margin),
                self.open_ms
                    .saturating_add(self.long_position_ms.max(margin)),
            )),
            Some(close_ms) => ReplayWindow {
                from_ms: self.open_ms.saturating_sub(margin),
                to_ms: close_ms.saturating_add(margin),
                open_ms: self.open_ms,
                close_ms,
                margin_ms: margin,
                long_position_ms: self.long_position_ms,
                over_budget: false,
            }
            .focus_spans(),
        }
    }
}

/// What one answer filed and lost, for the recorder to write and log.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct Filing {
    /// Stretches to write, each exhaustive: an empty one is a stretch with no prints.
    pub file: Coverage,
    /// Stretches needed and now out of reach of any answer.
    pub lost: Coverage,
}

/// The plan of one key.
#[derive(Clone, Debug)]
pub(super) struct KeyTask {
    trades: Vec<Trade>,
    /// Written this session.
    filed: Coverage,
    /// Needed and gone before any answer reached them.
    lost: Coverage,
    step_ms: i64,
    next_due_ms: i64,
}

impl KeyTask {
    pub(super) fn new() -> Self {
        Self {
            trades: Vec::new(),
            filed: Coverage::none(),
            lost: Coverage::none(),
            step_ms: MAX_STEP_MS,
            next_due_ms: i64::MAX,
        }
    }

    /// A trade opened: ask at once, while the ring still holds the run-up.
    pub(super) fn opened(&mut self, now_ms: i64, open_ms: i64, margin_ms: i64, long_ms: i64) {
        if self.matching_open(open_ms).is_none() {
            self.trades.push(Trade {
                open_ms,
                close_ms: None,
                margin_ms,
                long_position_ms: long_ms,
            });
        }
        self.next_due_ms = now_ms;
    }

    /// A trade closed: reshape its window around both ends and ask at once.
    ///
    /// The settings are the ones in force at the close, like the close-time capture's.
    pub(super) fn closed(
        &mut self,
        now_ms: i64,
        open_ms: i64,
        close_ms: i64,
        margin_ms: i64,
        long_ms: i64,
    ) {
        let trade = Trade {
            open_ms,
            close_ms: Some(close_ms),
            margin_ms,
            long_position_ms: long_ms,
        };
        match self.matching_open(open_ms) {
            Some(index) => self.trades[index] = trade,
            None => self.trades.push(trade),
        }
        self.next_due_ms = now_ms;
    }

    /// The open trade whose entry lands nearest `open_ms`, within [`SAME_TRADE_MS`].
    fn matching_open(&self, open_ms: i64) -> Option<usize> {
        self.trades
            .iter()
            .enumerate()
            .filter(|(_, t)| t.close_ms.is_none())
            .map(|(index, t)| (index, (t.open_ms - open_ms).abs()))
            .filter(|&(_, distance)| distance <= SAME_TRADE_MS)
            .min_by_key(|&(_, distance)| distance)
            .map(|(index, _)| index)
    }

    /// Everything the key's trades need.
    pub(super) fn needed(&self) -> Coverage {
        let mut out = Coverage::none();
        for trade in &self.trades {
            for &span in trade.needed().spans() {
                out.add(span);
            }
        }
        out
    }

    /// Written or lost — nothing left to ask for there.
    fn resolved(&self) -> Coverage {
        let mut out = self.filed.clone();
        for &span in self.lost.spans() {
            out.add(span);
        }
        out
    }

    /// The last millisecond an unresolved trade still needs, or `None` when every trade is
    /// resolved and the key has nothing to ask for.
    fn deadline(&self) -> Option<i64> {
        let resolved = self.resolved();
        self.trades
            .iter()
            .map(|t| t.needed())
            .filter(|needed| !resolved.covers(needed))
            .filter_map(|needed| needed.hull().map(|hull| hull.1))
            .max()
    }

    /// When the key asks next, or `None` while it waits for an exit or is finished.
    pub(super) fn due_ms(&self) -> Option<i64> {
        self.deadline().map(|_| self.next_due_ms)
    }

    /// Every trade closed and every stretch resolved: the key can be dropped.
    pub(super) fn is_done(&self) -> bool {
        self.trades.iter().all(|t| t.close_ms.is_some()) && self.deadline().is_none()
    }

    /// Forget trades that opened before `horizon_ms` and never closed — an exit this session
    /// did not see, which would otherwise keep the key forever.
    pub(super) fn drop_stale_opens(&mut self, horizon_ms: i64) {
        self.trades
            .retain(|t| t.close_ms.is_some() || t.open_ms >= horizon_ms);
    }

    /// File one answer: the ring held prints from `oldest_ms` to `newest_ms`, or nothing.
    ///
    /// Args:
    ///     now_ms: When the answer arrived, true-UTC milliseconds.
    ///     ring: The ring's oldest and newest print, `None` for an empty ring.
    ///
    /// Returns:
    ///     What to write and what is lost; the next ask is scheduled inside.
    pub(super) fn on_answer(&mut self, now_ms: i64, ring: Option<(i64, i64)>) -> Filing {
        let Some((oldest_ms, newest_ms)) = ring else {
            // Nothing to measure a depth by: the slowest step, and nothing claimed.
            self.step_ms = MAX_STEP_MS;
            self.schedule(now_ms);
            return Filing::default();
        };
        let covered_to = newest_ms
            .saturating_sub(LIVE_TAIL_MS)
            .max(now_ms.saturating_sub(QUIET_TAIL_MS));
        let needed = self.needed();
        let file = needed
            .clip(&Coverage::one((oldest_ms, covered_to)))
            .minus(&self.filed);
        let lost = needed
            .clip(&Coverage::one((i64::MIN, oldest_ms.saturating_sub(1))))
            .minus(&self.resolved());
        for &span in file.spans() {
            self.filed.add(span);
        }
        for &span in lost.spans() {
            self.lost.add(span);
        }
        let step = (newest_ms.saturating_sub(oldest_ms) / 10).clamp(MIN_STEP_MS, MAX_STEP_MS);
        self.step_ms = match lost.is_empty() {
            true => step,
            false => (step / 2).max(MIN_STEP_MS),
        };
        self.schedule(now_ms);
        Filing { file, lost }
    }

    /// A request failed or never came back: ask again shortly.
    pub(super) fn on_failure(&mut self, now_ms: i64) {
        self.next_due_ms = now_ms.saturating_add(RETRY_MS);
    }

    /// Next ask: one step on, but never later than the last ask past the deadline.
    fn schedule(&mut self, now_ms: i64) {
        let step_due = now_ms.saturating_add(self.step_ms);
        self.next_due_ms = match self.deadline() {
            Some(deadline) => step_due
                .min(deadline.saturating_add(LAST_ASK_SLACK_MS))
                .max(now_ms.saturating_add(MIN_STEP_MS)),
            None => step_due,
        };
    }
}

#[cfg(test)]
mod tests;
