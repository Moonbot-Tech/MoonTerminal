//! Рендер-состояние чарта (`impl RenderState`): сведение per-pane GPU-состояния,
//! тайминг present (пейсер ~60Гц), курсор/ридаут, отрисовка слоёв own-pass.
//! Вынесено из `mod.rs`; структура `RenderState` объявлена там.

use super::*;

const READOUT_FALLBACK_FONT_W: f32 = 8.5;
const READOUT_PAD_X: f32 = 5.0;
const READOUT_PAD_Y: f32 = 2.5;
const READOUT_INSET: f32 = 2.0;

#[cfg(windows)]
fn bounds_clip(bounds: [f32; 4], res: [f32; 2]) -> [f32; 4] {
    // ВАЖНО: clamp(min, max) паникует при min > max. При вырожденных границах панели
    // (нулевая ширина / панель упёрта в правый/нижний край) `l` может равняться res,
    // тогда `l + 1.0 > res` и clamp(min>max) убивал бы кадр (баг: f32 clamp min=1681
    // max=1680 при ресайзе/реконнекте). Поэтому верхнюю границу берём как max(res, l+1).
    let l = bounds[0].floor().clamp(0.0, res[0].max(1.0));
    let t = bounds[1].floor().clamp(0.0, res[1].max(1.0));
    let r = (bounds[0] + bounds[2])
        .ceil()
        .clamp(l + 1.0, res[0].max(l + 1.0));
    let b = (bounds[1] + bounds[3])
        .ceil()
        .clamp(t + 1.0, res[1].max(t + 1.0));
    [l, t, r, b]
}

fn readout_text_width(label: &str, measured: f32) -> f32 {
    measured.max(label.chars().count() as f32 * READOUT_FALLBACK_FONT_W)
}

fn readout_rect_dst(
    anchor_x: f32,
    anchor_y: f32,
    text_w: f32,
    line_h: f32,
    ax: f32,
    ay: f32,
    scale: f32,
) -> [f32; 4] {
    let x = anchor_x - text_w * ax - READOUT_PAD_X;
    let y = anchor_y - line_h * ay - READOUT_PAD_Y;
    [
        x * scale,
        y * scale,
        (text_w + READOUT_PAD_X * 2.0) * scale,
        (line_h + READOUT_PAD_Y * 2.0) * scale,
    ]
}

fn clamp_anchor(value: f32, min: f32, max: f32) -> f32 {
    if min <= max {
        value.clamp(min, max)
    } else {
        (min + max) * 0.5
    }
}

fn sync_readout_resolution(rects: &mut [ReadoutRect], res: [f32; 2]) {
    let w = res[0].max(1.0);
    let h = res[1].max(1.0);
    for rect in rects {
        rect.m[1] = w;
        rect.m[2] = h;
    }
}

impl RenderState {
    pub(super) fn set_target_present_rate_hz(&mut self, hz: f32) {
        let hz = hz.clamp(1.0, 240.0);
        self.target_present_interval_ms = 1000.0 / hz as f64;
    }

    pub(super) fn record_camera_shift(&mut self, now_ms: f64) {
        if self.camera_shift_window_start_ms <= 0.0 {
            self.camera_shift_window_start_ms = now_ms;
        }
        self.camera_shift_count = self.camera_shift_count.saturating_add(1);
        self.update_camera_shift_hz(now_ms);
    }

    pub(super) fn camera_shift_hz(&mut self, now_ms: f64) -> f32 {
        self.update_camera_shift_hz(now_ms);
        self.camera_shift_hz
    }

    pub(super) fn update_camera_shift_hz(&mut self, now_ms: f64) {
        if self.camera_shift_window_start_ms <= 0.0 {
            self.camera_shift_window_start_ms = now_ms;
            return;
        }
        let elapsed = now_ms - self.camera_shift_window_start_ms;
        if elapsed < 1000.0 {
            return;
        }
        self.camera_shift_hz = self.camera_shift_count as f32 * 1000.0 / elapsed.max(1.0) as f32;
        self.camera_shift_count = 0;
        self.camera_shift_window_start_ms = now_ms;
    }

    pub(super) fn set_slot_origin(&mut self, x: f32, y: f32) {
        let next = [x, y];
        if self.slot_origin != next {
            self.slot_origin = next;
            self.base_dirty = true;
            self.needs_present = true;
            self.sync_cursor_params();
            if self.cursor.is_some() {
                self.needs_present = true;
            }
        }
    }

    pub(super) fn set_cursor_style(&mut self, color: [f32; 4], thickness: f32) {
        let thickness = thickness.max(1.0);
        if self.cursor_color != color || self.cursor_thickness != thickness {
            self.cursor_color = color;
            self.cursor_thickness = thickness;
            self.sync_cursor_params();
            if self.cursor.is_some() {
                self.needs_present = true;
            }
        }
    }

    pub(super) fn set_readout_style(
        &mut self,
        bg: [f32; 4],
        soft_bg: [f32; 4],
        order_bg: [f32; 4],
        border: [f32; 4],
        border_px: f32,
    ) {
        let border_px = border_px.max(0.0);
        if self.readout_bg != bg
            || self.readout_soft_bg != soft_bg
            || self.readout_order_bg != order_bg
            || self.readout_border != border
            || (self.readout_border_px - border_px).abs() > 0.001
        {
            self.readout_bg = bg;
            self.readout_soft_bg = soft_bg;
            self.readout_order_bg = order_bg;
            self.readout_border = border;
            self.readout_border_px = border_px;
            self.sync_readout_params();
            self.needs_present = true;
        }
    }

    pub(super) fn set_pixel_scale(&mut self, scale: f32) {
        let scale = scale.max(0.1);
        if (self.pixel_scale - scale).abs() > 0.001 {
            self.pixel_scale = scale;
            self.sync_cursor_params();
            if self.cursor.is_some() {
                self.needs_present = true;
            }
        }
    }

    pub(super) fn set_cursor(&mut self, cursor: Option<CursorState>) -> bool {
        if self.cursor == cursor {
            return false;
        }
        self.cursor = cursor;
        self.sync_cursor_params();
        self.needs_present = true;
        true
    }

    pub(super) fn set_firetest_force_present(&mut self, enabled: bool) -> bool {
        if self.firetest_force_present == enabled {
            return false;
        }
        self.firetest_force_present = enabled;
        if enabled {
            self.needs_present = true;
        }
        true
    }

    pub(super) fn sync_cursor_params(&mut self) {
        for (idx, pr) in self.panes.iter_mut().enumerate() {
            let right = (pr.orderbook_view.bounds[0] + pr.orderbook_view.bounds[2])
                .max(pr.view.bounds[0] + pr.view.bounds[2]);
            let bounds = [
                pr.view.bounds[0],
                pr.view.bounds[1],
                (right - pr.view.bounds[0]).max(1.0),
                pr.view.bounds[3].max(1.0),
            ];
            let mut params = CursorParams {
                bounds,
                resolution: pr.view.resolution,
                color: self.cursor_color,
                thickness: self.cursor_thickness.max(1.0),
                ..CursorParams::default()
            };
            if pr.active {
                if let Some(cursor) = self.cursor.filter(|c| c.pane == idx) {
                    params.cursor = [
                        self.slot_origin[0] + cursor.local[0],
                        self.slot_origin[1] + cursor.local[1],
                    ];
                    params.enabled = 1.0;
                }
            }
            #[cfg(not(windows))]
            let changed = pr.cursor_params != params;
            pr.cursor_params = params;
            #[cfg(not(windows))]
            if changed {
                // Cursor uniforms/readout rects are uploaded from the draw callback on
                // Metal/wgpu. Treating cursor motion as prepare-dirty turns mouse-only
                // frames into full chart prepares and defeats the retained cursor path.
                self.needs_present = true;
            }
        }
        self.sync_readout_params();
    }

    pub(super) fn sync_readout_params(&mut self) {
        let sf = self.pixel_scale.max(0.1);
        let bg = self.readout_bg;
        let border = self.readout_border;
        let border_px = self.readout_border_px;
        let m = [border_px, 1.0, 1.0, 0.0];
        let tz_offset_sec = crate::axes::local_offset_sec();
        let cursor = self.cursor;
        let slot_origin = self.slot_origin;

        for (idx, pr) in self.panes.iter_mut().enumerate() {
            pr.readout_rects.clear();
            if !pr.active {
                continue;
            }

            let pane_left = pr.pane_bounds[0] / sf;
            let pane_right = (pr.pane_bounds[0] + pr.pane_bounds[2]) / sf;
            let pane_bottom = (pr.pane_bounds[1] + pr.pane_bounds[3]) / sf;
            let plot_left = pr.view.bounds[0] / sf;
            let plot_top = pr.view.bounds[1] / sf;
            let plot_w = pr.view.bounds[2] / sf;
            let plot_h = pr.view.bounds[3] / sf;
            let plot_right = plot_left + plot_w;
            // Сторона оси цен: Hide → плашку курсора-цены не рисуем (нет оси/жёлоба); Right → у
            // правого края панели (за стаканом). Держим синхронно с text.rs.
            use crate::chart_persist::PriceAxisPos;
            let axis_hidden = matches!(pr.price_axis_pos, PriceAxisPos::Hide);
            let axis_on_right = matches!(pr.price_axis_pos, PriceAxisPos::Right);

            // Прозрачная плашка-подложка под угловую подпись (alpha 0.2 — 80% прозрачности).
            // Якорь совпадает с текстом (text.rs): есть стакан → у края панели, нет → у края плота.
            // Рисуем ДО гейта `plot_w<60` — иначе в режиме «только стакан» (чарт схлопнут) подложки
            // под подписью не было (как сейчас у соседей с метлой).
            if pr.caption_w > 0.0 {
                let lines = (!pr.core_name.is_empty()) as u32 + (!pr.market.is_empty()) as u32;
                if lines > 0 {
                    let right_edge = if pr.orderbook_enabled {
                        pane_right
                    } else {
                        plot_right
                    };
                    let cap_x = right_edge - super::text::CAPTION_PAD_X;
                    let cap_y = plot_top + super::text::CAPTION_PAD_Y;
                    let (pad_l, pad_r, pad_y) = (5.0_f32, 3.0_f32, 2.0_f32);
                    let dst = [
                        (cap_x - pr.caption_w - pad_l) * sf,
                        (cap_y - pad_y) * sf,
                        (pr.caption_w + pad_l + pad_r) * sf,
                        (lines as f32 * super::text::LINE_H + pad_y * 2.0) * sf,
                    ];
                    pr.readout_rects.push(ReadoutRect {
                        dst,
                        bg: self.readout_soft_bg,
                        border,
                        m,
                    });
                }
            }

            // Дальше — курсорные плашки/оси, только для нормального (не схлопнутого) чарта.
            if plot_w < 60.0 || plot_h < 60.0 || pr.view.price_to_px <= 0.0 {
                continue;
            }

            let plot_bottom = plot_top + plot_h;

            // Плашки-подложки под подписи ордеров/курсора (раскладку дал `prepare_text`). Лёгкая
            // плашка (как угловая подпись монеты, alpha 0.2) для ордерных; плотная (alpha 0.96) для
            // курсорных — они приоритетные, на переднем плане. Строим ДО гейта по курсору — ордерные
            // подписи видны и без курсора.
            let placed = std::mem::take(&mut pr.label_placed);
            for pl in &placed {
                let dst = readout_rect_dst(pl.x, pl.y, pl.w, pl.h, pl.ax, pl.ay, sf);
                // solid → плотная курсорная плашка; иначе → полу-плотная ордерная (просвечивает,
                // младшая «заходит под» старшую при наложении).
                let pbg = if pl.solid { bg } else { self.readout_order_bg };
                pr.readout_rects.push(ReadoutRect {
                    dst,
                    bg: pbg,
                    border,
                    m,
                });
            }
            pr.label_placed = placed;

            let Some(cursor) = cursor.filter(|c| c.pane == idx) else {
                continue;
            };
            let cx_log = (slot_origin[0] + cursor.local[0]) / sf;
            let cy_log = (slot_origin[1] + cursor.local[1]) / sf;

            let time_to_px = (pr.view.time_to_px / sf).max(1e-6);
            if cx_log >= plot_left && cx_log <= plot_right {
                let left_unix = pr.epoch_ms + pr.view.view_time0 as f64;
                let unix = left_unix + (cx_log - plot_left) as f64 / time_to_px as f64;
                let label = moon_chart::axes::fmt_clock(unix, tz_offset_sec, true);
                let text_w = readout_text_width(&label, pr.readout_time_width);
                let line_h = pr.readout_time_line_h.max(1.0);
                let half_w = text_w * 0.5;
                let x = clamp_anchor(
                    cx_log,
                    plot_left + half_w + READOUT_PAD_X + READOUT_INSET,
                    plot_right - half_w - READOUT_PAD_X - READOUT_INSET,
                );
                let dst = readout_rect_dst(x, pane_bottom - 1.0, text_w, line_h, 0.5, 1.0, sf);
                pr.readout_rects.push(ReadoutRect { dst, bg, border, m });
            }

            if !axis_hidden && cy_log >= plot_top && cy_log <= plot_bottom {
                let price_to_px = pr.view.price_to_px / sf;
                let price_range = plot_h / price_to_px.max(1e-6);
                let y_min = pr.view.view_price0;
                let dec = moon_chart::axes::price_decimals(y_min + price_range * 0.5);
                let price = y_min + (plot_bottom - cy_log) / price_to_px.max(1e-6);
                let label = format!("{price:.dec$}");
                let text_w = readout_text_width(&label, pr.readout_price_width);
                let line_h = pr.readout_price_line_h.max(1.0);
                let x = if axis_on_right {
                    pane_right - 3.0
                } else {
                    (plot_left - 3.0).max(pane_left + READOUT_INSET + READOUT_PAD_X + text_w)
                };
                let dst = readout_rect_dst(x, cy_log, text_w, line_h, 1.0, 0.5, sf);
                pr.readout_rects.push(ReadoutRect { dst, bg, border, m });
            }
        }
    }

    pub(super) fn frame(&mut self, info: GpuFrameInfo) -> GpuFrameDecision {
        crate::diag::bump(&crate::diag::CHART_FRAME);
        if !info.presentable || info.bounds.is_empty() {
            crate::diag::bump(&crate::diag::CHART_FRAME_SKIP_NOT_PRESENTABLE);
            return GpuFrameDecision::Skip;
        }

        let now_ms = now_unix_ms();
        let mut wants_present = std::mem::take(&mut self.needs_present);
        if self.firetest_force_present {
            wants_present = true;
        }
        let cap_due = self.last_present_ms <= 0.0
            || now_ms - self.last_present_ms >= self.target_present_interval_ms;
        let mut camera_moved = false;
        for pr in &mut self.panes {
            if pr.active && (wants_present || cap_due) && pr.advance_camera(now_ms) {
                crate::diag::bump(&crate::diag::CHART_CAM_STEP);
                camera_moved = true;
                self.base_dirty = true;
                wants_present = true;
            }
        }
        if camera_moved {
            self.record_camera_shift(now_ms);
        }

        if wants_present {
            self.last_present_ms = now_ms;
            crate::diag::bump(&crate::diag::CHART_FRAME_REQUEST);
            GpuFrameDecision::RequestPresent
        } else {
            crate::diag::bump(&crate::diag::CHART_FRAME_SKIP_IDLE);
            GpuFrameDecision::Skip
        }
    }

    pub(super) fn prepare_gpu(&mut self, gpu: &RawGpuAccess) -> anyhow::Result<()> {
        let width = gpu.width();
        let height = gpu.height();
        if width == 0 || height == 0 {
            return Ok(());
        }

        let generation = gpu.device_generation();
        if self.last_gpu_prepare_generation != generation {
            self.last_gpu_prepare_generation = generation;
            self.base_dirty = true;
            for pr in &mut self.panes {
                pr.gpu_prepare_dirty = true;
            }
        }

        match gpu.backend() {
            #[cfg(windows)]
            GpuBackend::D3d11 => {
                let Some((device, context, _rtv)) = gpu::borrow_d3d(gpu) else {
                    anyhow::bail!("chart dx11 prepare received empty D3D11 raw gpu handles");
                };
                let res = [width as f32, height as f32];
                for pr in &mut self.panes {
                    if !pr.active || !pr.gpu_prepare_dirty {
                        continue;
                    }
                    let mut view = pr.view;
                    let mut orderbook_view = pr.orderbook_view;
                    view.resolution = res;
                    orderbook_view.resolution = res;
                    crate::diag::bump(&crate::diag::CHART_GPU_PREPARE);
                    pr.layers.prepare_d3d(
                        &view,
                        &orderbook_view,
                        &pr.book_style,
                        &device,
                        &context,
                        gpu,
                    );
                    pr.finish_order_gpu_prepare(now_unix_ms());
                    pr.gpu_prepare_dirty = false;
                }
                Ok(())
            }
            #[cfg(target_os = "linux")]
            GpuBackend::Wgpu => {
                let res = [width as f32, height as f32];
                let rebuild_base = self.base_dirty;
                for pr in &mut self.panes {
                    if !pr.active {
                        continue;
                    }
                    let needs_base = rebuild_base || pr.layers.needs_base_cache(gpu);
                    if !pr.gpu_prepare_dirty && !needs_base {
                        continue;
                    }
                    let mut view = pr.view;
                    let mut background_params = pr.background_params;
                    let mut grid_params = pr.grid_params;
                    let mut cursor_params = pr.cursor_params;
                    let mut orderbook_view = pr.orderbook_view;
                    view.resolution = res;
                    background_params.resolution = res;
                    grid_params.resolution = res;
                    cursor_params.resolution = res;
                    orderbook_view.resolution = res;
                    crate::diag::bump(&crate::diag::CHART_GPU_PREPARE);
                    pr.layers.prepare_wgpu(
                        &view,
                        &background_params,
                        &grid_params,
                        &cursor_params,
                        &orderbook_view,
                        &pr.book_style,
                        gpu,
                        needs_base,
                    )?;
                    pr.finish_order_gpu_prepare(now_unix_ms());
                    pr.gpu_prepare_dirty = false;
                }
                if rebuild_base {
                    self.base_dirty = false;
                }
                Ok(())
            }
            #[cfg(target_os = "macos")]
            GpuBackend::Metal => {
                let res = [width as f32, height as f32];
                let rebuild_base = self.base_dirty;
                for pr in &mut self.panes {
                    if !pr.active {
                        continue;
                    }
                    let needs_base = rebuild_base || pr.layers.needs_base_cache(gpu);
                    if !pr.gpu_prepare_dirty && !needs_base {
                        continue;
                    }
                    let mut view = pr.view;
                    let mut background_params = pr.background_params;
                    let mut grid_params = pr.grid_params;
                    let mut cursor_params = pr.cursor_params;
                    let mut orderbook_view = pr.orderbook_view;
                    view.resolution = res;
                    background_params.resolution = res;
                    grid_params.resolution = res;
                    cursor_params.resolution = res;
                    orderbook_view.resolution = res;
                    crate::diag::bump(&crate::diag::CHART_GPU_PREPARE);
                    pr.layers.prepare_metal(
                        &view,
                        &background_params,
                        &grid_params,
                        &cursor_params,
                        &orderbook_view,
                        &pr.book_style,
                        gpu,
                        needs_base,
                    )?;
                    pr.finish_order_gpu_prepare(now_unix_ms());
                    pr.gpu_prepare_dirty = false;
                }
                if rebuild_base {
                    self.base_dirty = false;
                }
                Ok(())
            }
            _ => Ok(()),
        }
    }

    #[cfg(windows)]
    pub(super) fn render_window_background_d3d(
        &mut self,
        res: [f32; 2],
        device: &ID3D11Device,
        context: &ID3D11DeviceContext,
        rtv: &ID3D11RenderTargetView,
        gpu: &RawGpuAccess,
    ) {
        let base = BackgroundParams {
            dst: [0.0, 0.0, res[0], res[1]],
            resolution: res,
            uv_off: [0.0, 0.0],
            uv_scale: [1.0, 1.0],
            opacity: 0.0,
            _pad: 0.0,
            bg: self.window_bg_color,
        };
        self.window_bg.render(&base, device, context, rtv, gpu);
    }

    #[cfg(windows)]
    pub(super) fn render_chart_base_d3d(
        &mut self,
        res: [f32; 2],
        device: &ID3D11Device,
        context: &ID3D11DeviceContext,
        rtv: &ID3D11RenderTargetView,
        gpu: &RawGpuAccess,
        scissor_rs: &ID3D11RasterizerState,
    ) {
        for pr in &mut self.panes {
            if !pr.active {
                continue;
            }
            let mut view = pr.view;
            let mut background_params = pr.background_params;
            let mut grid_params = pr.grid_params;
            let mut orderbook_view = pr.orderbook_view;
            view.resolution = res;
            background_params.resolution = res;
            grid_params.resolution = res;
            orderbook_view.resolution = res;
            let panel_clip = [
                view.bounds[0],
                view.bounds[1],
                orderbook_view.bounds[0] + orderbook_view.bounds[2],
                view.bounds[1] + view.bounds[3],
            ];
            gpu::set_scissor(
                context,
                scissor_rs,
                panel_clip[0],
                panel_clip[1],
                panel_clip[2],
                panel_clip[3],
            );
            pr.layers.render_base_d3d(
                &view,
                &background_params,
                &grid_params,
                &orderbook_view,
                &pr.book_style,
                device,
                context,
                rtv,
                gpu,
                panel_clip,
            );
        }
    }

    pub(super) fn draw_gpu(&mut self, gpu: &RawGpuAccess) -> anyhow::Result<()> {
        let width = gpu.width();
        let height = gpu.height();
        if width == 0 || height == 0 {
            return Ok(());
        }

        crate::diag::bump(&crate::diag::CHART_PRESENT);
        let present_ms = now_unix_ms();

        match gpu.backend() {
            #[cfg(windows)]
            GpuBackend::D3d11 => {
                let Some((device, context, rtv)) = gpu::borrow_d3d(gpu) else {
                    anyhow::bail!("chart dx11 draw received empty D3D11 raw gpu handles");
                };

                let generation = gpu.device_generation();
                if self.scissor_rs.is_none() || self.scissor_generation != generation {
                    self.scissor_rs = Some(gpu::create_scissor_rasterizer(&device));
                    self.scissor_generation = generation;
                }
                let res = [width as f32, height as f32];
                let scissor_rs = self.scissor_rs.clone().unwrap();
                let prev_rs = unsafe { context.RSGetState().ok() };

                if self.base_dirty || self.base_cache.needs_rebuild(gpu) {
                    let base_rtv = self.base_cache.begin_rebuild(&device, &context, gpu)?;
                    self.render_window_background_d3d(res, &device, &context, &base_rtv, gpu);
                    self.render_chart_base_d3d(res, &device, &context, &base_rtv, gpu, &scissor_rs);
                    self.base_dirty = false;
                }
                // Клип блита = слот ЭТОГО чарта (объединение баундов его активных панелей),
                // НЕ весь бэкбуфер: при нескольких gpu_canvas в одном окне (стек выносного
                // окна) полноэкранный блит window_bg затирал бы соседние чарты. Нет активных
                // панелей → не блитим (пусто = логотип GPUI поверх).
                let mut blit_clip: Option<[f32; 4]> = None;
                for pr in &self.panes {
                    if !pr.active {
                        continue;
                    }
                    let c = bounds_clip(pr.pane_bounds, res);
                    blit_clip = Some(match blit_clip {
                        Some(u) => [
                            u[0].min(c[0]),
                            u[1].min(c[1]),
                            u[2].max(c[2]),
                            u[3].max(c[3]),
                        ],
                        None => c,
                    });
                }
                if let Some(clip) = blit_clip {
                    self.base_cache.blit_to(&context, &rtv, gpu, clip);
                }

                for pr in &mut self.panes {
                    if !pr.active {
                        continue;
                    }
                    let mut view = pr.view;
                    let mut cursor_params = pr.cursor_params;
                    view.resolution = res;
                    cursor_params.resolution = res;
                    sync_readout_resolution(&mut pr.readout_rects, res);
                    let pane_clip = bounds_clip(pr.pane_bounds, res);
                    gpu::set_scissor(
                        &context,
                        &scissor_rs,
                        pane_clip[0],
                        pane_clip[1],
                        pane_clip[2],
                        pane_clip[3],
                    );
                    pr.layers
                        .render_userdata_lines_d3d(&view, &context, &rtv, gpu);
                    pr.layers.render_cursor_d3d(
                        &cursor_params,
                        &pr.readout_rects,
                        &device,
                        &context,
                        &rtv,
                        gpu,
                    );
                    pr.finish_order_present(present_ms);
                }
                unsafe {
                    context.RSSetState(prev_rs.as_ref());
                }
                Ok(())
            }
            #[cfg(target_os = "linux")]
            GpuBackend::Wgpu => {
                let res = [width as f32, height as f32];
                for pr in &mut self.panes {
                    if pr.active {
                        let mut view = pr.view;
                        let mut background_params = pr.background_params;
                        let mut grid_params = pr.grid_params;
                        let mut cursor_params = pr.cursor_params;
                        let mut orderbook_view = pr.orderbook_view;
                        view.resolution = res;
                        background_params.resolution = res;
                        grid_params.resolution = res;
                        cursor_params.resolution = res;
                        orderbook_view.resolution = res;
                        sync_readout_resolution(&mut pr.readout_rects, res);
                        pr.layers.render_wgpu(
                            &view,
                            pr.pane_bounds,
                            &background_params,
                            &grid_params,
                            &cursor_params,
                            &pr.readout_rects,
                            &orderbook_view,
                            gpu,
                        )?;
                        pr.finish_order_present(present_ms);
                    }
                }
                Ok(())
            }
            #[cfg(target_os = "macos")]
            GpuBackend::Metal => {
                let res = [width as f32, height as f32];
                for pr in &mut self.panes {
                    if pr.active {
                        let mut view = pr.view;
                        let mut background_params = pr.background_params;
                        let mut grid_params = pr.grid_params;
                        let mut cursor_params = pr.cursor_params;
                        let mut orderbook_view = pr.orderbook_view;
                        view.resolution = res;
                        background_params.resolution = res;
                        grid_params.resolution = res;
                        cursor_params.resolution = res;
                        orderbook_view.resolution = res;
                        sync_readout_resolution(&mut pr.readout_rects, res);
                        pr.layers.render_metal(
                            &view,
                            pr.pane_bounds,
                            &background_params,
                            &grid_params,
                            &cursor_params,
                            &pr.readout_rects,
                            &orderbook_view,
                            gpu,
                        )?;
                        pr.finish_order_present(present_ms);
                    }
                }
                Ok(())
            }
            _ => Ok(()),
        }
    }
}
