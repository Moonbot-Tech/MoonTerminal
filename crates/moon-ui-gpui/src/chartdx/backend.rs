//! Platform GPU layer bundle. The chart orchestrator owns one `PlatformLayers`
//! per pane and feeds it backend-neutral data; this module maps that data to
//! the current native GPUI backend.

use moon_chart::layers::{LineInstance, MarkerInstance, SegInstance, ZoneInstance};
use moon_core::data::{LevelInstance, PriceLinePoint};

use super::types::{
    BackgroundParams, BookStyle, ChartCross, ChartViewGpu, CursorParams, GridParams, ReadoutRect,
};

#[cfg(target_os = "macos")]
use super::metal_backend::MetalLayers;
#[cfg(target_os = "linux")]
use super::wgpu_backend::WgpuLayers;

#[cfg(windows)]
use super::{
    background::{BACKGROUND_3DLOGO_PNG, BackgroundLayer},
    combo::ComboLayer,
    cursor::CursorLayer,
    grid::GridLayer,
    orderbook::OrderBookLayer,
    readout::ReadoutLayer,
    userdata::UserDataLayer,
};

#[cfg(windows)]
use windows::Win32::Graphics::Direct3D11::{
    ID3D11Device, ID3D11DeviceContext, ID3D11RenderTargetView,
};

pub struct PlatformLayers {
    #[cfg(windows)]
    background: BackgroundLayer,
    #[cfg(windows)]
    combo: ComboLayer,
    #[cfg(windows)]
    grid: GridLayer,
    #[cfg(windows)]
    cursor: CursorLayer,
    #[cfg(windows)]
    readout: ReadoutLayer,
    #[cfg(windows)]
    orderbook: OrderBookLayer,
    #[cfg(windows)]
    userdata: UserDataLayer,
    #[cfg(target_os = "linux")]
    wgpu: WgpuLayers,
    #[cfg(target_os = "macos")]
    metal: MetalLayers,
}

impl PlatformLayers {
    pub fn new() -> Self {
        Self {
            #[cfg(windows)]
            background: BackgroundLayer::new(BACKGROUND_3DLOGO_PNG),
            #[cfg(windows)]
            combo: ComboLayer::new(),
            #[cfg(windows)]
            grid: GridLayer::new(),
            #[cfg(windows)]
            cursor: CursorLayer::new(),
            #[cfg(windows)]
            readout: ReadoutLayer::new(),
            #[cfg(windows)]
            orderbook: OrderBookLayer::new(),
            #[cfg(windows)]
            userdata: UserDataLayer::new(),
            #[cfg(target_os = "linux")]
            wgpu: WgpuLayers::new(),
            #[cfg(target_os = "macos")]
            metal: MetalLayers::new(),
        }
    }

    pub fn device_gen(&self) -> u64 {
        #[cfg(windows)]
        {
            return self.combo.device_gen();
        }
        #[allow(unreachable_code)]
        0
    }

    pub fn set_combo_capacity(&mut self, cross_capacity: usize, price_line_capacity: usize) {
        #[cfg(windows)]
        self.combo.set_capacity(cross_capacity, price_line_capacity);
        #[cfg(target_os = "linux")]
        self.wgpu
            .set_combo_capacity(cross_capacity, price_line_capacity);
        #[cfg(target_os = "macos")]
        self.metal
            .set_combo_capacity(cross_capacity, price_line_capacity);
        #[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
        {
            let _ = (cross_capacity, price_line_capacity);
        }
    }

    pub fn reset_combo(&mut self, data: Vec<ChartCross>) {
        #[cfg(windows)]
        self.combo.reset(data);
        #[cfg(target_os = "linux")]
        self.wgpu.reset_combo(data);
        #[cfg(target_os = "macos")]
        self.metal.reset_combo(data);
        #[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
        {
            let _ = data;
        }
    }

    pub fn append_combo(&mut self, data: &[ChartCross]) {
        #[cfg(windows)]
        self.combo.append(data);
        #[cfg(target_os = "linux")]
        self.wgpu.append_combo(data);
        #[cfg(target_os = "macos")]
        self.metal.append_combo(data);
        #[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
        {
            let _ = data;
        }
    }

    pub fn set_price_lines(&mut self, last: &[PriceLinePoint], mark: &[PriceLinePoint]) {
        #[cfg(windows)]
        self.combo.set_price_lines(last, mark);
        #[cfg(target_os = "linux")]
        self.wgpu.set_price_lines(last, mark);
        #[cfg(target_os = "macos")]
        self.metal.set_price_lines(last, mark);
        #[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
        {
            let _ = (last, mark);
        }
    }

    pub fn set_orderbook(&mut self, levels: Vec<LevelInstance>) {
        #[cfg(windows)]
        self.orderbook.set(levels);
        #[cfg(target_os = "linux")]
        self.wgpu.set_orderbook(levels);
        #[cfg(target_os = "macos")]
        self.metal.set_orderbook(levels);
        #[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
        {
            let _ = levels;
        }
    }

    pub fn set_userdata(
        &mut self,
        zones: &[ZoneInstance],
        hlines: &[LineInstance],
        segs: &[SegInstance],
        markers: &[MarkerInstance],
    ) {
        #[cfg(windows)]
        self.userdata.set(zones, hlines, segs, markers);
        #[cfg(target_os = "linux")]
        self.wgpu.set_userdata(zones, hlines, segs, markers);
        #[cfg(target_os = "macos")]
        self.metal.set_userdata(zones, hlines, segs, markers);
        #[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
        {
            let _ = (zones, hlines, segs, markers);
        }
    }

    #[cfg(windows)]
    pub fn prepare_d3d(
        &mut self,
        view: &ChartViewGpu,
        orderbook_view: &ChartViewGpu,
        book_style: &BookStyle,
        device: &ID3D11Device,
        context: &ID3D11DeviceContext,
        gpu: &gpui::RawGpuAccess,
    ) {
        self.combo.prepare(view, device, context, gpu);
        self.orderbook
            .prepare(orderbook_view, book_style, device, context, gpu);
        self.userdata.prepare(device, context, gpu);
    }

    #[cfg(target_os = "linux")]
    pub fn prepare_wgpu(
        &mut self,
        view: &ChartViewGpu,
        background_params: &BackgroundParams,
        grid_params: &GridParams,
        cursor_params: &CursorParams,
        orderbook_view: &ChartViewGpu,
        book_style: &BookStyle,
        gpu: &gpui::RawGpuAccess,
        rebuild_base: bool,
    ) -> anyhow::Result<()> {
        self.wgpu.prepare(
            view,
            background_params,
            grid_params,
            cursor_params,
            orderbook_view,
            book_style,
            gpu,
            rebuild_base,
        )
    }

    #[cfg(target_os = "macos")]
    pub fn prepare_metal(
        &mut self,
        view: &ChartViewGpu,
        background_params: &BackgroundParams,
        grid_params: &GridParams,
        cursor_params: &CursorParams,
        orderbook_view: &ChartViewGpu,
        book_style: &BookStyle,
        gpu: &gpui::RawGpuAccess,
        rebuild_base: bool,
    ) -> anyhow::Result<()> {
        self.metal.prepare(
            view,
            background_params,
            grid_params,
            cursor_params,
            orderbook_view,
            book_style,
            gpu,
            rebuild_base,
        )
    }

    #[cfg(target_os = "linux")]
    pub fn needs_base_cache(&self, gpu: &gpui::RawGpuAccess) -> bool {
        self.wgpu.needs_base_cache(gpu)
    }

    #[cfg(target_os = "macos")]
    pub fn needs_base_cache(&self, gpu: &gpui::RawGpuAccess) -> bool {
        self.metal.needs_base_cache(gpu)
    }

    #[cfg(windows)]
    pub fn render_base_d3d(
        &mut self,
        view: &ChartViewGpu,
        background_params: &BackgroundParams,
        grid_params: &GridParams,
        orderbook_view: &ChartViewGpu,
        _book_style: &BookStyle,
        device: &ID3D11Device,
        context: &ID3D11DeviceContext,
        rtv: &ID3D11RenderTargetView,
        gpu: &gpui::RawGpuAccess,
        panel_clip: [f32; 4],
    ) {
        // Послойные DRAW-счётчики (чокпоинт, мандат AGENTS.md): раз на present на каждый слой.
        crate::diag::bump(&crate::diag::CHART_BG_DRAW);
        self.background
            .render(background_params, device, context, rtv, gpu);
        self.userdata.render_zones(view, context, rtv, gpu);
        crate::diag::bump(&crate::diag::CHART_GRID_DRAW);
        self.grid.render(grid_params, device, context, rtv, gpu);
        crate::diag::bump(&crate::diag::CHART_COMBO_DRAW);
        self.combo.render(view, context, rtv, gpu, panel_clip);
        crate::diag::bump(&crate::diag::CHART_BOOK_DRAW);
        self.orderbook
            .render(orderbook_view, context, rtv, gpu, panel_clip);
        if self.combo.has_data() {
            super::gpu::debug_dump_rtv_once(device, context, rtv);
        }
    }

    #[cfg(windows)]
    pub fn render_userdata_lines_d3d(
        &mut self,
        view: &ChartViewGpu,
        context: &ID3D11DeviceContext,
        rtv: &ID3D11RenderTargetView,
        gpu: &gpui::RawGpuAccess,
    ) {
        crate::diag::bump(&crate::diag::CHART_USER_DRAW);
        self.userdata.render_lines(view, context, rtv, gpu);
    }

    #[cfg(windows)]
    pub fn render_cursor_d3d(
        &mut self,
        cursor_params: &CursorParams,
        readout_rects: &[ReadoutRect],
        device: &ID3D11Device,
        context: &ID3D11DeviceContext,
        rtv: &ID3D11RenderTargetView,
        gpu: &gpui::RawGpuAccess,
    ) {
        if cursor_params.enabled > 0.0 {
            crate::diag::bump(&crate::diag::CHART_CURSOR_DRAW);
        }
        self.cursor.render(cursor_params, device, context, rtv, gpu);
        self.readout
            .render(readout_rects, device, context, rtv, gpu);
    }

    #[cfg(target_os = "linux")]
    pub fn render_wgpu(
        &mut self,
        view: &ChartViewGpu,
        pane_bounds: [f32; 4],
        background_params: &BackgroundParams,
        grid_params: &GridParams,
        cursor_params: &CursorParams,
        readout_rects: &[ReadoutRect],
        orderbook_view: &ChartViewGpu,
        gpu: &gpui::RawGpuAccess,
    ) -> anyhow::Result<()> {
        self.wgpu.render(
            view,
            pane_bounds,
            background_params,
            grid_params,
            cursor_params,
            readout_rects,
            orderbook_view,
            gpu,
        )
    }

    #[cfg(target_os = "macos")]
    pub fn render_metal(
        &mut self,
        view: &ChartViewGpu,
        pane_bounds: [f32; 4],
        background_params: &BackgroundParams,
        grid_params: &GridParams,
        cursor_params: &CursorParams,
        readout_rects: &[ReadoutRect],
        orderbook_view: &ChartViewGpu,
        gpu: &gpui::RawGpuAccess,
    ) -> anyhow::Result<()> {
        self.metal.render(
            view,
            pane_bounds,
            background_params,
            grid_params,
            cursor_params,
            readout_rects,
            orderbook_view,
            gpu,
        )
    }
}
