//! Данные слоёв `WgpuLayers`: combo-кольцо, price lines, стакан, userdata, скейл объёма
//! (вынос из wgpu_backend.rs, verbatim).

use super::*;

impl WgpuLayers {
    pub fn new() -> Self {
        Self {
            device_generation: 0,
            format: None,
            pipelines: None,
            background_texture: None,
            prepared_binds: None,
            base_cache: BaseCache::default(),
            combo_texture: None,
            combo_dirty_ranges: Vec::new(),
            crosses: Vec::new(),
            cross_head: 0,
            cross_count: 0,
            last_line: Vec::new(),
            mark_line: Vec::new(),
            combo_capacity: MIN_COMBO_CAPACITY,
            price_line_capacity: MIN_COMBO_CAPACITY,
            levels: Vec::new(),
            zones: Vec::new(),
            hlines: Vec::new(),
            segs: Vec::new(),
            markers: Vec::new(),
            volume_buy_max: 1e-6,
            volume_sell_max: 1e-6,
            bg_uniform: BufferSlot::default(),
            grid_uniform: BufferSlot::default(),
            cursor_uniform: BufferSlot::default(),
            readout_rect_buffer: BufferSlot::default(),
            view_uniform: BufferSlot::default(),
            combo_view_uniform: BufferSlot::default(),
            book_view_uniform: BufferSlot::default(),
            book_style_uniform: BufferSlot::default(),
            cross_buffer: BufferSlot::default(),
            last_line_buffer: BufferSlot::default(),
            mark_line_buffer: BufferSlot::default(),
            level_buffer: BufferSlot::default(),
            zone_buffer: BufferSlot::default(),
            hline_buffer: BufferSlot::default(),
            seg_buffer: BufferSlot::default(),
            marker_buffer: BufferSlot::default(),
            combo_buffers_dirty: true,
            price_line_buffers_dirty: true,
            book_buffer_dirty: true,
            userdata_buffers_dirty: true,
        }
    }

    pub fn set_combo_capacity(&mut self, combo_capacity: usize, price_line_capacity: usize) {
        let combo_capacity = sanitize_capacity(combo_capacity);
        let price_line_capacity = sanitize_capacity(price_line_capacity);
        if self.combo_capacity == combo_capacity && self.price_line_capacity == price_line_capacity
        {
            return;
        }
        let ordered = ordered_cross_ring(
            &self.crosses,
            self.cross_head,
            self.cross_count,
            self.combo_capacity,
        );
        self.combo_capacity = combo_capacity;
        self.price_line_capacity = price_line_capacity;
        reset_cross_ring(
            &mut self.crosses,
            &mut self.cross_head,
            &mut self.cross_count,
            self.combo_capacity,
            &ordered,
        );
        if self.crosses.len() < self.combo_capacity {
            self.crosses
                .resize(self.combo_capacity, ChartCross::zeroed());
        }
        if self.last_line.len() > self.price_line_capacity {
            self.last_line = tail_vec(&self.last_line, self.price_line_capacity);
        }
        if self.mark_line.len() > self.price_line_capacity {
            self.mark_line = tail_vec(&self.mark_line, self.price_line_capacity);
        }
        self.recalc_volume_scale();
        self.combo_buffers_dirty = true;
        self.price_line_buffers_dirty = true;
        self.combo_texture = None;
        self.combo_dirty_ranges.clear();
    }

    pub fn reset_combo(&mut self, data: Vec<ChartCross>) {
        reset_cross_ring(
            &mut self.crosses,
            &mut self.cross_head,
            &mut self.cross_count,
            self.combo_capacity,
            &data,
        );
        if self.crosses.len() < self.combo_capacity {
            self.crosses
                .resize(self.combo_capacity, ChartCross::zeroed());
        }
        self.recalc_volume_scale();
        self.combo_buffers_dirty = true;
        if let Some(tex) = self.combo_texture.as_mut() {
            tex.valid = false;
        }
        self.combo_dirty_ranges.clear();
    }

    pub fn append_combo(&mut self, data: &[ChartCross]) {
        if data.is_empty() {
            return;
        }
        let before_scale = (self.volume_buy_max, self.volume_sell_max);
        let old_head = self.cross_head;
        let old_count = self.cross_count;
        let full_reset = data.len() >= self.combo_capacity;
        let evicted_ranges =
            evicted_cross_ranges(old_head, old_count, self.combo_capacity, data.len());
        let evicted_any = ranges_have_entries(&evicted_ranges);
        let evicted_scale_max =
            ranges_touch_volume_max(&self.crosses, &evicted_ranges, before_scale);
        append_cross_ring(
            &mut self.crosses,
            &mut self.cross_head,
            &mut self.cross_count,
            self.combo_capacity,
            data,
        );
        if self.crosses.len() < self.combo_capacity {
            self.crosses
                .resize(self.combo_capacity, ChartCross::zeroed());
        }
        if full_reset || evicted_scale_max {
            self.recalc_volume_scale();
        } else {
            self.update_volume_scale(data);
        }
        self.combo_buffers_dirty = true;
        if full_reset || evicted_any || before_scale != (self.volume_buy_max, self.volume_sell_max)
        {
            if let Some(tex) = self.combo_texture.as_mut() {
                tex.valid = false;
            }
            self.combo_dirty_ranges.clear();
        } else {
            let appended = data.len().min(self.combo_capacity);
            for (start, count) in cross_append_ranges(old_head, appended, self.combo_capacity) {
                if count > 0 {
                    self.combo_dirty_ranges.push((start, count));
                }
            }
        }
    }

    pub fn set_price_lines(&mut self, last: &[PriceLinePoint], mark: &[PriceLinePoint]) {
        self.last_line = tail_vec(last, self.price_line_capacity);
        self.mark_line = tail_vec(mark, self.price_line_capacity);
        self.price_line_buffers_dirty = true;
    }

    pub fn set_orderbook(&mut self, levels: Vec<LevelInstance>) {
        self.levels = levels;
        self.book_buffer_dirty = true;
        self.base_cache.valid = false;
    }

    pub fn set_userdata(
        &mut self,
        zones: &[ZoneInstance],
        hlines: &[LineInstance],
        segs: &[SegInstance],
        markers: &[MarkerInstance],
    ) {
        self.zones = zones.iter().map(zone_of).collect();
        self.hlines = hlines.iter().map(hl_of).collect();
        self.segs = segs.iter().map(seg_of).collect();
        self.markers = markers.iter().map(mk_of).collect();
        self.userdata_buffers_dirty = true;
        self.base_cache.valid = false;
    }

    pub fn needs_base_cache(&self, gpu: &RawGpuAccess) -> bool {
        self.base_cache.needs_rebuild(gpu)
    }

    pub(super) fn reset_gpu_objects(&mut self) {
        self.pipelines = None;
        self.background_texture = None;
        self.prepared_binds = None;
        self.base_cache = BaseCache::default();
        self.bg_uniform = BufferSlot::default();
        self.grid_uniform = BufferSlot::default();
        self.cursor_uniform = BufferSlot::default();
        self.readout_rect_buffer = BufferSlot::default();
        self.view_uniform = BufferSlot::default();
        self.combo_view_uniform = BufferSlot::default();
        self.book_view_uniform = BufferSlot::default();
        self.book_style_uniform = BufferSlot::default();
        self.cross_buffer = BufferSlot::default();
        self.last_line_buffer = BufferSlot::default();
        self.mark_line_buffer = BufferSlot::default();
        self.level_buffer = BufferSlot::default();
        self.zone_buffer = BufferSlot::default();
        self.hline_buffer = BufferSlot::default();
        self.seg_buffer = BufferSlot::default();
        self.marker_buffer = BufferSlot::default();
        self.combo_buffers_dirty = true;
        self.price_line_buffers_dirty = true;
        self.book_buffer_dirty = true;
        self.userdata_buffers_dirty = true;
    }

    fn recalc_volume_scale(&mut self) {
        let (buy, sell) = cross_volume_max(self.crosses.iter().take(self.cross_count));
        self.volume_buy_max = buy;
        self.volume_sell_max = sell;
    }

    fn update_volume_scale(&mut self, data: &[ChartCross]) {
        let mut max = (self.volume_buy_max, self.volume_sell_max);
        update_cross_volume_max(&mut max, data);
        self.volume_buy_max = max.0;
        self.volume_sell_max = max.1;
    }
}

fn tail_vec<T: Clone>(data: &[T], cap: usize) -> Vec<T> {
    let start = data.len().saturating_sub(cap);
    data[start..].to_vec()
}

fn sanitize_capacity(capacity: usize) -> usize {
    capacity.max(MIN_COMBO_CAPACITY)
}
