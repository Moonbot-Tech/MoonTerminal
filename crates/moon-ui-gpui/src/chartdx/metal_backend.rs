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
use moon_core::data::{LevelInstance, PriceLinePoint};
use objc::{msg_send, sel, sel_impl};
use std::ffi::c_void;

use super::types::{
    append_cross_ring, cross_append_ranges, cross_volume_max, evicted_cross_ranges,
    ordered_cross_ring, ranges_have_entries, ranges_touch_volume_max, reset_cross_ring,
    update_cross_volume_max, BackgroundParams, BookStyle, ChartCross, ChartViewGpu, CursorParams,
    GridParams, HLineGpu, MarkerGpu, ReadoutRect, SegGpu, ZoneGpu, DEFAULT_VOLUME_ALPHA,
};

const SHADER: &str = include_str!("shaders/chart_native.metal");
const BACKGROUND_PNG: &[u8] = include_bytes!("../../../../assets/img/3Dlogo_s01.png");
const MIN_COMBO_CAPACITY: usize = 1;

#[inline]
fn texel_aligned_time0(time0: f32, time_to_px: f32) -> f32 {
    if !(time_to_px > 1e-9) {
        return time0;
    }
    (time0 * time_to_px).floor() / time_to_px
}

fn hl_of(h: &LineInstance) -> HLineGpu {
    HLineGpu {
        color: h.color,
        m: [h.price, h.style, h.thickness, 0.0],
    }
}

fn zone_of(z: &ZoneInstance) -> ZoneGpu {
    ZoneGpu {
        color: z.color,
        m: [z.price0, z.price1, 0.0, 0.0],
    }
}

fn seg_of(s: &SegInstance) -> SegGpu {
    SegGpu {
        pts: [s.t0_rel, s.p0, s.t1_rel, s.p1],
        color: s.color,
        m: [s.thickness, s.pattern, s.extend, 0.0],
    }
}

fn mk_of(m: &MarkerInstance) -> MarkerGpu {
    MarkerGpu {
        color: m.color,
        pos: [m.t_rel, m.price, m.size, m.thickness],
        m: [m.shape, 0.0, 0.0, 0.0],
    }
}

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

struct ComboTexture {
    texture: metal::Texture,
    blit_uniform: BufferSlot,
    w: u32,
    h: u32,
    generation: u64,
    pixel_format: MTLPixelFormat,
    bake_t0: f32,
    last_baked_head: usize,
    last_time_to_px: f32,
    last_price_to_px: f32,
    last_view_price0: f32,
    last_marker_half: f32,
    valid: bool,
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
        gpu: &RawGpuAccess,
    ) {
        let dst = panel_dst(view, orderbook_view, gpu.width(), gpu.height());
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

pub struct MetalLayers {
    device_generation: u64,
    pixel_format: Option<MTLPixelFormat>,
    pipelines: Option<Pipelines>,
    background_texture: Option<BackgroundTexture>,
    base_cache: BaseCache,
    combo_texture: Option<ComboTexture>,
    combo_dirty_ranges: Vec<(usize, usize)>,
    crosses: Vec<ChartCross>,
    cross_head: usize,
    cross_count: usize,
    last_line: Vec<PriceLinePoint>,
    mark_line: Vec<PriceLinePoint>,
    combo_capacity: usize,
    price_line_capacity: usize,
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
    level_buffer: BufferSlot,
    zone_buffer: BufferSlot,
    hline_buffer: BufferSlot,
    seg_buffer: BufferSlot,
    marker_buffer: BufferSlot,
    combo_buffers_dirty: bool,
    price_line_buffers_dirty: bool,
    book_buffer_dirty: bool,
    userdata_buffers_dirty: bool,
}

impl MetalLayers {
    pub fn new() -> Self {
        Self {
            device_generation: 0,
            pixel_format: None,
            pipelines: None,
            background_texture: None,
            base_cache: BaseCache::default(),
            combo_texture: None,
            combo_dirty_ranges: Vec::new(),
            crosses: Vec::new(),
            cross_head: 0,
            cross_count: 0,
            last_line: Vec::new(),
            mark_line: Vec::new(),
            combo_capacity: MIN_COMBO_CAPACITY,
            price_line_capacity: MIN_COMBO_CAPACITY,
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
            level_buffer: BufferSlot::default(),
            zone_buffer: BufferSlot::default(),
            hline_buffer: BufferSlot::default(),
            seg_buffer: BufferSlot::default(),
            marker_buffer: BufferSlot::default(),
            combo_buffers_dirty: true,
            price_line_buffers_dirty: true,
            book_buffer_dirty: true,
            userdata_buffers_dirty: true,
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
        self.combo_dirty_ranges.clear();
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
        if let Some(tex) = self.combo_texture.as_mut() {
            tex.valid = false;
        }
        self.combo_dirty_ranges.clear();
    }

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
        let evicted_any = ranges_have_entries(&evicted_ranges);
        let evicted_scale_max =
            ranges_touch_volume_max(&self.crosses, &evicted_ranges, before_scale);
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
        if full_reset || evicted_any || before_scale != (self.volume_buy_max, self.volume_sell_max)
        {
            if let Some(tex) = self.combo_texture.as_mut() {
                tex.valid = false;
            }
            self.combo_dirty_ranges.clear();
        } else {
            let appended = data.len().min(self.combo_capacity);
            for (start, count) in cross_append_ranges(old_head, appended, self.combo_capacity) {
                if count > 0 {
                    self.combo_dirty_ranges.push((start, count));
                }
            }
        }
    }

    pub fn set_price_lines(&mut self, last: &[PriceLinePoint], mark: &[PriceLinePoint]) {
        self.last_line = tail_vec(last, self.price_line_capacity);
        self.mark_line = tail_vec(mark, self.price_line_capacity);
        self.price_line_buffers_dirty = true;
        if let Some(tex) = self.combo_texture.as_mut() {
            tex.valid = false;
        }
        self.combo_dirty_ranges.clear();
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

    pub fn needs_base_cache(&self, gpu: &RawGpuAccess) -> bool {
        self.base_cache.needs_rebuild(gpu, self.pixel_format)
    }

    fn reset_gpu_objects(&mut self) {
        self.pipelines = None;
        self.background_texture = None;
        self.base_cache = BaseCache::default();
        self.combo_texture = None;
        self.combo_dirty_ranges.clear();
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
        self.level_buffer = BufferSlot::default();
        self.zone_buffer = BufferSlot::default();
        self.hline_buffer = BufferSlot::default();
        self.seg_buffer = BufferSlot::default();
        self.marker_buffer = BufferSlot::default();
        self.combo_buffers_dirty = true;
        self.price_line_buffers_dirty = true;
        self.book_buffer_dirty = true;
        self.userdata_buffers_dirty = true;
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
            self.draw_base_layers(encoder);
            self.draw_cached_combo(device, encoder, view);
        }
        encoder.set_scissor_rect(bounds_scissor(pane_bounds, gpu.width(), gpu.height()));
        self.draw_cursor_layer(encoder, cursor_params, readout_rects);
        Ok(())
    }

    fn draw_base_layers(&self, encoder: &RenderCommandEncoderRef) {
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

        crate::diag::bump(&crate::diag::CHART_BOOK_DRAW);
        set_uniform(encoder, 0, self.book_view_uniform.buffer());
        encoder.set_vertex_buffer(1, Some(self.book_style_uniform.buffer()), 0);
        encoder.set_fragment_buffer(1, Some(self.book_style_uniform.buffer()), 0);
        set_storage(encoder, 2, self.level_buffer.buffer());
        draw(encoder, &pipelines.book_bg, 6, 1);
        if !self.levels.is_empty() {
            draw(encoder, &pipelines.book_bars, 6, self.levels.len() as u64);
        }

        set_uniform(encoder, 0, self.view_uniform.buffer());
        if !self.zones.is_empty() {
            crate::diag::bump(&crate::diag::CHART_USER_DRAW);
            set_storage(encoder, 1, self.zone_buffer.buffer());
            draw(encoder, &pipelines.zone, 6, self.zones.len() as u64);
        }
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

    fn ensure_combo_texture(
        &mut self,
        device: &DeviceRef,
        pixel_format: MTLPixelFormat,
        tex_w: u32,
        tex_h: u32,
        generation: u64,
    ) {
        let recreate = self.combo_texture.as_ref().is_none_or(|tex| {
            tex.w != tex_w
                || tex.h != tex_h
                || tex.generation != generation
                || tex.pixel_format != pixel_format
        });
        if recreate {
            let desc = TextureDescriptor::new();
            desc.set_texture_type(metal::MTLTextureType::D2);
            desc.set_pixel_format(pixel_format);
            desc.set_width(tex_w as u64);
            desc.set_height(tex_h as u64);
            desc.set_depth(1);
            desc.set_mipmap_level_count(1);
            desc.set_array_length(1);
            desc.set_usage(MTLTextureUsage::RenderTarget | MTLTextureUsage::ShaderRead);
            desc.set_storage_mode(metal::MTLStorageMode::Private);
            let texture = device.new_texture(&desc);
            self.combo_texture = Some(ComboTexture {
                texture,
                blit_uniform: BufferSlot::default(),
                w: tex_w,
                h: tex_h,
                generation,
                pixel_format,
                bake_t0: 0.0,
                last_baked_head: usize::MAX,
                last_time_to_px: 0.0,
                last_price_to_px: 0.0,
                last_view_price0: 0.0,
                last_marker_half: 0.0,
                valid: false,
            });
        }
    }

    fn prepare_combo_cache(
        &mut self,
        device: &DeviceRef,
        command_buffer: &CommandBufferRef,
        gpu: &RawGpuAccess,
        pixel_format: MTLPixelFormat,
        view: &ChartViewGpu,
    ) -> bool {
        if self.cross_count == 0 && self.last_line.len() <= 1 && self.mark_line.len() <= 1 {
            return false;
        }
        let bw = view.bounds[2];
        let bh = view.bounds[3];
        if bw <= 0.0 || bh <= 0.0 {
            return false;
        }
        let margin_px = (bw * 0.2).max(128.0);
        let tex_w = (bw + margin_px).round().max(1.0) as u32;
        let tex_h = bh.round().max(1.0) as u32;
        self.ensure_combo_texture(device, pixel_format, tex_w, tex_h, gpu.device_generation());

        let (need_full, bake_t0, combo_texture) = {
            let tex = self.combo_texture.as_mut().unwrap();
            if tex.last_time_to_px != view.time_to_px
                || tex.last_price_to_px != view.price_to_px
                || tex.last_view_price0 != view.view_price0
                || tex.last_marker_half != view.marker_half
            {
                tex.valid = false;
            }

            let u_left_px = (view.view_time0 - tex.bake_t0) * view.time_to_px;
            let need_full = !tex.valid || u_left_px < 0.0 || u_left_px > margin_px;
            let bake_t0 = if need_full {
                texel_aligned_time0(view.view_time0, view.time_to_px)
            } else {
                tex.bake_t0
            };
            (need_full, bake_t0, tex.texture.to_owned())
        };
        if !need_full && self.combo_dirty_ranges.is_empty() {
            return false;
        }
        let bake_view = ChartViewGpu {
            bounds: [0.0, 0.0, tex_w as f32, tex_h as f32],
            resolution: [tex_w as f32, tex_h as f32],
            time_to_px: view.time_to_px,
            view_time0: bake_t0,
            price_to_px: view.price_to_px,
            view_price0: view.view_price0,
            marker_half: view.marker_half,
            pad: 0.0,
            volume_buy_inv: 1.0 / self.volume_buy_max.max(1e-6),
            volume_sell_inv: 1.0 / self.volume_sell_max.max(1e-6),
            volume_alpha: DEFAULT_VOLUME_ALPHA,
            _pad2: 0.0,
        };

        let pass = metal::RenderPassDescriptor::new();
        let color = pass.color_attachments().object_at(0).unwrap();
        color.set_texture(Some(combo_texture.as_ref()));
        color.set_load_action(if need_full {
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
            width: tex_w as u64,
            height: tex_h as u64,
        });
        let mut keepalive_buffers = Vec::new();
        if need_full {
            let cross_count = self.cross_count.min(self.crosses.len());
            keepalive_buffers.extend(self.draw_combo_layers(
                device,
                encoder,
                bake_view,
                &self.crosses[..cross_count],
                true,
            ));
            let tex = self.combo_texture.as_mut().unwrap();
            tex.bake_t0 = bake_t0;
            tex.last_baked_head = self.cross_head;
            tex.last_time_to_px = view.time_to_px;
            tex.last_price_to_px = view.price_to_px;
            tex.last_view_price0 = view.view_price0;
            tex.last_marker_half = view.marker_half;
            tex.valid = true;
            self.combo_dirty_ranges.clear();
            crate::diag::bump(&crate::diag::CHART_COMBO_BAKE);
        } else {
            let ranges = std::mem::take(&mut self.combo_dirty_ranges);
            for (start, count) in ranges {
                if count == 0 {
                    continue;
                }
                let end = start.saturating_add(count).min(self.crosses.len());
                if start < end {
                    keepalive_buffers.extend(self.draw_combo_layers(
                        device,
                        encoder,
                        bake_view,
                        &self.crosses[start..end],
                        false,
                    ));
                }
            }
            self.combo_texture.as_mut().unwrap().last_baked_head = self.cross_head;
        }
        encoder.end_encoding();
        keep_buffers_alive(command_buffer, keepalive_buffers);
        true
    }

    fn draw_combo_layers(
        &self,
        device: &DeviceRef,
        encoder: &RenderCommandEncoderRef,
        view: ChartViewGpu,
        crosses: &[ChartCross],
        include_price_lines: bool,
    ) -> Vec<metal::Buffer> {
        let pipelines = self.pipelines.as_ref().unwrap();
        let mut keepalive = Vec::new();
        let view_buffer = snapshot_buffer(device, "moon_chart_combo_view_uniform", &[view]);
        set_uniform(encoder, 0, view_buffer.as_ref());
        keepalive.push(view_buffer);
        let cross_buffer = (!crosses.is_empty())
            .then(|| snapshot_buffer(device, "moon_chart_combo_crosses", crosses));
        if !crosses.is_empty() {
            let cross_buffer = cross_buffer.as_ref().unwrap();
            set_storage(encoder, 1, cross_buffer.as_ref());
            crate::diag::bump(&crate::diag::CHART_COMBO_DRAW);
            draw(encoder, &pipelines.volume, 6, crosses.len() as u64);
        }
        if include_price_lines && self.last_line.len() > 1 {
            let last_line_buffer =
                snapshot_buffer(device, "moon_chart_combo_last_line", &self.last_line);
            crate::diag::bump(&crate::diag::CHART_COMBO_DRAW);
            set_storage(encoder, 1, last_line_buffer.as_ref());
            draw(
                encoder,
                &pipelines.price_last,
                6,
                (self.last_line.len() - 1) as u64,
            );
            keepalive.push(last_line_buffer);
        }
        if include_price_lines && self.mark_line.len() > 1 {
            let mark_line_buffer =
                snapshot_buffer(device, "moon_chart_combo_mark_line", &self.mark_line);
            crate::diag::bump(&crate::diag::CHART_COMBO_DRAW);
            set_storage(encoder, 1, mark_line_buffer.as_ref());
            draw(
                encoder,
                &pipelines.price_mark,
                6,
                (self.mark_line.len() - 1) as u64,
            );
            keepalive.push(mark_line_buffer);
        }
        if !crosses.is_empty() {
            let cross_buffer = cross_buffer.as_ref().unwrap();
            set_storage(encoder, 1, cross_buffer.as_ref());
            crate::diag::bump(&crate::diag::CHART_COMBO_DRAW);
            draw(encoder, &pipelines.crosses, 6, crosses.len() as u64);
        }
        if let Some(cross_buffer) = cross_buffer {
            keepalive.push(cross_buffer);
        }
        keepalive
    }

    fn draw_cached_combo(
        &mut self,
        device: &DeviceRef,
        encoder: &RenderCommandEncoderRef,
        view: &ChartViewGpu,
    ) {
        let Some(tex) = self.combo_texture.as_mut() else {
            return;
        };
        if !tex.valid || view.bounds[2] <= 0.0 {
            return;
        }
        let u_left_px = ((view.view_time0 - tex.bake_t0) * view.time_to_px)
            .round()
            .clamp(0.0, (tex.w as f32 - view.bounds[2]).max(0.0));
        let params = BackgroundParams {
            dst: view.bounds,
            resolution: view.resolution,
            uv_off: [u_left_px / tex.w as f32, 0.0],
            uv_scale: [view.bounds[2] / tex.w as f32, 1.0],
            opacity: 1.0,
            _pad: 0.0,
            bg: [0.0, 0.0, 0.0, 0.0],
        };
        tex.blit_uniform
            .write(device, "moon_chart_combo_blit_uniform", &[params]);
        let pipelines = self.pipelines.as_ref().unwrap();
        crate::diag::bump(&crate::diag::CHART_BASE_BLIT);
        set_uniform(encoder, 0, tex.blit_uniform.buffer());
        encoder.set_fragment_texture(0, Some(tex.texture.as_ref()));
        encoder.set_fragment_sampler_state(0, Some(pipelines.point_sampler.as_ref()));
        draw(encoder, &pipelines.blit, 6, 1);
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
            .write_blit_uniform(device, view, orderbook_view, gpu);
        let pipelines = self.pipelines.as_ref().unwrap();
        let texture = self.base_cache.texture.as_ref().unwrap().texture.as_ref();
        crate::diag::bump(&crate::diag::CHART_BASE_BLIT);
        set_uniform(encoder, 0, self.base_cache.blit_uniform.buffer());
        encoder.set_fragment_texture(0, Some(texture));
        encoder.set_fragment_sampler_state(0, Some(pipelines.sampler.as_ref()));
        draw(encoder, &pipelines.background, 6, 1);
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
        encoder.set_scissor_rect(scissor_rect(
            view,
            orderbook_view,
            gpu.width(),
            gpu.height(),
        ));
        self.draw_base_layers(encoder);
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
        view.volume_alpha = DEFAULT_VOLUME_ALPHA;
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
        {
            self.last_line_buffer
                .write(device, "moon_chart_last_line", &self.last_line);
            self.mark_line_buffer
                .write(device, "moon_chart_mark_line", &self.mark_line);
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
        view.volume_alpha = DEFAULT_VOLUME_ALPHA;
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
    // device — NonNull<c_void> (по контракту не null): берём сырой указатель и кастуем
    // к *mut MTLDevice, как dx11-путь делает через `.as_ptr()`.
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
    // command_encoder — Option<NonNull<c_void>>: None во время prepare (энкодера ещё нет).
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

fn panel_dst(
    view: &ChartViewGpu,
    orderbook_view: &ChartViewGpu,
    width: u32,
    height: u32,
) -> [f32; 4] {
    let sc = scissor_rect(view, orderbook_view, width, height);
    [sc.x as f32, sc.y as f32, sc.width as f32, sc.height as f32]
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
