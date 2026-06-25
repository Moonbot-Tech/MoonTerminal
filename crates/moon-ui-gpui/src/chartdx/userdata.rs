//! Слой UserData (ордера юзера): МУТИРУЕТ задним числом (юзер двигает ордер → линия едет),
//! поэтому НЕ в combo — отдельный слой, перерисовка по событию. Три геометрии (порт
//! moon-chart order_lines): горизонтали (вход/стоп/liq), отрезки (лестница), маркеры
//! (крест начала/конца, узелки). Геометрию строит `moon_chart::build_order_geometry`
//! (логические time_rel/price), мы конвертим в 16-байт-выровненные GPU-структы и рисуем
//! own-pass тем же chart-трансформом (view = chart_area, линии тянутся в зону стакана).

use gpui::RawGpuAccess;
use moon_chart::layers::{LineInstance, MarkerInstance, SegInstance, ZoneInstance};
use windows::Win32::Graphics::Direct3D::D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST;
use windows::Win32::Graphics::Direct3D11::*;

use super::gpu::{
    ChartViewGpu, create_alpha_blend, create_dynamic_cb, create_srv, create_structured,
    full_viewport, make_ps, make_vs, update_dynamic,
};
use super::types::{HLineGpu, MarkerGpu, SegGpu, ZoneGpu};

const HLSL: &str = include_str!("shaders/order_lines.hlsl");
const INITIAL_ZONE_BUFFER_CAPACITY: u32 = 64;
const INITIAL_HLINE_BUFFER_CAPACITY: u32 = 256;
const INITIAL_SEG_BUFFER_CAPACITY: u32 = 512;
const INITIAL_MARKER_BUFFER_CAPACITY: u32 = 512;

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

struct UdPipe {
    zone_vs: ID3D11VertexShader,
    zone_ps: ID3D11PixelShader,
    hl_vs: ID3D11VertexShader,
    hl_ps: ID3D11PixelShader,
    seg_vs: ID3D11VertexShader,
    seg_ps: ID3D11PixelShader,
    mk_vs: ID3D11VertexShader,
    mk_ps: ID3D11PixelShader,
    blend: ID3D11BlendState,
    view_cb: ID3D11Buffer,
    zone_buf: ID3D11Buffer,
    zone_srv: ID3D11ShaderResourceView,
    zone_cap: u32,
    hl_buf: ID3D11Buffer,
    hl_srv: ID3D11ShaderResourceView,
    hl_cap: u32,
    seg_buf: ID3D11Buffer,
    seg_srv: ID3D11ShaderResourceView,
    seg_cap: u32,
    mk_buf: ID3D11Buffer,
    mk_srv: ID3D11ShaderResourceView,
    mk_cap: u32,
}

#[derive(Default)]
struct Pending {
    zone: Vec<ZoneGpu>,
    hl: Vec<HLineGpu>,
    seg: Vec<SegGpu>,
    mk: Vec<MarkerGpu>,
}

pub struct UserDataLayer {
    pipe: Option<UdPipe>,
    zone_count: u32,
    hl_count: u32,
    seg_count: u32,
    mk_count: u32,
    pending: Option<Pending>,
    device_generation: u64,
}

impl UserDataLayer {
    pub fn new() -> Self {
        Self {
            pipe: None,
            zone_count: 0,
            hl_count: 0,
            seg_count: 0,
            mk_count: 0,
            pending: None,
            device_generation: 0,
        }
    }

    /// Залить геометрию ордеров (целиком). Зовётся по изменению ордеров/вида (мутация).
    pub fn set(
        &mut self,
        zones: &[ZoneInstance],
        hlines: &[LineInstance],
        segs: &[SegInstance],
        markers: &[MarkerInstance],
    ) {
        self.pending = Some(Pending {
            zone: zones.iter().map(zone_of).collect(),
            hl: hlines.iter().map(hl_of).collect(),
            seg: segs.iter().map(seg_of).collect(),
            mk: markers.iter().map(mk_of).collect(),
        });
    }

    /// Prepare phase: creates resources and uploads pending user geometry.
    pub fn prepare(
        &mut self,
        device: &ID3D11Device,
        context: &ID3D11DeviceContext,
        gpu: &RawGpuAccess,
    ) {
        // device-lost: пересоздать pipe; счётчики 0 — буферы пересоздаются пустыми (prepare зальёт
        // ордера заново этим же кадром через set()/pending, инвариант: новый device = 0 валидных).
        let generation = gpu.device_generation();
        if self.device_generation != generation {
            self.pipe = None;
            self.zone_count = 0;
            self.hl_count = 0;
            self.seg_count = 0;
            self.mk_count = 0;
            self.device_generation = generation;
        }
        if self.pipe.is_none() {
            self.pipe = Some(Self::create_pipe(
                device,
                INITIAL_ZONE_BUFFER_CAPACITY,
                INITIAL_HLINE_BUFFER_CAPACITY,
                INITIAL_SEG_BUFFER_CAPACITY,
                INITIAL_MARKER_BUFFER_CAPACITY,
            ));
        }
        if let Some(p) = self.pending.take() {
            let zone_cap = next_buffer_cap(p.zone.len(), INITIAL_ZONE_BUFFER_CAPACITY);
            let hl_cap = next_buffer_cap(p.hl.len(), INITIAL_HLINE_BUFFER_CAPACITY);
            let seg_cap = next_buffer_cap(p.seg.len(), INITIAL_SEG_BUFFER_CAPACITY);
            let mk_cap = next_buffer_cap(p.mk.len(), INITIAL_MARKER_BUFFER_CAPACITY);
            let needs_resize = self.pipe.as_ref().is_none_or(|pipe| {
                pipe.zone_cap < zone_cap
                    || pipe.hl_cap < hl_cap
                    || pipe.seg_cap < seg_cap
                    || pipe.mk_cap < mk_cap
            });
            if needs_resize {
                self.pipe = Some(Self::create_pipe(device, zone_cap, hl_cap, seg_cap, mk_cap));
            }
            let pipe = self.pipe.as_ref().unwrap();
            self.zone_count = upload_all(context, &pipe.zone_buf, &p.zone);
            self.hl_count = upload_all(context, &pipe.hl_buf, &p.hl);
            self.seg_count = upload_all(context, &pipe.seg_buf, &p.seg);
            self.mk_count = upload_all(context, &pipe.mk_buf, &p.mk);
        }
    }

    /// Рисует ордера поверх данных. `view` — тот же chart_area-трансформ, что у combo
    /// (линии тянутся в зону стакана; scissor не ставим — как у движка друга).
    pub fn render(
        &mut self,
        view: &ChartViewGpu,
        context: &ID3D11DeviceContext,
        rtv: &ID3D11RenderTargetView,
        gpu: &RawGpuAccess,
    ) {
        if self.zone_count == 0 && self.hl_count == 0 && self.seg_count == 0 && self.mk_count == 0 {
            return;
        }
        let Some(pipe) = self.pipe.as_ref() else {
            return;
        };
        update_dynamic(context, &pipe.view_cb, &[*view]);
        let vp = full_viewport(gpu);
        unsafe {
            context.OMSetRenderTargets(Some(&[Some(rtv.clone())]), None);
            context.RSSetViewports(Some(&[vp]));
            context.IASetPrimitiveTopology(D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
            context.VSSetConstantBuffers(0, Some(&[Some(pipe.view_cb.clone())]));
            context.OMSetBlendState(&pipe.blend, None, 0xFFFFFFFF);
            // Зоны → горизонтали (вход/стоп/liq) → отрезки (лестница) → маркеры (поверх).
            if self.zone_count > 0 {
                context.VSSetShaderResources(1, Some(&[Some(pipe.zone_srv.clone())]));
                context.VSSetShader(&pipe.zone_vs, None);
                context.PSSetShader(&pipe.zone_ps, None);
                context.DrawInstanced(6, self.zone_count, 0, 0);
            }
            if self.hl_count > 0 {
                context.VSSetShaderResources(1, Some(&[Some(pipe.hl_srv.clone())]));
                context.VSSetShader(&pipe.hl_vs, None);
                context.PSSetShader(&pipe.hl_ps, None);
                context.DrawInstanced(6, self.hl_count, 0, 0);
            }
            if self.seg_count > 0 {
                context.VSSetShaderResources(1, Some(&[Some(pipe.seg_srv.clone())]));
                context.VSSetShader(&pipe.seg_vs, None);
                context.PSSetShader(&pipe.seg_ps, None);
                context.DrawInstanced(6, self.seg_count, 0, 0);
            }
            if self.mk_count > 0 {
                context.VSSetShaderResources(1, Some(&[Some(pipe.mk_srv.clone())]));
                context.VSSetShader(&pipe.mk_vs, None);
                context.PSSetShader(&pipe.mk_ps, None);
                context.DrawInstanced(6, self.mk_count, 0, 0);
            }
        }
    }

    fn create_pipe(
        device: &ID3D11Device,
        zone_cap: u32,
        hl_cap: u32,
        seg_cap: u32,
        mk_cap: u32,
    ) -> UdPipe {
        let zone_cap = zone_cap.max(1);
        let hl_cap = hl_cap.max(1);
        let seg_cap = seg_cap.max(1);
        let mk_cap = mk_cap.max(1);
        let hl_buf = create_structured(device, std::mem::size_of::<HLineGpu>() as u32, hl_cap);
        let zone_buf = create_structured(device, std::mem::size_of::<ZoneGpu>() as u32, zone_cap);
        let seg_buf = create_structured(device, std::mem::size_of::<SegGpu>() as u32, seg_cap);
        let mk_buf = create_structured(device, std::mem::size_of::<MarkerGpu>() as u32, mk_cap);
        UdPipe {
            zone_vs: make_vs(device, HLSL, "zone_vertex"),
            zone_ps: make_ps(device, HLSL, "zone_fragment"),
            hl_vs: make_vs(device, HLSL, "hline_vertex"),
            hl_ps: make_ps(device, HLSL, "hline_fragment"),
            seg_vs: make_vs(device, HLSL, "seg_vertex"),
            seg_ps: make_ps(device, HLSL, "seg_fragment"),
            mk_vs: make_vs(device, HLSL, "marker_vertex"),
            mk_ps: make_ps(device, HLSL, "marker_fragment"),
            blend: create_alpha_blend(device),
            view_cb: create_dynamic_cb(device, std::mem::size_of::<ChartViewGpu>() as u32),
            zone_srv: create_srv(device, &zone_buf),
            zone_cap,
            hl_srv: create_srv(device, &hl_buf),
            hl_cap,
            seg_srv: create_srv(device, &seg_buf),
            seg_cap,
            mk_srv: create_srv(device, &mk_buf),
            mk_cap,
            zone_buf,
            hl_buf,
            seg_buf,
            mk_buf,
        }
    }
}

fn upload_all<T: Copy>(context: &ID3D11DeviceContext, buf: &ID3D11Buffer, data: &[T]) -> u32 {
    if !data.is_empty() {
        update_dynamic(context, buf, data);
    }
    data.len() as u32
}

fn next_buffer_cap(len: usize, floor: u32) -> u32 {
    (len as u32).max(1).max(floor).next_power_of_two()
}
