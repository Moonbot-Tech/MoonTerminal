//! DX11-фундамент own-pass рендера чарта: общие GPU-типы (layout совпадает с HLSL) и
//! helper'ы создания/заливки ресурсов. Всё рисование слоёв (`combo`/`orderbook`/…) идёт
//! через эти примитивы.
//!
//! Шейдеры компилируются из ВКОМПИЛЕННОЙ строки (`include_str!` → `D3DCompile`), а не из
//! файла на диске — бинарь самодостаточен при деплое (нет внешних .hlsl рядом с exe).

use std::ffi::{CString, c_void};

use gpui::RawGpuAccess;
use windows::Win32::Foundation::RECT;
use windows::Win32::Graphics::Direct3D::Fxc::D3DCompile;
use windows::Win32::Graphics::Direct3D::{D3D11_SRV_DIMENSION_BUFFER, ID3DBlob};
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_UNKNOWN;
use windows::core::{Interface, PCSTR};

pub use super::types::{BlitParams, ChartCross, ChartViewGpu};

// ───────────────────────── GPU-типы (layout = HLSL) ─────────────────────────

// ───────────────────────── Компиляция шейдеров ─────────────────────────

/// Компилирует HLSL из строки (вкомпиленной `include_str!`) в байткод. Паникует с
/// сообщением компилятора при ошибке — шейдеры наши, ошибка = баг сборки, не рантайма.
pub fn compile_shader(src: &str, entry: &str, target: &str) -> ID3DBlob {
    let entry_c = CString::new(entry).unwrap();
    let target_c = CString::new(target).unwrap();
    let mut blob: Option<ID3DBlob> = None;
    let mut err: Option<ID3DBlob> = None;
    let r = unsafe {
        D3DCompile(
            src.as_ptr() as *const c_void,
            src.len(),
            PCSTR::null(),
            None,
            None,
            PCSTR(entry_c.as_ptr() as *const u8),
            PCSTR(target_c.as_ptr() as *const u8),
            0,
            0,
            &mut blob,
            Some(&mut err),
        )
    };
    if r.is_err() {
        let msg = err
            .as_ref()
            .map(|e| unsafe {
                std::ffi::CStr::from_ptr(e.GetBufferPointer() as *const i8)
                    .to_string_lossy()
                    .into_owned()
            })
            .unwrap_or_else(|| format!("{r:?}"));
        panic!("[chartdx] compile {entry} failed: {msg}");
    }
    blob.unwrap()
}

pub fn blob_bytes(blob: &ID3DBlob) -> &[u8] {
    unsafe {
        std::slice::from_raw_parts(blob.GetBufferPointer() as *const u8, blob.GetBufferSize())
    }
}

pub fn make_vs(device: &ID3D11Device, src: &str, entry: &str) -> ID3D11VertexShader {
    let blob = compile_shader(src, entry, "vs_4_1");
    unsafe {
        let mut s = None;
        device
            .CreateVertexShader(blob_bytes(&blob), None, Some(&mut s))
            .unwrap();
        s.unwrap()
    }
}

pub fn make_ps(device: &ID3D11Device, src: &str, entry: &str) -> ID3D11PixelShader {
    let blob = compile_shader(src, entry, "ps_4_1");
    unsafe {
        let mut s = None;
        device
            .CreatePixelShader(blob_bytes(&blob), None, Some(&mut s))
            .unwrap();
        s.unwrap()
    }
}

// ───────────────────────── Буферы / ресурсы ─────────────────────────

/// StructuredBuffer (DYNAMIC, CPU write) на `count` элементов по `elem_size` байт.
pub fn create_structured(device: &ID3D11Device, elem_size: u32, count: u32) -> ID3D11Buffer {
    let desc = D3D11_BUFFER_DESC {
        ByteWidth: elem_size * count,
        Usage: D3D11_USAGE_DYNAMIC,
        BindFlags: D3D11_BIND_SHADER_RESOURCE.0 as u32,
        CPUAccessFlags: D3D11_CPU_ACCESS_WRITE.0 as u32,
        MiscFlags: D3D11_RESOURCE_MISC_BUFFER_STRUCTURED.0 as u32,
        StructureByteStride: elem_size,
    };
    unsafe {
        let mut b = None;
        device.CreateBuffer(&desc, None, Some(&mut b)).unwrap();
        b.unwrap()
    }
}

pub fn create_srv(device: &ID3D11Device, buffer: &ID3D11Buffer) -> ID3D11ShaderResourceView {
    unsafe {
        let mut v = None;
        device
            .CreateShaderResourceView(buffer, None, Some(&mut v))
            .unwrap();
        v.unwrap()
    }
}

/// Ranged SRV над структурным буфером [first, first+count) — для инкрементального bake combo.
pub fn create_srv_range(
    device: &ID3D11Device,
    buffer: &ID3D11Buffer,
    first: u32,
    count: u32,
) -> ID3D11ShaderResourceView {
    let desc = D3D11_SHADER_RESOURCE_VIEW_DESC {
        Format: DXGI_FORMAT_UNKNOWN,
        ViewDimension: D3D11_SRV_DIMENSION_BUFFER,
        Anonymous: D3D11_SHADER_RESOURCE_VIEW_DESC_0 {
            Buffer: D3D11_BUFFER_SRV {
                Anonymous1: D3D11_BUFFER_SRV_0 {
                    FirstElement: first,
                },
                Anonymous2: D3D11_BUFFER_SRV_1 { NumElements: count },
            },
        },
    };
    unsafe {
        let mut v = None;
        device
            .CreateShaderResourceView(buffer, Some(&desc), Some(&mut v))
            .unwrap();
        v.unwrap()
    }
}

pub fn create_dynamic_cb(device: &ID3D11Device, size: u32) -> ID3D11Buffer {
    let desc = D3D11_BUFFER_DESC {
        ByteWidth: size,
        Usage: D3D11_USAGE_DYNAMIC,
        BindFlags: D3D11_BIND_CONSTANT_BUFFER.0 as u32,
        CPUAccessFlags: D3D11_CPU_ACCESS_WRITE.0 as u32,
        ..Default::default()
    };
    unsafe {
        let mut b = None;
        device.CreateBuffer(&desc, None, Some(&mut b)).unwrap();
        b.unwrap()
    }
}

/// Стандартный alpha-blend (src.a, 1-src.a) — общий для крестов/линий/баров.
pub fn create_alpha_blend(device: &ID3D11Device) -> ID3D11BlendState {
    let mut desc = D3D11_BLEND_DESC::default();
    desc.RenderTarget[0].BlendEnable = true.into();
    desc.RenderTarget[0].BlendOp = D3D11_BLEND_OP_ADD;
    desc.RenderTarget[0].BlendOpAlpha = D3D11_BLEND_OP_ADD;
    desc.RenderTarget[0].SrcBlend = D3D11_BLEND_SRC_ALPHA;
    desc.RenderTarget[0].SrcBlendAlpha = D3D11_BLEND_ONE;
    desc.RenderTarget[0].DestBlend = D3D11_BLEND_INV_SRC_ALPHA;
    desc.RenderTarget[0].DestBlendAlpha = D3D11_BLEND_ONE;
    desc.RenderTarget[0].RenderTargetWriteMask = D3D11_COLOR_WRITE_ENABLE_ALL.0 as u8;
    unsafe {
        let mut s = None;
        device.CreateBlendState(&desc, Some(&mut s)).unwrap();
        s.unwrap()
    }
}

/// Point-семпл clamp-сэмплер (combo-блит 1:1).
pub fn create_point_sampler(device: &ID3D11Device) -> ID3D11SamplerState {
    let d = D3D11_SAMPLER_DESC {
        Filter: D3D11_FILTER_MIN_MAG_MIP_POINT,
        AddressU: D3D11_TEXTURE_ADDRESS_CLAMP,
        AddressV: D3D11_TEXTURE_ADDRESS_CLAMP,
        AddressW: D3D11_TEXTURE_ADDRESS_CLAMP,
        ComparisonFunc: D3D11_COMPARISON_NEVER,
        MaxLOD: f32::MAX,
        ..Default::default()
    };
    unsafe {
        let mut o = None;
        device.CreateSamplerState(&d, Some(&mut o)).unwrap();
        o.unwrap()
    }
}

// ───────────────────────── Заливка ─────────────────────────

/// MAP_WRITE_DISCARD: переписать буфер целиком из [0..data.len()).
pub fn update_dynamic<T: Copy>(context: &ID3D11DeviceContext, buffer: &ID3D11Buffer, data: &[T]) {
    unsafe {
        let mut m = D3D11_MAPPED_SUBRESOURCE::default();
        if context
            .Map(buffer, 0, D3D11_MAP_WRITE_DISCARD, 0, Some(&mut m))
            .is_ok()
        {
            std::ptr::copy_nonoverlapping(data.as_ptr(), m.pData as *mut T, data.len());
            context.Unmap(buffer, 0);
        }
    }
}

/// MAP_WRITE_NO_OVERWRITE по кольцу с заворотом (живой край, дешёвый append без сброса GPU).
pub fn ring_write_no_overwrite<T: Copy>(
    context: &ID3D11DeviceContext,
    buffer: &ID3D11Buffer,
    head: u32,
    cap: u32,
    data: &[T],
) {
    let n = data.len() as u32;
    unsafe {
        let mut m = D3D11_MAPPED_SUBRESOURCE::default();
        if context
            .Map(buffer, 0, D3D11_MAP_WRITE_NO_OVERWRITE, 0, Some(&mut m))
            .is_err()
        {
            return;
        }
        let dst = m.pData as *mut T;
        if head + n <= cap {
            std::ptr::copy_nonoverlapping(data.as_ptr(), dst.add(head as usize), n as usize);
        } else {
            let first = (cap - head) as usize;
            std::ptr::copy_nonoverlapping(data.as_ptr(), dst.add(head as usize), first);
            std::ptr::copy_nonoverlapping(data.as_ptr().add(first), dst, n as usize - first);
        }
        context.Unmap(buffer, 0);
    }
}

// ───────────────────────── Мелочи ─────────────────────────

/// Полный backbuffer-viewport (px из хука). Слои ставят его перед draw в backbuffer.
pub fn full_viewport(gpu: &RawGpuAccess) -> D3D11_VIEWPORT {
    D3D11_VIEWPORT {
        TopLeftX: 0.0,
        TopLeftY: 0.0,
        Width: gpu.width() as f32,
        Height: gpu.height() as f32,
        MinDepth: 0.0,
        MaxDepth: 1.0,
    }
}

/// borrowed-каст raw-указателей хука (`RawGpuAccess`) в наши windows-rs COM-типы.
/// Без AddRef — валидны только на время колбэка. Возвращает None, если хук пуст
/// (не-D3D11 backend / device-lost кадр).
pub fn borrow_d3d(
    gpu: &RawGpuAccess,
) -> Option<(ID3D11Device, ID3D11DeviceContext, ID3D11RenderTargetView)> {
    let RawGpuAccess::D3d11(gpu) = gpu else {
        return None;
    };
    unsafe {
        let device = ID3D11Device::from_raw_borrowed(&gpu.device.as_ptr())?.clone();
        let context = ID3D11DeviceContext::from_raw_borrowed(&gpu.context.as_ptr())?.clone();
        let rtv = ID3D11RenderTargetView::from_raw_borrowed(&gpu.render_target.as_ptr())?.clone();
        Some((device, context, rtv))
    }
}

// ───────────────────────── Scissor (обрезка слоёв к зоне) ─────────────────────────

/// Растеризатор с включённым scissor (копия дефолта GPUI + ScissorEnable). GPUI рисует
/// сцену с ScissorEnable=false → own-pass обязан поставить свой стейт, чтобы бары стакана/
/// линии ордеров (позиционируются по ЦЕНЕ, могут попасть за пределы плота) не лезли на
/// тулбар/шкалы. После прохода вернуть стейт GPUI (см. `mod.rs` callback).
pub fn create_scissor_rasterizer(device: &ID3D11Device) -> ID3D11RasterizerState {
    let desc = D3D11_RASTERIZER_DESC {
        FillMode: D3D11_FILL_SOLID,
        CullMode: D3D11_CULL_NONE,
        FrontCounterClockwise: false.into(),
        DepthBias: 0,
        DepthBiasClamp: 0.0,
        SlopeScaledDepthBias: 0.0,
        DepthClipEnable: true.into(),
        ScissorEnable: true.into(),
        MultisampleEnable: true.into(),
        AntialiasedLineEnable: false.into(),
    };
    unsafe {
        let mut s = None;
        device.CreateRasterizerState(&desc, Some(&mut s)).unwrap();
        s.unwrap()
    }
}

/// Поставить scissor-прямоугольник (px окна) + scissor-растеризатор. Прямоугольник =
/// зона рисования слоя (плот+стакан панели); всё вне него растеризатор отбросит.
pub fn set_scissor(
    context: &ID3D11DeviceContext,
    rs: &ID3D11RasterizerState,
    l: f32,
    t: f32,
    r: f32,
    b: f32,
) {
    unsafe {
        context.RSSetState(Some(rs));
    }
    set_scissor_rect(context, l, t, r, b);
}

/// Обновить только прямоугольник scissor, не трогая rasterizer state.
pub fn set_scissor_rect(context: &ID3D11DeviceContext, l: f32, t: f32, r: f32, b: f32) {
    let rect = RECT {
        left: l.floor() as i32,
        top: t.floor() as i32,
        right: r.ceil() as i32,
        bottom: b.ceil() as i32,
    };
    unsafe {
        context.RSSetScissorRects(Some(&[rect]));
    }
}
