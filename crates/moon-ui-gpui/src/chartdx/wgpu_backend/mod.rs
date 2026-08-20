//! Linux GPUI native wgpu chart backend. This is an own-pass renderer inside
//! GPUI's existing wgpu frame, not the old moon-chart offscreen/readback path.

use std::num::NonZeroU64;

use bytemuck::Zeroable;
use gpui::RawGpuAccess;
use moon_chart::layers::{LineInstance, MarkerInstance, SegInstance, ZoneInstance};
use moon_core::data::{LevelInstance, PriceLinePoint};

use super::types::{
    BackgroundParams, BookStyle, CandleGpu, CandleStyleGpu, ChartCross, ChartViewGpu, CursorParams,
    GridParams, HLineGpu, MarkerGpu, PriceStyleGpu, ReadoutRect, SegGpu, VolumeStyleGpu, ZoneGpu,
    append_cross_ring, cross_append_ranges, cross_volume_max, evicted_cross_ranges, hl_of, mk_of,
    ordered_cross_ring, ranges_have_entries, ranges_touch_volume_max, reset_cross_ring, seg_of,
    update_cross_volume_max, zone_of,
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
const CANDLES_SHADER: &str = include_str!("shaders/native_candles.wgsl");
const BACKGROUND_PNG: &[u8] = include_bytes!("../../../../assets/img/3Dlogo_s01.png");
const MIN_COMBO_CAPACITY: usize = 1;

#[inline]
fn texel_aligned_time0(time0: f32, time_to_px: f32) -> f32 {
    if !(time_to_px > 1e-9) {
        return time0;
    }
    (time0 * time_to_px).floor() / time_to_px
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
    /// The two price pipelines only. NOT `view_storage_layout`: that one is shared with
    /// crosses, volume, zone, hline, seg and marker, and a third binding on it would
    /// invalidate all six of their bind groups.
    price_layout: wgpu::BindGroupLayout,
    book_layout: wgpu::BindGroupLayout,
    candle_layout: wgpu::BindGroupLayout,
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
    candles: wgpu::RenderPipeline,
    volume_bars: wgpu::RenderPipeline,
    volume_scale: wgpu::RenderPipeline,
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
    /// Trade-volume opacity the texture was baked with.
    ///
    /// Part of the cache key because the bars are baked INTO the texture. It was a
    /// compile-time constant until `ChartGraphicsCfg` gained `trade_volume_alpha`, which is why the
    /// other four fields were once a complete key and no longer are.
    last_volume_alpha: f32,
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
    candle: wgpu::BindGroup,
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
    /// Complete stored candle series, replaced as a unit when its revision changes.
    candles: Vec<CandleGpu>,
    candle_style: CandleStyleGpu,
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
    /// Combo texture is baked in its own coordinate space; never reuse live view_uniform.
    combo_view_uniform: BufferSlot,
    book_view_uniform: BufferSlot,
    book_style_uniform: BufferSlot,
    cross_buffer: BufferSlot,
    last_line_buffer: BufferSlot,
    mark_line_buffer: BufferSlot,
    price_style_uniform: BufferSlot,
    price_style: PriceStyleGpu,
    level_buffer: BufferSlot,
    zone_buffer: BufferSlot,
    hline_buffer: BufferSlot,
    seg_buffer: BufferSlot,
    marker_buffer: BufferSlot,
    candle_buffer: BufferSlot,
    candle_style_uniform: BufferSlot,
    volume_style_uniform: BufferSlot,
    volume_style: VolumeStyleGpu,
    combo_buffers_dirty: bool,
    price_line_buffers_dirty: bool,
    book_buffer_dirty: bool,
    userdata_buffers_dirty: bool,
    candle_buffers_dirty: bool,
}

// Split by responsibility: layers manages data, capacities, and volume scale; render handles
// drawing and the prepare pass; upload manages uniforms and bind groups; pipelines creates
// pipelines, shaders, and the background texture.
mod layers;
mod pipelines;
mod render;
mod upload;
