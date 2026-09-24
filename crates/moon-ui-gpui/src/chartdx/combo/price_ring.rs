//! CPU mirror of one DX11 price-line ring: which slots an append must write, which logical range a
//! view can see, and the min/max-per-column decimation drawn when that range is denser than pixels.
//! Free of any D3D type so it is unit-testable on its own.

use bytemuck::Zeroable;
use moon_core::data::PriceLinePoint;

use super::super::types::{append_cross_ring, reset_cross_ring};

/// Upload owed to one price-line GPU buffer.
pub(crate) enum RingPending {
    /// The buffer matches the mirror.
    None,
    /// Rewrite the whole buffer from `slots`.
    Reset,
    /// Write `rows` into the slots starting at physical slot `start`, wrapping.
    Append {
        start: usize,
        rows: Vec<PriceLinePoint>,
    },
}

/// Decimation cache key: ring head, ring count and the view's pixels per millisecond.
pub(crate) type DecimKey = (usize, usize, u32);

/// One price line held as a fixed-capacity ring whose slot layout equals the GPU buffer's.
pub(crate) struct PriceRing {
    /// Capacity in points; the GPU buffer holds exactly this many.
    cap: usize,
    /// Ring slots, `cap` long once used.
    pub(crate) slots: Vec<PriceLinePoint>,
    /// Next physical slot to write.
    pub(crate) head: usize,
    /// Points held, at most `cap`.
    pub(crate) count: usize,
    /// Upload owed to the GPU buffer.
    pub(crate) pending: RingPending,
    /// Last decimation result.
    pub(crate) decim: Vec<PriceLinePoint>,
    /// Key `decim` was computed for; `None` = stale.
    pub(crate) decim_key: Option<DecimKey>,
    /// Points of `decim` uploaded to the decimation buffer.
    pub(crate) decim_len: u32,
}

impl PriceRing {
    /// Creates an empty ring with at least one GPU slot.
    pub(crate) fn new(cap: usize) -> Self {
        Self {
            cap: cap.max(1),
            slots: Vec::new(),
            head: 0,
            count: 0,
            pending: RingPending::None,
            decim: Vec::new(),
            decim_key: None,
            decim_len: 0,
        }
    }

    /// Replaces the ring with the newest `cap` points of `data`.
    pub(crate) fn reset(&mut self, data: &[PriceLinePoint]) {
        reset_cross_ring(
            &mut self.slots,
            &mut self.head,
            &mut self.count,
            self.cap,
            data,
        );
        self.slots.resize(self.cap, PriceLinePoint::zeroed());
        self.pending = RingPending::Reset;
        self.decim_key = None;
    }

    /// Appends `data` after the newest point, evicting the oldest past `cap`.
    pub(crate) fn append(&mut self, data: &[PriceLinePoint]) {
        if data.is_empty() {
            return;
        }
        if data.len() >= self.cap {
            self.reset(data);
            return;
        }
        let start = self.head;
        append_cross_ring(
            &mut self.slots,
            &mut self.head,
            &mut self.count,
            self.cap,
            data,
        );
        self.slots.resize(self.cap, PriceLinePoint::zeroed());
        self.pending = match std::mem::replace(&mut self.pending, RingPending::None) {
            RingPending::Reset => RingPending::Reset,
            RingPending::Append { start, mut rows } => {
                rows.extend_from_slice(data);
                if rows.len() >= self.cap {
                    RingPending::Reset
                } else {
                    RingPending::Append { start, rows }
                }
            }
            RingPending::None => RingPending::Append {
                start,
                rows: data.to_vec(),
            },
        };
        self.decim_key = None;
    }

    /// The GPU buffer was lost: rewrite the whole ring at the next upload.
    pub(crate) fn invalidate_gpu(&mut self) {
        if self.count > 0 {
            self.pending = RingPending::Reset;
        }
        self.decim_key = None;
    }

    /// Physical slot of the oldest point.
    pub(crate) fn start(&self) -> usize {
        (self.head + self.cap - self.count) % self.cap
    }

    /// Physical slot of logical point `i` (0 = oldest).
    pub(crate) fn physical(&self, i: usize) -> usize {
        (self.start() + i) % self.cap
    }

    /// Returns the relative time of logical point `i`.
    fn time(&self, i: usize) -> f32 {
        self.slots[self.physical(i)].time_rel_ms
    }

    /// Logical `[lo, hi)` of the points a view spanning `[left, right]` needs, with one neighbour
    /// each side so the segments crossing either edge are drawn.
    pub(crate) fn visible(&self, left: f32, right: f32) -> (usize, usize) {
        let first = partition_point(self.count, |i| self.time(i) < left);
        let past = partition_point(self.count, |i| self.time(i) <= right);
        (first.saturating_sub(1), (past + 1).min(self.count))
    }

    /// Min/max-per-pixel-column decimation of the whole ring on ABSOLUTE columns
    /// `floor(time * time_to_px)`, so a view that only scrolls reuses it: per column its first,
    /// lowest, highest and last point, in time order and without repeats, so the endpoints are kept
    /// verbatim and the drawn envelope matches the full line's. Never longer than the ring.
    pub(crate) fn m4(&self, time_to_px: f32, out: &mut Vec<PriceLinePoint>) {
        out.clear();
        let column = |t: f32| (t * time_to_px).floor() as i64;
        let mut i = 0;
        while i < self.count {
            let c = column(self.time(i));
            let first = i;
            let (mut lowest, mut highest, mut last) = (i, i, i);
            i += 1;
            while i < self.count && column(self.time(i)) == c {
                let p = self.slots[self.physical(i)].price;
                if p < self.slots[self.physical(lowest)].price {
                    lowest = i;
                }
                if p > self.slots[self.physical(highest)].price {
                    highest = i;
                }
                last = i;
                i += 1;
            }
            let mut picks = [first, lowest, highest, last];
            picks.sort_unstable();
            let mut prev = usize::MAX;
            for k in picks {
                if k != prev {
                    out.push(self.slots[self.physical(k)]);
                    prev = k;
                }
            }
        }
    }

    /// `[lo, hi)` of `decim` a view spanning `[left, right]` needs, widened by one neighbour each
    /// side like [`Self::visible`].
    pub(crate) fn decim_visible(&self, left: f32, right: f32) -> (usize, usize) {
        let d = &self.decim;
        let first = partition_point(d.len(), |i| d[i].time_rel_ms < left);
        let past = partition_point(d.len(), |i| d[i].time_rel_ms <= right);
        (first.saturating_sub(1), (past + 1).min(d.len()))
    }
}

/// First index in `0..n` for which `pred` is false; `pred` must be true then false in order.
fn partition_point(n: usize, pred: impl Fn(usize) -> bool) -> usize {
    let (mut lo, mut hi) = (0usize, n);
    while lo < hi {
        let mid = lo + (hi - lo) / 2;
        if pred(mid) {
            lo = mid + 1;
        } else {
            hi = mid;
        }
    }
    lo
}

#[cfg(test)]
mod tests;
