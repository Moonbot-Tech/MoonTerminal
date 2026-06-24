//! Combo-слой: вся неизменная рыночная история (Trades; PriceLines/Volume — добавятся
//! сюда же). Кресты лежат в резидентном кольце VRAM; «фон-битмап» combo шире экрана на
//! +20% запекается крестовым шейдером и блитится с UV-паном. Прошлое неизменно → двигаем
//! готовый битмап (scroll) + дорисовываем (append) живой край, НЕ перерисовывая историю.
//!
//! Защита от device-lost (P0-4): при смене поколения device хука (GPUI пересоздал
//! устройство) сбрасываем ВСЕ ресурсы — иначе рисовали бы stale-буферами на новом контексте.

use gpui::RawGpuAccess;
use moon_core::data::PriceLinePoint;
use windows::Win32::Graphics::Direct3D::D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST;
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC};

use super::gpu::{
    BlitParams, ChartCross, ChartViewGpu, create_alpha_blend, create_dynamic_cb,
    create_point_sampler, create_srv, create_srv_range, create_structured, full_viewport,
    ring_write_no_overwrite, set_scissor_rect, update_dynamic,
};
use super::types::DEFAULT_VOLUME_ALPHA;

const MIN_COMBO_CAPACITY: u32 = 1;
const CROSSES_HLSL: &str = include_str!("shaders/crosses.hlsl");
const BLIT_HLSL: &str = include_str!("shaders/blit.hlsl");

#[inline]
fn texel_aligned_time0(time0: f32, time_to_px: f32) -> f32 {
    if !(time_to_px > 1e-9) {
        return time0;
    }
    (time0 * time_to_px).floor() / time_to_px
}

/// Pipeline крестов + резидентное кольцо тиков в VRAM.
struct CrossPipe {
    cross_vs: ID3D11VertexShader,
    cross_ps: ID3D11PixelShader,
    volume_vs: ID3D11VertexShader,
    volume_ps: ID3D11PixelShader,
    price_vs: ID3D11VertexShader,
    price_last_ps: ID3D11PixelShader,
    price_mark_ps: ID3D11PixelShader,
    blend: ID3D11BlendState,
    buffer: ID3D11Buffer,
    srv: ID3D11ShaderResourceView,
    last_line_buf: ID3D11Buffer,
    last_line_srv: ID3D11ShaderResourceView,
    mark_line_buf: ID3D11Buffer,
    mark_line_srv: ID3D11ShaderResourceView,
    view_cb: ID3D11Buffer,
}

/// Фон-битмап combo (W*1.2 × H): запечённая история + точка привязки UV-скролла.
struct ComboTex {
    _tex: ID3D11Texture2D, // RAII: держит текстуру (rtv/srv ссылаются)
    rtv: ID3D11RenderTargetView,
    srv: ID3D11ShaderResourceView,
    tex_w: u32,
    tex_h: u32,
    blit_vs: ID3D11VertexShader,
    blit_fs: ID3D11PixelShader,
    blit_cb: ID3D11Buffer,
    sampler: ID3D11SamplerState,
    bake_t0: f32,
    last_baked_head: u32,
    last_time_to_px: f32,
    last_price_to_px: f32,
    last_view_price0: f32,
    last_marker_half: f32,
    valid: bool,
}

pub struct ComboLayer {
    pipe: Option<CrossPipe>,
    tex: Option<ComboTex>,
    count: u32,
    head: u32,
    pending_reset: Option<Vec<ChartCross>>,
    pending_append: Vec<ChartCross>,
    pending_lines: Option<(Vec<PriceLinePoint>, Vec<PriceLinePoint>)>,
    last_line_count: u32,
    mark_line_count: u32,
    cross_capacity: u32,
    price_line_capacity: u32,
    /// Поколение RawGpuAccess device, на котором созданы ресурсы. Сменилось → device-lost.
    device_generation_seen: u64,
    /// Поколение device: ++ при пересоздании (device-lost). Оркестратор сравнивает со своим
    /// last → перезаливает ВСЮ историю (кольцо новое и пустое, append живого края не хватит).
    device_gen: u64,
    volume_buy_max: f32,
    volume_sell_max: f32,
    volume_scale_dirty: bool,
}

impl ComboLayer {
    pub fn new() -> Self {
        Self {
            pipe: None,
            tex: None,
            count: 0,
            head: 0,
            pending_reset: None,
            pending_append: Vec::new(),
            pending_lines: None,
            last_line_count: 0,
            mark_line_count: 0,
            cross_capacity: MIN_COMBO_CAPACITY,
            price_line_capacity: MIN_COMBO_CAPACITY,
            device_generation_seen: 0,
            device_gen: 0,
            volume_buy_max: 1e-6,
            volume_sell_max: 1e-6,
            volume_scale_dirty: false,
        }
    }

    /// Поколение device combo (++ на каждый device-lost). Оркестратор сравнивает со своим
    /// last_device_gen: сменилось → кольцо пустое, нужна полная перезаливка истории.
    pub fn device_gen(&self) -> u64 {
        self.device_gen
    }

    pub fn set_capacity(&mut self, cross_capacity: usize, price_line_capacity: usize) {
        let cross_capacity = sanitize_capacity(cross_capacity);
        let price_line_capacity = sanitize_capacity(price_line_capacity);
        if self.cross_capacity == cross_capacity && self.price_line_capacity == price_line_capacity
        {
            return;
        }
        self.cross_capacity = cross_capacity;
        self.price_line_capacity = price_line_capacity;
        self.pipe = None;
        self.tex = None;
        self.count = 0;
        self.head = 0;
        self.last_line_count = 0;
        self.mark_line_count = 0;
        self.pending_append.clear();
    }

    /// Полная перезаливка набора тиков (reload истории монеты). Сбрасывает append.
    pub fn reset(&mut self, data: Vec<ChartCross>) {
        self.pending_reset = Some(data);
        self.pending_append.clear();
    }

    /// Дополнить кольцо новыми тиками (живой край) — каждый приход данных.
    pub fn append(&mut self, data: &[ChartCross]) {
        if !data.is_empty() {
            self.pending_append.extend_from_slice(data);
        }
    }

    pub fn set_price_lines(&mut self, last: &[PriceLinePoint], mark: &[PriceLinePoint]) {
        self.pending_lines = Some((last.to_vec(), mark.to_vec()));
        if let Some(tex) = self.tex.as_mut() {
            tex.valid = false;
        }
    }

    /// Prepare phase: uploads pending data and bakes/extends the offscreen combo texture.
    /// This may switch render targets and must run from `GpuCanvasDriver::prepare_gpu`.
    pub fn prepare(
        &mut self,
        view: &ChartViewGpu,
        device: &ID3D11Device,
        context: &ID3D11DeviceContext,
        gpu: &RawGpuAccess,
    ) {
        // device-lost guard (P0-4): новый device → старые буферы/шейдеры/кольцо невалидны.
        // Сбрасываем ресурсы И счётчики кольца: пересозданный буфер пуст, а stale count заставил
        // бы DrawInstanced читать мусор. device_gen++ → prepare перезальёт всю историю (collect_all).
        let generation = gpu.device_generation();
        if self.device_generation_seen != generation {
            self.pipe = None;
            self.tex = None;
            self.count = 0;
            self.head = 0;
            self.last_line_count = 0;
            self.mark_line_count = 0;
            self.device_generation_seen = generation;
            self.device_gen = self.device_gen.wrapping_add(1);
        }
        if self.pipe.is_none() {
            self.pipe = Some(self.create_pipe(device));
        }
        self.apply_uploads(context);
        if self.volume_scale_dirty {
            if let Some(tex) = self.tex.as_mut() {
                tex.valid = false;
            }
            self.volume_scale_dirty = false;
        }
        if self.count == 0 {
            return;
        }
        self.prepare_combo(view, device, context);
    }

    /// Рисует Combo в backbuffer хука (фаза UnderScene). `prepare()` уже сделал upload/bake.
    pub fn render(
        &mut self,
        view: &ChartViewGpu,
        context: &ID3D11DeviceContext,
        rtv: &ID3D11RenderTargetView,
        gpu: &RawGpuAccess,
        panel_clip: [f32; 4],
    ) {
        if self.count == 0 {
            return;
        }
        self.blit_combo(view, context, rtv, gpu, panel_clip);
    }

    /// Combo: инкрементальный bake новых тиков в текстуру.
    /// Полный re-bake при исчерпании 20%-запаса или невалидном битмапе (зум/resize/первый кадр).
    fn prepare_combo(
        &mut self,
        view: &ChartViewGpu,
        device: &ID3D11Device,
        context: &ID3D11DeviceContext,
    ) {
        let bw = view.bounds[2];
        let bh = view.bounds[3];
        if bw <= 0.0 || bh <= 0.0 {
            return;
        }
        let margin_px = (bw * 0.2).max(128.0);
        let tex_w = (bw + margin_px).round().max(1.0) as u32;
        let tex_h = bh.round().max(1.0) as u32;
        let need_new = self
            .tex
            .as_ref()
            .map_or(true, |c| c.tex_w != tex_w || c.tex_h != tex_h);
        if need_new {
            self.tex = Some(Self::create_tex(device, tex_w, tex_h));
        }
        let pipe = self.pipe.as_ref().unwrap();
        let last_line_count = self.last_line_count;
        let mark_line_count = self.mark_line_count;
        let tex = self.tex.as_mut().unwrap();
        if tex.last_time_to_px != view.time_to_px
            || tex.last_price_to_px != view.price_to_px
            || tex.last_view_price0 != view.view_price0
            || tex.last_marker_half != view.marker_half
        {
            tex.valid = false;
        }
        let ttp = view.time_to_px;
        let u_left_px = (view.view_time0 - tex.bake_t0) * ttp;
        let need_full = !tex.valid || u_left_px < 0.0 || u_left_px > margin_px;
        let bake_t0 = if need_full {
            texel_aligned_time0(view.view_time0, ttp)
        } else {
            tex.bake_t0
        };
        // bake-юнформ: левый край времени = bake_t0 (фикс), viewport = весь битмап.
        // При full re-bake держим bake_t0 на глобальной texel-фазе. Иначе формула
        // "старый bake + rounded UV scroll" и формула "новый сырой view_time0" расходятся
        // на ±1 px, и исторические крестики визуально подпрыгивают при исчерпании margin.
        let bake_view = ChartViewGpu {
            bounds: [0.0, 0.0, tex_w as f32, tex_h as f32],
            resolution: [tex_w as f32, tex_h as f32],
            time_to_px: ttp,
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
        update_dynamic(context, &pipe.view_cb, &[bake_view]);
        let tex_vp = D3D11_VIEWPORT {
            TopLeftX: 0.0,
            TopLeftY: 0.0,
            Width: tex_w as f32,
            Height: tex_h as f32,
            MinDepth: 0.0,
            MaxDepth: 1.0,
        };
        unsafe {
            context.OMSetRenderTargets(Some(&[Some(tex.rtv.clone())]), None);
            context.RSSetViewports(Some(&[tex_vp]));
            set_scissor_rect(context, 0.0, 0.0, tex_w as f32, tex_h as f32);
            context.IASetPrimitiveTopology(D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
            context.VSSetConstantBuffers(0, Some(&[Some(pipe.view_cb.clone())]));
            context.OMSetBlendState(&pipe.blend, None, 0xFFFFFFFF);
            if need_full {
                crate::diag::bump(&crate::diag::CHART_COMBO_BAKE);
                tex.bake_t0 = bake_t0;
                // ПРОЗРАЧНЫЙ фон битмапа: только кресты непрозрачны → при блите (alpha) сетка/фон
                // нижнего слоя (grid) просвечивают между крестами. Фон #131416 красит grid-слой.
                context.ClearRenderTargetView(&tex.rtv, &[0.0, 0.0, 0.0, 0.0]);
                context.VSSetShaderResources(1, Some(&[Some(pipe.srv.clone())]));
                context.VSSetShader(&pipe.volume_vs, None);
                context.PSSetShader(&pipe.volume_ps, None);
                context.DrawInstanced(6, self.count, 0, 0);
                Self::draw_price_lines(context, pipe, last_line_count, mark_line_count);
                context.VSSetShader(&pipe.cross_vs, None);
                context.PSSetShader(&pipe.cross_ps, None);
                context.DrawInstanced(6, self.count, 0, 0);
                tex.last_baked_head = self.head;
                tex.last_time_to_px = view.time_to_px;
                tex.last_price_to_px = view.price_to_px;
                tex.last_view_price0 = view.view_price0;
                tex.last_marker_half = view.marker_half;
                tex.valid = true;
            } else if self.head != tex.last_baked_head {
                // инкрементально: только новые тики кольца [last_head, head) (с заворотом)
                let cap = self.cross_capacity;
                let delta = (self.head + cap - tex.last_baked_head) % cap;
                let runs: [(u32, u32); 2] = if tex.last_baked_head + delta <= cap {
                    [(tex.last_baked_head, delta), (0, 0)]
                } else {
                    [
                        (tex.last_baked_head, cap - tex.last_baked_head),
                        (0, delta - (cap - tex.last_baked_head)),
                    ]
                };
                for (rf, rc) in runs {
                    if rc == 0 {
                        continue;
                    }
                    let srv_r = create_srv_range(device, &pipe.buffer, rf, rc);
                    context.VSSetShaderResources(1, Some(&[Some(srv_r)]));
                    context.VSSetShader(&pipe.volume_vs, None);
                    context.PSSetShader(&pipe.volume_ps, None);
                    context.DrawInstanced(6, rc, 0, 0);
                    context.VSSetShader(&pipe.cross_vs, None);
                    context.PSSetShader(&pipe.cross_ps, None);
                    context.DrawInstanced(6, rc, 0, 0);
                }
                tex.last_baked_head = self.head;
            }
        }
    }

    fn blit_combo(
        &mut self,
        view: &ChartViewGpu,
        context: &ID3D11DeviceContext,
        rtv: &ID3D11RenderTargetView,
        gpu: &RawGpuAccess,
        panel_clip: [f32; 4],
    ) {
        let bw = view.bounds[2];
        let Some(pipe) = self.pipe.as_ref() else {
            return;
        };
        let Some(tex) = self.tex.as_mut() else {
            return;
        };
        if !tex.valid || bw <= 0.0 {
            return;
        }
        // Композит: блит видимого окна битмапа → чарт-область backbuffer (point-семпл).
        // UV-сдвиг держим в целых texel'ах: дробный сдвиг под point sampler даёт
        // полупиксельный flicker на live-scroll.
        let u_left_px = (view.view_time0 - tex.bake_t0) * view.time_to_px;
        let tex_w = tex.tex_w;
        let u_left_px = u_left_px.round().clamp(0.0, (tex_w as f32 - bw).max(0.0));
        let u_left = u_left_px / tex_w as f32;
        let u_span = bw / tex_w as f32;
        let bp = BlitParams {
            dst: view.bounds,
            resolution: view.resolution,
            uv_off: [u_left, 0.0],
            uv_scale: [u_span, 1.0],
            pad: [0.0, 0.0],
        };
        update_dynamic(context, &tex.blit_cb, &[bp]);
        let vp = full_viewport(gpu);
        unsafe {
            context.OMSetRenderTargets(Some(&[Some(rtv.clone())]), None);
            context.RSSetViewports(Some(&[vp]));
            set_scissor_rect(
                context,
                panel_clip[0],
                panel_clip[1],
                panel_clip[2],
                panel_clip[3],
            );
            context.IASetPrimitiveTopology(D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
            context.VSSetShader(&tex.blit_vs, None);
            context.PSSetShader(&tex.blit_fs, None);
            context.VSSetConstantBuffers(0, Some(&[Some(tex.blit_cb.clone())]));
            context.PSSetConstantBuffers(0, Some(&[Some(tex.blit_cb.clone())]));
            context.PSSetShaderResources(0, Some(&[Some(tex.srv.clone())]));
            context.PSSetSamplers(0, Some(&[Some(tex.sampler.clone())]));
            context.OMSetBlendState(&pipe.blend, None, 0xFFFFFFFF);
            context.Draw(6, 0);
        }
    }

    fn apply_uploads(&mut self, context: &ID3D11DeviceContext) {
        let (tick_buffer, last_line_buf, mark_line_buf) = {
            let pipe = self.pipe.as_ref().unwrap();
            (
                pipe.buffer.clone(),
                pipe.last_line_buf.clone(),
                pipe.mark_line_buf.clone(),
            )
        };
        if let Some((last, mark)) = self.pending_lines.take() {
            self.last_line_count =
                upload_points(context, &last_line_buf, &last, self.price_line_capacity);
            self.mark_line_count =
                upload_points(context, &mark_line_buf, &mark, self.price_line_capacity);
        }
        if let Some(data) = self.pending_reset.take() {
            // при переполнении оставляем последний хвост ёмкости
            let cap = self.cross_capacity;
            let data: &[ChartCross] = if data.len() as u32 > cap {
                &data[data.len() - cap as usize..]
            } else {
                &data
            };
            update_dynamic(context, &tick_buffer, data);
            self.count = data.len() as u32;
            self.head = (data.len() as u32) % cap;
            self.reset_volume_scale(data);
            self.volume_scale_dirty = true;
        }
        if !self.pending_append.is_empty() {
            let data = std::mem::take(&mut self.pending_append);
            let cap = self.cross_capacity;
            let data: &[ChartCross] = if data.len() as u32 > cap {
                &data[data.len() - cap as usize..]
            } else {
                &data
            };
            let n = data.len() as u32;
            ring_write_no_overwrite(context, &tick_buffer, self.head, cap, data);
            self.head = (self.head + n) % cap;
            self.count = (self.count + n).min(cap);
            if self.update_volume_scale(data) {
                self.volume_scale_dirty = true;
            }
        }
    }

    fn reset_volume_scale(&mut self, data: &[ChartCross]) {
        self.volume_buy_max = 1e-6;
        self.volume_sell_max = 1e-6;
        for c in data {
            if c.side == 0 {
                self.volume_buy_max = self.volume_buy_max.max(c.qty);
            } else {
                self.volume_sell_max = self.volume_sell_max.max(c.qty);
            }
        }
    }

    fn update_volume_scale(&mut self, data: &[ChartCross]) -> bool {
        let before = (self.volume_buy_max, self.volume_sell_max);
        for c in data {
            if c.side == 0 {
                self.volume_buy_max = self.volume_buy_max.max(c.qty);
            } else {
                self.volume_sell_max = self.volume_sell_max.max(c.qty);
            }
        }
        before != (self.volume_buy_max, self.volume_sell_max)
    }

    fn draw_price_lines(
        context: &ID3D11DeviceContext,
        pipe: &CrossPipe,
        last_line_count: u32,
        mark_line_count: u32,
    ) {
        unsafe {
            context.VSSetShader(&pipe.price_vs, None);
            if last_line_count > 1 {
                context.VSSetShaderResources(2, Some(&[Some(pipe.last_line_srv.clone())]));
                context.PSSetShader(&pipe.price_last_ps, None);
                context.DrawInstanced(6, last_line_count - 1, 0, 0);
            }
            if mark_line_count > 1 {
                context.VSSetShaderResources(2, Some(&[Some(pipe.mark_line_srv.clone())]));
                context.PSSetShader(&pipe.price_mark_ps, None);
                context.DrawInstanced(6, mark_line_count - 1, 0, 0);
            }
        }
    }

    fn create_pipe(&self, device: &ID3D11Device) -> CrossPipe {
        let cross_vs = super::gpu::make_vs(device, CROSSES_HLSL, "crosses_vertex");
        let cross_ps = super::gpu::make_ps(device, CROSSES_HLSL, "crosses_fragment");
        let volume_vs = super::gpu::make_vs(device, CROSSES_HLSL, "volume_vertex");
        let volume_ps = super::gpu::make_ps(device, CROSSES_HLSL, "volume_fragment");
        let price_vs = super::gpu::make_vs(device, CROSSES_HLSL, "price_line_vertex");
        let price_last_ps = super::gpu::make_ps(device, CROSSES_HLSL, "price_last_fragment");
        let price_mark_ps = super::gpu::make_ps(device, CROSSES_HLSL, "price_mark_fragment");
        let blend = create_alpha_blend(device);
        let buffer = create_structured(
            device,
            std::mem::size_of::<ChartCross>() as u32,
            self.cross_capacity,
        );
        let srv = create_srv(device, &buffer);
        let last_line_buf = create_structured(
            device,
            std::mem::size_of::<PriceLinePoint>() as u32,
            self.price_line_capacity,
        );
        let last_line_srv = create_srv(device, &last_line_buf);
        let mark_line_buf = create_structured(
            device,
            std::mem::size_of::<PriceLinePoint>() as u32,
            self.price_line_capacity,
        );
        let mark_line_srv = create_srv(device, &mark_line_buf);
        let view_cb = create_dynamic_cb(device, std::mem::size_of::<ChartViewGpu>() as u32);
        CrossPipe {
            cross_vs,
            cross_ps,
            volume_vs,
            volume_ps,
            price_vs,
            price_last_ps,
            price_mark_ps,
            blend,
            buffer,
            srv,
            last_line_buf,
            last_line_srv,
            mark_line_buf,
            mark_line_srv,
            view_cb,
        }
    }

    fn create_tex(device: &ID3D11Device, tex_w: u32, tex_h: u32) -> ComboTex {
        let desc = D3D11_TEXTURE2D_DESC {
            Width: tex_w,
            Height: tex_h,
            MipLevels: 1,
            ArraySize: 1,
            Format: DXGI_FORMAT_B8G8R8A8_UNORM,
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            Usage: D3D11_USAGE_DEFAULT,
            BindFlags: (D3D11_BIND_RENDER_TARGET.0 | D3D11_BIND_SHADER_RESOURCE.0) as u32,
            CPUAccessFlags: 0,
            MiscFlags: 0,
        };
        let tex = unsafe {
            let mut o = None;
            device.CreateTexture2D(&desc, None, Some(&mut o)).unwrap();
            o.unwrap()
        };
        let rtv = unsafe {
            let mut o = None;
            device
                .CreateRenderTargetView(&tex, None, Some(&mut o))
                .unwrap();
            o.unwrap()
        };
        let srv = unsafe {
            let mut o = None;
            device
                .CreateShaderResourceView(&tex, None, Some(&mut o))
                .unwrap();
            o.unwrap()
        };
        let blit_vs = super::gpu::make_vs(device, BLIT_HLSL, "blit_vertex");
        let blit_fs = super::gpu::make_ps(device, BLIT_HLSL, "blit_fragment");
        let blit_cb = create_dynamic_cb(device, std::mem::size_of::<BlitParams>() as u32);
        let sampler = create_point_sampler(device);
        ComboTex {
            _tex: tex,
            rtv,
            srv,
            tex_w,
            tex_h,
            blit_vs,
            blit_fs,
            blit_cb,
            sampler,
            bake_t0: 0.0,
            last_baked_head: u32::MAX,
            last_time_to_px: f32::NAN,
            last_price_to_px: f32::NAN,
            last_view_price0: f32::NAN,
            last_marker_half: f32::NAN,
            valid: false,
        }
    }
}

fn upload_points(
    context: &ID3D11DeviceContext,
    buffer: &ID3D11Buffer,
    data: &[PriceLinePoint],
    cap: u32,
) -> u32 {
    let data = if data.len() as u32 > cap {
        &data[data.len() - cap as usize..]
    } else {
        data
    };
    if !data.is_empty() {
        update_dynamic(context, buffer, data);
    }
    data.len() as u32
}

fn sanitize_capacity(capacity: usize) -> u32 {
    capacity.clamp(MIN_COMBO_CAPACITY as usize, u32::MAX as usize) as u32
}
