//! What the recorder holds of one closed trade, asked by whoever draws it (the Telegram deal
//! chart) seconds after the close — long before the key's tail is filed.
//!
//! The answer is built on the recorder's own thread: what the file holds through the recorder's
//! own [`TradeCache`] handle — its queue is the one every filing went through, so the read sees
//! every print already handed to it — and then the prints still buffered past the last filing.
//! Read from outside, the two halves could miss each other: a flush moves prints out of the buffer
//! before the file has them.

use std::collections::HashMap;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use super::{Cmd, Key, Recorder, TX, TradeId};
use crate::feed::Tick;
use crate::market::trade_replay::Coverage;

/// How long a closed trade is remembered for a reader. A picture is drawn within minutes of the
/// close or not at all.
const CLOSED_MEMORY: Duration = Duration::from_secs(3600);

/// How far past the window's end the stream must be seen before the window counts as settled: the
/// stream's last sign of life moves every pump (250 ms), the buffer only every drain (1 s), so the
/// last prints of the window may still sit in the donor's ring when the stream first passes it.
const SETTLE_LAG_MS: i64 = 2_000;

/// A closed trade as the recorder saw it close.
pub(super) struct Closed {
    key: Key,
    open_ms: i64,
    close_ms: i64,
    seen: Instant,
}

/// Closed trades by id, for [`trade_tape`].
#[derive(Default)]
pub(super) struct ClosedTrades(HashMap<TradeId, Closed>);

impl ClosedTrades {
    /// Remember a close, replacing an earlier one of the same trade.
    pub(super) fn closed(&mut self, trade: TradeId, key: &Key, open_ms: i64, close_ms: i64) {
        self.0.insert(
            trade,
            Closed {
                key: key.clone(),
                open_ms,
                close_ms,
                seen: Instant::now(),
            },
        );
    }

    /// Forget closes older than [`CLOSED_MEMORY`].
    pub(super) fn prune(&mut self, now: Instant) {
        self.0
            .retain(|_, closed| now.duration_since(closed.seen) < CLOSED_MEMORY);
    }
}

/// One closed trade's tape, as the recorder holds it at the moment of asking.
#[derive(Clone, Debug)]
pub struct TradeTape {
    /// The key's exchange, `"{code}:{dex:08x}"`.
    pub exchange: String,
    /// The market the trade's prints are recorded under.
    pub market: String,
    /// Entry and exit, true-UTC milliseconds.
    pub open_ms: i64,
    pub close_ms: i64,
    /// Every print held inside the asked window, ascending by time.
    pub ticks: Vec<Tick>,
    /// Nothing more will come for the window: the stream was seen past its end, or the key's
    /// recording is over and everything it had is filed.
    pub settled: bool,
    /// Every contiguous recorded stretch in the requested window, ascending by time.
    pub covered: Vec<(i64, i64)>,
}

/// What one requested market window holds, without treating an empty or partial tape as full.
#[derive(Clone, Debug)]
pub struct RecorderSpan {
    pub ticks: Vec<Tick>,
    /// No more recording is expected for this window; this does not imply complete coverage.
    pub settled: bool,
    /// Every clipped contiguous recorded stretch, ascending and excluding known stream gaps.
    pub covered: Vec<(i64, i64)>,
}

/// Why [`trade_tape`] has no tape to give.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TapeMiss {
    /// The recorder is not running, or does not remember the trade's close: it has not seen it
    /// yet, it closed before this process saw it, or longer ago than an hour.
    Unknown,
    /// The recorder or its file did not answer in time; asking again later may.
    Busy,
}

/// The prints of `trade` from `lead_ms` before its entry to `tail_ms` after its exit.
pub fn trade_tape(
    trade: TradeId,
    lead_ms: i64,
    tail_ms: i64,
    timeout: Duration,
) -> Result<TradeTape, TapeMiss> {
    let Some(Some(tx)) = TX.get() else {
        return Err(TapeMiss::Unknown);
    };
    let (reply, answer) = mpsc::channel();
    tx.send(Cmd::Peek {
        trade,
        lead_ms,
        tail_ms,
        reply,
    })
    .map_err(|_| TapeMiss::Unknown)?;
    // A recorder switched off drops the question unanswered: the sender is gone, not late.
    match answer.recv_timeout(timeout) {
        Ok(tape) => tape,
        Err(mpsc::RecvTimeoutError::Timeout) => Err(TapeMiss::Busy),
        Err(mpsc::RecvTimeoutError::Disconnected) => Err(TapeMiss::Unknown),
    }
}

/// Read a market's filed and buffered prints, settlement and contiguous coverage inside inclusive
/// UTC bounds without starting the recorder. Returns `Unknown` when absent and `Busy` on timeout.
pub fn market_tape(
    exchange_key: &str,
    market: &str,
    from_ms: i64,
    to_ms: i64,
    timeout: Duration,
) -> Result<RecorderSpan, TapeMiss> {
    let Some(Some(tx)) = TX.get() else {
        return Err(TapeMiss::Unknown);
    };
    let (reply, answer) = mpsc::channel();
    tx.send(Cmd::PeekKey {
        key: (exchange_key.to_owned(), market.to_owned()),
        from_ms,
        to_ms,
        reply,
    })
    .map_err(|_| TapeMiss::Unknown)?;
    match answer.recv_timeout(timeout) {
        Ok(tape) => tape,
        Err(mpsc::RecvTimeoutError::Timeout) => Err(TapeMiss::Busy),
        Err(mpsc::RecvTimeoutError::Disconnected) => Err(TapeMiss::Unknown),
    }
}

impl Recorder {
    /// Read only the requested span through the recorder's own cache queue before adding
    /// unfiled prints, so a concurrent flush cannot leave a gap between the two halves. Return
    /// settlement separately from coverage: a recording can stop with holes still unfilled.
    pub(super) fn read_key(
        &self,
        key: &Key,
        from_ms: i64,
        to_ms: i64,
    ) -> Result<RecorderSpan, TapeMiss> {
        if from_ms > to_ms {
            return Ok(RecorderSpan {
                ticks: Vec::new(),
                settled: true,
                covered: Vec::new(),
            });
        }
        let spans = self
            .cache
            .read(&key.0, &key.1, from_ms, to_ms)
            .ok_or(TapeMiss::Busy)?;
        let mut covered = Coverage::from_spans(spans.iter().map(|span| (span.from_ms, span.to_ms)));
        let mut ticks: Vec<Tick> = spans
            .into_iter()
            .flat_map(|span| span.ticks)
            .filter(|t| (from_ms..=to_ms).contains(&(t.time_ms as i64)))
            .collect();
        let mut settled = true;
        if let Some(task) = self.tasks.get(key) {
            ticks.extend(task.unfiled(from_ms, to_ms));
            let alive_ms = task
                .live_epoch()
                .and_then(|epoch| self.donors.get(&key.0)?.alive_ms(epoch));
            if let Some(alive_ms) = alive_ms {
                for &span in task
                    .buffered_coverage(alive_ms.saturating_sub(SETTLE_LAG_MS))
                    .spans()
                {
                    covered.add(span);
                }
            }
            settled = alive_ms.is_some_and(|alive| alive >= to_ms.saturating_add(SETTLE_LAG_MS))
                || !task.wants_pair();
        }
        ticks.sort_by(|a, b| a.time_ms.total_cmp(&b.time_ms));
        let covered = covered
            .clip(&Coverage::one((from_ms, to_ms)))
            .spans()
            .to_vec();
        Ok(RecorderSpan {
            ticks,
            settled,
            covered,
        })
    }

    /// Answer [`trade_tape`] on the recorder's thread.
    pub(super) fn peek(
        &self,
        trade: TradeId,
        lead_ms: i64,
        tail_ms: i64,
    ) -> Result<TradeTape, TapeMiss> {
        let closed = self.closed.0.get(&trade).ok_or(TapeMiss::Unknown)?;
        let key = &closed.key;
        let from_ms = closed.open_ms.saturating_sub(lead_ms.max(0));
        let to_ms = closed.close_ms.saturating_add(tail_ms.max(0));
        let span = self.read_key(key, from_ms, to_ms)?;
        Ok(TradeTape {
            exchange: key.0.clone(),
            market: key.1.clone(),
            open_ms: closed.open_ms,
            close_ms: closed.close_ms,
            ticks: span.ticks,
            settled: span.settled,
            covered: span.covered,
        })
    }
}

#[cfg(test)]
mod tests;
