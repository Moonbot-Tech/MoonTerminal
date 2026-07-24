//! Backend-neutral GPU structs shared by DX11, Metal, and wgpu chart passes.
//! Also includes small builders and helpers that convert feed data into these GPU instances.

use bytemuck::Zeroable;
use moon_chart::layers::{LineInstance, MarkerInstance, SegInstance, ZoneInstance};
use moon_core::data::PriceLinePoint;
use moon_core::feed::{PricePoint, Side, Tick};

/// Shared default volume opacity for all native backends.
pub const DEFAULT_VOLUME_ALPHA: f32 = 0.34;

/// Convert sRGB `[u8; 3]` to `[f32; 4]` with alpha one for cbuffer colors converted to linear by shaders.
pub fn rgb4(c: [u8; 3]) -> [f32; 4] {
    [
        c[0] as f32 / 255.0,
        c[1] as f32 / 255.0,
        c[2] as f32 / 255.0,
        1.0,
    ]
}

/// Fill the GPU trade-cross buffer from ticks, converting time relative to the epoch.
pub fn fill_cross_upload(ticks: &[Tick], epoch_ms: f64, out: &mut Vec<ChartCross>) {
    out.clear();
    out.reserve(ticks.len());
    out.extend(ticks.iter().map(|t| ChartCross {
        time_rel: (t.time_ms - epoch_ms) as f32,
        price: t.price,
        side: match t.side {
            Side::Buy => 0,
            Side::Sell => 1,
        },
        qty: t.qty.max(0.0),
    }));
}

/// Fill the GPU LIQUIDATION trade-cross buffer from ticks.
///
/// Tick quantity is an absolute magnitude and side is stored separately. Every liquidation is
/// deliberately uploaded with `side = 2`, so the shader selects the liquidation color and the
/// volume pass skips it. Cross geometry matches ordinary trades.
pub fn fill_liq_upload(ticks: &[Tick], epoch_ms: f64, out: &mut Vec<ChartCross>) {
    out.clear();
    out.reserve(ticks.len());
    out.extend(ticks.iter().map(|t| ChartCross {
        time_rel: (t.time_ms - epoch_ms) as f32,
        price: t.price,
        side: 2,
        qty: t.qty.max(0.0),
    }));
}

/// Fill a last or mark price-line point buffer from `PricePoint`, discarding non-finite and
/// non-positive prices.
pub fn fill_price_upload(points: &[PricePoint], epoch_ms: f64, out: &mut Vec<PriceLinePoint>) {
    out.clear();
    out.reserve(points.len());
    out.extend(points.iter().filter_map(|p| {
        (p.price.is_finite() && p.price > 0.0).then_some(PriceLinePoint {
            time_rel_ms: (p.time_ms - epoch_ms) as f32,
            price: p.price,
        })
    }));
}

/// One trade marker in GPU memory. Layout matches chart shaders.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct ChartCross {
    pub time_rel: f32,
    pub price: f32,
    pub side: u32,
    pub qty: f32,
}

/// One candle in the candle-layer GPU buffer, matching `Candle` in candles.hlsl.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct CandleGpu {
    pub t_open_rel: f32,
    pub open: f32,
    pub high: f32,
    pub low: f32,
    pub close: f32,
    pub volume: f32,
    /// Candle-specific time frame in relative milliseconds; zero uses series `style.tf_rel_ms`.
    ///
    /// Higher time frames fill the history tail with wider, muted candles.
    pub tf_rel: f32,
}

/// Candle-layer style constants matching cbuffer `CandleStyle` at b1 in candles.hlsl.
#[repr(C)]
#[derive(Clone, Copy, Default, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct CandleStyleGpu {
    pub up: [f32; 4],
    pub down: [f32; 4],
    pub neutral: [f32; 4],
    /// Bucket width in relative milliseconds.
    pub tf_rel_ms: f32,
    /// Trade-zone start in relative milliseconds; `f32::MAX` means no zone.
    pub zone_start_rel: f32,
    /// Mode: zero is filled, one is outline, and two is outline in the zone.
    pub mode: f32,
    /// Outline thickness in physical pixels.
    pub outline_px: f32,
    pub wicks_in_zone: f32,
    pub neutral_in_zone: f32,
    pub fill_alpha: f32,
    /// Hidden-candle zone start in relative milliseconds; `f32::MAX` disables hiding.
    ///
    /// Candles with `t_open` at or beyond this boundary do not render, leaving only trades.
    pub hide_start_rel: f32,
}

/// Fill the candle GPU buffer from a series, converting time relative to the epoch.
///
/// `tf_ms` is the parallel per-candle time-frame array; an empty array uses the series time frame for
/// every candle. Higher time frames fill the history tail at their own widths.
pub fn fill_candle_upload(
    candles: &[moon_core::market::ChartCandle],
    tf_ms: &[f32],
    epoch_ms: f64,
    out: &mut Vec<CandleGpu>,
) {
    out.clear();
    out.reserve(candles.len());
    out.extend(candles.iter().enumerate().map(|(i, c)| CandleGpu {
        t_open_rel: (c.t_open_ms - epoch_ms) as f32,
        open: c.open,
        high: c.high,
        low: c.low,
        close: c.close,
        volume: c.volume,
        tf_rel: tf_ms.get(i).copied().unwrap_or(0.0),
    }));
}

// ── GPU cross ring: shared backend helpers ───────────────────────────────────
// The functions below serve DX11 combo, Metal, and wgpu backends. The compiled subset depends on the
// platform because other backends are excluded, so some functions appear unused on each OS and need
// targeted `#[allow(dead_code)]`. This is NOT dead code; removal would break other platforms.
#[allow(dead_code)]
pub fn cross_append_ranges(start: usize, len: usize, capacity: usize) -> [(usize, usize); 2] {
    if len == 0 || capacity == 0 {
        return [(0, 0), (0, 0)];
    }
    let start = start.min(capacity - 1);
    let first = len.min(capacity - start);
    let second = len.saturating_sub(first);
    [(start, first), (0, second)]
}

#[allow(dead_code)]
pub fn evicted_cross_ranges(
    head: usize,
    count: usize,
    capacity: usize,
    append_len: usize,
) -> [(usize, usize); 2] {
    if append_len == 0 || capacity == 0 || count == 0 {
        return [(0, 0), (0, 0)];
    }
    let count = count.min(capacity);
    let append_len = append_len.min(capacity);
    if count == capacity {
        return cross_append_ranges(head, append_len, capacity);
    }
    let evicted = count.saturating_add(append_len).saturating_sub(capacity);
    if evicted == 0 {
        [(0, 0), (0, 0)]
    } else {
        [(0, evicted.min(count)), (0, 0)]
    }
}

#[allow(dead_code)]
pub fn cross_volume_max<'a>(crosses: impl IntoIterator<Item = &'a ChartCross>) -> (f32, f32) {
    let mut buy = 1e-6f32;
    let mut sell = 1e-6f32;
    for c in crosses {
        match c.side {
            0 => buy = buy.max(c.qty),
            1 => sell = sell.max(c.qty),
            _ => {} // side >= 2 liquidations have no volume bars and do not affect scale
        }
    }
    (buy, sell)
}

#[allow(dead_code)]
pub fn update_cross_volume_max(max: &mut (f32, f32), data: &[ChartCross]) -> bool {
    let before = *max;
    for c in data {
        match c.side {
            0 => max.0 = max.0.max(c.qty),
            1 => max.1 = max.1.max(c.qty),
            _ => {} // side >= 2 liquidations are excluded from volume scale
        }
    }
    before != *max
}

#[allow(dead_code)]
pub fn ranges_touch_volume_max(
    crosses: &[ChartCross],
    ranges: &[(usize, usize); 2],
    volume_max: (f32, f32),
) -> bool {
    for &(start, count) in ranges {
        let end = start.saturating_add(count).min(crosses.len());
        for c in &crosses[start.min(end)..end] {
            if (c.side == 0 && c.qty >= volume_max.0) || (c.side == 1 && c.qty >= volume_max.1) {
                return true;
            }
        }
    }
    false
}

#[allow(dead_code)]
pub fn ranges_have_entries(ranges: &[(usize, usize); 2]) -> bool {
    ranges.iter().any(|&(_, count)| count > 0)
}

#[cfg(test)]
mod tests;

#[allow(dead_code)]
pub fn ordered_cross_ring(
    buf: &[ChartCross],
    head: usize,
    count: usize,
    capacity: usize,
) -> Vec<ChartCross> {
    let capacity = capacity.max(1);
    let count = count.min(capacity).min(buf.len());
    if count == 0 {
        return Vec::new();
    }
    let start = if count == capacity {
        head % capacity
    } else {
        0
    };
    let mut out = Vec::with_capacity(count);
    for i in 0..count {
        let idx = (start + i) % capacity;
        if let Some(cross) = buf.get(idx) {
            out.push(*cross);
        }
    }
    out
}

#[allow(dead_code)]
pub fn reset_cross_ring(
    buf: &mut Vec<ChartCross>,
    head: &mut usize,
    count: &mut usize,
    capacity: usize,
    data: &[ChartCross],
) {
    let capacity = capacity.max(1);
    let start = data.len().saturating_sub(capacity);
    buf.clear();
    buf.extend_from_slice(&data[start..]);
    *count = buf.len();
    *head = *count % capacity;
}

#[allow(dead_code)]
pub fn append_cross_ring(
    buf: &mut Vec<ChartCross>,
    head: &mut usize,
    count: &mut usize,
    capacity: usize,
    data: &[ChartCross],
) {
    let capacity = capacity.max(1);
    if data.is_empty() {
        return;
    }
    if data.len() >= capacity {
        reset_cross_ring(buf, head, count, capacity, data);
        return;
    }
    if buf.len() < capacity {
        buf.resize(capacity, ChartCross::zeroed());
    }
    for cross in data {
        buf[*head] = *cross;
        *head = (*head + 1) % capacity;
        *count = (*count + 1).min(capacity);
    }
}

/// Chart transform uniform. Keep field order in sync with HLSL/MSL/WGSL.
#[repr(C)]
#[derive(Clone, Copy, Default, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct ChartViewGpu {
    pub bounds: [f32; 4],
    pub resolution: [f32; 2],
    pub time_to_px: f32,
    pub view_time0: f32,
    pub price_to_px: f32,
    pub view_price0: f32,
    pub marker_half: f32,
    pub pad: f32,
    pub volume_buy_inv: f32,
    pub volume_sell_inv: f32,
    pub volume_alpha: f32,
    pub _pad2: f32,
}

/// Quad blit/background uniform.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct BlitParams {
    pub dst: [f32; 4],
    pub resolution: [f32; 2],
    pub uv_off: [f32; 2],
    pub uv_scale: [f32; 2],
    pub pad: [f32; 2],
}

#[repr(C)]
#[derive(Clone, Copy, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct BackgroundParams {
    pub dst: [f32; 4],
    pub resolution: [f32; 2],
    pub uv_off: [f32; 2],
    pub uv_scale: [f32; 2],
    pub opacity: f32,
    pub _pad: f32,
    pub bg: [f32; 4],
}

impl Default for BackgroundParams {
    fn default() -> Self {
        Self {
            dst: [0.0, 0.0, 1.0, 1.0],
            resolution: [1.0, 1.0],
            uv_off: [0.0, 0.0],
            uv_scale: [1.0, 1.0],
            opacity: 0.0,
            _pad: 0.0,
            bg: [0.0, 0.0, 0.0, 1.0],
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Default, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct GridParams {
    pub bounds: [f32; 4],
    pub resolution: [f32; 2],
    /// Number of static vertical width divisions, independent of time.
    pub n_vert: f32,
    /// Number of static horizontal height divisions, independent of price.
    pub n_horiz: f32,
    /// Reserved, formerly `price_to_px` or `view_price0`.
    ///
    /// Keep it zero so price movement does not invalidate the static grid.
    pub _pad0: f32,
    pub _pad1: f32,
    pub grid_alpha: f32,
    pub bg_alpha: f32,
    pub bg: [f32; 4],
    pub grid_col: [f32; 4],
}

/// Native cursor/crosshair overlay. Coordinates are physical window pixels.
#[repr(C)]
#[derive(Clone, Copy, Default, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct CursorParams {
    /// Combined chart+book area: x, y, w, h.
    pub bounds: [f32; 4],
    pub resolution: [f32; 2],
    pub cursor: [f32; 2],
    pub color: [f32; 4],
    pub thickness: f32,
    pub enabled: f32,
    pub _pad: [f32; 2],
}

/// Native cursor readout chip background. Coordinates are physical window pixels.
#[repr(C)]
#[derive(Clone, Copy, Default, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct ReadoutRect {
    pub dst: [f32; 4],
    pub bg: [f32; 4],
    pub border: [f32; 4],
    /// x = border width in px.
    pub m: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct BookStyle {
    /// Spread-gap background between best bid and ask, or the entire zone without a book.
    pub book_bg: [f32; 4],
    pub bid: [f32; 4],
    pub ask: [f32; 4],
    /// x = level-line opacity, y = level-line height in physical px.
    pub level: [f32; 4],
    /// Ask-half background above the best ask.
    pub bg_ask: [f32; 4],
    /// Bid-half background below the best bid.
    pub bg_bid: [f32; 4],
    /// `x` is best ask price, `y` is best bid price, and `z` indicates book presence as zero or one.
    pub edges: [f32; 4],
}

impl Default for BookStyle {
    fn default() -> Self {
        Self {
            book_bg: [0.0745, 0.0784, 0.0863, 1.0],
            bid: [0.1294, 0.5137, 0.1922, 1.0],
            ask: [1.0, 0.4980, 0.3137, 1.0],
            level: [0.5, 1.5, 0.0, 0.0],
            bg_ask: [0.0745, 0.0784, 0.0863, 1.0],
            bg_bid: [0.0745, 0.0784, 0.0863, 1.0],
            edges: [0.0; 4],
        }
    }
}

impl BookStyle {
    /// Compare without live bid and ask edges.
    ///
    /// Every non-edge style field is compared: the book or spread background, bid and ask fill RGBA
    /// including opacity, level opacity and thickness, and ask and bid region backgrounds. A change
    /// to any of them requires an immediate rebake, while live edge movement uses the throttled bake.
    pub fn eq_ignore_edges(&self, other: &Self) -> bool {
        self.book_bg == other.book_bg
            && self.bid == other.bid
            && self.ask == other.ask
            && self.level == other.level
            && self.bg_ask == other.bg_ask
            && self.bg_bid == other.bg_bid
    }
}

pub fn cover_uv(dst_w: f32, dst_h: f32, img_aspect: f32) -> ([f32; 2], [f32; 2]) {
    let dst_aspect = dst_w.max(1.0) / dst_h.max(1.0);
    if img_aspect > dst_aspect {
        let u = (dst_aspect / img_aspect).clamp(0.0, 1.0);
        ([(1.0 - u) * 0.5, 0.0], [u, 1.0])
    } else {
        let v = (img_aspect / dst_aspect).clamp(0.0, 1.0);
        ([0.0, (1.0 - v) * 0.5], [1.0, v])
    }
}

#[repr(C)]
#[derive(Clone, Copy, Default, bytemuck::Pod, bytemuck::Zeroable)]
pub struct HLineGpu {
    pub color: [f32; 4],
    pub m: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Default, bytemuck::Pod, bytemuck::Zeroable)]
pub struct ZoneGpu {
    pub color: [f32; 4],
    pub m: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Default, bytemuck::Pod, bytemuck::Zeroable)]
pub struct SegGpu {
    pub pts: [f32; 4],
    pub color: [f32; 4],
    pub m: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Default, bytemuck::Pod, bytemuck::Zeroable)]
pub struct MarkerGpu {
    pub color: [f32; 4],
    pub pos: [f32; 4],
    pub m: [f32; 4],
}

// Convert order-line instances from `moon_chart::layers` into 16-byte-aligned GPU structs. Shared by
// DX11 userdata, Metal, and wgpu backends after removing their former duplication.
pub fn hl_of(h: &LineInstance) -> HLineGpu {
    HLineGpu {
        color: h.color,
        m: [h.price, h.style, h.thickness, 0.0],
    }
}

pub fn zone_of(z: &ZoneInstance) -> ZoneGpu {
    ZoneGpu {
        color: z.color,
        m: [z.price0, z.price1, 0.0, 0.0],
    }
}

pub fn seg_of(s: &SegInstance) -> SegGpu {
    SegGpu {
        pts: [s.t0_rel, s.p0, s.t1_rel, s.p1],
        color: s.color,
        m: [s.thickness, s.pattern, s.extend, 0.0],
    }
}

pub fn mk_of(m: &MarkerInstance) -> MarkerGpu {
    MarkerGpu {
        color: m.color,
        pos: [m.t_rel, m.price, m.size, m.thickness],
        // m.y carries the anchor mode: the shaders read `pos.y` as a price or, when it is set, as
        // an offset above the plot's bottom edge. m.z/m.w slice the news gem into tag-coloured
        // wedges (one instance per wedge); every other marker passes 0 of 1.
        m: [m.shape, m.anchor, m.sector, m.sectors],
    }
}
