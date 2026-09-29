//! macOS GPUI native Metal chart backend. It renders inside GPUI's CAMetalLayer
//! command encoder via the custom GPU pass hook.

use block::ConcreteBlock;
use bytemuck::Zeroable;
use foreign_types::ForeignTypeRef;
use gpui::RawGpuAccess;
use metal::{
    CommandBufferRef, CompileOptions, DeviceRef, MTLBlendFactor, MTLBlendOperation, MTLLoadAction,
    MTLPixelFormat, MTLPrimitiveType, MTLResourceOptions, MTLSamplerMinMagFilter, MTLScissorRect,
    MTLSize, MTLStoreAction, MTLTextureUsage, RenderCommandEncoderRef, RenderPipelineDescriptor,
    RenderPipelineState, SamplerDescriptor, TextureDescriptor,
};
use moon_chart::layers::{LineInstance, MarkerInstance, SegInstance, ZoneInstance};
use moon_chart::tick_volume::{tick_bake_span, tick_touches_bake};
use moon_core::data::{LevelInstance, PriceLinePoint};
use objc::{msg_send, sel, sel_impl};
use std::ffi::c_void;
use std::time::{Duration, Instant};

use super::types::{
    BackgroundParams, BookStyle, CandleGpu, CandleStyleGpu, ChartCross, ChartViewGpu, CursorParams,
    GridParams, HLineGpu, HvolRowGpu, HvolStyleGpu, MarkerGpu, PriceStyleGpu, ReadoutRect, SegGpu,
    SideVolumeGpu, TickStyleGpu, VolumeStyleGpu, ZoneGpu, append_cross_ring, cross_volume_max,
    evicted_cross_ranges, hl_of, mk_of, ordered_cross_ring, queue_appended_ranges,
    ranges_touch_volume_max, reset_cross_ring, seg_of, update_cross_volume_max, zone_of,
};

const SHADER: &str = include_str!("shaders/chart_native.metal");
const BACKGROUND_PNG: &[u8] = include_bytes!("../../../../assets/img/3Dlogo_s01.png");
const MIN_COMBO_CAPACITY: usize = 1;

/// The DX11 combo's backend-free bake planner, shared so both backends decide rebakes and blit
/// windows by one rule. Its LOD helpers have no Metal caller yet.
#[path = "combo/plan.rs"]
#[allow(dead_code)]
mod combo_plan;

use combo_plan::{
    AppendBakeDamage, ComboBakeKey, VolumeBakeKey, append_bake_damage, combo_tex_w,
    combo_v_margin_px, cross_blit_uv, plan_cross_bake, plan_volume_bake, volume_band_px,
    volume_blit_uv,
};

/// How long eviction damage alone may wait before it forces a full rebake of both bitmaps.
const EVICTION_REBAKE_INTERVAL: Duration = Duration::from_secs(1);

#[derive(Default)]
struct BufferSlot {
    buffer: Option<metal::Buffer>,
    size: u64,
}

impl BufferSlot {
    fn write<T: bytemuck::Pod>(&mut self, device: &DeviceRef, label: &str, data: &[T]) {
        let bytes = bytemuck::cast_slice(data);
        let need = bytes.len().max(4) as u64;
        if self.buffer.as_ref().is_none() || self.size < need {
            let buffer = device.new_buffer(
                need.next_power_of_two(),
                MTLResourceOptions::StorageModeShared
                    | MTLResourceOptions::CPUCacheModeWriteCombined,
            );
            buffer.set_label(label);
            self.buffer = Some(buffer);
            self.size = need.next_power_of_two();
        }
        if !bytes.is_empty() {
            unsafe {
                std::ptr::copy_nonoverlapping(
                    bytes.as_ptr(),
                    self.buffer.as_ref().unwrap().contents() as *mut u8,
                    bytes.len(),
                );
            }
        }
    }

    fn write_range<T: bytemuck::Pod>(
        &mut self,
        device: &DeviceRef,
        label: &str,
        start: usize,
        data: &[T],
        total_len: usize,
    ) -> bool {
        let elem = std::mem::size_of::<T>();
        let need = (total_len.max(1) * elem).max(4) as u64;
        let recreated = self.buffer.as_ref().is_none() || self.size < need;
        if recreated {
            let buffer = device.new_buffer(
                need.next_power_of_two(),
                MTLResourceOptions::StorageModeShared
                    | MTLResourceOptions::CPUCacheModeWriteCombined,
            );
            buffer.set_label(label);
            self.buffer = Some(buffer);
            self.size = need.next_power_of_two();
        }
        let bytes = bytemuck::cast_slice(data);
        if !bytes.is_empty() {
            unsafe {
                std::ptr::copy_nonoverlapping(
                    bytes.as_ptr(),
                    (self.buffer.as_ref().unwrap().contents() as *mut u8).add(start * elem),
                    bytes.len(),
                );
            }
        }
        recreated
    }

    fn buffer(&self) -> &metal::BufferRef {
        self.buffer.as_ref().unwrap().as_ref()
    }
}

fn snapshot_buffer<T: bytemuck::Pod>(device: &DeviceRef, label: &str, data: &[T]) -> metal::Buffer {
    let bytes = bytemuck::cast_slice(data);
    let need = bytes.len().max(4) as u64;
    let buffer = device.new_buffer(
        need.next_power_of_two(),
        MTLResourceOptions::StorageModeShared | MTLResourceOptions::CPUCacheModeWriteCombined,
    );
    buffer.set_label(label);
    if !bytes.is_empty() {
        unsafe {
            std::ptr::copy_nonoverlapping(
                bytes.as_ptr(),
                buffer.contents() as *mut u8,
                bytes.len(),
            );
        }
    }
    buffer
}

fn keep_buffers_alive(command_buffer: &CommandBufferRef, buffers: Vec<metal::Buffer>) {
    if buffers.is_empty() {
        return;
    }
    let block = ConcreteBlock::new(move |_completed: &CommandBufferRef| {
        let _ = buffers.len();
    });
    let block = block.copy();
    command_buffer.add_completed_handler(&block);
}

struct Pipelines {
    background: RenderPipelineState,
    blit: RenderPipelineState,
    grid: RenderPipelineState,
    cursor: RenderPipelineState,
    readout_rect: RenderPipelineState,
    candles: RenderPipelineState,
    volume_bars: RenderPipelineState,
    side_volume: RenderPipelineState,
    side_scale: RenderPipelineState,
    hvol_rows: RenderPipelineState,
    hvol_bg: RenderPipelineState,
    crosses: RenderPipelineState,
    volume: RenderPipelineState,
    price_last: RenderPipelineState,
    price_mark: RenderPipelineState,
    book_bg: RenderPipelineState,
    book_bars: RenderPipelineState,
    zone: RenderPipelineState,
    hline: RenderPipelineState,
    seg: RenderPipelineState,
    marker: RenderPipelineState,
    sampler: metal::SamplerState,
    point_sampler: metal::SamplerState,
}

struct BackgroundTexture {
    texture: metal::Texture,
}

struct BaseTexture {
    texture: metal::Texture,
    w: u32,
    h: u32,
    generation: u64,
    pixel_format: MTLPixelFormat,
}

/// Cross bitmap `(W + 2 * x_margin) x (H + 2 * v_margin)`: a pan on either axis inside the margins
/// is a UV shift of the blit, never a rebake.
struct ComboTexture {
    texture: metal::Texture,
    blit_uniform: BufferSlot,
    generation: u64,
    pixel_format: MTLPixelFormat,
    key: ComboBakeKey,
}

/// Volume band bitmap `(W + 2 * x_margin) x band`, baked without a price axis so Y motion never
/// touches it; blitted at the chart's bottom.
struct VolumeTexture {
    texture: metal::Texture,
    blit_uniform: BufferSlot,
    generation: u64,
    pixel_format: MTLPixelFormat,
    key: VolumeBakeKey,
}

/// Sizes of the cross and volume bitmaps for one chart size.
#[derive(Clone, Copy)]
struct ComboDims {
    tex_w: u32,
    /// The chart's own height in texels, without the vertical margins.
    tex_h: u32,
    tex_h_total: u32,
    v_margin: f32,
    band_px: u32,
}

#[derive(Default)]
struct BaseCache {
    texture: Option<BaseTexture>,
    blit_uniform: BufferSlot,
    valid: bool,
}

impl BaseCache {
    fn is_valid_for(&self, gpu: &RawGpuAccess, pixel_format: MTLPixelFormat) -> bool {
        let w = gpu.width();
        let h = gpu.height();
        let generation = gpu.device_generation();
        self.valid
            && self.texture.as_ref().is_some_and(|tex| {
                tex.w == w
                    && tex.h == h
                    && tex.generation == generation
                    && tex.pixel_format == pixel_format
            })
    }

    fn needs_rebuild(&self, gpu: &RawGpuAccess, pixel_format: Option<MTLPixelFormat>) -> bool {
        let Some(pixel_format) = pixel_format else {
            return true;
        };
        !self.is_valid_for(gpu, pixel_format)
    }

    fn ensure_texture(
        &mut self,
        device: &DeviceRef,
        gpu: &RawGpuAccess,
        pixel_format: MTLPixelFormat,
    ) -> &metal::TextureRef {
        let w = gpu.width().max(1);
        let h = gpu.height().max(1);
        let generation = gpu.device_generation();
        let recreate = self.texture.as_ref().is_none_or(|tex| {
            tex.w != w
                || tex.h != h
                || tex.generation != generation
                || tex.pixel_format != pixel_format
        });
        if recreate {
            let desc = TextureDescriptor::new();
            desc.set_texture_type(metal::MTLTextureType::D2);
            desc.set_pixel_format(pixel_format);
            desc.set_width(w as u64);
            desc.set_height(h as u64);
            desc.set_depth(1);
            desc.set_mipmap_level_count(1);
            desc.set_array_length(1);
            desc.set_usage(MTLTextureUsage::RenderTarget | MTLTextureUsage::ShaderRead);
            desc.set_storage_mode(metal::MTLStorageMode::Private);
            let texture = device.new_texture(&desc);
            self.texture = Some(BaseTexture {
                texture,
                w,
                h,
                generation,
                pixel_format,
            });
            self.valid = false;
        }
        self.texture.as_ref().unwrap().texture.as_ref()
    }

    fn write_blit_uniform(
        &mut self,
        device: &DeviceRef,
        view: &ChartViewGpu,
        orderbook_view: &ChartViewGpu,
        hvol_zone: [f32; 4],
        gpu: &RawGpuAccess,
    ) {
        let sc = blit_scissor(view, orderbook_view, hvol_zone, gpu.width(), gpu.height());
        let dst = [sc.x as f32, sc.y as f32, sc.width as f32, sc.height as f32];
        let w = gpu.width().max(1) as f32;
        let h = gpu.height().max(1) as f32;
        let params = BackgroundParams {
            dst,
            resolution: [w, h],
            uv_off: [dst[0] / w, dst[1] / h],
            uv_scale: [dst[2] / w, dst[3] / h],
            opacity: 1.0,
            _pad: 0.0,
            bg: [0.0, 0.0, 0.0, 1.0],
        };
        self.blit_uniform
            .write(device, "moon_chart_base_blit_uniform", &[params]);
    }
}

/// Retained chart data and appearance with snapshot uniforms for combo bakes.
pub struct MetalLayers {
    device_generation: u64,
    pixel_format: Option<MTLPixelFormat>,
    pipelines: Option<Pipelines>,
    background_texture: Option<BackgroundTexture>,
    base_cache: BaseCache,
    combo_texture: Option<ComboTexture>,
    volume_texture: Option<VolumeTexture>,
    /// Ring runs appended since the last bake, drawn incrementally into both bitmaps.
    combo_dirty_ranges: Vec<(usize, usize)>,
    /// When rows evicted from inside a baked span are finally erased by a full rebake. Evicted
    /// rows stay painted until then, so a full ring sheds history in steps, not one bake per tick.
    eviction_rebake_at: Option<Instant>,
    crosses: Vec<ChartCross>,
    cross_head: usize,
    cross_count: usize,
    last_line: Vec<PriceLinePoint>,
    mark_line: Vec<PriceLinePoint>,
    combo_capacity: usize,
    price_line_capacity: usize,
    /// Candles as a complete series replaced on revision changes, plus layer style.
    candles: Vec<CandleGpu>,
    candle_style: CandleStyleGpu,
    /// The sides band's buckets, replaced as a unit when its series is re-read.
    sides: Vec<SideVolumeGpu>,
    /// The horizontal volumes' per-pixel samples, replaced as a unit when they are retaken.
    hvol: Vec<HvolRowGpu>,
    hvol_style: HvolStyleGpu,
    levels: Vec<LevelInstance>,
    zones: Vec<ZoneGpu>,
    hlines: Vec<HLineGpu>,
    segs: Vec<SegGpu>,
    markers: Vec<MarkerGpu>,
    volume_buy_max: f32,
    volume_sell_max: f32,
    bg_uniform: BufferSlot,
    grid_uniform: BufferSlot,
    cursor_uniform: BufferSlot,
    readout_rect_buffer: BufferSlot,
    view_uniform: BufferSlot,
    book_view_uniform: BufferSlot,
    book_style_uniform: BufferSlot,
    cross_buffer: BufferSlot,
    last_line_buffer: BufferSlot,
    mark_line_buffer: BufferSlot,
    price_style_uniform: BufferSlot,
    price_style: PriceStyleGpu,
    /// Trade-tick style retained across resource recreation and compared before rebaking.
    tick_style: TickStyleGpu,
    volume_style_uniform: BufferSlot,
    volume_style: VolumeStyleGpu,
    level_buffer: BufferSlot,
    zone_buffer: BufferSlot,
    hline_buffer: BufferSlot,
    seg_buffer: BufferSlot,
    marker_buffer: BufferSlot,
    candle_buffer: BufferSlot,
    candle_style_uniform: BufferSlot,
    side_buffer: BufferSlot,
    hvol_buffer: BufferSlot,
    hvol_style_uniform: BufferSlot,
    combo_buffers_dirty: bool,
    price_line_buffers_dirty: bool,
    book_buffer_dirty: bool,
    userdata_buffers_dirty: bool,
    /// Only markers changed since the last upload, by a hover patch; zones stay in the base cache.
    marker_buffer_dirty: bool,
    candle_buffers_dirty: bool,
    side_buffer_dirty: bool,
    hvol_buffers_dirty: bool,
}

impl MetalLayers {
    /// Creates empty GPU resources with retained default appearance.
    pub fn new() -> Self {
        Self {
            device_generation: 0,
            pixel_format: None,
            pipelines: None,
            background_texture: None,
            base_cache: BaseCache::default(),
            combo_texture: None,
            volume_texture: None,
            combo_dirty_ranges: Vec::new(),
            eviction_rebake_at: None,
            crosses: Vec::new(),
            cross_head: 0,
            cross_count: 0,
            last_line: Vec::new(),
            mark_line: Vec::new(),
            combo_capacity: MIN_COMBO_CAPACITY,
            price_line_capacity: MIN_COMBO_CAPACITY,
            candles: Vec::new(),
            candle_style: CandleStyleGpu::default(),
            sides: Vec::new(),
            hvol: Vec::new(),
            hvol_style: HvolStyleGpu::default(),
            levels: Vec::new(),
            zones: Vec::new(),
            hlines: Vec::new(),
            segs: Vec::new(),
            markers: Vec::new(),
            volume_buy_max: 1e-6,
            volume_sell_max: 1e-6,
            bg_uniform: BufferSlot::default(),
            grid_uniform: BufferSlot::default(),
            cursor_uniform: BufferSlot::default(),
            readout_rect_buffer: BufferSlot::default(),
            view_uniform: BufferSlot::default(),
            book_view_uniform: BufferSlot::default(),
            book_style_uniform: BufferSlot::default(),
            cross_buffer: BufferSlot::default(),
            last_line_buffer: BufferSlot::default(),
            mark_line_buffer: BufferSlot::default(),
            price_style_uniform: BufferSlot::default(),
            price_style: PriceStyleGpu::default(),
            tick_style: TickStyleGpu::default(),
            volume_style_uniform: BufferSlot::default(),
            volume_style: VolumeStyleGpu::default(),
            level_buffer: BufferSlot::default(),
            zone_buffer: BufferSlot::default(),
            hline_buffer: BufferSlot::default(),
            seg_buffer: BufferSlot::default(),
            marker_buffer: BufferSlot::default(),
            candle_buffer: BufferSlot::default(),
            candle_style_uniform: BufferSlot::default(),
            side_buffer: BufferSlot::default(),
            hvol_buffer: BufferSlot::default(),
            hvol_style_uniform: BufferSlot::default(),
            combo_buffers_dirty: true,
            price_line_buffers_dirty: true,
            book_buffer_dirty: true,
            userdata_buffers_dirty: true,
            marker_buffer_dirty: false,
            candle_buffers_dirty: true,
            side_buffer_dirty: true,
            hvol_buffers_dirty: true,
        }
    }

    /// Replace the horizontal volumes' samples when they are retaken.
    ///
    /// The zone resides in the base cache, so this invalidates it for rebaking.
    pub fn set_hvol(&mut self, data: Vec<HvolRowGpu>) {
        self.hvol = data;
        self.hvol_buffers_dirty = true;
        self.base_cache.valid = false;
    }

    /// Idempotently set the horizontal volumes' zone, colours, kind and normalisation.
    pub fn set_hvol_style(&mut self, style: HvolStyleGpu) {
        if self.hvol_style != style {
            self.hvol_style = style;
            self.hvol_buffers_dirty = true;
            self.base_cache.valid = false;
        }
    }

    /// Replace the sides band's bucket set when its series is re-read.
    ///
    /// The band resides in the base cache, so this invalidates it for rebaking.
    pub fn set_side_volume(&mut self, data: Vec<SideVolumeGpu>) {
        self.sides = data;
        self.side_buffer_dirty = true;
        self.base_cache.valid = false;
    }

    /// Replace the complete candle set when the series revision changes.
    ///
    /// Candles live in the base cache, which must be rebaked.
    pub fn set_candles(&mut self, data: Vec<CandleGpu>) {
        self.candles = data;
        self.candle_buffers_dirty = true;
        self.base_cache.valid = false;
    }

    /// Idempotently set the complete candle-layer style: time-frame width, mode, trade and hidden-
    /// candle boundaries, up/down/neutral colors and fill alpha, outline thickness, wick visibility,
    /// and neutral-zone behavior.
    pub fn set_candle_style(&mut self, style: CandleStyleGpu) {
        if self.candle_style != style {
            self.candle_style = style;
            self.candle_buffers_dirty = true;
            self.base_cache.valid = false;
        }
    }

    pub fn set_combo_capacity(&mut self, combo_capacity: usize, price_line_capacity: usize) {
        let combo_capacity = sanitize_capacity(combo_capacity);
        let price_line_capacity = sanitize_capacity(price_line_capacity);
        if self.combo_capacity == combo_capacity && self.price_line_capacity == price_line_capacity
        {
            return;
        }
        let ordered = ordered_cross_ring(
            &self.crosses,
            self.cross_head,
            self.cross_count,
            self.combo_capacity,
        );
        self.combo_capacity = combo_capacity;
        self.price_line_capacity = price_line_capacity;
        reset_cross_ring(
            &mut self.crosses,
            &mut self.cross_head,
            &mut self.cross_count,
            self.combo_capacity,
            &ordered,
        );
        if self.crosses.len() < self.combo_capacity {
            self.crosses
                .resize(self.combo_capacity, ChartCross::zeroed());
        }
        if self.last_line.len() > self.price_line_capacity {
            self.last_line = tail_vec(&self.last_line, self.price_line_capacity);
        }
        if self.mark_line.len() > self.price_line_capacity {
            self.mark_line = tail_vec(&self.mark_line, self.price_line_capacity);
        }
        self.recalc_volume_scale();
        self.combo_buffers_dirty = true;
        self.price_line_buffers_dirty = true;
        self.combo_texture = None;
        self.volume_texture = None;
        self.combo_dirty_ranges.clear();
        self.eviction_rebake_at = None;
    }

    pub fn reset_combo(&mut self, data: Vec<ChartCross>) {
        reset_cross_ring(
            &mut self.crosses,
            &mut self.cross_head,
            &mut self.cross_count,
            self.combo_capacity,
            &data,
        );
        if self.crosses.len() < self.combo_capacity {
            self.crosses
                .resize(self.combo_capacity, ChartCross::zeroed());
        }
        self.recalc_volume_scale();
        self.combo_buffers_dirty = true;
        self.invalidate_bakes();
        self.combo_dirty_ranges.clear();
    }

    /// Appends live ticks to the ring. The new rows are queued for an incremental draw into both
    /// bitmaps; eviction of rows that may be baked is deferred to one coalesced rebake, while a
    /// capacity-sized batch (or a queue that would cover the ring) invalidates both bakes at once.
    /// A volume scale change still rebakes the volume bitmap, via its planner's scale check.
    pub fn append_combo(&mut self, data: &[ChartCross]) {
        if data.is_empty() {
            return;
        }
        let before_scale = (self.volume_buy_max, self.volume_sell_max);
        let old_head = self.cross_head;
        let old_count = self.cross_count;
        let full_reset = data.len() >= self.combo_capacity;
        let evicted_ranges =
            evicted_cross_ranges(old_head, old_count, self.combo_capacity, data.len());
        let evicted_scale_max =
            ranges_touch_volume_max(&self.crosses, &evicted_ranges, before_scale);
        let evicts_baked = full_reset || self.evicts_baked(&evicted_ranges);
        append_cross_ring(
            &mut self.crosses,
            &mut self.cross_head,
            &mut self.cross_count,
            self.combo_capacity,
            data,
        );
        if self.crosses.len() < self.combo_capacity {
            self.crosses
                .resize(self.combo_capacity, ChartCross::zeroed());
        }
        if full_reset || evicted_scale_max {
            self.recalc_volume_scale();
        } else {
            self.update_volume_scale(data);
        }
        self.combo_buffers_dirty = true;
        let queued = !full_reset
            && queue_appended_ranges(
                &mut self.combo_dirty_ranges,
                old_head,
                data.len(),
                self.combo_capacity,
            );
        match append_bake_damage(queued, evicts_baked) {
            AppendBakeDamage::None => {}
            AppendBakeDamage::Defer => {
                if self.eviction_rebake_at.is_none() {
                    self.eviction_rebake_at = Some(Instant::now() + EVICTION_REBAKE_INTERVAL);
                }
            }
            AppendBakeDamage::Invalidate => {
                self.invalidate_bakes();
                self.combo_dirty_ranges.clear();
            }
        }
    }

    /// Whether any row in the evicted ring `ranges` may be painted inside either baked span.
    fn evicts_baked(&self, ranges: &[(usize, usize); 2]) -> bool {
        let spans = [
            self.combo_texture.as_ref().map(|tex| {
                tick_bake_span(
                    tex.key.bake_t0,
                    tex.key.tex_w as f32,
                    tex.key.time_to_px,
                    tex.key.marker_half,
                )
            }),
            self.volume_texture.as_ref().map(|tex| {
                tick_bake_span(
                    tex.key.bake_t0,
                    tex.key.tex_w as f32,
                    tex.key.time_to_px,
                    0.0,
                )
            }),
        ];
        spans.into_iter().flatten().any(|span| {
            ranges.iter().any(|&(start, count)| {
                let end = start.saturating_add(count).min(self.crosses.len());
                start < end
                    && self.crosses[start..end]
                        .iter()
                        .any(|cross| tick_touches_bake(cross.time_rel, span))
            })
        })
    }

    /// Whether deferred eviction damage is due for its full rebake, so a frame must prepare.
    pub fn eviction_rebake_due(&self, now: Instant) -> bool {
        self.eviction_rebake_at.is_some_and(|at| now >= at)
    }

    /// Forces the next prepare to fully rebake both the cross and the volume bitmaps; that bake
    /// also erases rows whose eviction was deferred.
    fn invalidate_bakes(&mut self) {
        self.eviction_rebake_at = None;
        if let Some(tex) = self.combo_texture.as_mut() {
            tex.key.valid = false;
        }
        if let Some(tex) = self.volume_texture.as_mut() {
            tex.key.valid = false;
        }
    }

    /// Idempotently sets the bottom-volume band style.
    pub fn set_volume_style(&mut self, style: VolumeStyleGpu) {
        if self.volume_style != style {
            self.volume_style = style;
            self.candle_buffers_dirty = true;
            // The band is drawn in the BASE pass, so a style change with no cache
            // invalidation would not appear until something else forced a rebake.
            self.base_cache.valid = false;
        }
    }

    /// Updates tick colours and invalidates history baked with the previous style.
    pub fn set_tick_style(&mut self, style: TickStyleGpu) {
        if self.tick_style != style {
            self.tick_style = style;
            self.combo_buffers_dirty = true;
            self.invalidate_bakes();
        }
    }

    /// Idempotently sets the price-line colours and half-width.
    pub fn set_price_style(&mut self, style: PriceStyleGpu) {
        if self.price_style != style {
            self.price_style = style;
            self.price_line_buffers_dirty = true;
        }
    }

    pub fn set_price_lines(&mut self, last: &[PriceLinePoint], mark: &[PriceLinePoint]) {
        self.last_line = tail_vec(last, self.price_line_capacity);
        self.mark_line = tail_vec(mark, self.price_line_capacity);
        self.price_line_buffers_dirty = true;
    }

    pub fn set_orderbook(&mut self, levels: Vec<LevelInstance>) {
        self.levels = levels;
        self.book_buffer_dirty = true;
        self.base_cache.valid = false;
    }

    pub fn set_userdata(
        &mut self,
        zones: &[ZoneInstance],
        hlines: &[LineInstance],
        segs: &[SegInstance],
        markers: &[MarkerInstance],
    ) {
        self.zones = zones.iter().map(zone_of).collect();
        self.hlines = hlines.iter().map(hl_of).collect();
        self.segs = segs.iter().map(seg_of).collect();
        self.markers = markers.iter().map(mk_of).collect();
        self.userdata_buffers_dirty = true;
        self.base_cache.valid = false;
    }

    /// Rewrites markers in place by index and re-uploads only the marker buffer. Markers draw
    /// after the base cache, so unlike `set_userdata` this leaves the cached base valid.
    ///
    /// Args:
    ///     patches: `(marker index, new instance)` pairs, indices into the last `set_userdata`.
    ///
    /// Returns:
    ///     Whether every index was in range and the patch was applied; nothing changes otherwise.
    pub fn patch_markers(&mut self, patches: &[(u32, MarkerInstance)]) -> bool {
        if patches
            .iter()
            .any(|(ix, _)| *ix as usize >= self.markers.len())
        {
            return false;
        }
        for (ix, marker) in patches {
            self.markers[*ix as usize] = mk_of(marker);
        }
        self.marker_buffer_dirty = true;
        true
    }

    pub fn needs_base_cache(&self, gpu: &RawGpuAccess) -> bool {
        self.base_cache.needs_rebuild(gpu, self.pixel_format)
    }

    fn reset_gpu_objects(&mut self) {
        self.pipelines = None;
        self.background_texture = None;
        self.base_cache = BaseCache::default();
        self.combo_texture = None;
        self.volume_texture = None;
        self.combo_dirty_ranges.clear();
        self.eviction_rebake_at = None;
        self.bg_uniform = BufferSlot::default();
        self.grid_uniform = BufferSlot::default();
        self.cursor_uniform = BufferSlot::default();
        self.readout_rect_buffer = BufferSlot::default();
        self.view_uniform = BufferSlot::default();
        self.book_view_uniform = BufferSlot::default();
        self.book_style_uniform = BufferSlot::default();
        self.cross_buffer = BufferSlot::default();
        self.last_line_buffer = BufferSlot::default();
        self.mark_line_buffer = BufferSlot::default();
        self.price_style_uniform = BufferSlot::default();
        self.volume_style_uniform = BufferSlot::default();
        self.level_buffer = BufferSlot::default();
        self.zone_buffer = BufferSlot::default();
        self.hline_buffer = BufferSlot::default();
        self.seg_buffer = BufferSlot::default();
        self.marker_buffer = BufferSlot::default();
        self.candle_buffer = BufferSlot::default();
        self.candle_style_uniform = BufferSlot::default();
        self.side_buffer = BufferSlot::default();
        self.hvol_buffer = BufferSlot::default();
        self.hvol_style_uniform = BufferSlot::default();
        self.combo_buffers_dirty = true;
        self.price_line_buffers_dirty = true;
        self.book_buffer_dirty = true;
        self.userdata_buffers_dirty = true;
        self.candle_buffers_dirty = true;
        self.side_buffer_dirty = true;
        self.hvol_buffers_dirty = true;
    }

    pub fn render(
        &mut self,
        view: &ChartViewGpu,
        pane_bounds: [f32; 4],
        background_params: &BackgroundParams,
        grid_params: &GridParams,
        cursor_params: &CursorParams,
        readout_rects: &[ReadoutRect],
        orderbook_view: &ChartViewGpu,
        gpu: &RawGpuAccess,
    ) -> anyhow::Result<()> {
        let Some((device, command_buffer, encoder)) = (unsafe { borrow_metal_draw(gpu) }) else {
            anyhow::bail!("chart Metal draw received empty Metal raw gpu handles");
        };
        attach_gpu_frame_timing(command_buffer);
        self.upload_frame_uniforms(
            device,
            view,
            orderbook_view,
            background_params,
            grid_params,
            cursor_params,
            readout_rects,
        );
        let sc = scissor_rect(view, orderbook_view, gpu.width(), gpu.height());
        encoder.set_scissor_rect(sc);

        let pixel_format = self
            .pixel_format
            .expect("Metal pixel format must be prepared");
        if self.base_cache.is_valid_for(gpu, pixel_format) {
            self.draw_cached_base(device, encoder, view, orderbook_view, gpu);
        } else {
            self.draw_base_layers(encoder, sc);
            self.draw_cached_combo(device, encoder, view);
        }
        self.draw_price_lines_layer(encoder);
        // Order lines and trade marks stop at the horizontal-volume zone; the cursor pass keeps
        // the whole pane, as the crosshair and the volume readout live in the zone.
        let pane_sc = bounds_scissor(pane_bounds, gpu.width(), gpu.height());
        encoder.set_scissor_rect(userdata_scissor(pane_sc, &self.hvol_style));
        self.draw_user_layers(encoder);
        encoder.set_scissor_rect(pane_sc);
        self.draw_cursor_layer(encoder, cursor_params, readout_rects);
        Ok(())
    }

    /// `base_sc` is the pass's own scissor, restored after the horizontal volumes draw under the
    /// scissor of their zone alone; see the wgpu backend's `draw_base_layers` for why.
    fn draw_base_layers(&self, encoder: &RenderCommandEncoderRef, base_sc: MTLScissorRect) {
        let pipelines = self.pipelines.as_ref().unwrap();
        let bg = self.background_texture.as_ref().unwrap();

        crate::diag::bump(&crate::diag::CHART_BG_DRAW);
        set_uniform(encoder, 0, self.bg_uniform.buffer());
        encoder.set_fragment_texture(0, Some(bg.texture.as_ref()));
        encoder.set_fragment_sampler_state(0, Some(pipelines.sampler.as_ref()));
        draw(encoder, &pipelines.background, 6, 1);

        crate::diag::bump(&crate::diag::CHART_GRID_DRAW);
        set_uniform(encoder, 0, self.grid_uniform.buffer());
        draw(encoder, &pipelines.grid, 6, 1);

        // Zones come AFTER the grid: the grid pass paints the plot's background across its whole
        // rect (`chart_native.metal` grid_fragment, alpha = `gp.bg_alpha`, which is 1 whenever the
        // photo backdrop is off — its default), so a band drawn before it is erased. Unconditional
        // on purpose: with a backdrop the grid draws lines only, and a fill over them is what every
        // charting package does. Still below the candles, which is where a band belongs.
        if !self.zones.is_empty() {
            crate::diag::bump(&crate::diag::CHART_USER_DRAW);
            set_uniform(encoder, 0, self.view_uniform.buffer());
            set_storage(encoder, 1, self.zone_buffer.buffer());
            draw(encoder, &pipelines.zone, 6, self.zones.len() as u64);
        }

        // The horizontal volumes BEFORE the candles, like the bottom band: laid over the plot they
        // must sit under the bodies and the crosses; carved out beside it the order is moot, as
        // nothing else draws in their zone. HvolStyle rides slot 3, the style slot the band
        // pipelines use too, and the candle block below rebinds it.
        if self.hvol_style.zone[2] >= 1.0 {
            crate::diag::bump(&crate::diag::CHART_HVOL_DRAW);
            encoder.set_scissor_rect(bounds_scissor(
                self.hvol_style.zone,
                (base_sc.x + base_sc.width) as u32,
                (base_sc.y + base_sc.height) as u32,
            ));
            set_uniform(encoder, 0, self.view_uniform.buffer());
            set_storage(encoder, 2, self.hvol_buffer.buffer());
            set_uniform(encoder, 3, self.hvol_style_uniform.buffer());
            draw(
                encoder,
                &pipelines.hvol_bg,
                6,
                u64::from(crate::chartdx::types::HVOL_BG_INSTANCES),
            );
            if !self.hvol.is_empty() {
                draw(encoder, &pipelines.hvol_rows, 12, self.hvol.len() as u64);
            }
            encoder.set_scissor_rect(base_sc);
        }

        // Candles render beneath trade crosses because combo blits over the base cache.
        if !self.candles.is_empty() {
            crate::diag::bump(&crate::diag::CHART_CANDLE_DRAW);
            set_uniform(encoder, 0, self.view_uniform.buffer());
            set_uniform(encoder, 1, self.candle_style_uniform.buffer());
            set_storage(encoder, 2, self.candle_buffer.buffer());
            set_uniform(encoder, 3, self.volume_style_uniform.buffer());
            // The band draws BEFORE the bodies so the candles sit on top. The shader culls the
            // candles past the split boundary, and the sides layer below draws the band's scale
            // bracket for both halves.
            if self.volume_style.m[0] >= 0.5 {
                crate::diag::bump(&crate::diag::CHART_CANDLE_VOLUME_DRAW);
                // Hills read `candles[iid + 1]`, so they take one instance fewer.
                let bars = self.candles.len().saturating_sub(1);
                if bars > 0 {
                    draw(encoder, &pipelines.volume_bars, 6, bars as u64);
                }
            }
            draw(encoder, &pipelines.candles, 18, self.candles.len() as u64);
        }
        // The bought/sold half AFTER the candle layer: it covers the candle band's bucket that
        // straddles the split boundary, and draws the shared scale whether or not samples exist.
        if self.volume_style.m3[0] >= 0.5 {
            crate::diag::bump(&crate::diag::CHART_SIDE_VOLUME_DRAW);
            set_uniform(encoder, 0, self.view_uniform.buffer());
            set_storage(encoder, 2, self.side_buffer.buffer());
            set_uniform(encoder, 3, self.volume_style_uniform.buffer());
            if !self.sides.is_empty() {
                draw(encoder, &pipelines.side_volume, 12, self.sides.len() as u64);
            }
            draw(
                encoder,
                &pipelines.side_scale,
                6,
                moon_chart::volume_bars::VOLUME_SCALE_INSTANCES as u64,
            );
        }

        crate::diag::bump(&crate::diag::CHART_BOOK_DRAW);
        set_uniform(encoder, 0, self.book_view_uniform.buffer());
        encoder.set_vertex_buffer(1, Some(self.book_style_uniform.buffer()), 0);
        encoder.set_fragment_buffer(1, Some(self.book_style_uniform.buffer()), 0);
        set_storage(encoder, 2, self.level_buffer.buffer());
        draw(encoder, &pipelines.book_bg, 6, 1);
        if !self.levels.is_empty() {
            draw(encoder, &pipelines.book_bars, 6, self.levels.len() as u64);
        }
    }

    fn draw_user_layers(&self, encoder: &RenderCommandEncoderRef) {
        let pipelines = self.pipelines.as_ref().unwrap();
        set_uniform(encoder, 0, self.view_uniform.buffer());
        if !self.hlines.is_empty() {
            crate::diag::bump(&crate::diag::CHART_USER_DRAW);
            set_storage(encoder, 1, self.hline_buffer.buffer());
            draw(encoder, &pipelines.hline, 6, self.hlines.len() as u64);
        }
        if !self.segs.is_empty() {
            crate::diag::bump(&crate::diag::CHART_USER_DRAW);
            set_storage(encoder, 1, self.seg_buffer.buffer());
            draw(encoder, &pipelines.seg, 6, self.segs.len() as u64);
        }
        if !self.markers.is_empty() {
            crate::diag::bump(&crate::diag::CHART_USER_DRAW);
            set_storage(encoder, 1, self.marker_buffer.buffer());
            draw(encoder, &pipelines.marker, 6, self.markers.len() as u64);
        }
    }

    /// Creates, or recreates on a size, device or format change, the cross and volume bitmaps.
    fn ensure_combo_textures(
        &mut self,
        device: &DeviceRef,
        pixel_format: MTLPixelFormat,
        dims: ComboDims,
        generation: u64,
    ) {
        if self.combo_texture.as_ref().is_none_or(|tex| {
            tex.key.tex_w != dims.tex_w
                || tex.key.tex_h_total != dims.tex_h_total
                || tex.key.v_margin.to_bits() != dims.v_margin.to_bits()
                || tex.generation != generation
                || tex.pixel_format != pixel_format
        }) {
            self.combo_texture = Some(ComboTexture {
                texture: new_cache_texture(device, pixel_format, dims.tex_w, dims.tex_h_total),
                blit_uniform: BufferSlot::default(),
                generation,
                pixel_format,
                key: ComboBakeKey::unbaked(dims.tex_w, dims.tex_h_total, dims.v_margin),
            });
        }
        if self.volume_texture.as_ref().is_none_or(|tex| {
            tex.key.tex_w != dims.tex_w
                || tex.key.band_px != dims.band_px
                || tex.key.chart_h != dims.tex_h
                || tex.generation != generation
                || tex.pixel_format != pixel_format
        }) {
            self.volume_texture = Some(VolumeTexture {
                texture: new_cache_texture(device, pixel_format, dims.tex_w, dims.band_px),
                blit_uniform: BufferSlot::default(),
                generation,
                pixel_format,
                key: VolumeBakeKey::unbaked(dims.tex_w, dims.band_px, dims.tex_h),
            });
        }
    }

    /// Bakes the cross and volume bitmaps. Each planner decides a full rebake when its baked
    /// window no longer covers the view (or zoom, marker size, style or opacity changed); a pan
    /// inside the margins bakes nothing and only moves the blit's UV window.
    fn prepare_combo_cache(
        &mut self,
        device: &DeviceRef,
        command_buffer: &CommandBufferRef,
        gpu: &RawGpuAccess,
        pixel_format: MTLPixelFormat,
        view: &ChartViewGpu,
    ) -> bool {
        // Before any early return, so a due deadline is always consumed and never re-arms frames.
        if self.eviction_rebake_due(Instant::now()) {
            self.invalidate_bakes();
        }
        if self.cross_count == 0 {
            return false;
        }
        let bw = view.bounds[2];
        let bh = view.bounds[3];
        if bw <= 0.0 || bh <= 0.0 {
            return false;
        }
        let tex_h = bh.round().max(1.0) as u32;
        let v_margin = combo_v_margin_px(bh);
        let dims = ComboDims {
            tex_w: combo_tex_w(bw),
            tex_h,
            tex_h_total: tex_h + 2 * v_margin as u32,
            v_margin,
            band_px: volume_band_px(bh),
        };
        self.ensure_combo_textures(device, pixel_format, dims, gpu.device_generation());

        let cross_plan = plan_cross_bake(&self.combo_texture.as_ref().unwrap().key, view, bw);
        let scale = (self.volume_buy_max, self.volume_sell_max);
        let vol_plan =
            plan_volume_bake(&self.volume_texture.as_ref().unwrap().key, view, bw, |_| {
                scale
            });
        let dirty = !self.combo_dirty_ranges.is_empty();
        if !cross_plan.full && !vol_plan.full && !dirty {
            return false;
        }
        let tex_w = dims.tex_w as f32;
        let cross_view = ChartViewGpu {
            bounds: [0.0, 0.0, tex_w, dims.tex_h_total as f32],
            resolution: [tex_w, dims.tex_h_total as f32],
            time_to_px: view.time_to_px,
            view_time0: cross_plan.bake_t0,
            price_to_px: view.price_to_px,
            view_price0: cross_plan.bake_p0,
            marker_half: view.marker_half,
            pad: 0.0,
            volume_buy_inv: 1.0 / self.volume_buy_max.max(1e-6),
            volume_sell_inv: 1.0 / self.volume_sell_max.max(1e-6),
            volume_alpha: view.volume_alpha,
            _pad2: 0.0,
        };
        // The band keeps the chart's full height as its bounds so bar heights match a direct draw;
        // only the bottom `band_px` rows land inside the target.
        let vol_view = ChartViewGpu {
            bounds: [0.0, dims.band_px as f32 - tex_h as f32, tex_w, tex_h as f32],
            resolution: [tex_w, dims.band_px as f32],
            view_time0: vol_plan.bake_t0,
            view_price0: view.view_price0,
            ..cross_view
        };
        let cross_count = self.cross_count.min(self.crosses.len());
        let ranges = std::mem::take(&mut self.combo_dirty_ranges);
        let all = [(0, cross_count)];
        let pipelines = self.pipelines.as_ref().unwrap();
        let mut keepalive_buffers = Vec::new();

        if vol_plan.full || dirty {
            let tex = self.volume_texture.as_ref().unwrap();
            keepalive_buffers.extend(self.bake_pass(
                device,
                command_buffer,
                &tex.texture,
                (dims.tex_w, dims.band_px),
                vol_plan.full,
                vol_view,
                &pipelines.volume,
                if vol_plan.full { &all[..] } else { &ranges[..] },
            ));
        }
        if cross_plan.full || dirty {
            let tex = self.combo_texture.as_ref().unwrap();
            keepalive_buffers.extend(self.bake_pass(
                device,
                command_buffer,
                &tex.texture,
                (dims.tex_w, dims.tex_h_total),
                cross_plan.full,
                cross_view,
                &pipelines.crosses,
                if cross_plan.full {
                    &all[..]
                } else {
                    &ranges[..]
                },
            ));
        }
        if vol_plan.full {
            let baked = &mut self.volume_texture.as_mut().unwrap().key;
            baked.bake_t0 = vol_plan.bake_t0;
            baked.time_to_px = view.time_to_px;
            baked.volume_alpha = view.volume_alpha;
            baked.scale = vol_plan.scale;
            baked.valid = true;
            crate::diag::bump(&crate::diag::CHART_COMBO_VOLUME_BAKE);
        }
        if cross_plan.full {
            let baked = &mut self.combo_texture.as_mut().unwrap().key;
            baked.bake_t0 = cross_plan.bake_t0;
            baked.bake_p0 = cross_plan.bake_p0;
            baked.time_to_px = view.time_to_px;
            baked.price_to_px = view.price_to_px;
            baked.marker_half = view.marker_half;
            baked.valid = true;
            crate::diag::bump(&crate::diag::CHART_COMBO_BAKE);
        }
        keep_buffers_alive(command_buffer, keepalive_buffers);
        true
    }

    /// Encodes one render pass into a cache bitmap: cleared for a full bake, loaded for an
    /// incremental one, drawing each ring range of `ranges` with `pipeline`. The snapshot view,
    /// tick-style and cross buffers are returned to be retained until the command buffer completes.
    #[allow(clippy::too_many_arguments)]
    fn bake_pass(
        &self,
        device: &DeviceRef,
        command_buffer: &CommandBufferRef,
        texture: &metal::TextureRef,
        (w, h): (u32, u32),
        clear: bool,
        view: ChartViewGpu,
        pipeline: &RenderPipelineState,
        ranges: &[(usize, usize)],
    ) -> Vec<metal::Buffer> {
        let pass = metal::RenderPassDescriptor::new();
        let color = pass.color_attachments().object_at(0).unwrap();
        color.set_texture(Some(texture));
        color.set_load_action(if clear {
            MTLLoadAction::Clear
        } else {
            MTLLoadAction::Load
        });
        color.set_store_action(MTLStoreAction::Store);
        color.set_clear_color(metal::MTLClearColor::new(0.0, 0.0, 0.0, 0.0));
        let encoder = command_buffer.new_render_command_encoder(pass);
        encoder.set_scissor_rect(MTLScissorRect {
            x: 0,
            y: 0,
            width: w as u64,
            height: h as u64,
        });
        let mut keepalive = Vec::new();
        let view_buffer = snapshot_buffer(device, "moon_chart_combo_view_uniform", &[view]);
        set_uniform(encoder, 0, view_buffer.as_ref());
        keepalive.push(view_buffer);
        let tick_buffer =
            snapshot_buffer(device, "moon_chart_combo_tick_style", &[self.tick_style]);
        set_uniform(encoder, 2, tick_buffer.as_ref());
        keepalive.push(tick_buffer);
        for &(start, count) in ranges {
            let end = start.saturating_add(count).min(self.crosses.len());
            if start >= end {
                continue;
            }
            let crosses = &self.crosses[start..end];
            let cross_buffer = snapshot_buffer(device, "moon_chart_combo_crosses", crosses);
            set_storage(encoder, 1, cross_buffer.as_ref());
            crate::diag::bump(&crate::diag::CHART_COMBO_DRAW);
            draw(encoder, pipeline, 6, crosses.len() as u64);
            keepalive.push(cross_buffer);
        }
        encoder.end_encoding();
        keepalive
    }

    fn draw_price_lines_layer(&self, encoder: &RenderCommandEncoderRef) {
        let pipelines = self.pipelines.as_ref().unwrap();
        set_uniform(encoder, 0, self.view_uniform.buffer());
        set_uniform(encoder, 2, self.price_style_uniform.buffer());
        if self.last_line.len() > 1 {
            crate::diag::bump(&crate::diag::CHART_COMBO_DRAW);
            set_storage(encoder, 1, self.last_line_buffer.buffer());
            draw(
                encoder,
                &pipelines.price_last,
                6,
                (self.last_line.len() - 1) as u64,
            );
        }
        if self.mark_line.len() > 1 {
            crate::diag::bump(&crate::diag::CHART_COMBO_DRAW);
            set_storage(encoder, 1, self.mark_line_buffer.buffer());
            draw(
                encoder,
                &pipelines.price_mark,
                6,
                (self.mark_line.len() - 1) as u64,
            );
        }
    }

    /// Composites the volume band at the chart's bottom, then the crosses over it, each through
    /// its whole-texel UV window; point sampling at a fractional offset would flicker by half a
    /// pixel. Each bitmap keeps its own blit uniform, as both draws read theirs at execution.
    fn draw_cached_combo(
        &mut self,
        device: &DeviceRef,
        encoder: &RenderCommandEncoderRef,
        view: &ChartViewGpu,
    ) {
        let [x, y, bw, bh] = view.bounds;
        if bw <= 0.0 {
            return;
        }
        let pipelines = self.pipelines.as_ref().unwrap();
        let blit = |slot: &mut BufferSlot,
                    texture: &metal::TextureRef,
                    dst: [f32; 4],
                    (uv_off, uv_scale): ([f32; 2], [f32; 2])| {
            let params = BackgroundParams {
                dst,
                resolution: view.resolution,
                uv_off,
                uv_scale,
                opacity: 1.0,
                _pad: 0.0,
                bg: [0.0, 0.0, 0.0, 0.0],
            };
            slot.write(device, "moon_chart_combo_blit_uniform", &[params]);
            crate::diag::bump(&crate::diag::CHART_BASE_BLIT);
            set_uniform(encoder, 0, slot.buffer());
            encoder.set_fragment_texture(0, Some(texture));
            encoder.set_fragment_sampler_state(0, Some(pipelines.point_sampler.as_ref()));
            draw(encoder, &pipelines.blit, 6, 1);
        };
        if let Some(tex) = self.volume_texture.as_mut().filter(|t| t.key.valid) {
            let band = tex.key.band_px as f32;
            let uv = volume_blit_uv(&tex.key, view);
            blit(
                &mut tex.blit_uniform,
                &tex.texture,
                [x, y + bh - band, bw, band],
                uv,
            );
        }
        if let Some(tex) = self.combo_texture.as_mut().filter(|t| t.key.valid) {
            let uv = cross_blit_uv(&tex.key, view);
            blit(&mut tex.blit_uniform, &tex.texture, view.bounds, uv);
        }
    }

    fn draw_cursor_layer(
        &self,
        encoder: &RenderCommandEncoderRef,
        cursor_params: &CursorParams,
        readout_rects: &[ReadoutRect],
    ) {
        let pipelines = self.pipelines.as_ref().unwrap();
        if cursor_params.enabled > 0.0 {
            crate::diag::bump(&crate::diag::CHART_CURSOR_DRAW);
            set_uniform(encoder, 0, self.cursor_uniform.buffer());
            draw(encoder, &pipelines.cursor, 12, 1);
        }
        if !readout_rects.is_empty() {
            set_storage(encoder, 1, self.readout_rect_buffer.buffer());
            draw(
                encoder,
                &pipelines.readout_rect,
                6,
                readout_rects.len() as u64,
            );
        }
    }

    fn draw_cached_base(
        &mut self,
        device: &DeviceRef,
        encoder: &RenderCommandEncoderRef,
        view: &ChartViewGpu,
        orderbook_view: &ChartViewGpu,
        gpu: &RawGpuAccess,
    ) {
        self.base_cache
            .write_blit_uniform(device, view, orderbook_view, self.hvol_style.zone, gpu);
        let pipelines = self.pipelines.as_ref().unwrap();
        let texture = self.base_cache.texture.as_ref().unwrap().texture.as_ref();
        crate::diag::bump(&crate::diag::CHART_BASE_BLIT);
        // The blit alone reaches across the horizontal-volume zone; see the wgpu backend's
        // `draw_cached_base` for why the pass's own clip stays plot-to-book.
        encoder.set_scissor_rect(blit_scissor(
            view,
            orderbook_view,
            self.hvol_style.zone,
            gpu.width(),
            gpu.height(),
        ));
        set_uniform(encoder, 0, self.base_cache.blit_uniform.buffer());
        encoder.set_fragment_texture(0, Some(texture));
        encoder.set_fragment_sampler_state(0, Some(pipelines.sampler.as_ref()));
        draw(encoder, &pipelines.background, 6, 1);
        encoder.set_scissor_rect(scissor_rect(
            view,
            orderbook_view,
            gpu.width(),
            gpu.height(),
        ));
    }

    pub fn prepare(
        &mut self,
        view: &ChartViewGpu,
        background_params: &BackgroundParams,
        grid_params: &GridParams,
        cursor_params: &CursorParams,
        orderbook_view: &ChartViewGpu,
        book_style: &BookStyle,
        gpu: &RawGpuAccess,
        rebuild_base: bool,
    ) -> anyhow::Result<()> {
        let Some((device, command_buffer, pixel_format)) = (unsafe { borrow_metal_prepare(gpu) })
        else {
            anyhow::bail!("chart Metal prepare received empty Metal raw gpu handles");
        };
        if self.device_generation != gpu.device_generation()
            || self.pixel_format != Some(pixel_format)
        {
            self.device_generation = gpu.device_generation();
            self.pixel_format = Some(pixel_format);
            self.reset_gpu_objects();
            self.pipelines = Some(create_pipelines(device, pixel_format));
            self.background_texture = Some(create_background_texture(device));
        }
        self.upload_common(
            device,
            view,
            orderbook_view,
            background_params,
            grid_params,
            cursor_params,
            book_style,
        );
        let combo_changed =
            self.prepare_combo_cache(device, command_buffer, gpu, pixel_format, view);
        if rebuild_base || combo_changed || self.base_cache.needs_rebuild(gpu, Some(pixel_format)) {
            self.rebuild_base_cache(
                device,
                command_buffer,
                gpu,
                pixel_format,
                view,
                orderbook_view,
            )?;
        }
        Ok(())
    }

    fn rebuild_base_cache(
        &mut self,
        device: &DeviceRef,
        command_buffer: &CommandBufferRef,
        gpu: &RawGpuAccess,
        pixel_format: MTLPixelFormat,
        view: &ChartViewGpu,
        orderbook_view: &ChartViewGpu,
    ) -> anyhow::Result<()> {
        let texture = self
            .base_cache
            .ensure_texture(device, gpu, pixel_format)
            .to_owned();
        let pass = metal::RenderPassDescriptor::new();
        let color = pass.color_attachments().object_at(0).unwrap();
        color.set_texture(Some(texture.as_ref()));
        color.set_load_action(MTLLoadAction::Clear);
        color.set_store_action(MTLStoreAction::Store);
        color.set_clear_color(metal::MTLClearColor::new(0.0, 0.0, 0.0, 0.0));
        let encoder = command_buffer.new_render_command_encoder(pass);
        let sc = scissor_rect(view, orderbook_view, gpu.width(), gpu.height());
        encoder.set_scissor_rect(sc);
        self.draw_base_layers(encoder, sc);
        self.draw_cached_combo(device, encoder, view);
        encoder.end_encoding();
        self.base_cache.valid = true;
        crate::diag::bump(&crate::diag::CHART_BASE_BAKE);
        Ok(())
    }

    fn upload_common(
        &mut self,
        device: &DeviceRef,
        view: &ChartViewGpu,
        orderbook_view: &ChartViewGpu,
        background_params: &BackgroundParams,
        grid_params: &GridParams,
        cursor_params: &CursorParams,
        book_style: &BookStyle,
    ) {
        let mut view = *view;
        view.volume_buy_inv = 1.0 / self.volume_buy_max.max(1e-6);
        view.volume_sell_inv = 1.0 / self.volume_sell_max.max(1e-6);
        self.bg_uniform
            .write(device, "moon_chart_bg_uniform", &[*background_params]);
        self.grid_uniform
            .write(device, "moon_chart_grid_uniform", &[*grid_params]);
        self.cursor_uniform
            .write(device, "moon_chart_cursor_uniform", &[*cursor_params]);
        self.readout_rect_buffer
            .write(device, "moon_chart_readout_rects", &[] as &[ReadoutRect]);
        self.view_uniform
            .write(device, "moon_chart_view_uniform", &[view]);
        self.book_view_uniform
            .write(device, "moon_chart_book_view_uniform", &[*orderbook_view]);
        self.book_style_uniform
            .write(device, "moon_chart_book_style_uniform", &[*book_style]);
        if self.combo_buffers_dirty || self.cross_buffer.buffer.is_none() {
            let can_partial =
                !self.combo_dirty_ranges.is_empty() && self.cross_buffer.buffer.is_some();
            if can_partial {
                let mut recreated = false;
                for (start, count) in &self.combo_dirty_ranges {
                    let end = start.saturating_add(*count).min(self.crosses.len());
                    if *start < end {
                        recreated |= self.cross_buffer.write_range(
                            device,
                            "moon_chart_crosses",
                            *start,
                            &self.crosses[*start..end],
                            self.crosses.len(),
                        );
                    }
                }
                if recreated {
                    self.cross_buffer
                        .write(device, "moon_chart_crosses", &self.crosses);
                }
            } else {
                self.cross_buffer
                    .write(device, "moon_chart_crosses", &self.crosses);
            }
            self.combo_buffers_dirty = false;
        }
        if self.price_line_buffers_dirty
            || self.last_line_buffer.buffer.is_none()
            || self.mark_line_buffer.buffer.is_none()
            || self.price_style_uniform.buffer.is_none()
        {
            self.last_line_buffer
                .write(device, "moon_chart_last_line", &self.last_line);
            self.mark_line_buffer
                .write(device, "moon_chart_mark_line", &self.mark_line);
            self.price_style_uniform
                .write(device, "moon_chart_price_style", &[self.price_style]);
            self.price_line_buffers_dirty = false;
        }
        if self.book_buffer_dirty || self.level_buffer.buffer.is_none() {
            self.level_buffer
                .write(device, "moon_chart_book_levels", &self.levels);
            self.book_buffer_dirty = false;
        }
        if self.userdata_buffers_dirty
            || self.zone_buffer.buffer.is_none()
            || self.hline_buffer.buffer.is_none()
            || self.seg_buffer.buffer.is_none()
            || self.marker_buffer.buffer.is_none()
        {
            self.zone_buffer
                .write(device, "moon_chart_zones", &self.zones);
            self.hline_buffer
                .write(device, "moon_chart_hlines", &self.hlines);
            self.seg_buffer.write(device, "moon_chart_segs", &self.segs);
            self.marker_buffer
                .write(device, "moon_chart_markers", &self.markers);
            self.userdata_buffers_dirty = false;
            self.marker_buffer_dirty = false;
        } else if self.marker_buffer_dirty {
            self.marker_buffer
                .write(device, "moon_chart_markers", &self.markers);
            self.marker_buffer_dirty = false;
        }
        if self.candle_buffers_dirty
            || self.candle_buffer.buffer.is_none()
            || self.candle_style_uniform.buffer.is_none()
            || self.volume_style_uniform.buffer.is_none()
        {
            self.candle_buffer
                .write(device, "moon_chart_candles", &self.candles);
            self.candle_style_uniform.write(
                device,
                "moon_chart_candle_style",
                &[self.candle_style],
            );
            self.volume_style_uniform.write(
                device,
                "moon_chart_volume_style",
                &[self.volume_style],
            );
            self.candle_buffers_dirty = false;
        }
        if self.side_buffer_dirty || self.side_buffer.buffer.is_none() {
            self.side_buffer
                .write(device, "moon_chart_side_volume", &self.sides);
            self.side_buffer_dirty = false;
        }
        if self.hvol_buffers_dirty
            || self.hvol_buffer.buffer.is_none()
            || self.hvol_style_uniform.buffer.is_none()
        {
            self.hvol_buffer
                .write(device, "moon_chart_hvol", &self.hvol);
            self.hvol_style_uniform
                .write(device, "moon_chart_hvol_style", &[self.hvol_style]);
            self.hvol_buffers_dirty = false;
        }
    }

    fn upload_frame_uniforms(
        &mut self,
        device: &DeviceRef,
        view: &ChartViewGpu,
        orderbook_view: &ChartViewGpu,
        background_params: &BackgroundParams,
        grid_params: &GridParams,
        cursor_params: &CursorParams,
        readout_rects: &[ReadoutRect],
    ) {
        let mut view = *view;
        view.volume_buy_inv = 1.0 / self.volume_buy_max.max(1e-6);
        view.volume_sell_inv = 1.0 / self.volume_sell_max.max(1e-6);
        self.bg_uniform
            .write(device, "moon_chart_bg_uniform", &[*background_params]);
        self.grid_uniform
            .write(device, "moon_chart_grid_uniform", &[*grid_params]);
        self.cursor_uniform
            .write(device, "moon_chart_cursor_uniform", &[*cursor_params]);
        self.readout_rect_buffer
            .write(device, "moon_chart_readout_rects", readout_rects);
        self.view_uniform
            .write(device, "moon_chart_view_uniform", &[view]);
        self.book_view_uniform
            .write(device, "moon_chart_book_view_uniform", &[*orderbook_view]);
    }

    /// Borrow the retained Metal tick ring without a per-cursor copy.
    pub(super) fn tick_samples(&self) -> impl Iterator<Item = &ChartCross> {
        moon_chart::tick_volume::pending_ring(
            &self.crosses,
            self.cross_head,
            self.cross_count,
            self.combo_capacity,
            None,
            &[],
        )
    }

    fn recalc_volume_scale(&mut self) {
        let (buy, sell) = cross_volume_max(self.crosses.iter().take(self.cross_count));
        self.volume_buy_max = buy;
        self.volume_sell_max = sell;
    }

    fn update_volume_scale(&mut self, data: &[ChartCross]) {
        let mut max = (self.volume_buy_max, self.volume_sell_max);
        update_cross_volume_max(&mut max, data);
        self.volume_buy_max = max.0;
        self.volume_sell_max = max.1;
    }
}

fn set_uniform(encoder: &RenderCommandEncoderRef, index: u64, buffer: &metal::BufferRef) {
    encoder.set_vertex_buffer(index, Some(buffer), 0);
    encoder.set_fragment_buffer(index, Some(buffer), 0);
}

fn set_storage(encoder: &RenderCommandEncoderRef, index: u64, buffer: &metal::BufferRef) {
    encoder.set_vertex_buffer(index, Some(buffer), 0);
}

fn draw(
    encoder: &RenderCommandEncoderRef,
    pipeline: &RenderPipelineState,
    vertices: u64,
    instances: u64,
) {
    encoder.set_render_pipeline_state(pipeline);
    encoder.draw_primitives_instanced(MTLPrimitiveType::Triangle, 0, vertices, instances);
}

unsafe fn borrow_metal_prepare<'a>(
    gpu: &RawGpuAccess,
) -> Option<(&'a DeviceRef, &'a CommandBufferRef, MTLPixelFormat)> {
    let RawGpuAccess::Metal(gpu) = gpu else {
        return None;
    };
    if gpu.render_target_format == 0 {
        return None;
    }
    // `device` is contractually non-null `NonNull<c_void>`; take its raw pointer and cast to
    // `*mut MTLDevice`, matching the DX11 path's `.as_ptr()` approach.
    Some((
        unsafe { DeviceRef::from_ptr(gpu.device.as_ptr().cast()) },
        unsafe { CommandBufferRef::from_ptr(gpu.command_buffer.as_ptr().cast()) },
        unsafe { std::mem::transmute::<u64, MTLPixelFormat>(gpu.render_target_format) },
    ))
}

unsafe fn borrow_metal_draw<'a>(
    gpu: &RawGpuAccess,
) -> Option<(
    &'a DeviceRef,
    &'a CommandBufferRef,
    &'a RenderCommandEncoderRef,
)> {
    let RawGpuAccess::Metal(gpu) = gpu else {
        return None;
    };
    // `command_encoder` is `Option<NonNull<c_void>>` and is `None` during prepare before creation.
    let encoder = gpu.command_encoder?;
    Some((
        unsafe { DeviceRef::from_ptr(gpu.device.as_ptr().cast()) },
        unsafe { CommandBufferRef::from_ptr(gpu.command_buffer.as_ptr().cast()) },
        unsafe { RenderCommandEncoderRef::from_ptr(encoder.as_ptr().cast()) },
    ))
}

fn attach_gpu_frame_timing(command_buffer: &CommandBufferRef) {
    if !crate::diag::is_enabled() {
        return;
    }
    let block = ConcreteBlock::new(|completed: &CommandBufferRef| {
        let start: f64 = unsafe { msg_send![completed, GPUStartTime] };
        let end: f64 = unsafe { msg_send![completed, GPUEndTime] };
        let ms = (end - start) * 1000.0;
        crate::diag::record_gpu_frame_ms(ms);
    });
    let block = block.copy();
    command_buffer.add_completed_handler(&block);
}

/// The base pass's clip, plot-to-book and deliberately not widened to the horizontal-volume
/// zone; see the wgpu backend's `scissor_rect` for why, and [`blit_scissor`] for the blit.
fn scissor_rect(
    view: &ChartViewGpu,
    orderbook_view: &ChartViewGpu,
    width: u32,
    height: u32,
) -> MTLScissorRect {
    let x = view.bounds[0].floor().max(0.0) as u64;
    let y = view.bounds[1].floor().max(0.0) as u64;
    let r = (orderbook_view.bounds[0] + orderbook_view.bounds[2])
        .ceil()
        .clamp(x as f32 + 1.0, width.max(1) as f32) as u64;
    let b = (view.bounds[1] + view.bounds[3])
        .ceil()
        .clamp(y as f32 + 1.0, height.max(1) as f32) as u64;
    MTLScissorRect {
        x,
        y,
        width: (r - x).max(1),
        height: (b - y).max(1),
    }
}

/// The pane scissor less a horizontal-volume zone carved out at its left edge, for the user
/// layers; see `HvolStyleGpu::user_clip_left`.
fn userdata_scissor(pane: MTLScissorRect, hvol: &HvolStyleGpu) -> MTLScissorRect {
    let Some(edge) = hvol.user_clip_left() else {
        return pane;
    };
    let right = pane.x + pane.width;
    let left = (edge.ceil().max(0.0) as u64).clamp(pane.x, right - 1);
    MTLScissorRect {
        x: left,
        y: pane.y,
        width: right - left,
        height: pane.height,
    }
}

fn bounds_scissor(bounds: [f32; 4], width: u32, height: u32) -> MTLScissorRect {
    let x = bounds[0].floor().max(0.0) as u64;
    let y = bounds[1].floor().max(0.0) as u64;
    let r = (bounds[0] + bounds[2])
        .ceil()
        .clamp(x as f32 + 1.0, width.max(1) as f32) as u64;
    let b = (bounds[1] + bounds[3])
        .ceil()
        .clamp(y as f32 + 1.0, height.max(1) as f32) as u64;
    MTLScissorRect {
        x,
        y,
        width: (r - x).max(1),
        height: (b - y).max(1),
    }
}

/// The cached base's blit clip: [`scissor_rect`] widened to take in the horizontal-volume zone;
/// see the wgpu backend's `blit_scissor`.
fn blit_scissor(
    view: &ChartViewGpu,
    orderbook_view: &ChartViewGpu,
    hvol_zone: [f32; 4],
    width: u32,
    height: u32,
) -> MTLScissorRect {
    let sc = scissor_rect(view, orderbook_view, width, height);
    if hvol_zone[2] < 1.0 {
        return sc;
    }
    let zone_l = hvol_zone[0].floor().max(0.0) as u64;
    let zone_r = (hvol_zone[0] + hvol_zone[2])
        .ceil()
        .clamp(0.0, width.max(1) as f32) as u64;
    let x = sc.x.min(zone_l);
    let r = (sc.x + sc.width).max(zone_r).max(x + 1);
    MTLScissorRect {
        x,
        y: sc.y,
        width: r - x,
        height: sc.height,
    }
}

fn create_pipelines(device: &DeviceRef, pixel_format: MTLPixelFormat) -> Pipelines {
    let library = device
        .new_library_with_source(SHADER, &CompileOptions::new())
        .expect("chart Metal shaders must compile");
    let sampler_desc = SamplerDescriptor::new();
    sampler_desc.set_min_filter(MTLSamplerMinMagFilter::Linear);
    sampler_desc.set_mag_filter(MTLSamplerMinMagFilter::Linear);
    let sampler = device.new_sampler(&sampler_desc);
    let point_sampler_desc = SamplerDescriptor::new();
    point_sampler_desc.set_min_filter(MTLSamplerMinMagFilter::Nearest);
    point_sampler_desc.set_mag_filter(MTLSamplerMinMagFilter::Nearest);
    let point_sampler = device.new_sampler(&point_sampler_desc);
    Pipelines {
        background: pipeline(
            device,
            &library,
            pixel_format,
            "background_vertex",
            "background_fragment",
        ),
        blit: pipeline(
            device,
            &library,
            pixel_format,
            "background_vertex",
            "blit_fragment",
        ),
        grid: pipeline(
            device,
            &library,
            pixel_format,
            "grid_vertex",
            "grid_fragment",
        ),
        cursor: pipeline(
            device,
            &library,
            pixel_format,
            "cursor_vertex",
            "cursor_fragment",
        ),
        readout_rect: pipeline(
            device,
            &library,
            pixel_format,
            "readout_rect_vertex",
            "readout_rect_fragment",
        ),
        candles: pipeline(
            device,
            &library,
            pixel_format,
            "candles_vertex",
            "candles_fragment",
        ),
        volume_bars: pipeline(
            device,
            &library,
            pixel_format,
            "volume_bars_vertex",
            "volume_bars_fragment",
        ),
        side_volume: pipeline(
            device,
            &library,
            pixel_format,
            "side_band_vertex",
            "side_band_fragment",
        ),
        side_scale: pipeline(
            device,
            &library,
            pixel_format,
            "side_scale_vertex",
            "side_scale_fragment",
        ),
        hvol_rows: pipeline(
            device,
            &library,
            pixel_format,
            "hvol_row_vertex",
            "hvol_row_fragment",
        ),
        hvol_bg: pipeline(
            device,
            &library,
            pixel_format,
            "hvol_bg_vertex",
            "hvol_bg_fragment",
        ),
        crosses: pipeline(
            device,
            &library,
            pixel_format,
            "crosses_vertex",
            "crosses_fragment",
        ),
        volume: pipeline(
            device,
            &library,
            pixel_format,
            "volume_vertex",
            "volume_fragment",
        ),
        price_last: pipeline(
            device,
            &library,
            pixel_format,
            "price_line_vertex",
            "price_last_fragment",
        ),
        price_mark: pipeline(
            device,
            &library,
            pixel_format,
            "price_line_vertex",
            "price_mark_fragment",
        ),
        book_bg: opaque_pipeline(
            device,
            &library,
            pixel_format,
            "book_bg_vertex",
            "book_bg_fragment",
        ),
        book_bars: pipeline(
            device,
            &library,
            pixel_format,
            "book_bars_vertex",
            "book_bars_fragment",
        ),
        zone: pipeline(
            device,
            &library,
            pixel_format,
            "zone_vertex",
            "zone_fragment",
        ),
        hline: pipeline(
            device,
            &library,
            pixel_format,
            "hline_vertex",
            "hline_fragment",
        ),
        seg: pipeline(device, &library, pixel_format, "seg_vertex", "seg_fragment"),
        marker: pipeline(
            device,
            &library,
            pixel_format,
            "marker_vertex",
            "marker_fragment",
        ),
        sampler,
        point_sampler,
    }
}

fn pipeline(
    device: &DeviceRef,
    library: &metal::Library,
    pixel_format: MTLPixelFormat,
    vertex: &str,
    fragment: &str,
) -> RenderPipelineState {
    pipeline_with_blend(device, library, pixel_format, vertex, fragment, true)
}

fn opaque_pipeline(
    device: &DeviceRef,
    library: &metal::Library,
    pixel_format: MTLPixelFormat,
    vertex: &str,
    fragment: &str,
) -> RenderPipelineState {
    pipeline_with_blend(device, library, pixel_format, vertex, fragment, false)
}

fn pipeline_with_blend(
    device: &DeviceRef,
    library: &metal::Library,
    pixel_format: MTLPixelFormat,
    vertex: &str,
    fragment: &str,
    alpha_blend: bool,
) -> RenderPipelineState {
    let vertex_fn = library
        .get_function(vertex, None)
        .expect("chart vertex function exists");
    let fragment_fn = library
        .get_function(fragment, None)
        .expect("chart fragment function exists");
    let descriptor = RenderPipelineDescriptor::new();
    descriptor.set_vertex_function(Some(vertex_fn.as_ref()));
    descriptor.set_fragment_function(Some(fragment_fn.as_ref()));
    let color = descriptor.color_attachments().object_at(0).unwrap();
    color.set_pixel_format(pixel_format);
    color.set_blending_enabled(alpha_blend);
    if alpha_blend {
        color.set_rgb_blend_operation(MTLBlendOperation::Add);
        color.set_alpha_blend_operation(MTLBlendOperation::Add);
        color.set_source_rgb_blend_factor(MTLBlendFactor::SourceAlpha);
        color.set_source_alpha_blend_factor(MTLBlendFactor::One);
        color.set_destination_rgb_blend_factor(MTLBlendFactor::OneMinusSourceAlpha);
        color.set_destination_alpha_blend_factor(MTLBlendFactor::One);
    }
    device
        .new_render_pipeline_state(&descriptor)
        .expect("chart render pipeline must build")
}

/// A private render-target bitmap the combo bakes into and the blit samples.
fn new_cache_texture(
    device: &DeviceRef,
    pixel_format: MTLPixelFormat,
    w: u32,
    h: u32,
) -> metal::Texture {
    let desc = TextureDescriptor::new();
    desc.set_texture_type(metal::MTLTextureType::D2);
    desc.set_pixel_format(pixel_format);
    desc.set_width(w as u64);
    desc.set_height(h as u64);
    desc.set_depth(1);
    desc.set_mipmap_level_count(1);
    desc.set_array_length(1);
    desc.set_usage(MTLTextureUsage::RenderTarget | MTLTextureUsage::ShaderRead);
    desc.set_storage_mode(metal::MTLStorageMode::Private);
    device.new_texture(&desc)
}

fn create_background_texture(device: &DeviceRef) -> BackgroundTexture {
    let image = image::load_from_memory(BACKGROUND_PNG)
        .expect("embedded chart background must decode")
        .to_rgba8();
    let desc = TextureDescriptor::new();
    desc.set_texture_type(metal::MTLTextureType::D2);
    desc.set_pixel_format(MTLPixelFormat::RGBA8Unorm);
    desc.set_width(image.width() as u64);
    desc.set_height(image.height() as u64);
    desc.set_depth(1);
    desc.set_mipmap_level_count(1);
    desc.set_array_length(1);
    desc.set_usage(MTLTextureUsage::ShaderRead);
    desc.set_storage_mode(metal::MTLStorageMode::Managed);
    let texture = device.new_texture(&desc);
    let region = metal::MTLRegion {
        origin: metal::MTLOrigin { x: 0, y: 0, z: 0 },
        size: MTLSize {
            width: image.width() as u64,
            height: image.height() as u64,
            depth: 1,
        },
    };
    texture.replace_region(
        region,
        0,
        image.as_ptr() as *const c_void,
        image.width() as u64 * 4,
    );
    BackgroundTexture { texture }
}

fn tail_vec<T: Clone>(data: &[T], cap: usize) -> Vec<T> {
    let start = data.len().saturating_sub(cap);
    data[start..].to_vec()
}

fn sanitize_capacity(capacity: usize) -> usize {
    capacity.max(MIN_COMBO_CAPACITY)
}
