//! Linux GPUI native wgpu chart backend. This is an own-pass renderer inside
//! GPUI's existing wgpu frame, not the old moon-chart offscreen/readback path.

use std::num::NonZeroU64;

use bytemuck::Zeroable;
use gpui::RawGpuAccess;
use moon_chart::layers::{LineInstance, MarkerInstance, SegInstance, ZoneInstance};
use moon_core::data::{LevelInstance, PriceLinePoint};

use super::types::{
    BackgroundParams, BookStyle, ChartCross, ChartViewGpu, CursorParams, DEFAULT_VOLUME_ALPHA,
    GridParams, HLineGpu, MarkerGpu, ReadoutRect, SegGpu, ZoneGpu, append_cross_ring,
    ordered_cross_ring, reset_cross_ring,
};

const BACKGROUND_SHADER: &str = include_str!("shaders/native_background.wgsl");
const GRID_SHADER: &str = include_str!("shaders/native_grid.wgsl");
const CURSOR_SHADER: &str = include_str!("shaders/native_cursor.wgsl");
const CROSSES_SHADER: &str = include_str!("shaders/native_crosses.wgsl");
const PRICE_SHADER: &str = include_str!("shaders/native_price.wgsl");
const BOOK_SHADER: &str = include_str!("shaders/native_book.wgsl");
const ZONE_SHADER: &str = include_str!("shaders/native_zone.wgsl");
const HLINE_SHADER: &str = include_str!("shaders/native_hline.wgsl");
const SEG_SHADER: &str = include_str!("shaders/native_seg.wgsl");
const MARKER_SHADER: &str = include_str!("shaders/native_marker.wgsl");
const READOUT_SHADER: &str = include_str!("shaders/native_readout.wgsl");
const BACKGROUND_PNG: &[u8] = include_bytes!("../../../../assets/img/3Dlogo_s01.png");
const MIN_COMBO_CAPACITY: usize = 1;

#[inline]
fn texel_aligned_time0(time0: f32, time_to_px: f32) -> f32 {
    if !(time_to_px > 1e-9) {
        return time0;
    }
    (time0 * time_to_px).floor() / time_to_px
}

fn append_ranges(start: usize, len: usize, capacity: usize) -> [(usize, usize); 2] {
    if len == 0 || capacity == 0 {
        return [(0, 0), (0, 0)];
    }
    let first = len.min(capacity - start.min(capacity - 1));
    let second = len.saturating_sub(first);
    [(start, first), (0, second)]
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
    buffer: Option<wgpu::Buffer>,
    size: u64,
}

impl BufferSlot {
    fn write<T: bytemuck::Pod>(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        label: &str,
        usage: wgpu::BufferUsages,
        data: &[T],
    ) -> bool {
        let bytes = bytemuck::cast_slice(data);
        let need = bytes.len().max(4) as u64;
        let mut recreated = false;
        if self.buffer.as_ref().is_none() || self.size < need {
            self.buffer = Some(device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size: need.next_power_of_two(),
                usage: usage | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }));
            self.size = need.next_power_of_two();
            recreated = true;
        }
        if !bytes.is_empty() {
            queue.write_buffer(self.buffer.as_ref().unwrap(), 0, bytes);
        }
        recreated
    }

    fn write_range<T: bytemuck::Pod>(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        label: &str,
        usage: wgpu::BufferUsages,
        start: usize,
        data: &[T],
        total_len: usize,
    ) -> bool {
        let elem = std::mem::size_of::<T>();
        let need = (total_len.max(1) * elem).max(4) as u64;
        let recreated = self.buffer.as_ref().is_none() || self.size < need;
        if recreated {
            self.buffer = Some(device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size: need.next_power_of_two(),
                usage: usage | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }));
            self.size = need.next_power_of_two();
        }
        let bytes = bytemuck::cast_slice(data);
        if !bytes.is_empty() {
            queue.write_buffer(self.buffer.as_ref().unwrap(), (start * elem) as u64, bytes);
        }
        recreated
    }

    fn binding(&self) -> wgpu::BindingResource<'_> {
        self.buffer.as_ref().unwrap().as_entire_binding()
    }
}

struct Pipelines {
    bg_layout: wgpu::BindGroupLayout,
    grid_layout: wgpu::BindGroupLayout,
    cursor_layout: wgpu::BindGroupLayout,
    readout_layout: wgpu::BindGroupLayout,
    view_storage_layout: wgpu::BindGroupLayout,
    book_layout: wgpu::BindGroupLayout,
    background: wgpu::RenderPipeline,
    blit: wgpu::RenderPipeline,
    grid: wgpu::RenderPipeline,
    cursor: wgpu::RenderPipeline,
    readout_rect: wgpu::RenderPipeline,
    crosses: wgpu::RenderPipeline,
    volume: wgpu::RenderPipeline,
    price_last: wgpu::RenderPipeline,
    price_mark: wgpu::RenderPipeline,
    book_bg: wgpu::RenderPipeline,
    book_bars: wgpu::RenderPipeline,
    zone: wgpu::RenderPipeline,
    hline: wgpu::RenderPipeline,
    seg: wgpu::RenderPipeline,
    marker: wgpu::RenderPipeline,
    sampler: wgpu::Sampler,
    point_sampler: wgpu::Sampler,
}

struct BackgroundTexture {
    _texture: wgpu::Texture,
    view: wgpu::TextureView,
}

struct BaseTexture {
    _texture: wgpu::Texture,
    view: wgpu::TextureView,
    w: u32,
    h: u32,
    generation: u64,
}

struct ComboTexture {
    _texture: wgpu::Texture,
    view: wgpu::TextureView,
    bind_group: Option<wgpu::BindGroup>,
    blit_uniform: BufferSlot,
    w: u32,
    h: u32,
    generation: u64,
    bake_t0: f32,
    last_baked_head: usize,
    last_time_to_px: f32,
    last_price_to_px: f32,
    last_view_price0: f32,
    last_marker_half: f32,
    valid: bool,
}

impl ComboTexture {
    fn prepare_blit_bind_group(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        layout: &wgpu::BindGroupLayout,
        sampler: &wgpu::Sampler,
        params: BackgroundParams,
    ) -> &wgpu::BindGroup {
        let recreated = self.blit_uniform.write(
            device,
            queue,
            "moon_chart_combo_blit_uniform",
            wgpu::BufferUsages::UNIFORM,
            &[params],
        );
        if recreated {
            self.bind_group = None;
        }
        if self.bind_group.is_none() {
            self.bind_group = Some(device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("moon_chart_combo_blit_bind"),
                layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: self.blit_uniform.binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(&self.view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::Sampler(sampler),
                    },
                ],
            }));
        }
        self.bind_group.as_ref().unwrap()
    }
}

#[derive(Default)]
struct BaseCache {
    texture: Option<BaseTexture>,
    bind_group: Option<wgpu::BindGroup>,
    blit_uniform: BufferSlot,
    valid: bool,
}

impl BaseCache {
    fn is_valid_for(&self, gpu: &RawGpuAccess) -> bool {
        let w = gpu.width();
        let h = gpu.height();
        let generation = gpu.device_generation();
        self.valid
            && self
                .texture
                .as_ref()
                .is_some_and(|tex| tex.w == w && tex.h == h && tex.generation == generation)
    }

    fn needs_rebuild(&self, gpu: &RawGpuAccess) -> bool {
        !self.is_valid_for(gpu)
    }

    fn ensure_texture(
        &mut self,
        device: &wgpu::Device,
        gpu: &RawGpuAccess,
        format: wgpu::TextureFormat,
    ) -> &wgpu::TextureView {
        let w = gpu.width().max(1);
        let h = gpu.height().max(1);
        let generation = gpu.device_generation();
        let recreate = self
            .texture
            .as_ref()
            .is_none_or(|tex| tex.w != w || tex.h != h || tex.generation != generation);
        if recreate {
            let texture = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("moon_chart_base_cache"),
                size: wgpu::Extent3d {
                    width: w,
                    height: h,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                    | wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            });
            let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
            self.texture = Some(BaseTexture {
                _texture: texture,
                view,
                w,
                h,
                generation,
            });
            self.bind_group = None;
            self.valid = false;
        }
        &self.texture.as_ref().unwrap().view
    }

    fn prepare_blit_bind_group(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        layout: &wgpu::BindGroupLayout,
        sampler: &wgpu::Sampler,
        params: BackgroundParams,
    ) -> &wgpu::BindGroup {
        let recreated = self.blit_uniform.write(
            device,
            queue,
            "moon_chart_base_blit_uniform",
            wgpu::BufferUsages::UNIFORM,
            &[params],
        );
        if recreated {
            self.bind_group = None;
        }
        if self.bind_group.is_none() {
            let view = &self.texture.as_ref().unwrap().view;
            self.bind_group = Some(device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("moon_chart_base_blit_bind"),
                layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: self.blit_uniform.binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::Sampler(sampler),
                    },
                ],
            }));
        }
        self.bind_group.as_ref().unwrap()
    }
}

struct PreparedBindGroups {
    bg: wgpu::BindGroup,
    grid: wgpu::BindGroup,
    cursor: wgpu::BindGroup,
    readout: wgpu::BindGroup,
    cross: wgpu::BindGroup,
    last: wgpu::BindGroup,
    mark: wgpu::BindGroup,
    book: wgpu::BindGroup,
    zone: wgpu::BindGroup,
    hline: wgpu::BindGroup,
    seg: wgpu::BindGroup,
    marker: wgpu::BindGroup,
}

pub struct WgpuLayers {
    device_generation: u64,
    format: Option<wgpu::TextureFormat>,
    pipelines: Option<Pipelines>,
    background_texture: Option<BackgroundTexture>,
    prepared_binds: Option<PreparedBindGroups>,
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

impl WgpuLayers {
    pub fn new() -> Self {
        Self {
            device_generation: 0,
            format: None,
            pipelines: None,
            background_texture: None,
            prepared_binds: None,
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
        let full_reset = data.len() >= self.combo_capacity;
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
        self.update_volume_scale(data);
        self.combo_buffers_dirty = true;
        if full_reset || before_scale != (self.volume_buy_max, self.volume_sell_max) {
            if let Some(tex) = self.combo_texture.as_mut() {
                tex.valid = false;
            }
            self.combo_dirty_ranges.clear();
        } else {
            let appended = data.len().min(self.combo_capacity);
            for (start, count) in append_ranges(old_head, appended, self.combo_capacity) {
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
        self.base_cache.needs_rebuild(gpu)
    }

    fn reset_gpu_objects(&mut self) {
        self.pipelines = None;
        self.background_texture = None;
        self.prepared_binds = None;
        self.base_cache = BaseCache::default();
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
        let Some((device, queue, pass)) = (unsafe { borrow_wgpu_draw(gpu) }) else {
            anyhow::bail!("chart wgpu draw received empty wgpu raw gpu handles");
        };
        let _ = (background_params, grid_params);
        self.upload_frame_uniforms(
            device,
            queue,
            view,
            orderbook_view,
            background_params,
            grid_params,
            cursor_params,
            readout_rects,
        );
        self.prepare_bind_groups(device);
        let sc = scissor_rect(view, orderbook_view, gpu.width(), gpu.height());
        pass.set_scissor_rect(sc.0, sc.1, sc.2, sc.3);

        if self.base_cache.is_valid_for(gpu) {
            self.draw_cached_base(device, queue, pass, view, orderbook_view, gpu);
        } else {
            self.draw_base_layers(pass);
            self.draw_cached_combo(device, queue, pass, view);
        }
        let sc = bounds_scissor(pane_bounds, gpu.width(), gpu.height());
        pass.set_scissor_rect(sc.0, sc.1, sc.2, sc.3);
        self.draw_cursor_layer(pass, cursor_params, readout_rects);
        Ok(())
    }

    fn draw_base_layers(&self, pass: &mut wgpu::RenderPass<'_>) {
        let pipelines = self.pipelines.as_ref().unwrap();
        let binds = self.prepared_binds.as_ref().unwrap();
        crate::diag::bump(&crate::diag::CHART_BG_DRAW);
        draw_pipeline(pass, &pipelines.background, &binds.bg, 6, 1);
        crate::diag::bump(&crate::diag::CHART_GRID_DRAW);
        draw_pipeline(pass, &pipelines.grid, &binds.grid, 6, 1);
        crate::diag::bump(&crate::diag::CHART_BOOK_DRAW);
        draw_pipeline(pass, &pipelines.book_bg, &binds.book, 6, 1);
        if !self.levels.is_empty() {
            draw_pipeline(
                pass,
                &pipelines.book_bars,
                &binds.book,
                6,
                self.levels.len() as u32,
            );
        }
        if !self.zones.is_empty() {
            crate::diag::bump(&crate::diag::CHART_USER_DRAW);
            draw_pipeline(
                pass,
                &pipelines.zone,
                &binds.zone,
                6,
                self.zones.len() as u32,
            );
        }
        if !self.hlines.is_empty() {
            crate::diag::bump(&crate::diag::CHART_USER_DRAW);
            draw_pipeline(
                pass,
                &pipelines.hline,
                &binds.hline,
                6,
                self.hlines.len() as u32,
            );
        }
        if !self.segs.is_empty() {
            crate::diag::bump(&crate::diag::CHART_USER_DRAW);
            draw_pipeline(pass, &pipelines.seg, &binds.seg, 6, self.segs.len() as u32);
        }
        if !self.markers.is_empty() {
            crate::diag::bump(&crate::diag::CHART_USER_DRAW);
            draw_pipeline(
                pass,
                &pipelines.marker,
                &binds.marker,
                6,
                self.markers.len() as u32,
            );
        }
    }

    fn ensure_combo_texture(
        &mut self,
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
        tex_w: u32,
        tex_h: u32,
        generation: u64,
    ) {
        let recreate = self
            .combo_texture
            .as_ref()
            .is_none_or(|tex| tex.w != tex_w || tex.h != tex_h || tex.generation != generation);
        if recreate {
            let texture = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("moon_chart_combo_cache"),
                size: wgpu::Extent3d {
                    width: tex_w,
                    height: tex_h,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                    | wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            });
            let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
            self.combo_texture = Some(ComboTexture {
                _texture: texture,
                view,
                bind_group: None,
                blit_uniform: BufferSlot::default(),
                w: tex_w,
                h: tex_h,
                generation,
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
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        gpu: &RawGpuAccess,
        format: wgpu::TextureFormat,
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
        self.ensure_combo_texture(device, format, tex_w, tex_h, gpu.device_generation());

        let (need_full, bake_t0, combo_view) = {
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
            (need_full, bake_t0, tex.view.clone())
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
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("moon_chart_combo_cache_pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &combo_view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: if need_full {
                            wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT)
                        } else {
                            wgpu::LoadOp::Load
                        },
                        store: wgpu::StoreOp::Store,
                    },
                    depth_slice: None,
                })],
                depth_stencil_attachment: None,
                ..Default::default()
            });
            pass.set_scissor_rect(0, 0, tex_w, tex_h);
            if need_full {
                self.draw_combo_layers(
                    device,
                    queue,
                    &mut pass,
                    bake_view,
                    0,
                    self.cross_count,
                    true,
                );
            } else {
                let ranges = std::mem::take(&mut self.combo_dirty_ranges);
                for (start, count) in ranges {
                    if count > 0 {
                        self.draw_combo_layers(
                            device, queue, &mut pass, bake_view, start, count, false,
                        );
                    }
                }
            }
        }
        let tex = self.combo_texture.as_mut().unwrap();
        if need_full {
            tex.bake_t0 = bake_t0;
            tex.last_time_to_px = view.time_to_px;
            tex.last_price_to_px = view.price_to_px;
            tex.last_view_price0 = view.view_price0;
            tex.last_marker_half = view.marker_half;
            tex.valid = true;
            self.combo_dirty_ranges.clear();
            crate::diag::bump(&crate::diag::CHART_COMBO_BAKE);
        }
        tex.last_baked_head = self.cross_head;
        true
    }

    fn draw_combo_layers(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        pass: &mut wgpu::RenderPass<'_>,
        view: ChartViewGpu,
        start: usize,
        count: usize,
        include_price_lines: bool,
    ) {
        let recreated = self.view_uniform.write(
            device,
            queue,
            "moon_chart_combo_view_uniform",
            wgpu::BufferUsages::UNIFORM,
            &[view],
        );
        if recreated || self.prepared_binds.is_none() {
            self.prepared_binds = None;
            self.prepare_bind_groups(device);
        }
        let pipelines = self.pipelines.as_ref().unwrap();
        let binds = self.prepared_binds.as_ref().unwrap();
        if count > 0 {
            crate::diag::bump(&crate::diag::CHART_COMBO_DRAW);
            draw_pipeline_range(pass, &pipelines.volume, &binds.cross, 6, start, count);
        }
        if include_price_lines && self.last_line.len() > 1 {
            crate::diag::bump(&crate::diag::CHART_COMBO_DRAW);
            draw_pipeline(
                pass,
                &pipelines.price_last,
                &binds.last,
                6,
                (self.last_line.len() - 1) as u32,
            );
        }
        if include_price_lines && self.mark_line.len() > 1 {
            crate::diag::bump(&crate::diag::CHART_COMBO_DRAW);
            draw_pipeline(
                pass,
                &pipelines.price_mark,
                &binds.mark,
                6,
                (self.mark_line.len() - 1) as u32,
            );
        }
        if count > 0 {
            crate::diag::bump(&crate::diag::CHART_COMBO_DRAW);
            draw_pipeline_range(pass, &pipelines.crosses, &binds.cross, 6, start, count);
        }
    }

    fn draw_cached_combo(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        pass: &mut wgpu::RenderPass<'_>,
        view: &ChartViewGpu,
    ) {
        let Some(tex) = self.combo_texture.as_mut() else {
            return;
        };
        if !tex.valid || view.bounds[2] <= 0.0 || view.bounds[3] <= 0.0 {
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
        let pipelines = self.pipelines.as_ref().unwrap();
        let bind = tex.prepare_blit_bind_group(
            device,
            queue,
            &pipelines.bg_layout,
            &pipelines.point_sampler,
            params,
        );
        crate::diag::bump(&crate::diag::CHART_BASE_BLIT);
        draw_pipeline(pass, &pipelines.blit, bind, 6, 1);
    }

    fn draw_cursor_layer(
        &self,
        pass: &mut wgpu::RenderPass<'_>,
        cursor_params: &CursorParams,
        readout_rects: &[ReadoutRect],
    ) {
        let pipelines = self.pipelines.as_ref().unwrap();
        let binds = self.prepared_binds.as_ref().unwrap();
        if cursor_params.enabled > 0.0 {
            crate::diag::bump(&crate::diag::CHART_CURSOR_DRAW);
            draw_pipeline(pass, &pipelines.cursor, &binds.cursor, 12, 1);
        }
        if !readout_rects.is_empty() {
            draw_pipeline(
                pass,
                &pipelines.readout_rect,
                &binds.readout,
                6,
                readout_rects.len() as u32,
            );
        }
    }

    fn draw_cached_base(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        pass: &mut wgpu::RenderPass<'_>,
        view: &ChartViewGpu,
        orderbook_view: &ChartViewGpu,
        gpu: &RawGpuAccess,
    ) {
        let pipelines = self.pipelines.as_ref().unwrap();
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
        let bind = self.base_cache.prepare_blit_bind_group(
            device,
            queue,
            &pipelines.bg_layout,
            &pipelines.sampler,
            params,
        );
        crate::diag::bump(&crate::diag::CHART_BASE_BLIT);
        draw_pipeline(pass, &pipelines.background, bind, 6, 1);
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
        let Some((device, queue, encoder, format)) = (unsafe { borrow_wgpu_prepare(gpu) }) else {
            anyhow::bail!("chart wgpu prepare received empty wgpu raw gpu handles");
        };
        if self.device_generation != gpu.device_generation() || self.format != Some(format) {
            self.device_generation = gpu.device_generation();
            self.format = Some(format);
            self.reset_gpu_objects();
            self.pipelines = Some(create_pipelines(device, format));
            self.background_texture = Some(create_background_texture(device, queue));
        }
        self.upload_common(
            device,
            queue,
            view,
            orderbook_view,
            background_params,
            grid_params,
            cursor_params,
            book_style,
        );
        self.prepare_bind_groups(device);
        let combo_changed = self.prepare_combo_cache(device, queue, encoder, gpu, format, view);
        if rebuild_base || combo_changed || self.base_cache.needs_rebuild(gpu) {
            self.rebuild_base_cache(device, queue, encoder, gpu, format, view, orderbook_view)?;
        }
        Ok(())
    }

    fn rebuild_base_cache(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        gpu: &RawGpuAccess,
        format: wgpu::TextureFormat,
        view: &ChartViewGpu,
        orderbook_view: &ChartViewGpu,
    ) -> anyhow::Result<()> {
        let base_view = self.base_cache.ensure_texture(device, gpu, format).clone();
        let sc = scissor_rect(view, orderbook_view, gpu.width(), gpu.height());
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("moon_chart_base_cache_pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &base_view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Store,
                    },
                    depth_slice: None,
                })],
                depth_stencil_attachment: None,
                ..Default::default()
            });
            pass.set_scissor_rect(sc.0, sc.1, sc.2, sc.3);
            self.draw_base_layers(&mut pass);
            self.draw_cached_combo(device, queue, &mut pass, view);
        }
        self.base_cache.valid = true;
        crate::diag::bump(&crate::diag::CHART_BASE_BAKE);
        Ok(())
    }

    fn upload_common(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
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
        let mut binds_dirty = false;
        binds_dirty |= self.bg_uniform.write(
            device,
            queue,
            "moon_chart_bg_uniform",
            wgpu::BufferUsages::UNIFORM,
            &[*background_params],
        );
        binds_dirty |= self.grid_uniform.write(
            device,
            queue,
            "moon_chart_grid_uniform",
            wgpu::BufferUsages::UNIFORM,
            &[*grid_params],
        );
        binds_dirty |= self.cursor_uniform.write(
            device,
            queue,
            "moon_chart_cursor_uniform",
            wgpu::BufferUsages::UNIFORM,
            &[*cursor_params],
        );
        binds_dirty |= self.readout_rect_buffer.write(
            device,
            queue,
            "moon_chart_readout_rects",
            wgpu::BufferUsages::STORAGE,
            &[] as &[ReadoutRect],
        );
        binds_dirty |= self.view_uniform.write(
            device,
            queue,
            "moon_chart_view_uniform",
            wgpu::BufferUsages::UNIFORM,
            &[view],
        );
        binds_dirty |= self.book_view_uniform.write(
            device,
            queue,
            "moon_chart_book_view_uniform",
            wgpu::BufferUsages::UNIFORM,
            &[*orderbook_view],
        );
        binds_dirty |= self.book_style_uniform.write(
            device,
            queue,
            "moon_chart_book_style_uniform",
            wgpu::BufferUsages::UNIFORM,
            &[*book_style],
        );
        if self.combo_buffers_dirty || self.cross_buffer.buffer.is_none() {
            if self.cross_buffer.buffer.is_some() && !self.combo_dirty_ranges.is_empty() {
                for &(start, count) in &self.combo_dirty_ranges {
                    let end = start.saturating_add(count).min(self.crosses.len());
                    if start < end {
                        binds_dirty |= self.cross_buffer.write_range(
                            device,
                            queue,
                            "moon_chart_crosses",
                            wgpu::BufferUsages::STORAGE,
                            start,
                            &self.crosses[start..end],
                            self.crosses.len(),
                        );
                    }
                }
            } else {
                binds_dirty |= self.cross_buffer.write(
                    device,
                    queue,
                    "moon_chart_crosses",
                    wgpu::BufferUsages::STORAGE,
                    &self.crosses,
                );
            }
            self.combo_buffers_dirty = false;
        }
        if self.price_line_buffers_dirty
            || self.last_line_buffer.buffer.is_none()
            || self.mark_line_buffer.buffer.is_none()
        {
            binds_dirty |= self.last_line_buffer.write(
                device,
                queue,
                "moon_chart_last_line",
                wgpu::BufferUsages::STORAGE,
                &self.last_line,
            );
            binds_dirty |= self.mark_line_buffer.write(
                device,
                queue,
                "moon_chart_mark_line",
                wgpu::BufferUsages::STORAGE,
                &self.mark_line,
            );
            self.price_line_buffers_dirty = false;
        }
        if self.book_buffer_dirty || self.level_buffer.buffer.is_none() {
            binds_dirty |= self.level_buffer.write(
                device,
                queue,
                "moon_chart_book_levels",
                wgpu::BufferUsages::STORAGE,
                &self.levels,
            );
            self.book_buffer_dirty = false;
        }
        if self.userdata_buffers_dirty
            || self.zone_buffer.buffer.is_none()
            || self.hline_buffer.buffer.is_none()
            || self.seg_buffer.buffer.is_none()
            || self.marker_buffer.buffer.is_none()
        {
            binds_dirty |= self.zone_buffer.write(
                device,
                queue,
                "moon_chart_zones",
                wgpu::BufferUsages::STORAGE,
                &self.zones,
            );
            binds_dirty |= self.hline_buffer.write(
                device,
                queue,
                "moon_chart_hlines",
                wgpu::BufferUsages::STORAGE,
                &self.hlines,
            );
            binds_dirty |= self.seg_buffer.write(
                device,
                queue,
                "moon_chart_segs",
                wgpu::BufferUsages::STORAGE,
                &self.segs,
            );
            binds_dirty |= self.marker_buffer.write(
                device,
                queue,
                "moon_chart_markers",
                wgpu::BufferUsages::STORAGE,
                &self.markers,
            );
            self.userdata_buffers_dirty = false;
        }
        if binds_dirty {
            self.prepared_binds = None;
        }
    }

    fn upload_frame_uniforms(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
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
        let mut binds_dirty = false;
        binds_dirty |= self.bg_uniform.write(
            device,
            queue,
            "moon_chart_bg_uniform",
            wgpu::BufferUsages::UNIFORM,
            &[*background_params],
        );
        binds_dirty |= self.grid_uniform.write(
            device,
            queue,
            "moon_chart_grid_uniform",
            wgpu::BufferUsages::UNIFORM,
            &[*grid_params],
        );
        binds_dirty |= self.cursor_uniform.write(
            device,
            queue,
            "moon_chart_cursor_uniform",
            wgpu::BufferUsages::UNIFORM,
            &[*cursor_params],
        );
        binds_dirty |= self.readout_rect_buffer.write(
            device,
            queue,
            "moon_chart_readout_rects",
            wgpu::BufferUsages::STORAGE,
            readout_rects,
        );
        binds_dirty |= self.view_uniform.write(
            device,
            queue,
            "moon_chart_view_uniform",
            wgpu::BufferUsages::UNIFORM,
            &[view],
        );
        binds_dirty |= self.book_view_uniform.write(
            device,
            queue,
            "moon_chart_book_view_uniform",
            wgpu::BufferUsages::UNIFORM,
            &[*orderbook_view],
        );
        if binds_dirty {
            self.prepared_binds = None;
        }
    }

    fn bind_uniform<'a>(
        &'a self,
        device: &wgpu::Device,
        layout: &'a wgpu::BindGroupLayout,
        uniform: &'a BufferSlot,
    ) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("moon_chart_uniform_bind"),
            layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform.binding(),
            }],
        })
    }

    fn bind_view_storage<'a>(
        &'a self,
        device: &wgpu::Device,
        layout: &'a wgpu::BindGroupLayout,
        storage: &'a BufferSlot,
    ) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("moon_chart_view_storage_bind"),
            layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: self.view_uniform.binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: storage.binding(),
                },
            ],
        })
    }

    fn bind_readout(
        &self,
        device: &wgpu::Device,
        layout: &wgpu::BindGroupLayout,
    ) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("moon_chart_readout_bind"),
            layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: self.readout_rect_buffer.binding(),
            }],
        })
    }

    fn prepare_bind_groups(&mut self, device: &wgpu::Device) {
        let pipelines = self.pipelines.as_ref().unwrap();
        let bg = self.background_texture.as_ref().unwrap();
        let bg_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("moon_chart_bg_bind"),
            layout: &pipelines.bg_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: self.bg_uniform.binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&bg.view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&pipelines.sampler),
                },
            ],
        });
        let grid_bind = self.bind_uniform(device, &pipelines.grid_layout, &self.grid_uniform);
        let cursor_bind = self.bind_uniform(device, &pipelines.cursor_layout, &self.cursor_uniform);
        let readout_bind = self.bind_readout(device, &pipelines.readout_layout);
        let cross_bind =
            self.bind_view_storage(device, &pipelines.view_storage_layout, &self.cross_buffer);
        let last_bind = self.bind_view_storage(
            device,
            &pipelines.view_storage_layout,
            &self.last_line_buffer,
        );
        let mark_bind = self.bind_view_storage(
            device,
            &pipelines.view_storage_layout,
            &self.mark_line_buffer,
        );
        let book_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("moon_chart_book_bind"),
            layout: &pipelines.book_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: self.book_view_uniform.binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: self.book_style_uniform.binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: self.level_buffer.binding(),
                },
            ],
        });
        let zone_bind =
            self.bind_view_storage(device, &pipelines.view_storage_layout, &self.zone_buffer);
        let hline_bind =
            self.bind_view_storage(device, &pipelines.view_storage_layout, &self.hline_buffer);
        let seg_bind =
            self.bind_view_storage(device, &pipelines.view_storage_layout, &self.seg_buffer);
        let marker_bind =
            self.bind_view_storage(device, &pipelines.view_storage_layout, &self.marker_buffer);
        self.prepared_binds = Some(PreparedBindGroups {
            bg: bg_bind,
            grid: grid_bind,
            cursor: cursor_bind,
            readout: readout_bind,
            cross: cross_bind,
            last: last_bind,
            mark: mark_bind,
            book: book_bind,
            zone: zone_bind,
            hline: hline_bind,
            seg: seg_bind,
            marker: marker_bind,
        });
    }

    fn recalc_volume_scale(&mut self) {
        self.volume_buy_max = 1e-6;
        self.volume_sell_max = 1e-6;
        for c in self.crosses.iter().take(self.cross_count) {
            if c.side == 0 {
                self.volume_buy_max = self.volume_buy_max.max(c.qty);
            } else {
                self.volume_sell_max = self.volume_sell_max.max(c.qty);
            }
        }
    }

    fn update_volume_scale(&mut self, data: &[ChartCross]) {
        for c in data {
            if c.side == 0 {
                self.volume_buy_max = self.volume_buy_max.max(c.qty);
            } else {
                self.volume_sell_max = self.volume_sell_max.max(c.qty);
            }
        }
    }
}

fn draw_pipeline(
    pass: &mut wgpu::RenderPass<'_>,
    pipeline: &wgpu::RenderPipeline,
    bind_group: &wgpu::BindGroup,
    vertices: u32,
    instances: u32,
) {
    pass.set_pipeline(pipeline);
    pass.set_bind_group(0, bind_group, &[]);
    pass.draw(0..vertices, 0..instances);
}

fn draw_pipeline_range(
    pass: &mut wgpu::RenderPass<'_>,
    pipeline: &wgpu::RenderPipeline,
    bind_group: &wgpu::BindGroup,
    vertices: u32,
    first_instance: usize,
    instances: usize,
) {
    let first = first_instance as u32;
    let last = first_instance.saturating_add(instances) as u32;
    pass.set_pipeline(pipeline);
    pass.set_bind_group(0, bind_group, &[]);
    pass.draw(0..vertices, first..last);
}

fn tail_vec<T: Clone>(data: &[T], cap: usize) -> Vec<T> {
    let start = data.len().saturating_sub(cap);
    data[start..].to_vec()
}

fn sanitize_capacity(capacity: usize) -> usize {
    capacity.max(MIN_COMBO_CAPACITY)
}

unsafe fn borrow_wgpu_prepare<'a>(
    gpu: &RawGpuAccess,
) -> Option<(
    &'a wgpu::Device,
    &'a wgpu::Queue,
    &'a mut wgpu::CommandEncoder,
    wgpu::TextureFormat,
)> {
    let RawGpuAccess::Wgpu(gpu) = gpu else {
        return None;
    };
    // Все поля — NonNull<c_void> (по контракту не null): берём сырой указатель `.as_ptr()`.
    Some((
        unsafe { &*(gpu.device.as_ptr() as *const wgpu::Device) },
        unsafe { &*(gpu.queue.as_ptr() as *const wgpu::Queue) },
        unsafe { &mut *(gpu.command_encoder.as_ptr() as *mut wgpu::CommandEncoder) },
        unsafe { *(gpu.render_target_format.as_ptr() as *const wgpu::TextureFormat) },
    ))
}

unsafe fn borrow_wgpu_draw<'a>(
    gpu: &RawGpuAccess,
) -> Option<(
    &'a wgpu::Device,
    &'a wgpu::Queue,
    &'a mut wgpu::RenderPass<'a>,
)> {
    let RawGpuAccess::Wgpu(gpu) = gpu else {
        return None;
    };
    // render_pass — Option<NonNull<c_void>>: None во время prepare (пасса ещё нет).
    let render_pass = gpu.render_pass?;
    Some((
        unsafe { &*(gpu.device.as_ptr() as *const wgpu::Device) },
        unsafe { &*(gpu.queue.as_ptr() as *const wgpu::Queue) },
        unsafe { &mut *(render_pass.as_ptr() as *mut wgpu::RenderPass<'a>) },
    ))
}

fn scissor_rect(
    view: &ChartViewGpu,
    orderbook_view: &ChartViewGpu,
    width: u32,
    height: u32,
) -> (u32, u32, u32, u32) {
    let l = view.bounds[0].floor().max(0.0) as u32;
    let t = view.bounds[1].floor().max(0.0) as u32;
    let r = (orderbook_view.bounds[0] + orderbook_view.bounds[2])
        .ceil()
        .clamp(l as f32 + 1.0, width.max(1) as f32) as u32;
    let b = (view.bounds[1] + view.bounds[3])
        .ceil()
        .clamp(t as f32 + 1.0, height.max(1) as f32) as u32;
    (l, t, (r - l).max(1), (b - t).max(1))
}

fn bounds_scissor(bounds: [f32; 4], width: u32, height: u32) -> (u32, u32, u32, u32) {
    let l = bounds[0].floor().max(0.0) as u32;
    let t = bounds[1].floor().max(0.0) as u32;
    let r = (bounds[0] + bounds[2])
        .ceil()
        .clamp(l as f32 + 1.0, width.max(1) as f32) as u32;
    let b = (bounds[1] + bounds[3])
        .ceil()
        .clamp(t as f32 + 1.0, height.max(1) as f32) as u32;
    (l, t, (r - l).max(1), (b - t).max(1))
}

fn panel_dst(
    view: &ChartViewGpu,
    orderbook_view: &ChartViewGpu,
    width: u32,
    height: u32,
) -> [f32; 4] {
    let (x, y, w, h) = scissor_rect(view, orderbook_view, width, height);
    [x as f32, y as f32, w as f32, h as f32]
}

fn create_pipelines(device: &wgpu::Device, format: wgpu::TextureFormat) -> Pipelines {
    let background_shader = shader(device, "moon_chart_background_wgsl", BACKGROUND_SHADER);
    let grid_shader = shader(device, "moon_chart_grid_wgsl", GRID_SHADER);
    let cursor_shader = shader(device, "moon_chart_cursor_wgsl", CURSOR_SHADER);
    let crosses_shader = shader(device, "moon_chart_crosses_wgsl", CROSSES_SHADER);
    let price_shader = shader(device, "moon_chart_price_wgsl", PRICE_SHADER);
    let book_shader = shader(device, "moon_chart_book_wgsl", BOOK_SHADER);
    let zone_shader = shader(device, "moon_chart_zone_wgsl", ZONE_SHADER);
    let hline_shader = shader(device, "moon_chart_hline_wgsl", HLINE_SHADER);
    let seg_shader = shader(device, "moon_chart_seg_wgsl", SEG_SHADER);
    let marker_shader = shader(device, "moon_chart_marker_wgsl", MARKER_SHADER);
    let bg_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("moon_chart_bg_layout"),
        entries: &[
            uniform_entry(0, std::mem::size_of::<BackgroundParams>()),
            wgpu::BindGroupLayoutEntry {
                binding: 1,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 2,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                count: None,
            },
        ],
    });
    let grid_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("moon_chart_grid_layout"),
        entries: &[uniform_entry(0, std::mem::size_of::<GridParams>())],
    });
    let cursor_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("moon_chart_cursor_layout"),
        entries: &[uniform_entry(0, std::mem::size_of::<CursorParams>())],
    });
    let readout_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("moon_chart_readout_layout"),
        entries: &[storage_entry(0)],
    });
    let view_storage_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("moon_chart_view_storage_layout"),
        entries: &[
            uniform_entry(0, std::mem::size_of::<ChartViewGpu>()),
            storage_entry(1),
        ],
    });
    let book_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("moon_chart_book_layout"),
        entries: &[
            uniform_entry(0, std::mem::size_of::<ChartViewGpu>()),
            uniform_entry(1, std::mem::size_of::<BookStyle>()),
            storage_entry(2),
        ],
    });
    let background = pipeline(
        device,
        format,
        &background_shader,
        &bg_layout,
        "background_vertex",
        "background_fragment",
    );
    let blit = pipeline(
        device,
        format,
        &background_shader,
        &bg_layout,
        "background_vertex",
        "blit_fragment",
    );
    let grid = pipeline(
        device,
        format,
        &grid_shader,
        &grid_layout,
        "grid_vertex",
        "grid_fragment",
    );
    let cursor = pipeline(
        device,
        format,
        &cursor_shader,
        &cursor_layout,
        "cursor_vertex",
        "cursor_fragment",
    );
    let readout_shader = shader(device, "moon_chart_readout_wgsl", READOUT_SHADER);
    let readout_rect = pipeline(
        device,
        format,
        &readout_shader,
        &readout_layout,
        "readout_rect_vertex",
        "readout_rect_fragment",
    );
    let crosses = pipeline(
        device,
        format,
        &crosses_shader,
        &view_storage_layout,
        "crosses_vertex",
        "crosses_fragment",
    );
    let volume = pipeline(
        device,
        format,
        &crosses_shader,
        &view_storage_layout,
        "volume_vertex",
        "volume_fragment",
    );
    let price_last = pipeline(
        device,
        format,
        &price_shader,
        &view_storage_layout,
        "price_line_vertex",
        "price_last_fragment",
    );
    let price_mark = pipeline(
        device,
        format,
        &price_shader,
        &view_storage_layout,
        "price_line_vertex",
        "price_mark_fragment",
    );
    let book_bg = opaque_pipeline(
        device,
        format,
        &book_shader,
        &book_layout,
        "book_bg_vertex",
        "book_bg_fragment",
    );
    let book_bars = pipeline(
        device,
        format,
        &book_shader,
        &book_layout,
        "book_bars_vertex",
        "book_bars_fragment",
    );
    let zone = pipeline(
        device,
        format,
        &zone_shader,
        &view_storage_layout,
        "zone_vertex",
        "zone_fragment",
    );
    let hline = pipeline(
        device,
        format,
        &hline_shader,
        &view_storage_layout,
        "hline_vertex",
        "hline_fragment",
    );
    let seg = pipeline(
        device,
        format,
        &seg_shader,
        &view_storage_layout,
        "seg_vertex",
        "seg_fragment",
    );
    let marker = pipeline(
        device,
        format,
        &marker_shader,
        &view_storage_layout,
        "marker_vertex",
        "marker_fragment",
    );
    let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("moon_chart_bg_sampler"),
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Linear,
        ..Default::default()
    });
    let point_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("moon_chart_point_sampler"),
        mag_filter: wgpu::FilterMode::Nearest,
        min_filter: wgpu::FilterMode::Nearest,
        ..Default::default()
    });
    Pipelines {
        bg_layout,
        grid_layout,
        cursor_layout,
        readout_layout,
        view_storage_layout,
        book_layout,
        background,
        blit,
        grid,
        cursor,
        readout_rect,
        crosses,
        volume,
        price_last,
        price_mark,
        book_bg,
        book_bars,
        zone,
        hline,
        seg,
        marker,
        sampler,
        point_sampler,
    }
}

fn shader(device: &wgpu::Device, label: &'static str, source: &'static str) -> wgpu::ShaderModule {
    device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some(label),
        source: wgpu::ShaderSource::Wgsl(source.into()),
    })
}

fn uniform_entry(binding: u32, size: usize) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: false,
            min_binding_size: NonZeroU64::new(size as u64),
        },
        count: None,
    }
}

fn storage_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Storage { read_only: true },
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}

fn pipeline(
    device: &wgpu::Device,
    format: wgpu::TextureFormat,
    shader: &wgpu::ShaderModule,
    bind_group_layout: &wgpu::BindGroupLayout,
    vs: &str,
    fs: &str,
) -> wgpu::RenderPipeline {
    pipeline_with_blend(
        device,
        format,
        shader,
        bind_group_layout,
        vs,
        fs,
        Some(alpha_blend_state()),
    )
}

fn opaque_pipeline(
    device: &wgpu::Device,
    format: wgpu::TextureFormat,
    shader: &wgpu::ShaderModule,
    bind_group_layout: &wgpu::BindGroupLayout,
    vs: &str,
    fs: &str,
) -> wgpu::RenderPipeline {
    pipeline_with_blend(device, format, shader, bind_group_layout, vs, fs, None)
}

fn pipeline_with_blend(
    device: &wgpu::Device,
    format: wgpu::TextureFormat,
    shader: &wgpu::ShaderModule,
    bind_group_layout: &wgpu::BindGroupLayout,
    vs: &str,
    fs: &str,
    blend: Option<wgpu::BlendState>,
) -> wgpu::RenderPipeline {
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("moon_chart_pipeline_layout"),
        bind_group_layouts: &[Some(bind_group_layout)],
        immediate_size: 0,
    });
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("moon_chart_pipeline"),
        layout: Some(&layout),
        vertex: wgpu::VertexState {
            module: shader,
            entry_point: Some(vs),
            buffers: &[],
            compilation_options: wgpu::PipelineCompilationOptions::default(),
        },
        primitive: wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleList,
            ..Default::default()
        },
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        fragment: Some(wgpu::FragmentState {
            module: shader,
            entry_point: Some(fs),
            targets: &[Some(wgpu::ColorTargetState {
                format,
                blend,
                write_mask: wgpu::ColorWrites::ALL,
            })],
            compilation_options: wgpu::PipelineCompilationOptions::default(),
        }),
        multiview_mask: None,
        cache: None,
    })
}

fn alpha_blend_state() -> wgpu::BlendState {
    wgpu::BlendState {
        color: wgpu::BlendComponent {
            src_factor: wgpu::BlendFactor::SrcAlpha,
            dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
            operation: wgpu::BlendOperation::Add,
        },
        alpha: wgpu::BlendComponent {
            src_factor: wgpu::BlendFactor::One,
            dst_factor: wgpu::BlendFactor::One,
            operation: wgpu::BlendOperation::Add,
        },
    }
}

fn create_background_texture(device: &wgpu::Device, queue: &wgpu::Queue) -> BackgroundTexture {
    let image = image::load_from_memory(BACKGROUND_PNG)
        .expect("embedded chart background must decode")
        .to_rgba8();
    let size = wgpu::Extent3d {
        width: image.width().max(1),
        height: image.height().max(1),
        depth_or_array_layers: 1,
    };
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("moon_chart_background"),
        size,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        image.as_raw(),
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(image.width() * 4),
            rows_per_image: None,
        },
        size,
    );
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    BackgroundTexture {
        _texture: texture,
        view,
    }
}
