//! Sequence-indexed price contributions, summed in the original mini-then-trade order.

use moonproto::state::{SeqRingCursor, SeqRingReader, SeqRingTimedRow};
use std::collections::{HashMap, VecDeque};

/// One retained input's sequence identity and timestamp; prices live only in their bins.
#[derive(Default)]
pub(super) struct Tape {
    cursor: Option<SeqRingCursor>,
    rows: VecDeque<(u64, i64)>,
    next: u64,
    lower: i64,
}

impl Tape {
    /// Drain only new retained inputs, releasing the ring lock before any price folding.
    pub(super) fn drain<T: SeqRingTimedRow>(
        &mut self,
        reader: Option<&SeqRingReader<T>>,
        out: &mut Vec<T>,
    ) {
        out.clear();
        let Some(reader) = reader else {
            self.rows.clear();
            self.cursor = None;
            return;
        };
        let cursor = self
            .cursor
            .get_or_insert_with(|| reader.cursor_from_oldest());
        let meta = reader.drain_new_bounded(cursor, reader.capacity(), out);
        if meta.clipped {
            self.rows.clear();
        }
    }

    /// Add one input, retaining late resends after the first in-window row as the old copy did.
    pub(super) fn push(&mut self, at: i64, lower: i64) -> Option<u64> {
        let id = self.next;
        self.next += 1;
        if self.rows.is_empty() && at < lower {
            return None;
        }
        self.rows.push_back((id, at));
        Some(id)
    }

    /// Expire the same sequence prefix as a fresh time seek, plus overwritten retained inputs.
    pub(super) fn trim(&mut self, capacity: usize, lower: i64) {
        let oldest = self.next.saturating_sub(capacity as u64);
        while self
            .rows
            .front()
            .is_some_and(|&(id, at)| id < oldest || at < lower)
        {
            self.rows.pop_front();
        }
        self.lower = lower;
    }

    /// The first contribution still selected by both retention and the trailing window.
    pub(super) fn first(&self) -> u64 {
        self.rows.front().map_or(self.next, |row| row.0)
    }

    /// The raw tail begins strictly after the newest selected aggregate, including late rows.
    pub(super) fn newest(&self) -> Option<i64> {
        self.rows.iter().map(|row| row.1).max()
    }

    /// A backwards seam needs inputs previously skipped by the time seek, so re-read that source.
    pub(super) fn moved_back(&self, lower: i64) -> bool {
        lower < self.lower
    }
}

/// One source's contribution to a price bin, in sequence order.
struct Value {
    id: u64,
    buy: f64,
    sell: f64,
}

/// A bin keeps its ordered addends so expiry never changes floating-point summation order.
#[derive(Default)]
struct Bin {
    minis: VecDeque<Value>,
    trades: VecDeque<Value>,
    total: (f64, f64),
    dirty: bool,
}

/// Incremental bin membership; only changed bins are re-summed, without re-binning old rows.
#[derive(Default)]
pub(super) struct Bins(HashMap<i64, Bin>);

impl Bins {
    /// Place one addend in its source's sequence order.
    pub(super) fn add(&mut self, mini: bool, seq: u64, id: i64, buy: f64, sell: f64) {
        let bin = self.0.entry(id).or_default();
        let values = if mini {
            &mut bin.minis
        } else {
            &mut bin.trades
        };
        values.push_back(Value { id: seq, buy, sell });
        bin.dirty = true;
    }

    /// Drop one source when its seam moves backwards, preserving the other source's addends.
    pub(super) fn clear_trades(&mut self) {
        for bin in self.0.values_mut() {
            if !bin.trades.is_empty() {
                bin.trades.clear();
                bin.dirty = true;
            }
        }
    }

    /// Expire prefixes and reproduce the old fold's exact ordered additions for each changed bin.
    pub(super) fn totals(
        &mut self,
        mini_first: u64,
        trade_first: u64,
    ) -> impl Iterator<Item = (i64, (f64, f64))> + '_ {
        self.0.retain(|_, bin| {
            for (values, first) in [(&mut bin.minis, mini_first), (&mut bin.trades, trade_first)] {
                while values.front().is_some_and(|value| value.id < first) {
                    values.pop_front();
                    bin.dirty = true;
                }
            }
            if bin.dirty {
                bin.total = bin
                    .minis
                    .iter()
                    .chain(&bin.trades)
                    .fold((0.0, 0.0), |(buy, sell), value| {
                        (buy + value.buy, sell + value.sell)
                    });
                bin.dirty = false;
            }
            !bin.minis.is_empty() || !bin.trades.is_empty()
        });
        self.0.iter().map(|(&id, bin)| (id, bin.total))
    }
}

#[cfg(test)]
mod tests;
