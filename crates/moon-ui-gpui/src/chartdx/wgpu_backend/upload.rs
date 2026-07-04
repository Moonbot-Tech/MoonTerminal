//! Заливка юниформ/сторадж-буферов и bind groups `WgpuLayers`
//! (вынос из wgpu_backend.rs, verbatim).

use super::*;

impl WgpuLayers {
    pub(super) fn upload_common(
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
        binds_dirty |= self.combo_view_uniform.write(
            device,
            queue,
            "moon_chart_combo_view_uniform",
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
                let mut recreated = false;
                for &(start, count) in &self.combo_dirty_ranges {
                    let end = start.saturating_add(count).min(self.crosses.len());
                    if start < end {
                        recreated |= self.cross_buffer.write_range(
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
                if recreated {
                    binds_dirty |= self.cross_buffer.write(
                        device,
                        queue,
                        "moon_chart_crosses",
                        wgpu::BufferUsages::STORAGE,
                        &self.crosses,
                    );
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

    pub(super) fn upload_frame_uniforms(
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
        uniform: &'a BufferSlot,
        storage: &'a BufferSlot,
    ) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("moon_chart_view_storage_bind"),
            layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: uniform.binding(),
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

    pub(super) fn prepare_bind_groups(&mut self, device: &wgpu::Device) {
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
        let cross_bind = self.bind_view_storage(
            device,
            &pipelines.view_storage_layout,
            &self.combo_view_uniform,
            &self.cross_buffer,
        );
        let last_bind = self.bind_view_storage(
            device,
            &pipelines.view_storage_layout,
            &self.view_uniform,
            &self.last_line_buffer,
        );
        let mark_bind = self.bind_view_storage(
            device,
            &pipelines.view_storage_layout,
            &self.view_uniform,
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
        let zone_bind = self.bind_view_storage(
            device,
            &pipelines.view_storage_layout,
            &self.view_uniform,
            &self.zone_buffer,
        );
        let hline_bind = self.bind_view_storage(
            device,
            &pipelines.view_storage_layout,
            &self.view_uniform,
            &self.hline_buffer,
        );
        let seg_bind = self.bind_view_storage(
            device,
            &pipelines.view_storage_layout,
            &self.view_uniform,
            &self.seg_buffer,
        );
        let marker_bind = self.bind_view_storage(
            device,
            &pipelines.view_storage_layout,
            &self.view_uniform,
            &self.marker_buffer,
        );
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
}
