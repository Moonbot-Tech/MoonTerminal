//! `WgpuLayers` draw methods and prepare pass for base/combo caches, cursor, and user layers.

use super::pipelines::{create_background_texture, create_pipelines};
use super::*;

impl WgpuLayers {
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
        self.draw_price_lines_layer(pass);
        let sc = bounds_scissor(pane_bounds, gpu.width(), gpu.height());
        pass.set_scissor_rect(sc.0, sc.1, sc.2, sc.3);
        self.draw_user_layers(pass);
        self.draw_cursor_layer(pass, cursor_params, readout_rects);
        Ok(())
    }

    fn draw_base_layers(&self, pass: &mut wgpu::RenderPass<'_>) {
        let pipelines = self.pipelines.as_ref().unwrap();
        let binds = self.prepared_binds.as_ref().unwrap();
        crate::diag::bump(&crate::diag::CHART_BG_DRAW);
        draw_pipeline(pass, &pipelines.background, &binds.bg, 6, 1);
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
        crate::diag::bump(&crate::diag::CHART_GRID_DRAW);
        draw_pipeline(pass, &pipelines.grid, &binds.grid, 6, 1);
        // Candles render beneath trade crosses; combo is blitted over the base cache.
        if !self.candles.is_empty() {
            crate::diag::bump(&crate::diag::CHART_CANDLE_DRAW);
            draw_pipeline(
                pass,
                &pipelines.candles,
                &binds.candle,
                18,
                self.candles.len() as u32,
            );
        }
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
    }

    fn draw_user_layers(&self, pass: &mut wgpu::RenderPass<'_>) {
        let pipelines = self.pipelines.as_ref().unwrap();
        let binds = self.prepared_binds.as_ref().unwrap();
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
        if self.cross_count == 0 {
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
                self.draw_combo_layers(device, queue, &mut pass, bake_view, 0, self.cross_count);
            } else {
                let ranges = std::mem::take(&mut self.combo_dirty_ranges);
                for (start, count) in ranges {
                    if count > 0 {
                        self.draw_combo_layers(device, queue, &mut pass, bake_view, start, count);
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
    ) {
        let recreated = self.combo_view_uniform.write(
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
        if count > 0 {
            crate::diag::bump(&crate::diag::CHART_COMBO_DRAW);
            draw_pipeline_range(pass, &pipelines.crosses, &binds.cross, 6, start, count);
        }
    }

    fn draw_price_lines_layer(&self, pass: &mut wgpu::RenderPass<'_>) {
        let pipelines = self.pipelines.as_ref().unwrap();
        let binds = self.prepared_binds.as_ref().unwrap();
        if self.last_line.len() > 1 {
            crate::diag::bump(&crate::diag::CHART_COMBO_DRAW);
            draw_pipeline(
                pass,
                &pipelines.price_last,
                &binds.last,
                6,
                (self.last_line.len() - 1) as u32,
            );
        }
        if self.mark_line.len() > 1 {
            crate::diag::bump(&crate::diag::CHART_COMBO_DRAW);
            draw_pipeline(
                pass,
                &pipelines.price_mark,
                &binds.mark,
                6,
                (self.mark_line.len() - 1) as u32,
            );
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
    // All fields are contractually non-null NonNull<c_void>; obtain each raw pointer with `.as_ptr()`.
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
    // render_pass is Option<NonNull<c_void>> and remains None during prepare before a pass exists.
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
