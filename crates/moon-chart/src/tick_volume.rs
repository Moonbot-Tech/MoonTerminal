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

/// Rows per pixel column above which a full bake is reduced before drawing.
pub const LOD_MIN_PER_COLUMN: usize = 2;
/// Distinct cross rows one column keeps before per-side row sampling starts. With three side
/// classes (buy, sell, liquidation) a sampled column keeps at most `3 * (LOD_MAX_ROWS + 2)` rows.
pub const LOD_MAX_ROWS: usize = 32;
/// Upper bound on volume bars per column that may cover any one bar height.
pub const LOD_VOLUME_CAP: usize = 64;
/// Tallest volume bar in whole pixels (crosses.hlsl caps the band at 72 px).
const LOD_BAR_MAX_PX: usize = 72;

/// Bake transform in the shader's units: `price0` is the bake origin price including the
/// vertical margin, `height` the full texture height including margins, `width_px` in texels.
#[derive(Clone, Copy, Debug)]
pub struct BakeColumns {
    pub time0: f32,
    pub time_to_px: f32,
    pub price0: f32,
    pub price_to_px: f32,
    pub height: f32,
    pub width_px: u32,
    pub volume_alpha: f32,
    /// Cross half-size; the shader culls crosses beyond `max(8, marker_half + 1)` past bounds.
    pub marker_half: f32,
    /// Volume bar scale exactly as the shader uniform receives it; unused by `reduce_crosses`.
    pub buy_inv: f32,
    pub sell_inv: f32,
}

/// One input row keyed to its texel; its index in `rows` is its draw order.
#[derive(Clone, Copy, Debug)]
struct LodRow {
    slot: u32,
    col: i64,
    row: i64,
    side: u32,
    qty: f32,
}

/// Ring slots kept by `reduce_crosses` / `reduce_volume`, each list in ascending original draw
/// order, plus scratch
/// buffers reused across calls so a dense bake allocates nothing once warmed up.
#[derive(Default, Debug)]
pub struct LodPick {
    pub cross: Vec<u32>,
    pub volume: Vec<u32>,
    rows: Vec<LodRow>,
    order: Vec<u32>,
    kept: Vec<u32>,
    keep: Vec<bool>,
    /// Kept later bars in the current column, counted per whole-pixel height.
    covering: Vec<u32>,
}

/// Whether a full bake over `rows_in_span` rows is dense enough to reduce.
pub fn lod_applies(rows_in_span: usize, width_px: u32) -> bool {
    rows_in_span > LOD_MIN_PER_COLUMN * width_px as usize
}

/// Overlapping bars of this alpha after which one more is invisible in 8-bit colour.
pub fn lod_volume_keep(alpha: f32) -> usize {
    if !(alpha > 0.0 && alpha < 1.0) {
        return 1;
    }
    let keep = ((1.0f32 / 255.0).ln() / (1.0 - alpha).ln()).ceil();
    if keep.is_finite() {
        (keep.max(1.0) as usize).clamp(1, LOD_VOLUME_CAP)
    } else {
        LOD_VOLUME_CAP
    }
}

/// Key rows to their texel in the shader's f32 operation order (HLSL round() ties to even),
/// keeping only rows `drawn` says the shader would not cull.
fn key_rows(
    rows: impl IntoIterator<Item = (u32, f32, f32, u32, f32)>,
    g: &BakeColumns,
    out: &mut LodPick,
    drawn: impl Fn(f32, f32, f32, u32, f32) -> bool,
) {
    out.rows.clear();
    for (slot, t, p, side, qty) in rows {
        let sx = 0.0 + (t - g.time0) * g.time_to_px;
        let col = sx.round_ties_even();
        let row = ((0.0 + g.height) - (p - g.price0) * g.price_to_px).round_ties_even();
        if !drawn(sx, col, row, side, qty) {
            continue;
        }
        out.rows.push(LodRow {
            slot,
            col: col as i64,
            row: row as i64,
            side,
            qty,
        });
    }
}

/// Reduce `(slot, time, price, side, qty)` rows to the crosses a bitmap can show: the last row
/// per texel, then per-side sampling past `LOD_MAX_ROWS` rows a column. Culled rows are dropped.
pub fn reduce_crosses(
    rows: impl IntoIterator<Item = (u32, f32, f32, u32, f32)>,
    g: &BakeColumns,
    out: &mut LodPick,
) {
    out.cross.clear();
    let cull = g.marker_half.max(8.0).max(g.marker_half + 1.0);
    let (w, h) = (g.width_px as f32, g.height);
    key_rows(rows, g, out, |_, col, row, _, _| {
        col.is_finite()
            && row.is_finite()
            && col >= -cull
            && col <= w + cull
            && row >= -cull
            && row <= h + cull
    });
    let LodPick {
        cross,
        rows,
        order,
        kept,
        keep,
        ..
    } = out;
    let n = rows.len();
    keep.clear();
    keep.resize(n, false);

    // Tier A: the last row per texel wins, as it would in the draw.
    order.clear();
    order.extend(0..n as u32);
    order.sort_unstable_by_key(|&k| {
        let r = &rows[k as usize];
        (r.col, r.row, k)
    });
    kept.clear();
    for (i, &k) in order.iter().enumerate() {
        let r = &rows[k as usize];
        let last_of_texel = order.get(i + 1).is_none_or(|&next| {
            let q = &rows[next as usize];
            (q.col, q.row) != (r.col, r.row)
        });
        if last_of_texel {
            kept.push(k);
        }
    }

    // Tier B: columns still holding too many distinct rows are sampled per side class.
    kept.sort_unstable_by_key(|&k| {
        let r = &rows[k as usize];
        (r.col, r.side.min(2), r.row, k)
    });
    let mut col_start = 0;
    while col_start < kept.len() {
        let col = rows[kept[col_start] as usize].col;
        let col_end = col_start
            + kept[col_start..]
                .iter()
                .take_while(|&&k| rows[k as usize].col == col)
                .count();
        if col_end - col_start <= LOD_MAX_ROWS {
            for &k in &kept[col_start..col_end] {
                keep[k as usize] = true;
            }
        } else {
            let mut side_start = col_start;
            while side_start < col_end {
                let class = rows[kept[side_start] as usize].side.min(2);
                let side_end = side_start
                    + kept[side_start..col_end]
                        .iter()
                        .take_while(|&&k| rows[k as usize].side.min(2) == class)
                        .count();
                let side_rows = &kept[side_start..side_end];
                let stride = side_rows.len().div_ceil(LOD_MAX_ROWS).max(1);
                for (pos, &k) in side_rows.iter().enumerate() {
                    if pos % stride == 0 {
                        keep[k as usize] = true;
                    }
                }
                keep[side_rows[side_rows.len() - 1] as usize] = true;
                side_start = side_end;
            }
        }
        col_start = col_end;
    }
    cross.extend((0..n).filter(|&k| keep[k]).map(|k| rows[k].slot));
}

/// Reduce `(slot, time, price, side, qty)` rows to the volume bars a bitmap can show: those
/// fewer than `lod_volume_keep` later kept bars at least as tall in pixels cover, plus the
/// tallest per side. Rows the volume pass culls (off the band, empty, liquidations) are dropped
/// first. A column keeps at most `V * (LOD_BAR_MAX_PX + 1) + 2` bars.
pub fn reduce_volume(
    rows: impl IntoIterator<Item = (u32, f32, f32, u32, f32)>,
    g: &BakeColumns,
    out: &mut LodPick,
) {
    out.volume.clear();
    let w = g.width_px as f32;
    key_rows(rows, g, out, |sx, _, _, side, qty| {
        sx >= -2.0 && sx <= w + 2.0 && qty > 0.0 && side < 2
    });
    let LodPick {
        volume,
        rows,
        order,
        kept,
        covering,
        ..
    } = out;
    let n = rows.len();
    // Volume: walking back from the last drawn bar, a bar stays visible unless `v` later kept
    // bars in its column already cover it; the tallest bar per column and side is always kept.
    let v = lod_volume_keep(g.volume_alpha) as u32;
    // Whole-pixel bar height, as volume_vertex computes it.
    let band_h = (g.height * 0.18).min(72.0);
    let height_px = |r: &LodRow| {
        let inv = if r.side == 0 { g.buy_inv } else { g.sell_inv };
        let norm = (r.qty * inv).clamp(0.0, 1.0);
        let h = (norm.sqrt() * band_h).max(1.0).ceil();
        if h.is_finite() {
            (h as usize).min(LOD_BAR_MAX_PX)
        } else {
            LOD_BAR_MAX_PX
        }
    };
    order.clear();
    order.extend(0..n as u32);
    order.sort_unstable_by_key(|&k| (rows[k as usize].col, k));
    kept.clear();
    let mut start = 0;
    while start < order.len() {
        let col = rows[order[start] as usize].col;
        let end = start
            + order[start..]
                .iter()
                .take_while(|&&k| rows[k as usize].col == col)
                .count();
        let group = &order[start..end];
        // Latest-drawn tallest bar per side; ties go to the later bar.
        let mut tallest = [u32::MAX; 3];
        for &k in group {
            let r = &rows[k as usize];
            let best = &mut tallest[r.side.min(2) as usize];
            if *best == u32::MAX || height_px(r) >= height_px(&rows[*best as usize]) {
                *best = k;
            }
        }
        covering.clear();
        covering.resize(LOD_BAR_MAX_PX + 1, 0);
        for &k in group.iter().rev() {
            let hp = height_px(&rows[k as usize]);
            let covered: u32 = covering[hp..].iter().sum();
            if tallest.contains(&k) || covered < v {
                covering[hp] += 1;
                kept.push(k);
            }
        }
        start = end;
    }
    kept.sort_unstable();
    volume.extend(kept.iter().map(|&k| rows[k as usize].slot));
}

#[cfg(test)]
mod tests;
