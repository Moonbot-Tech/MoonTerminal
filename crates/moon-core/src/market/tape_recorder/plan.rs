//! One key's recording plan — which stretches its trades need, when the key's pair is selected
//! and seeded with the core's archive, and what the drained live rows file. Pure: no client, no
//! clock, no disk, so every rule of the station's recording (`docs-internal/STATION.md` §4.3) is
//! a unit test.
//!
//! A key is `(exchange_key, market)`: thirty cores trading one coin on one exchange are one key and
//! one subscription. Its trades each need a stretch of prints:
//!
//! - an OPEN trade needs its run-up and everything after the entry up to the long-position
//!   threshold — it may still close as a short position, whose whole body the tuner reads. Past
//!   the threshold it is known to be long, its entry side is settled, and the key lets its pair go
//!   until the exit arrives;
//! - a CLOSED trade needs its [`ReplayWindow::focus_spans`]: the whole body of a short position,
//!   the two neighbourhoods of a long one.
//!
//! # The recording
//!
//! While any of that is unresolved the key wants its pair selected on the donor. A pair selected
//! afresh is SEEDED once: the core's chart archive merged into the donor's ring, which brings the
//! run-up — as far back as the core still holds it, the rest named lost. From then on the ring
//! only grows with the live stream, and the recorder drains it by cursor into the key's buffer.
//!
//! The buffer is filed up to a FRONTIER: [`QUIET_TAIL_MS`] behind the last moment the donor's
//! trade stream was seen alive. Up to there the stream is exhaustive — a stretch without prints is
//! a stretch the market was silent — and a print stamped earlier that arrives later (a resend) is
//! the rare exception it was built to absorb. A stream that went silent stops the frontier; one
//! that broke (a reconnect, a long silence) ends the recording, and the key is seeded again.
//!
//! A new need behind the frontier that was never filed — the exit of a long position while
//! another trade kept the pair selected, whose rows were drained when nothing needed them — is
//! seeded again the same way: the archive still holds what the core holds.

use crate::feed::Tick;
use crate::market::trade_replay::{Coverage, ReplayWindow};

/// How far behind the stream's last sign of life the tape is filed. It absorbs the core's feed
/// lag, a late resend, and the skew between this machine's clock and the venue's stamps: the
/// frontier is timed by the former, the prints by the latter.
pub(super) const QUIET_TAIL_MS: i64 = 10_000;
/// How much of a live recording is buffered before it is written: one row per write in the file,
/// so draining every second must not mean a row every second.
pub(super) const FLUSH_MS: i64 = 30_000;
/// How long past its deadline a key waits for a recording that never came — no donor, a seed
/// never answered on a stream that never came back — before the rest of it is given up as lost.
pub(super) const GIVE_UP_MS: i64 = 120_000;

/// One trade: the core that holds it and its report row. Thirty cores trading one coin open
/// thirty trades of one key within the same seconds, so stamps cannot tell them apart.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TradeId {
    pub core: u64,
    pub rec_id: i64,
}

/// One trade of a key, with the settings in force when its window was last shaped.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Trade {
    id: TradeId,
    open_ms: i64,
    close_ms: Option<i64>,
    margin_ms: i64,
    long_position_ms: i64,
    /// Handed to the comparison once settled ([`KeyTask::take_settled`]).
    reported: bool,
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

/// What one flush files: the stretches, each exhaustive (an empty one is a stretch with no
/// prints), and the prints inside them, ascending.
#[derive(Clone, Debug, Default)]
pub(super) struct Filing {
    pub file: Coverage,
    pub ticks: Vec<Tick>,
}

/// An archive asked for and not answered yet.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Seed {
    /// When it was asked, true-UTC milliseconds: the live rows start about here.
    pub asked_ms: i64,
    /// The donor's stream epoch at the ask.
    pub epoch: u64,
}

/// A recording in progress: the ring is being drained.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Live {
    /// Everything up to here is filed, lost, or was not needed when the frontier passed it.
    through_ms: i64,
    /// The newest print drained so far — where a clipped read's loss starts.
    newest_ms: i64,
    /// The donor's stream epoch the recording belongs to: a different one means it broke.
    epoch: u64,
}

/// The plan of one key.
#[derive(Clone, Debug)]
pub(super) struct KeyTask {
    trades: Vec<Trade>,
    /// Written this session — or before it, for a trade taken up again ([`Self::filed_before`]).
    filed: Coverage,
    /// Needed and out of reach: older than the archive, overwritten in the ring, given up.
    lost: Coverage,
    live: Option<Live>,
    seed: Option<Seed>,
    /// Drained prints newer than `through_ms`, waiting for the frontier.
    buffer: Vec<Tick>,
}

impl KeyTask {
    pub(super) fn new() -> Self {
        Self {
            trades: Vec::new(),
            filed: Coverage::none(),
            lost: Coverage::none(),
            live: None,
            seed: None,
            buffer: Vec::new(),
        }
    }

    /// Whether the key already has this trade.
    pub(super) fn knows(&self, id: TradeId) -> bool {
        self.trades.iter().any(|t| t.id == id)
    }

    /// Stretches the file already holds, from before this process: a trade taken up again after
    /// a restart does not record them twice, nor ask the archive for them.
    pub(super) fn filed_before(&mut self, spans: &[(i64, i64)]) {
        for &span in spans {
            self.filed.add(span);
        }
    }

    /// A trade opened.
    pub(super) fn opened(&mut self, id: TradeId, open_ms: i64, margin_ms: i64, long_ms: i64) {
        if !self.trades.iter().any(|t| t.id == id) {
            self.trades.push(Trade {
                id,
                open_ms,
                close_ms: None,
                margin_ms,
                long_position_ms: long_ms,
                reported: false,
            });
        }
    }

    /// A trade closed: its window is reshaped around both ends.
    ///
    /// The settings are the ones in force at the close, like the close-time capture's.
    pub(super) fn closed(
        &mut self,
        id: TradeId,
        open_ms: i64,
        close_ms: i64,
        margin_ms: i64,
        long_ms: i64,
    ) {
        let trade = Trade {
            id,
            open_ms,
            close_ms: Some(close_ms),
            margin_ms,
            long_position_ms: long_ms,
            reported: false,
        };
        match self.trades.iter().position(|t| t.id == id) {
            // A second closing upsert of a trade already settled is not a new window.
            Some(index) if self.trades[index].close_ms.is_some() => {}
            Some(index) => self.trades[index] = trade,
            None => self.trades.push(trade),
        }
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

    /// Written or lost — nothing left to record there.
    fn resolved(&self) -> Coverage {
        let mut out = self.filed.clone();
        for &span in self.lost.spans() {
            out.add(span);
        }
        out
    }

    /// Needed and not resolved yet.
    fn unresolved(&self) -> Coverage {
        self.needed().minus(&self.resolved())
    }

    /// The last millisecond still unresolved, or `None` when the key has nothing to record.
    pub(super) fn deadline(&self) -> Option<i64> {
        self.unresolved().hull().map(|hull| hull.1)
    }

    /// Whether the key wants its pair selected on the donor.
    pub(super) fn wants_pair(&self) -> bool {
        self.deadline().is_some()
    }

    /// Whether the key must ask for the archive now: a pair it wants and does not record yet, or
    /// a need behind its frontier that the live rows can no longer bring.
    pub(super) fn needs_seed(&self) -> bool {
        if self.seed.is_some() || !self.wants_pair() {
            return false;
        }
        match self.live {
            None => true,
            Some(live) => !self
                .unresolved()
                .clip(&Coverage::one((i64::MIN, live.through_ms)))
                .is_empty(),
        }
    }

    /// The archive asked for, if one is on its way.
    pub(super) fn seed(&self) -> Option<Seed> {
        self.seed
    }

    /// Whether the ring is being drained.
    pub(super) fn is_live(&self) -> bool {
        self.live.is_some()
    }

    /// The stream epoch the recording belongs to.
    pub(super) fn live_epoch(&self) -> Option<u64> {
        self.live.map(|live| live.epoch)
    }

    /// The archive was asked for at `asked_ms`, on the donor's stream epoch `epoch`.
    pub(super) fn seed_asked(&mut self, asked_ms: i64, epoch: u64) {
        self.seed = Some(Seed { asked_ms, epoch });
    }

    /// The archive merged: `rows` is the donor's whole ring for the pair, read from its oldest
    /// row. Whatever the key needs before the oldest print is lost; the recording starts there.
    ///
    /// Args:
    ///     rows: Every print the ring holds, in ring order.
    ///     asked_ms: When the archive was asked — where an empty ring's recording starts.
    ///     epoch: The stream epoch the ring's live rows belong to.
    ///
    /// Returns:
    ///     What is newly lost, for the log.
    pub(super) fn seeded(&mut self, rows: Vec<Tick>, asked_ms: i64, epoch: u64) -> Coverage {
        self.seed = None;
        let oldest = rows
            .iter()
            .map(|t| t.time_ms as i64)
            .min()
            .unwrap_or(asked_ms);
        let newest = rows
            .iter()
            .map(|t| t.time_ms as i64)
            .max()
            .unwrap_or(oldest);
        let lost = self.lose_before(oldest);
        self.live = Some(Live {
            through_ms: oldest.saturating_sub(1),
            newest_ms: newest,
            epoch,
        });
        self.buffer = rows;
        lost
    }

    /// The archive failed or never came: a pair not recording yet records the live stream from
    /// the ask on, the run-up lost; a pair re-seeded for a need behind its frontier loses that
    /// need.
    ///
    /// Returns:
    ///     What is newly lost, for the log.
    pub(super) fn seed_failed(&mut self) -> Coverage {
        let Some(seed) = self.seed.take() else {
            return Coverage::none();
        };
        match self.live {
            Some(live) => self.lose_before(live.through_ms.saturating_add(1)),
            None => {
                let lost = self.lose_before(seed.asked_ms);
                self.live = Some(Live {
                    through_ms: seed.asked_ms.saturating_sub(1),
                    newest_ms: seed.asked_ms,
                    epoch: seed.epoch,
                });
                self.buffer.clear();
                lost
            }
        }
    }

    /// Everything unresolved before `start_ms` is out of reach: name it lost.
    fn lose_before(&mut self, start_ms: i64) -> Coverage {
        let lost = self
            .unresolved()
            .clip(&Coverage::one((i64::MIN, start_ms.saturating_sub(1))));
        for &span in lost.spans() {
            self.lost.add(span);
        }
        lost
    }

    /// New rows drained from the ring, in ring order.
    ///
    /// Returns:
    ///     How many arrived stamped at or behind the frontier already filed — dropped.
    pub(super) fn drained(&mut self, rows: &[Tick]) -> usize {
        let Some(live) = self.live.as_mut() else {
            return 0;
        };
        let mut late = 0;
        for &row in rows {
            let at = row.time_ms as i64;
            if at <= live.through_ms {
                late += 1;
                continue;
            }
            live.newest_ms = live.newest_ms.max(at);
            self.buffer.push(row);
        }
        late
    }

    /// The ring overwrote rows before they were read: everything needed between the newest print
    /// drained and `resumed_ms`, the first print read after the overwrite, is lost.
    ///
    /// Returns:
    ///     What is newly lost, for the log.
    pub(super) fn clipped(&mut self, resumed_ms: i64) -> Coverage {
        let Some(live) = self.live else {
            return Coverage::none();
        };
        let lost = self.unresolved().clip(&Coverage::one((
            live.newest_ms.max(live.through_ms).saturating_add(1),
            resumed_ms.saturating_sub(1),
        )));
        for &span in lost.spans() {
            self.lost.add(span);
        }
        lost
    }

    /// File the buffer up to `frontier_ms` — when [`FLUSH_MS`] of it has built up, when the
    /// frontier reached the deadline, or when `force`d before the recording ends.
    ///
    /// Returns:
    ///     What to write, `None` when nothing is due.
    pub(super) fn flush(&mut self, frontier_ms: i64, force: bool) -> Option<Filing> {
        let live = self.live?;
        if frontier_ms <= live.through_ms {
            return None;
        }
        let due = force
            || frontier_ms - live.through_ms >= FLUSH_MS
            || self
                .deadline()
                .is_none_or(|deadline| deadline <= frontier_ms);
        if !due {
            return None;
        }
        let file = self
            .needed()
            .clip(&Coverage::one((
                live.through_ms.saturating_add(1),
                frontier_ms,
            )))
            .minus(&self.resolved());
        let mut ticks: Vec<Tick> = self
            .buffer
            .iter()
            .copied()
            .filter(|t| file.contains_ms(t.time_ms as i64))
            .collect();
        ticks.sort_by(|a, b| a.time_ms.total_cmp(&b.time_ms));
        self.buffer.retain(|t| t.time_ms as i64 > frontier_ms);
        for &span in file.spans() {
            self.filed.add(span);
        }
        if let Some(live) = self.live.as_mut() {
            live.through_ms = frontier_ms;
        }
        Some(Filing { file, ticks })
    }

    /// The recording ended — the pair was let go, or its stream broke and the key will be seeded
    /// again. What was not flushed before this is dropped.
    pub(super) fn stop(&mut self) {
        self.live = None;
        self.seed = None;
        self.buffer = Vec::new();
    }

    /// Still unresolved [`GIVE_UP_MS`] past the deadline, whatever kept the recording away.
    pub(super) fn overdue(&self, now_ms: i64) -> bool {
        self.deadline()
            .is_some_and(|deadline| now_ms > deadline.saturating_add(GIVE_UP_MS))
    }

    /// Name everything still unresolved lost and let the key settle.
    ///
    /// Returns:
    ///     What was given up.
    pub(super) fn give_up(&mut self) -> Coverage {
        let lost = self.unresolved();
        for &span in lost.spans() {
            self.lost.add(span);
        }
        lost
    }

    /// Closed trades whose every needed stretch is now filed or lost — each handed out once, as
    /// `(open_ms, close_ms, needed)`. Taken before [`Self::is_done`] drops the key.
    pub(super) fn take_settled(&mut self) -> Vec<(i64, i64, Coverage)> {
        let resolved = self.resolved();
        let mut out = Vec::new();
        for trade in &mut self.trades {
            let Some(close_ms) = trade.close_ms else {
                continue;
            };
            let needed = trade.needed();
            if !trade.reported && resolved.covers(&needed) {
                trade.reported = true;
                out.push((trade.open_ms, close_ms, needed));
            }
        }
        out
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
}

#[cfg(test)]
mod tests;
