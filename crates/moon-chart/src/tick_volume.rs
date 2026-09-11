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
) -> impl Iterator<Item = &'a T> {
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

#[cfg(test)]
mod tests;
