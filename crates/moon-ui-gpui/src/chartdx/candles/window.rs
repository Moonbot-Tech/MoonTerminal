//! CPU mirror of the DX11 candle buffer: which slots a patched tail dirtied, and which instances a
//! view can see. Free of any D3D type so it is unit-testable on its own.

use super::super::types::CandleGpu;

/// What the GPU buffer must receive at the next prepare.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CandleUpload {
    /// Nothing changed since the last upload.
    None,
    /// Rewrite every retained row.
    Full,
    /// Rewrite `len` slots from slot `first` (slot indices within the window).
    Range { first: usize, len: usize },
}

/// The instances one draw submits, starting at window slot `start`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct CandleDrawSlice {
    /// First window slot drawn; the shader adds it to `SV_InstanceID`.
    pub start: u32,
    /// Candle instances from `start`.
    pub candles: u32,
    /// Volume-hill instances from `start`; hill `i` reads rows `i` and `i + 1`.
    pub hills: u32,
}

impl CandleDrawSlice {
    /// The slice limited to the `resident` instances the GPU buffer actually holds.
    pub(crate) fn clamped(self, resident: u32) -> Self {
        if self.start >= resident {
            return Self {
                start: self.start,
                candles: 0,
                hills: 0,
            };
        }
        Self {
            start: self.start,
            candles: self.candles.min(resident - self.start),
            hills: self.hills.min(resident.saturating_sub(self.start + 1)),
        }
    }
}

/// The newest `cap` rows of the composed candle list, kept in step with the GPU buffer.
pub(crate) struct CandleWindow {
    /// The rows the GPU buffer holds (or will after the pending upload).
    rows: Vec<CandleGpu>,
    /// Running maximum of each row's right edge, so the first visible row is a binary search even
    /// though coarse fillers are wider than the series buckets after them.
    end_max: Vec<f32>,
    /// Instance capacity of the GPU buffer.
    cap: usize,
    /// Index within the full list at which the window starts.
    base: usize,
    /// Upload owed to the GPU buffer.
    pending: CandleUpload,
    /// Rows cut off the left by the capacity since the last `take_upload`.
    dropped: u64,
    /// Series bucket width in relative ms, used for rows that carry no own width.
    style_tf_rel: f32,
}

impl CandleWindow {
    /// Creates an empty window that retains at most `cap` newest candle rows.
    pub(crate) fn new(cap: usize) -> Self {
        Self {
            rows: Vec::new(),
            end_max: Vec::new(),
            cap,
            base: 0,
            pending: CandleUpload::None,
            dropped: 0,
            style_tf_rel: 0.0,
        }
    }

    /// Replaces the whole window from the full composed list.
    pub(crate) fn set(&mut self, full: &[CandleGpu]) {
        self.base = full.len().saturating_sub(self.cap);
        self.dropped = self.base as u64;
        self.rows.clear();
        self.rows.extend_from_slice(&full[self.base..]);
        self.recompute_end_max(0);
        self.pending = CandleUpload::Full;
    }

    /// Re-applies the full list's tail from `from` on; a window that moved is replaced whole.
    pub(crate) fn patch(&mut self, from: usize, full: &[CandleGpu]) {
        let new_base = full.len().saturating_sub(self.cap);
        if new_base != self.base || from < self.base || from > full.len() {
            self.set(full);
            return;
        }
        let first = from - self.base;
        if first > self.rows.len() {
            self.set(full);
            return;
        }
        self.rows.truncate(first);
        self.rows.extend_from_slice(&full[from..]);
        self.recompute_end_max(first);
        let len = self.rows.len() - first;
        self.pending = match self.pending {
            CandleUpload::Full => CandleUpload::Full,
            CandleUpload::None => CandleUpload::Range { first, len },
            CandleUpload::Range { first: f0, .. } => {
                let lo = f0.min(first);
                CandleUpload::Range {
                    first: lo,
                    len: self.rows.len() - lo,
                }
            }
        };
    }

    /// The GPU buffer was lost or recreated: everything retained must be written again.
    pub(crate) fn invalidate_gpu(&mut self) {
        if !self.rows.is_empty() {
            self.pending = CandleUpload::Full;
        }
    }

    /// Records the series bucket width rows without their own width are drawn at.
    pub(crate) fn set_style_tf(&mut self, tf_rel: f32) {
        if self.style_tf_rel.to_bits() != tf_rel.to_bits() {
            self.style_tf_rel = tf_rel;
            self.recompute_end_max(0);
        }
    }

    /// Hands out the owed upload with the rows it indexes and the rows dropped since the last
    /// call, then clears both.
    pub(crate) fn take_upload(&mut self) -> (CandleUpload, &[CandleGpu], u64) {
        let upload = std::mem::replace(&mut self.pending, CandleUpload::None);
        let dropped = std::mem::take(&mut self.dropped);
        (upload, &self.rows, dropped)
    }

    /// The instances a view spanning `[left_rel, right_rel]` can see.
    ///
    /// `start` is one row early on purpose: the hill INTO the first visible candle is drawn by the
    /// instance of the row before it. A non-finite or inverted span draws the whole window.
    pub(crate) fn draw_slice(&self, left_rel: f32, right_rel: f32) -> CandleDrawSlice {
        let n = self.rows.len();
        if n == 0 {
            return CandleDrawSlice {
                start: 0,
                candles: 0,
                hills: 0,
            };
        }
        if !(left_rel.is_finite() && right_rel.is_finite()) || right_rel < left_rel {
            return CandleDrawSlice {
                start: 0,
                candles: n as u32,
                hills: n.saturating_sub(1) as u32,
            };
        }
        let first = self.end_max.partition_point(|e| *e < left_rel);
        let last = self.rows.partition_point(|r| r.t_open_rel <= right_rel);
        let start = first.saturating_sub(1);
        let candles = last.saturating_sub(start);
        let hills = (last + 1).min(n).saturating_sub(start + 1);
        CandleDrawSlice {
            start: start as u32,
            candles: candles as u32,
            hills: hills as u32,
        }
    }

    /// Recomputes right-edge prefix maxima from the first changed row onward.
    fn recompute_end_max(&mut self, from: usize) {
        self.end_max.truncate(from);
        let mut acc = if from == 0 {
            f32::NEG_INFINITY
        } else {
            self.end_max[from - 1]
        };
        for r in &self.rows[from..] {
            let tf = if r.tf_rel > 0.0 {
                r.tf_rel
            } else {
                self.style_tf_rel
            };
            acc = acc.max(r.t_open_rel + tf);
            self.end_max.push(acc);
        }
    }
}

#[cfg(test)]
mod tests;
