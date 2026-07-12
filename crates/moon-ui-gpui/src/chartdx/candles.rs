//! Слой свечей own-pass (D3D11): инстансный дроу в base-проход МЕЖДУ grid и combo —
//! свечи лежат под крестами трейдов. Base перерисовывается только на data-change /
//! сдвиге камеры (existing cadence), т.е. слой не добавляет работы в present-путь.
//! Буфер (сотни свечей) перезаливается целиком по смене ревизии серии (дёшево);
//! вид (режим/зона/контур/цвета) — константами CandleStyle, без пересборки вершин.

use gpui::RawGpuAccess;
use windows::Win32::Graphics::Direct3D::D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST;
use windows::Win32::Graphics::Direct3D11::*;

use super::gpu::{
    create_alpha_blend, create_dynamic_cb, create_srv, create_structured, device_changed,
    full_viewport, set_scissor_rect, update_dynamic,
};
use super::types::{CandleGpu, CandleStyleGpu, ChartViewGpu};

/// Ёмкость GPU-буфера свечей (инстансов). Видимое окно + префетч — сотни; 4096 с запасом
/// (4096 × 24 B = 96 KB VRAM). Переполнение — оставляем последний хвост.
const CANDLE_CAPACITY: u32 = 4096;
const CANDLES_HLSL: &str = include_str!("shaders/candles.hlsl");

struct CandlePipe {
    vs: ID3D11VertexShader,
    ps: ID3D11PixelShader,
    blend: ID3D11BlendState,
    buffer: ID3D11Buffer,
    srv: ID3D11ShaderResourceView,
    view_cb: ID3D11Buffer,
    style_cb: ID3D11Buffer,
}

pub struct CandleLayer {
    pipe: Option<CandlePipe>,
    pending: Option<Vec<CandleGpu>>,
    count: u32,
    style: CandleStyleGpu,
    style_dirty: bool,
    device_generation_seen: u64,
}

impl CandleLayer {
    pub fn new() -> Self {
        Self {
            pipe: None,
            pending: None,
            count: 0,
            style: CandleStyleGpu::default(),
            style_dirty: true,
            device_generation_seen: 0,
        }
    }

    /// Полная замена набора свечей (ревизия серии изменилась).
    pub fn set(&mut self, data: Vec<CandleGpu>) {
        self.pending = Some(data);
    }

    /// Стиль слоя (режим/зона/цвета/контур). Идемпотентен.
    pub fn set_style(&mut self, style: CandleStyleGpu) {
        if self.style != style {
            self.style = style;
            self.style_dirty = true;
        }
    }

    /// Prepare phase: заливка pending-буфера и констант стиля. Из `prepare_gpu`.
    pub fn prepare(
        &mut self,
        device: &ID3D11Device,
        context: &ID3D11DeviceContext,
        gpu: &RawGpuAccess,
    ) {
        if device_changed(&mut self.device_generation_seen, gpu) {
            // device-lost: ресурсы невалидны. Данные держит оркестратор (data_state) —
            // он же перезальёт по ревизии; здесь достаточно пересоздать pipe и обнулиться.
            self.pipe = None;
            self.count = 0;
            self.style_dirty = true;
        }
        if self.pipe.is_none() {
            self.pipe = Some(Self::create_pipe(device));
        }
        let pipe = self.pipe.as_ref().unwrap();
        if let Some(data) = self.pending.take() {
            let cap = CANDLE_CAPACITY as usize;
            let data: &[CandleGpu] = if data.len() > cap {
                &data[data.len() - cap..]
            } else {
                &data
            };
            if !data.is_empty() {
                update_dynamic(context, &pipe.buffer, data);
            }
            self.count = data.len() as u32;
        }
        if self.style_dirty {
            update_dynamic(context, &pipe.style_cb, &[self.style]);
            self.style_dirty = false;
        }
    }

    /// Рисует свечи в base-проход (между grid и combo). `prepare()` уже залил данные.
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
        let Some(pipe) = self.pipe.as_ref() else {
            return;
        };
        update_dynamic(context, &pipe.view_cb, std::slice::from_ref(view));
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
            context.VSSetConstantBuffers(
                0,
                Some(&[Some(pipe.view_cb.clone()), Some(pipe.style_cb.clone())]),
            );
            context.PSSetConstantBuffers(
                0,
                Some(&[Some(pipe.view_cb.clone()), Some(pipe.style_cb.clone())]),
            );
            context.VSSetShaderResources(3, Some(&[Some(pipe.srv.clone())]));
            context.OMSetBlendState(&pipe.blend, None, 0xFFFFFFFF);
            context.VSSetShader(&pipe.vs, None);
            context.PSSetShader(&pipe.ps, None);
            // 18 вершин на свечу: тело + верхний/нижний фитили (3 quad'а).
            context.DrawInstanced(18, self.count, 0, 0);
        }
    }

    fn create_pipe(device: &ID3D11Device) -> CandlePipe {
        let vs = super::gpu::make_vs(device, CANDLES_HLSL, "candles_vertex");
        let ps = super::gpu::make_ps(device, CANDLES_HLSL, "candles_fragment");
        let blend = create_alpha_blend(device);
        let buffer = create_structured(
            device,
            std::mem::size_of::<CandleGpu>() as u32,
            CANDLE_CAPACITY,
        );
        let srv = create_srv(device, &buffer);
        let view_cb = create_dynamic_cb(device, std::mem::size_of::<ChartViewGpu>() as u32);
        let style_cb = create_dynamic_cb(device, std::mem::size_of::<CandleStyleGpu>() as u32);
        CandlePipe {
            vs,
            ps,
            blend,
            buffer,
            srv,
            view_cb,
            style_cb,
        }
    }
}
