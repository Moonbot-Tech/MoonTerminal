//! Quote-notional tick readouts, independent of candle buckets and GPU upload timing.

/// Convert a base quantity to a quote amount for the readout only; bar heights remain unchanged.
/// Invalid or unrepresentable inputs have no numeric readout, rather than an invented value.
pub fn quote_notional(price: f32, quantity: f32) -> f32 {
    if !price.is_finite() || !quantity.is_finite() || price <= 0.0 || quantity <= 0.0 {
        return 0.0;
    }
    let value = price * quantity;
    if value.is_finite() { value } else { 0.0 }
}

/// Count and individual-value range for one side of the cursor's nearby tick column.
/// Values are never summed: overlapping bars remain independent prints.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TickVolumeRange {
    pub count: usize,
    pub min: f32,
    pub max: f32,
}

/// Chart time under the cursor, relative to the chart epoch, for a cursor inside the plot.
///
/// The whole plot qualifies, not only the native tick-volume band along its floor: the readout
/// answers "what traded here" for the candle the pointer is over as well as for the prints in the
/// band, and a reader hovering a candle body is asking exactly that question. Bounds and cursor use
/// device pixels.
pub fn cursor_time(bounds: [f32; 4], time0: f32, time_to_px: f32, cursor: [f32; 2]) -> Option<f32> {
    let [left, top, width, height] = bounds;
    if bounds.iter().chain(cursor.iter()).any(|v| !v.is_finite())
        || !time0.is_finite()
        || !time_to_px.is_finite()
        || time_to_px <= 0.0
        || width <= 0.0
        || height <= 0.0
    {
        return None;
    }
    if !(left..=left + width).contains(&cursor[0]) || !(top..=top + height).contains(&cursor[1]) {
        return None;
    }
    let at = time0 + (cursor[0] - left) / time_to_px;
    at.is_finite().then_some(at)
}

/// Resolve the nearby time column for a cursor anywhere inside the visible plot.
///
/// Shares [`cursor_time`]'s validation so the column and the candle lookup can never disagree about
/// whether the pointer is on the chart at all. The +/-3px pick tolerance is in logical pixels and is
/// scaled once, here.
pub fn cursor_column(
    bounds: [f32; 4],
    time0: f32,
    time_to_px: f32,
    cursor: [f32; 2],
    scale: f32,
) -> Option<(f32, f32)> {
    if !scale.is_finite() || scale <= 0.0 {
        return None;
    }
    cursor_time(bounds, time0, time_to_px, cursor)?;
    let [left, _, width, _] = bounds;
    let from = time0 + (cursor[0] - left - 3.0 * scale).max(0.0) / time_to_px;
    let to = time0 + (cursor[0] - left + 3.0 * scale).min(width) / time_to_px;
    (from.is_finite() && to.is_finite()).then_some((from, to))
}

/// Whether the per-trade band the tick rows describe is drawn at all.
///
/// The rows report individual prints out of the native tick ring, so they stay tied to that ring
/// being visualized; the candle figure beside them is a property of the candle itself and is not
/// gated by this.
pub fn tick_ranges_visible(alpha: f32) -> bool {
    alpha.is_finite() && alpha > 0.0
}

/// Collect both sides in a bounded time column; skip liquidations and unusable values.
/// Equal timestamps are deliberately retained, including equal-valued independent prints.
pub fn nearby_ticks(
    samples: impl IntoIterator<Item = (f32, u32, f32)>,
    from: f32,
    to: f32,
) -> [Option<TickVolumeRange>; 2] {
    let mut result: [Option<TickVolumeRange>; 2] = [None, None];
    if !from.is_finite() || !to.is_finite() || from > to {
        return result;
    }
    for (time, side, value) in samples {
        if !time.is_finite()
            || time < from
            || time > to
            || side > 1
            || !value.is_finite()
            || value <= 0.0
        {
            continue;
        }
        match &mut result[side as usize] {
            Some(range) => {
                range.count += 1;
                range.min = range.min.min(value);
                range.max = range.max.max(value);
            }
            slot @ None => {
                *slot = Some(TickVolumeRange {
                    count: 1,
                    min: value,
                    max: value,
                })
            }
        }
    }
    result
}

/// Read the ring that the next upload will publish, without copying it or touching the GPU.
/// A pending reset replaces resident data; pending appends evict the oldest rows at capacity.
pub fn pending_ring<'a, T>(
    resident: &'a [T],
    head: usize,
    count: usize,
    capacity: usize,
    reset: Option<&'a [T]>,
    append: &'a [T],
) -> impl ExactSizeIterator<Item = &'a T> + Clone {
    let old_count = reset
        .map_or(count.min(resident.len()), <[T]>::len)
        .min(capacity);
    let total = old_count + append.len();
    (total.saturating_sub(capacity)..total).map(move |index| {
        if index >= old_count {
            &append[index - old_count]
        } else if let Some(reset) = reset {
            &reset[reset.len() - old_count + index]
        } else {
            let start = if old_count == capacity {
                head % capacity
            } else {
                0
            };
            &resident[(start + index) % capacity]
        }
    })
}

/// Index the next-upload ring in O(1), directly through resident/reset/append slices.
/// The index is relative to the retained chronological tail, exactly like [`pending_ring`].
/// Panics when the index is outside that tail; an empty/zero-capacity ring has no valid index.
pub fn pending_ring_at<'a, T>(
    resident: &'a [T],
    head: usize,
    count: usize,
    capacity: usize,
    reset: Option<&'a [T]>,
    append: &'a [T],
    index: usize,
) -> &'a T {
    let old_count = reset
        .map_or(count.min(resident.len()), <[T]>::len)
        .min(capacity);
    let total = old_count + append.len();
    assert!(index < total.min(capacity), "pending ring index is bounded");
    let index = total.saturating_sub(capacity) + index;
    if index >= old_count {
        &append[index - old_count]
    } else if let Some(reset) = reset {
        &reset[reset.len() - old_count + index]
    } else {
        let origin = if old_count == capacity {
            head % capacity
        } else {
            0
        };
        &resident[(origin + index) % capacity]
    }
}

/// Arrival-time evidence for conservative searches over late, interleaved ticks.
/// Lateness never shrinks until the owner resets the evidence with its storage.
#[derive(Clone, Copy, Debug, Default)]
pub struct TickTimeOrder {
    prefix_max: Option<f32>,
    max_lateness: f64,
}

impl TickTimeOrder {
    /// Track max(prefix maximum before each row - row time, 0) in O(batch).
    /// Non-finite input permanently forces a full walk until the evidence is reset.
    pub fn extend(&mut self, times: impl IntoIterator<Item = f32>) {
        for time in times {
            if !time.is_finite() {
                self.max_lateness = f64::INFINITY;
                continue;
            }
            if let Some(max) = self.prefix_max {
                self.max_lateness = self.max_lateness.max(f64::from(max) - f64::from(time));
                self.prefix_max = Some(max.max(time));
            } else {
                self.prefix_max = Some(time);
            }
        }
    }

    /// Maximum observed lateness, or infinity if any recorded time was non-finite.
    pub fn max_lateness(self) -> f64 {
        self.max_lateness
    }
}

/// Find conservative candidates for an inclusive window using O(log n) O(1) index probes.
/// Callers keep their exact per-row predicate; invalid bounds/evidence select the whole ring.
///
/// Let L bound every row's lateness from its preceding prefix maximum. After the first
/// prefix maximum reaches from, every later time is >= from - L. Thus plain binary
/// search for from - L cannot skip that first row: any false predicate is before it,
/// and all skipped rows are < from. For the upper bound, any row before the last
/// time <= to is <= to + L (otherwise that last row would exceed L). Searching for
/// the first time > to + L therefore ends after that last row. Neither predicate
/// needs to be monotone inside the widened fringe; widening only adds candidates.
/// Compute bounds in f64 so subtracting finite f32 extremes cannot overflow.
pub fn tick_time_range(
    len: usize,
    max_lateness: f64,
    from: f64,
    to: f64,
    time_at: impl Fn(usize) -> f32,
) -> std::ops::Range<usize> {
    if !max_lateness.is_finite()
        || max_lateness < 0.0
        || !from.is_finite()
        || !to.is_finite()
        || from > to
    {
        return 0..len;
    }
    let partition = |bound: f64, inclusive: bool| {
        let (mut lo, mut hi) = (0, len);
        while lo < hi {
            let mid = lo + (hi - lo) / 2;
            let time = f64::from(time_at(mid));
            if time < bound || (inclusive && time == bound) {
                lo = mid + 1;
            } else {
                hi = mid;
            }
        }
        lo
    };
    partition(from - max_lateness, false)..partition(to + max_lateness, true)
}

/// Convert chronological bounds into at most two physical runs, in original GPU draw order.
/// Keeping ascending slot order preserves overlapping tick colours and alpha blending.
pub fn tick_slot_runs(
    range: std::ops::Range<usize>,
    head: usize,
    count: usize,
    capacity: usize,
) -> [(usize, usize); 2] {
    if range.is_empty() || capacity == 0 {
        return [(0, 0), (0, 0)];
    }
    let origin = if count == capacity {
        head % capacity
    } else {
        0
    };
    let start = (origin + range.start) % capacity;
    let first = range.len().min(capacity - start);
    let second = range.len() - first;
    if second == 0 {
        [(start, first), (0, 0)]
    } else {
        [(0, second), (start, first)]
    }
}

/// Conservative bake bounds including marker size, volume half-width and shader rounding.
/// Invalid transforms select all rows, retaining the shader as the final culling authority.
pub fn tick_bake_span(time0: f32, width: f32, time_to_px: f32, marker_half: f32) -> (f64, f64) {
    if !time0.is_finite()
        || !width.is_finite()
        || !time_to_px.is_finite()
        || time_to_px <= 1e-9
        || !marker_half.is_finite()
    {
        return (f64::NEG_INFINITY, f64::INFINITY);
    }
    // The shader rounds cross centres, culls at max(8, half + 1), and draws bars up to 3px wide.
    let margin = marker_half.max(0.0).max(8.0) + 2.0;
    // Compute in f32 like the shader, then widen by one representable time on each side.
    let left = (time0 - margin / time_to_px).next_down();
    let right = (time0 + (width + margin) / time_to_px).next_up();
    (f64::from(left), f64::from(right))
}

/// Whether overwriting this tick might remove a pixel from the cached bitmap.
pub fn tick_touches_bake(time: f32, span: (f64, f64)) -> bool {
    !time.is_finite() || (f64::from(time) >= span.0 && f64::from(time) <= span.1)
}

#[cfg(test)]
mod tests;
