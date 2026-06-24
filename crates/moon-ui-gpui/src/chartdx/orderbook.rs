//! Слой стакана (OrderBook): СВОЯ зона справа (не временной ряд, без combo). Фон зоны +
//! кумулятивные fill-прямоугольники глубины и отдельные level-lines запекаются в
//! офскрин-текстуру `BookTex` (наш
//! аналог MoonBot `bmGlass`) и блитятся каждый present. Перепечатка текстуры — ТОЛЬКО при
//! смене уровней/Y-трансформа, НЕ каждый кадр: на статике и mouse-move стакан = дешёвый
//! блит готовой текстуры, а не повторная отрисовка сотен баров инстансами 240 раз/с.

use gpui::RawGpuAccess;
use moon_chart::paint::now_unix_ms;
use moon_core::data::LevelInstance;
use windows::Win32::Graphics::Direct3D::D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST;
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC};

use super::gpu::{
    BlitParams, ChartViewGpu, create_alpha_blend, create_dynamic_cb, create_point_sampler,
    create_srv, create_structured, full_viewport, make_ps, make_vs, set_scissor_rect,
    update_dynamic,
};
pub use super::types::BookStyle;

const BARS_HLSL: &str = include_str!("shaders/bars.hlsl");
const BLIT_HLSL: &str = include_str!("shaders/blit.hlsl");
const INITIAL_LEVEL_BUFFER_CAPACITY: u32 = 256;

struct BookPipe {
    bars_vs: ID3D11VertexShader,
    bars_ps: ID3D11PixelShader,
    bg_vs: ID3D11VertexShader,
    bg_ps: ID3D11PixelShader,
    blend: ID3D11BlendState,
    buffer: ID3D11Buffer,
    srv: ID3D11ShaderResourceView,
    level_cap: u32,
    view_cb: ID3D11Buffer,
    style_cb: ID3D11Buffer,
}

/// Офскрин-битмап стакана (наш `bmGlass`): запечённые фон+бары + признак валидности и Y-входы,
/// при которых текстура валидна. Размер = зона стакана (glass_area). Стакан не скроллит по X
/// (зона фиксирована), поэтому блит 1:1, без UV-пана — проще combo.
struct BookTex {
    _tex: ID3D11Texture2D, // RAII: держит текстуру (rtv/srv ссылаются)
    rtv: ID3D11RenderTargetView,
    srv: ID3D11ShaderResourceView,
    tex_w: u32,
    tex_h: u32,
    blit_vs: ID3D11VertexShader,
    blit_fs: ID3D11PixelShader,
    blit_cb: ID3D11Buffer,
    sampler: ID3D11SamplerState,
    last_price_to_px: f32,
    last_view_price0: f32,
    last_style: BookStyle,
    /// Текстура хоть раз отрисована (первый bake обязателен — иначе чёрный стакан).
    baked: bool,
    /// Входы сменились с прошлого bake → нужен ре-bake (троттлится 200мс).
    dirty: bool,
    /// Время прошлого bake (unix мс) — троттл ре-bake до ~5 Гц (MoonBot bmGlass: 200мс).
    last_bake_ms: f64,
}

pub struct OrderBookLayer {
    pipe: Option<BookPipe>,
    tex: Option<BookTex>,
    count: u32,
    pending: Option<Vec<LevelInstance>>,
    device_generation: u64,
}

impl OrderBookLayer {
    pub fn new() -> Self {
        Self {
            pipe: None,
            tex: None,
            count: 0,
            pending: None,
            device_generation: 0,
        }
    }

    /// Залить уровни стакана (целиком). Зовётся при изменении книги/окна → инвалидирует кэш.
    pub fn set(&mut self, levels: Vec<LevelInstance>) {
        self.pending = Some(levels);
    }

    /// Prepare phase: uploads levels and bakes the offscreen book texture when due.
    /// This may switch render targets and must run from `GpuCanvasDriver::prepare_gpu`.
    pub fn prepare(
        &mut self,
        view: &ChartViewGpu,
        style: &BookStyle,
        device: &ID3D11Device,
        context: &ID3D11DeviceContext,
        gpu: &RawGpuAccess,
    ) {
        let bw = view.bounds[2];
        let bh = view.bounds[3];
        if bw <= 0.0 || bh <= 0.0 {
            return;
        }
        // device-lost: пересоздать pipe и текстуру; count=0 (prepare зальёт уровни заново).
        let generation = gpu.device_generation();
        if self.device_generation != generation {
            self.pipe = None;
            self.tex = None;
            self.count = 0;
            self.device_generation = generation;
        }
        if self.pipe.is_none() {
            self.pipe = Some(Self::create_pipe(device, INITIAL_LEVEL_BUFFER_CAPACITY));
        }
        // Применить новые уровни (если пришли) → инвалидировать кэш текстуры.
        let mut levels_changed = false;
        if let Some(levels) = self.pending.take() {
            let need_cap = next_buffer_cap(levels.len(), INITIAL_LEVEL_BUFFER_CAPACITY);
            if self.pipe.as_ref().is_none_or(|p| p.level_cap < need_cap) {
                self.pipe = Some(Self::create_pipe(device, need_cap));
            }
            if !levels.is_empty() {
                let pipe = self.pipe.as_ref().unwrap();
                update_dynamic(context, &pipe.buffer, &levels);
            }
            self.count = levels.len() as u32;
            levels_changed = true;
        }

        let tex_w = bw.round().max(1.0) as u32;
        let tex_h = bh.round().max(1.0) as u32;
        let need_new = self
            .tex
            .as_ref()
            .map_or(true, |t| t.tex_w != tex_w || t.tex_h != tex_h);
        if need_new {
            self.tex = Some(Self::create_tex(device, tex_w, tex_h));
        }

        let pipe = self.pipe.as_ref().unwrap();
        let count = self.count;
        let tex = self.tex.as_mut().unwrap();
        // Стакан позиционируется по ЦЕНЕ → смена Y-трансформа (price_to_px/view_price0) делает
        // картинку другой. Плюс смена уровней. Только это инвалидирует кэш.
        if levels_changed
            || tex.last_price_to_px != view.price_to_px
            || tex.last_view_price0 != view.view_price0
            || *style != tex.last_style
        {
            tex.dirty = true;
        }

        // BAKE: фон+бары в текстуру (texture-local view). Book data may be throttled, but
        // camera/price-transform changes from user pan/zoom must bake immediately; otherwise
        // the chart moves while the glass layer visibly lags behind.
        let now_ms = now_unix_ms();
        let transform_changed = tex.last_price_to_px != view.price_to_px
            || tex.last_view_price0 != view.view_price0
            || tex.last_style != *style;
        let book_data_due = tex.dirty && now_ms - tex.last_bake_ms >= 200.0;
        if !tex.baked || transform_changed || book_data_due {
            crate::diag::bump(&crate::diag::CHART_BOOK_BAKE);
            // bake-view: зона = весь битмап [0,0,tex_w,tex_h], Y-трансформ тот же.
            let bake_view = ChartViewGpu {
                bounds: [0.0, 0.0, tex_w as f32, tex_h as f32],
                resolution: [tex_w as f32, tex_h as f32],
                time_to_px: view.time_to_px,
                view_time0: view.view_time0,
                price_to_px: view.price_to_px,
                view_price0: view.view_price0,
                marker_half: view.marker_half,
                pad: 0.0,
                volume_buy_inv: 0.0,
                volume_sell_inv: 0.0,
                volume_alpha: 0.0,
                _pad2: 0.0,
            };
            update_dynamic(context, &pipe.view_cb, &[bake_view]);
            update_dynamic(context, &pipe.style_cb, &[*style]);
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
                context.ClearRenderTargetView(&tex.rtv, &[0.0, 0.0, 0.0, 0.0]);
                context.IASetPrimitiveTopology(D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
                context.VSSetConstantBuffers(0, Some(&[Some(pipe.view_cb.clone())]));
                context.VSSetConstantBuffers(1, Some(&[Some(pipe.style_cb.clone())]));
                context.PSSetConstantBuffers(1, Some(&[Some(pipe.style_cb.clone())]));
                context.OMSetBlendState(None, None, 0xFFFFFFFF);
                // Фон зоны (всегда, даже при пустой книге) — opaque book_bg.
                context.VSSetShader(&pipe.bg_vs, None);
                context.PSSetShader(&pipe.bg_ps, None);
                context.Draw(6, 0);
                // Fill-прямоугольники и отдельные level-lines.
                if count > 0 {
                    context.OMSetBlendState(&pipe.blend, None, 0xFFFFFFFF);
                    context.VSSetShaderResources(1, Some(&[Some(pipe.srv.clone())]));
                    context.VSSetShader(&pipe.bars_vs, None);
                    context.PSSetShader(&pipe.bars_ps, None);
                    context.DrawInstanced(6, count, 0, 0);
                }
            }
            tex.last_price_to_px = view.price_to_px;
            tex.last_view_price0 = view.view_price0;
            tex.last_style = *style;
            tex.baked = true;
            tex.dirty = false;
            tex.last_bake_ms = now_ms;
        }
    }

    /// Блитит закэшированный стакан в зону `view.bounds`. `panel_clip` — scissor панели:
    /// восстанавливаем его для слоёв ПОСЛЕ нас.
    pub fn render(
        &mut self,
        view: &ChartViewGpu,
        context: &ID3D11DeviceContext,
        rtv: &ID3D11RenderTargetView,
        gpu: &RawGpuAccess,
        panel_clip: [f32; 4],
    ) {
        let Some(tex) = self.tex.as_ref() else {
            return;
        };
        if !tex.baked || view.bounds[2] <= 0.0 || view.bounds[3] <= 0.0 {
            return;
        }
        // BLIT: готовая текстура → зона стакана backbuffer (1:1, full UV). Scissor = panel_clip
        // (восстанавливаем после bake-scissor — иначе userdata-слой после нас обрежется к зоне).
        let bp = BlitParams {
            dst: view.bounds,
            resolution: view.resolution,
            uv_off: [0.0, 0.0],
            uv_scale: [1.0, 1.0],
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
            context.OMSetBlendState(None, None, 0xFFFFFFFF);
            context.Draw(6, 0);
        }
    }

    fn create_pipe(device: &ID3D11Device, level_cap: u32) -> BookPipe {
        let bars_vs = make_vs(device, BARS_HLSL, "bars_vertex");
        let bars_ps = make_ps(device, BARS_HLSL, "bars_fragment");
        let bg_vs = make_vs(device, BARS_HLSL, "bg_vertex");
        let bg_ps = make_ps(device, BARS_HLSL, "bg_fragment");
        let blend = create_alpha_blend(device);
        let buffer = create_structured(
            device,
            std::mem::size_of::<LevelInstance>() as u32,
            level_cap.max(1),
        );
        let srv = create_srv(device, &buffer);
        let view_cb = create_dynamic_cb(device, std::mem::size_of::<ChartViewGpu>() as u32);
        let style_cb = create_dynamic_cb(device, std::mem::size_of::<BookStyle>() as u32);
        BookPipe {
            bars_vs,
            bars_ps,
            bg_vs,
            bg_ps,
            blend,
            buffer,
            srv,
            level_cap: level_cap.max(1),
            view_cb,
            style_cb,
        }
    }

    fn create_tex(device: &ID3D11Device, tex_w: u32, tex_h: u32) -> BookTex {
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
        let blit_vs = make_vs(device, BLIT_HLSL, "blit_vertex");
        let blit_fs = make_ps(device, BLIT_HLSL, "blit_fragment");
        let blit_cb = create_dynamic_cb(device, std::mem::size_of::<BlitParams>() as u32);
        let sampler = create_point_sampler(device);
        BookTex {
            _tex: tex,
            rtv,
            srv,
            tex_w,
            tex_h,
            blit_vs,
            blit_fs,
            blit_cb,
            sampler,
            last_price_to_px: f32::NAN,
            last_view_price0: f32::NAN,
            last_style: BookStyle::default(),
            baked: false,
            dirty: false,
            last_bake_ms: 0.0,
        }
    }
}

fn next_buffer_cap(len: usize, floor: u32) -> u32 {
    (len as u32).max(1).max(floor).next_power_of_two()
}
