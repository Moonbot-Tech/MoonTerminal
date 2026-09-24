//! Trade price fit for the chart's auto-Y, answered from blocks instead of a per-pan ring copy.
//!
//! The cursor feeds every trade row it copies or drains into a [`PriceFitIndex`]; a price query
//! then folds whole blocks that sit inside the window and scans only the rows of blocks that
//! straddle an edge. Nothing assumes the rows are sorted by time: a block's own time bounds decide
//! whether it is inside, outside or straddling.

use moonproto::state::TradeHistoryRow;

/// Rows per block. Fixed and aligned to absolute row indices, so an append rebuilds only the
/// trailing partial block and a front trim rebuilds only the first live one.
const BLOCK: usize = 256;

#[derive(Clone, Copy)]
struct Block {
    t_min: i64,
    t_max: i64,
    p_min: f32,
    p_max: f32,
    count: usize,
}

/// Retained `(unix ms, price)` trade rows with per-block time and price bounds.
pub(crate) struct PriceFitIndex {
    rows: Vec<(i64, f32)>,
    /// Leading rows already trimmed away but not yet compacted out of `rows`.
    dead: usize,
    /// `blocks[i]` covers `rows[i * BLOCK .. (i + 1) * BLOCK]` clipped to `dead..`.
    blocks: Vec<Block>,
    /// Whether the live rows are exactly the ring's rows from the first live one on: every trade
    /// since the last full copy, in time order, none the ring has evicted. A copy that hit its row
    /// cap, a drain that did not catch up, or a row older than its predecessor breaks that, and
    /// the caller then falls back to the ring until the next full copy.
    /// Exactness assumes trade rows arrive time-ordered by seq; a row the reset cursor skipped
    /// (seq after it, time before `to_time`) is outside this guarantee.
    complete: bool,
    /// A drain did not catch up with the ring: answered again after the next full copy, which the
    /// caller makes as soon as a drain catches up. Kept apart from `complete` because an
    /// out-of-order row is not repaired by catching up.
    lagging: bool,
    /// Lowest time the index still fully covers: the last full copy's start, raised by trims.
    /// A query reaching below it falls back to the ring.
    covered_from: i64,
}

impl Default for PriceFitIndex {
    fn default() -> Self {
        Self {
            rows: Vec::new(),
            dead: 0,
            blocks: Vec::new(),
            complete: false,
            lagging: false,
            covered_from: i64::MAX,
        }
    }
}

impl PriceFitIndex {
    pub(crate) fn clear(&mut self) {
        self.rows.clear();
        self.blocks.clear();
        self.dead = 0;
        self.complete = false;
        self.lagging = false;
        self.covered_from = i64::MAX;
    }

    /// Rebuild from a full copy that started at `from_ms`; `complete` is false when that copy may
    /// have been truncated.
    pub(crate) fn replace_from(&mut self, rows: &[TradeHistoryRow], complete: bool, from_ms: i64) {
        self.clear();
        self.complete = complete;
        self.covered_from = from_ms;
        self.append(rows);
    }

    /// Add a drained delta, recomputing only the trailing partial block and the new ones.
    ///
    /// A row older than the one before it marks the index incomplete: the ring's time-range copy
    /// starts at the first row at or after its window start, which assumes time order, so an
    /// out-of-order row is one the ring may skip and the index would not.
    pub(crate) fn append(&mut self, rows: &[TradeHistoryRow]) {
        if rows.is_empty() {
            return;
        }
        let first_block = self.rows.len() / BLOCK;
        let mut last = self.rows.last().map_or(i64::MIN, |row| row.0);
        for row in rows {
            let t = row.unix_millis();
            if t < last {
                self.complete = false;
            }
            last = t;
            self.rows.push((t, row.price));
        }
        self.rebuild_blocks_from(first_block);
    }

    /// A drain left rows waiting in the ring; the index defers until it is re-copied.
    pub(crate) fn mark_lagging(&mut self) {
        self.lagging = true;
    }

    /// Whether the only thing keeping the index from answering is a drain that fell behind, so a
    /// fresh full copy restores it.
    pub(crate) fn needs_recopy(&self) -> bool {
        self.lagging && self.complete
    }

    /// Keep at most `capacity` newest live rows, mirroring the ring's own eviction.
    ///
    /// Exact while complete: the index then received every sequence number in order, so the ring
    /// holds exactly the newest `capacity` of them.
    pub(crate) fn cap_live(&mut self, capacity: usize) {
        let live = self.rows.len() - self.dead;
        if live > capacity {
            self.advance_dead(self.dead + (live - capacity));
        }
    }

    /// Drop the leading rows older than `t_ms`, only while they form a prefix, and raise the
    /// covered bound to `t_ms`. Below the covered bound there is nothing to trim.
    pub(crate) fn trim_before(&mut self, t_ms: i64) {
        if t_ms < self.covered_from {
            return;
        }
        self.covered_from = t_ms;
        let mut dead = self.dead;
        while dead < self.rows.len() && self.rows[dead].0 < t_ms {
            dead += 1;
        }
        self.advance_dead(dead);
    }

    /// Move the dead prefix to `dead`, compacting once more than half the rows are dead.
    fn advance_dead(&mut self, dead: usize) {
        if dead <= self.dead {
            return;
        }
        self.dead = dead;
        if self.dead * 2 > self.rows.len() {
            self.rows.drain(..self.dead);
            self.dead = 0;
            self.rebuild_blocks_from(0);
        } else {
            let first = self.dead / BLOCK;
            self.blocks[first] = self.build_block(first);
        }
    }

    /// Price range and row count of the rows with `from_ms <= t < to_ms`.
    ///
    /// `None` when the index is incomplete, lagging, or the window reaches below what it covers:
    /// the caller must answer from the ring instead.
    pub(crate) fn range(&self, from_ms: i64, to_ms: i64) -> Option<(Option<(f32, f32)>, usize)> {
        if !self.complete || self.lagging || from_ms < self.covered_from {
            return None;
        }
        let mut lo = f32::MAX;
        let mut hi = f32::MIN;
        let mut count = 0usize;
        if from_ms < to_ms {
            for (i, block) in self.blocks.iter().enumerate().skip(self.dead / BLOCK) {
                #[cfg(test)]
                PRICE_FIT_VISITED.with(|n| n.set(n.get() + 1));
                if block.count == 0 || block.t_max < from_ms || block.t_min >= to_ms {
                    continue;
                }
                if block.t_min >= from_ms && block.t_max < to_ms {
                    lo = lo.min(block.p_min);
                    hi = hi.max(block.p_max);
                    count += block.count;
                    continue;
                }
                for &(t, p) in &self.rows[self.block_span(i)] {
                    #[cfg(test)]
                    PRICE_FIT_VISITED.with(|n| n.set(n.get() + 1));
                    if t >= from_ms && t < to_ms {
                        lo = lo.min(p);
                        hi = hi.max(p);
                        count += 1;
                    }
                }
            }
        }
        Some(((count > 0).then_some((lo, hi)), count))
    }

    fn block_span(&self, i: usize) -> std::ops::Range<usize> {
        let start = (i * BLOCK).max(self.dead);
        let end = ((i + 1) * BLOCK).min(self.rows.len());
        start..end.max(start)
    }

    fn build_block(&self, i: usize) -> Block {
        let mut block = Block {
            t_min: i64::MAX,
            t_max: i64::MIN,
            p_min: f32::MAX,
            p_max: f32::MIN,
            count: 0,
        };
        for &(t, p) in &self.rows[self.block_span(i)] {
            block.t_min = block.t_min.min(t);
            block.t_max = block.t_max.max(t);
            block.p_min = block.p_min.min(p);
            block.p_max = block.p_max.max(p);
            block.count += 1;
        }
        block
    }

    fn rebuild_blocks_from(&mut self, first: usize) {
        self.blocks.truncate(first);
        let total = self.rows.len().div_ceil(BLOCK);
        for i in first..total {
            let block = self.build_block(i);
            self.blocks.push(block);
        }
    }
}

#[cfg(test)]
thread_local! {
    static PRICE_FIT_VISITED: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// Blocks plus rows `PriceFitIndex::range` inspected on this thread since the last call; resets it.
#[cfg(test)]
#[allow(dead_code)] // read by the prover's before/after measurement
pub(crate) fn take_price_fit_visited() -> u64 {
    PRICE_FIT_VISITED.with(|n| n.replace(0))
}

#[cfg(test)]
mod tests;
