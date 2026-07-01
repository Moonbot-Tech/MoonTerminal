//! Подготовка данных чарта per pane (`impl ChartDataState`): главный `prepare`
//! (БЕЗ рисования) — чтение истории/стакана/ордеров, заливка GPU-буферов, авто-Y,
//! сигнатуры изменений. Вынесено из `mod.rs`; структура `ChartDataState` объявлена там.

use super::*;

fn hash_order_zones(zones: &[moon_chart::layers::ZoneInstance]) -> u64 {
    let mut h = 0x9E37_79B9_7F4A_7C15u64 ^ zones.len() as u64;
    for z in zones {
        h = h.rotate_left(5) ^ z.price0.to_bits() as u64;
        h = h.rotate_left(7) ^ z.price1.to_bits() as u64;
        for c in z.color {
            h = h.rotate_left(11) ^ c.to_bits() as u64;
        }
    }
    h
}

impl ChartDataState {
    pub(super) fn new(
        container: Rc<RefCell<Container>>,
        render: Rc<RefCell<RenderState>>,
        theme: ChartTheme,
    ) -> Self {
        Self {
            container,
            render,
            theme,
            orders: OrdersStyle::default(),
            follow: true,
            present_rate_hz: 60.0,
            w: 1024,
            h: 576,
            origin: (0.0, 0.0),
            scene_visible: false,
            orderbook_enabled: true,
            liquidations_enabled: true,
            orderbook_only: false,
            price_axis_pos: crate::chart_persist::PriceAxisPos::Left,
            time_axis_visible: true,
            prospective_usd: None,
            order_highlight: None,
            order_drag_preview: None,
            market_source: None,
            last_frame_tick_ms: 0.0,
            present_rate_candidate_hz: 0.0,
            present_rate_candidate_hits: 0,
            last_ppp: 1.0,
            slot_bounds: None,
            last_order_sig: u64::MAX,
            last_prepared_market_sig: u64::MAX,
            last_source_market_sig: u64::MAX,
            view_dirty: true,
        }
    }

    pub(super) fn notify_signature(&self, session: &SessionManager) -> u64 {
        let mut sig = 0u64;
        if let Some(source) = &self.market_source {
            sig = self.market_signature(source);
        }
        sig.wrapping_mul(31)
            .wrapping_add(self.order_signature(session))
    }

    pub(super) fn order_signature(&self, session: &SessionManager) -> u64 {
        let mut sig = 0u64;
        if let Some((core, _market)) = self.container.borrow().target_ref(0) {
            if let Some(core_st) = session.store().core(core) {
                sig = sig.wrapping_add(core_st.order_lines_rev);
            }
        }
        sig
    }

    pub(super) fn sync_orders_if_visible(&mut self, session: &SessionManager, force: bool) -> bool {
        if !self.scene_visible {
            return false;
        }
        let sig = self.order_signature(session);
        if !force && sig == self.last_order_sig {
            return false;
        }
        crate::diag::bump(&crate::diag::CHART_PREPARE);
        let changed = self.sync_orders_from_session(session, force);
        self.last_order_sig = sig;
        changed
    }

    pub(super) fn market_signature(&self, source: &MarketDataSource) -> u64 {
        self.source_market_signature(source)
    }

    pub(super) fn source_market_signature(&self, source: &MarketDataSource) -> u64 {
        let container = self.container.borrow();
        let Some((core, market)) = container.target_ref(0) else {
            return 0;
        };

        let mut sig = 0xcbf29ce484222325;
        sig = mix_sig(sig, core);
        sig = mix_sig(sig, str_sig(&market));
        if let Some(revs) = source.market_revisions(core, &market) {
            sig = mix_sig(sig, revs.provider);
            sig = mix_sig(sig, revs.generation);
            sig = mix_sig(sig, revs.history);
            sig = mix_sig(sig, revs.book);
            sig = mix_sig(sig, revs.meta);
        }
        sig
    }

    pub(super) fn refresh_visible_markets(&self, source: &MarketDataSource) -> bool {
        let container = self.container.borrow();
        let Some((core, market)) = container.target_ref(0) else {
            return false;
        };
        source.refresh_market(core, market)
    }

    pub(super) fn mark_view_dirty(&mut self) {
        self.view_dirty = true;
    }

    pub(super) fn set_order_visual(
        &mut self,
        highlight: Option<(CoreId, u64)>,
        drag_preview: Option<(CoreId, u64, LineKind, f32)>,
    ) -> bool {
        if self.order_highlight == highlight && self.order_drag_preview == drag_preview {
            return false;
        }
        self.order_highlight = highlight;
        self.order_drag_preview = drag_preview;
        let mut st = self.render.borrow_mut();
        for pr in &mut st.panes {
            pr.last_order_highlight_uid = None;
            pr.last_order_drag_preview = None;
            pr.last_order_lines_rev = u64::MAX;
            pr.gpu_prepare_dirty = true;
        }
        st.needs_present = true;
        true
    }

    /// Применить геометрию слота из bounds канваса (логич. px) к движку: размер/origin/pixel-scale.
    /// Источник — `GpuFrameInfo` (форк), синхронно в `frame()` → own-pass всегда в актуальном слоте.
    fn apply_slot_geometry(&mut self, info: &GpuFrameInfo) {
        if info.bounds.is_empty() {
            return;
        }
        let sf = info.scale_factor.max(0.1);
        let w = (f32::from(info.bounds.size.width) * sf).round().max(1.0) as u32;
        let h = (f32::from(info.bounds.size.height) * sf).round().max(1.0) as u32;
        let ox = f32::from(info.bounds.origin.x) * sf;
        let oy = f32::from(info.bounds.origin.y) * sf;
        if self.w != w || self.h != h {
            self.w = w;
            self.h = h;
            self.mark_view_dirty();
        }
        if self.origin != (ox, oy) {
            self.origin = (ox, oy);
            self.mark_view_dirty();
        }
        self.last_ppp = sf;
        self.slot_bounds = Some(info.bounds);
        let mut st = self.render.borrow_mut();
        st.set_slot_origin(ox, oy); // self-guard: dirty/present только при смене
        st.set_pixel_scale(sf);
    }

    pub(super) fn set_market_source(&mut self, source: Option<MarketDataSource>) -> bool {
        let changed = match (&self.market_source, &source) {
            (Some(a), Some(b)) => !a.ptr_eq(b),
            (None, None) => false,
            _ => true,
        };
        if changed {
            self.market_source = source;
            self.view_dirty = true;
        }
        changed
    }

    pub(super) fn frame(&mut self, info: GpuFrameInfo) -> GpuFrameDecision {
        // Геометрия слота — СИНХРОННО из info.bounds (форк отдаёт реальные bounds канваса этого
        // кадра, ДО present). Применяем до pull/sync, чтобы own-pass рисовал в текущем слоте без
        // лага probe→notify→render→present (1–2 кадра): иначе при рефлоу стека освободившийся/
        // сдвинутый слот кадр-два мигал clear'ом окна.
        self.apply_slot_geometry(&info);
        let now_ms = now_unix_ms();
        if self.observe_present_rate(now_ms) {
            if let Some(source) = self.market_source.clone() {
                crate::diag::bump(&crate::diag::CHART_PREPARE);
                self.sync_from_market_source(&source, None);
            } else {
                self.view_dirty = true;
            }
        }
        if self.pull_market_source_if_visible() {
            crate::diag::bump(&crate::diag::CHART_PREPARE);
        }
        self.render.borrow_mut().frame(info)
    }

    pub(super) fn observe_present_rate(&mut self, now_ms: f64) -> bool {
        let prev_tick_ms = std::mem::replace(&mut self.last_frame_tick_ms, now_ms);
        if prev_tick_ms <= 0.0 {
            return false;
        }
        let dt_ms = now_ms - prev_tick_ms;
        if !(2.0..=40.0).contains(&dt_ms) {
            self.present_rate_candidate_hits = 0;
            return false;
        }
        let sample_hz = (1000.0 / dt_ms).round().clamp(30.0, 360.0) as f32;
        if (sample_hz - self.present_rate_hz).abs() < 0.5 {
            self.present_rate_candidate_hits = 0;
            self.present_rate_candidate_hz = 0.0;
            return false;
        }
        if (sample_hz - self.present_rate_candidate_hz).abs() < 0.5 {
            self.present_rate_candidate_hits = self.present_rate_candidate_hits.saturating_add(1);
        } else {
            self.present_rate_candidate_hz = sample_hz;
            self.present_rate_candidate_hits = 1;
        }
        if self.present_rate_candidate_hits < 6 {
            return false;
        }
        self.present_rate_candidate_hits = 0;
        self.present_rate_hz = sample_hz;
        self.render
            .borrow_mut()
            .set_target_present_rate_hz(self.present_rate_hz);
        true
    }

    pub(super) fn pull_market_source_if_visible(&mut self) -> bool {
        if !self.scene_visible {
            return false;
        }
        let Some(source) = self.market_source.clone() else {
            return false;
        };
        let source_sig = self.source_market_signature(&source);
        if !self.view_dirty && source_sig == self.last_source_market_sig {
            return false;
        }
        if self.container.borrow().target_ref(0).is_none() {
            self.last_source_market_sig = source_sig;
            return false;
        }
        let source_changed = source_sig != self.last_source_market_sig;
        let pulled_book = source_changed && self.refresh_visible_markets(&source);
        let sig = source_sig;
        if !self.view_dirty
            && !source_changed
            && !pulled_book
            && sig == self.last_prepared_market_sig
        {
            self.last_source_market_sig = source_sig;
            return false;
        }
        self.sync_from_market_source(&source, Some(sig));
        self.last_source_market_sig = source_sig;
        true
    }

    pub(super) fn sync_orders_from_session(
        &mut self,
        session: &SessionManager,
        force: bool,
    ) -> bool {
        let area = Rect {
            x: 0.0,
            y: 0.0,
            w: self.w as f32,
            h: self.h as f32,
        };
        let layout = self.container.borrow().layout(area);
        let now = now_unix_ms();
        let mut st = self.render.borrow_mut();
        let mut container = self.container.borrow_mut();
        let mut pixels_changed = false;
        let mut base_changed = false;
        // Смена числа панелей (в т.ч. удаление последней монеты → пусто) обязана пометить
        // base_dirty: иначе base-кэш продолжит блитить СТАРЫЙ чарт сквозь пустой слот (логотип
        // прозрачный). Зеркалит проверку в sync_from_market_source.
        if st.panes.len() != container.pane_count() {
            pixels_changed = true;
            base_changed = true;
        }
        st.panes
            .resize_with(container.pane_count(), PaneRender::new);

        for (idx, _) in &layout {
            let Some(pane) = container.pane_mut(*idx) else {
                continue;
            };
            let pr = &mut st.panes[*idx];
            if pr.core != Some(pane.core) || pr.market != pane.market {
                *pr = PaneRender::new();
                pr.core = Some(pane.core);
                pr.market = pane.market.clone();
                pixels_changed = true;
                base_changed = true;
            }
            // Имя ядра для угловой подписи: резолвим тут — только здесь под рукой `session`.
            // Меняется редко (смена ядра панели), поэтому флагаем present лишь при изменении.
            let core_name = session
                .sessions()
                .iter()
                .find(|s| s.id == pane.core)
                .map(|s| s.name.clone())
                .unwrap_or_default();
            if pr.core_name != core_name {
                pr.core_name = core_name;
                pixels_changed = true;
            }
            let device_gen = pr.layers.device_gen();
            let device_lost = pr.last_device_gen != device_gen;
            if device_lost {
                pr.last_order_lines_rev = u64::MAX;
                pr.gpu_prepare_dirty = true;
                pixels_changed = true;
            }

            let order_price = session
                .store()
                .core(pane.core)
                .and_then(|core_st| core_st.order_lines.buy_sell_range(&pane.market));
            if pr.cached_order_price != order_price {
                pr.cached_order_price = order_price;
                self.view_dirty = true;
                pixels_changed = true;
            }

            if let Some(core_st) = session.store().core(pane.core) {
                let highlight_uid = self
                    .order_highlight
                    .and_then(|(core, uid)| (core == pane.core).then_some(uid));
                let drag_preview = self
                    .order_drag_preview
                    .and_then(|(core, uid, kind, price)| {
                        (core == pane.core).then_some((uid, kind, price))
                    });
                let drag_preview_sig =
                    drag_preview.map(|(uid, kind, price)| (uid, kind, price.to_bits()));
                if force
                    || pr.last_order_lines_rev != core_st.order_lines_rev
                    || pr.last_order_highlight_uid != highlight_uid
                    || pr.last_order_drag_preview != drag_preview_sig
                {
                    let mut hlines = Vec::new();
                    let mut segs = Vec::new();
                    let mut markers = Vec::new();
                    let mut zones = Vec::new();
                    moon_chart::build_order_geometry(
                        &core_st.order_lines,
                        &pane.market,
                        &self.orders,
                        highlight_uid,
                        drag_preview,
                        pane.view.epoch_ms,
                        now,
                        f32::NEG_INFINITY,
                        f32::INFINITY,
                        0.0,
                        &mut zones,
                        &mut hlines,
                        &mut segs,
                        &mut markers,
                    );
                    let zone_sig = hash_order_zones(&zones);
                    if pr.last_order_zone_sig != zone_sig {
                        pr.last_order_zone_sig = zone_sig;
                        base_changed = true;
                    }
                    pr.layers.set_userdata(&zones, &hlines, &segs, &markers);
                    let quote_usd = self
                        .market_source
                        .as_ref()
                        .and_then(|s| s.quote_usd_rate(pane.core, &pane.market));
                    build_order_labels(
                        &mut pr.order_labels,
                        &mut pr.orderbook_labels,
                        &core_st.order_lines,
                        &pane.market,
                        &self.theme,
                        quote_usd,
                        drag_preview,
                    );
                    pr.last_order_lines_rev = core_st.order_lines_rev;
                    pr.last_order_lines_sync_ms = now;
                    pr.pending_order_gpu_rev = Some(core_st.order_lines_rev);
                    pr.last_order_highlight_uid = highlight_uid;
                    pr.last_order_drag_preview = drag_preview_sig;
                    pr.gpu_prepare_dirty = true;
                    pixels_changed = true;
                }
            } else if force || pr.last_order_lines_rev != u64::MAX {
                if pr.last_order_zone_sig != 0 {
                    pr.last_order_zone_sig = 0;
                    base_changed = true;
                }
                pr.layers.set_userdata(&[], &[], &[], &[]);
                pr.order_labels.clear();
                pr.orderbook_labels.clear();
                pr.last_order_lines_rev = u64::MAX;
                pr.last_order_lines_sync_ms = now;
                pr.pending_order_gpu_rev = Some(u64::MAX);
                pr.last_order_highlight_uid = None;
                pr.last_order_drag_preview = None;
                pr.gpu_prepare_dirty = true;
                pixels_changed = true;
            }
            pr.last_device_gen = device_gen;
        }

        if base_changed {
            st.base_dirty = true;
        }
        if pixels_changed {
            st.needs_present = true;
        }
        pixels_changed
    }

    pub(super) fn sync_from_market_source(
        &mut self,
        source: &MarketDataSource,
        prepared_sig: Option<u64>,
    ) {
        let area = Rect {
            x: 0.0,
            y: 0.0,
            w: self.w as f32,
            h: self.h as f32,
        };
        let layout = self.container.borrow().layout(area);
        let now = now_unix_ms();
        let res = [self.w as f32, self.h as f32];
        let mut st = self.render.borrow_mut();
        let mut container = self.container.borrow_mut();
        let mut pixels_changed = false;
        #[cfg(windows)]
        {
            let next_bg_color = rgb4(self.theme.bg);
            if st.window_bg_color != next_bg_color {
                st.window_bg_color = next_bg_color;
                pixels_changed = true;
            }
        }
        let was_active: Vec<bool> = st.panes.iter().map(|pane| pane.active).collect();
        if st.panes.len() != container.pane_count() {
            pixels_changed = true;
        }
        st.panes
            .resize_with(container.pane_count(), PaneRender::new);
        for pr in &mut st.panes {
            pr.active = false;
        }
        for (idx, rect) in &layout {
            let Some(pane) = container.pane_mut(*idx) else {
                continue;
            };
            let pr = &mut st.panes[*idx];
            if !was_active.get(*idx).copied().unwrap_or(false) {
                pixels_changed = true;
                pr.gpu_prepare_dirty = true;
            }
            if pr.core != Some(pane.core) || pr.market != pane.market {
                *pr = PaneRender::new();
                pr.core = Some(pane.core);
                pr.market = pane.market.clone();
                pixels_changed = true;
            }
            let next_pane_bounds = [
                self.origin.0 + rect.x,
                self.origin.1 + rect.y,
                rect.w.max(1.0),
                rect.h.max(1.0),
            ];
            if pr.pane_bounds != next_pane_bounds {
                pr.pane_bounds = next_pane_bounds;
                pixels_changed = true;
            }
            let device_gen = pr.layers.device_gen();
            let device_lost = pr.last_device_gen != device_gen;
            if device_lost {
                pr.last_book_rev = u64::MAX;
                pr.last_order_lines_rev = u64::MAX;
                pr.gpu_prepare_dirty = true;
                pixels_changed = true;
            }
            // Позиция оси цен (per-окно). Режим «только стакан» (метла) принудительно прячет ось,
            // перебивая per-tab настройку. Hide → жёлоба нет (место отдаётся графику).
            let axis_pos = if self.orderbook_only {
                crate::chart_persist::PriceAxisPos::Hide
            } else {
                self.price_axis_pos
            };
            let price_axis_w = if matches!(axis_pos, crate::chart_persist::PriceAxisPos::Hide) {
                0.0
            } else {
                moon_chart::PRICE_AXIS_W * self.last_ppp
            };
            // Ось времени скрыта → жёлоб под подписи не резервируем, плот занимает всю высоту.
            let time_axis_h = if self.time_axis_visible {
                moon_chart::TIME_AXIS_H * self.last_ppp
            } else {
                0.0
            };
            let plot_h = (rect.h - time_axis_h).max(1.0);
            // П.3: при узком графике стакан не должен съедать половину. База — GLASS_ZONE_PX
            // (ограничена половиной слота). Если ширина графика при базовом стакане < 2× самого
            // стакана (узко), сжимаем стакан до 0.8× зоны, отдавая место графику. В обычном
            // (широком) режиме ширина стакана не меняется.
            let glass_cap = rect.w * 0.5;
            let glass_base = moon_chart::GLASS_ZONE_PX.min(glass_cap);
            let chart_w_base = rect.w - price_axis_w - glass_base;
            // only → стакан на всю ширину; выкл → 0; иначе адаптивная зона.
            let glass_w = if self.orderbook_only {
                (rect.w - price_axis_w).max(1.0)
            } else if !self.orderbook_enabled {
                0.0
            } else if chart_w_base < glass_base * 2.0 {
                (moon_chart::GLASS_ZONE_PX * 0.8).min(glass_cap)
            } else {
                glass_base
            };
            // Left → жёлоб оси слева (чарт сдвинут вправо), стакан у правого края.
            // Right → чарт от левого края, стакан сразу за ним, ось — жёлоб справа ЗА стаканом.
            // Hide → оси нет, чарт от левого края, стакан у правого края.
            let axis_on_left = matches!(axis_pos, crate::chart_persist::PriceAxisPos::Left);
            let chart_x = if axis_on_left {
                rect.x + price_axis_w
            } else {
                rect.x
            };
            let chart_w = (rect.w - price_axis_w - glass_w).max(1.0);
            let glass_x = if matches!(axis_pos, crate::chart_persist::PriceAxisPos::Right) {
                chart_x + chart_w
            } else {
                rect.x + (rect.w - glass_w).max(1.0)
            };
            let chart_area = Rect {
                x: chart_x,
                y: rect.y,
                w: chart_w,
                h: plot_h,
            };
            let glass_area = Rect {
                x: glass_x,
                y: rect.y,
                w: glass_w,
                h: plot_h,
            };
            pane.view
                .ensure_default_window(chart_area.w, self.present_rate_hz);
            pane.view.follow_edge(now, now);
            let (view_time0, window_ms) = pane.view.visible_x(chart_area.w);
            let cam_px = ((pane.view.right_time_ms - pane.view.epoch_ms)
                * pane.view.px_per_ms.max(1e-9) as f64)
                .round() as i64;
            let marker_margin = view::cross_cull_margin_physical_px(&pane.view, self.last_ppp)
                / pane.view.px_per_ms.max(1e-6);
            let history_prefetch = (window_ms * 0.20).max(marker_margin);
            let history_from = view_time0 - history_prefetch;
            let history_to = view_time0 + window_ms + history_prefetch;
            let scan_price = device_lost || cam_px != pr.scan_cam_px;
            let source_revs = source.market_revisions(pane.core, &pane.market);
            let source_generation = source_revs.map(|revs| revs.generation).unwrap_or(0);
            let source_generation_changed = source_generation != pr.source_generation;
            let mut history_source_sig = 0xcbf29ce4_84222325u64;
            if let Some(revs) = source_revs {
                history_source_sig = mix_sig(history_source_sig, revs.provider);
                history_source_sig = mix_sig(history_source_sig, revs.generation);
                history_source_sig = mix_sig(history_source_sig, revs.history);
                history_source_sig = mix_sig(history_source_sig, revs.meta);
            }
            let history_source_changed = history_source_sig != pr.source_history_sig;
            // Смена галки «Ликвидации» → перезалить combo (добавить/убрать кресты ликвидаций).
            let liq_toggle_changed = pr.liquidations_enabled != self.liquidations_enabled;
            pr.liquidations_enabled = self.liquidations_enabled;
            let force_history_reset = device_lost
                || source_generation_changed
                || liq_toggle_changed
                || pr.resident_left_rel.is_nan()
                || history_from < pr.resident_left_rel
                || (!pane.view.follow && scan_price);
            let read_history = history_source_changed || force_history_reset;
            let mut history = if read_history {
                let history_read_started = force_history_reset.then(std::time::Instant::now);
                let history = source.read_chart_history_into(
                    pane.core,
                    &pane.market,
                    pane.view.epoch_ms,
                    history_from,
                    history_to,
                    force_history_reset,
                    scan_price,
                    &mut pr.history_cursor,
                    &mut pr.history_buffers,
                );
                if let Some(started) = history_read_started {
                    crate::diag::bump_by(
                        &crate::diag::CHART_HISTORY_RESET_MS,
                        started.elapsed().as_millis().max(1) as u64,
                    );
                }
                history
            } else {
                None
            };
            if read_history {
                pr.source_history_sig = history_source_sig;
                pr.source_generation = source_generation;
            }
            let capacity_changed = history.as_ref().is_some_and(|h| {
                (h.combo_capacity > 0 && h.combo_capacity != pr.combo_cross_capacity)
                    || (h.price_line_capacity > 0
                        && h.price_line_capacity != pr.combo_price_line_capacity)
            });
            if capacity_changed && history.as_ref().is_some_and(|h| !h.combo_reset) {
                let history_read_started = std::time::Instant::now();
                history = source.read_chart_history_into(
                    pane.core,
                    &pane.market,
                    pane.view.epoch_ms,
                    history_from,
                    history_to,
                    true,
                    scan_price,
                    &mut pr.history_cursor,
                    &mut pr.history_buffers,
                );
                crate::diag::bump_by(
                    &crate::diag::CHART_HISTORY_RESET_MS,
                    history_read_started.elapsed().as_millis().max(1) as u64,
                );
            }
            let last_price = if let Some(history) = history {
                if scan_price {
                    pr.cached_tick_price = history.tick_price_range;
                    pr.scan_cam_px = cam_px;
                }
                let last_price = history.last_price;
                if capacity_changed || history.combo_reset {
                    pr.combo_cross_capacity = history.combo_capacity;
                    pr.combo_price_line_capacity = history.price_line_capacity;
                    pr.layers
                        .set_combo_capacity(history.combo_capacity, history.price_line_capacity);
                }
                if history.combo_reset {
                    crate::diag::bump_by(
                        &crate::diag::CHART_HISTORY_RESET_ROWS,
                        (pr.history_buffers.ticks.len()
                            + pr.history_buffers.last_points.len()
                            + pr.history_buffers.mark_points.len()) as u64,
                    );
                    fill_cross_upload(
                        &pr.history_buffers.ticks,
                        pane.view.epoch_ms,
                        &mut pr.cross_upload,
                    );
                    crate::diag::bump_by(
                        &crate::diag::CHART_COMBO_UPLOAD_LEN,
                        pr.cross_upload.len() as u64,
                    );
                    pr.layers.reset_combo(std::mem::take(&mut pr.cross_upload));
                    // A full range read covers the requested left edge even when the first
                    // real trade is newer than that edge. Using the first tick as the resident
                    // left boundary makes a fresh live chart reset every frame while the
                    // 60s window extends into empty pre-connect history.
                    pr.resident_left_rel = history_from;
                    pr.gpu_prepare_dirty = true;
                    pixels_changed = true;
                } else if !pr.history_buffers.ticks.is_empty() {
                    fill_cross_upload(
                        &pr.history_buffers.ticks,
                        pane.view.epoch_ms,
                        &mut pr.cross_upload,
                    );
                    crate::diag::bump_by(
                        &crate::diag::CHART_COMBO_UPLOAD_LEN,
                        pr.cross_upload.len() as u64,
                    );
                    pr.layers.append_combo(&pr.cross_upload);
                    pr.gpu_prepare_dirty = true;
                    pixels_changed = true;
                }
                // Кресты ТРЕЙДОВ ЛИКВИДАЦИЙ (side=2) — в то же combo-кольцо, отдельным append
                // (порядок в кольце не влияет на позицию: шейдер ставит по time_rel). На combo_reset
                // источник отдал полный видимый диапазон, иначе — только новый живой край. Гейт
                // per-панель: выкл → не добавляем (а смена флага форсит reset выше, убирая старые).
                if pr.liquidations_enabled && !pr.history_buffers.liquidations.is_empty() {
                    fill_liq_upload(
                        &pr.history_buffers.liquidations,
                        pane.view.epoch_ms,
                        &mut pr.liq_upload,
                    );
                    pr.layers.append_combo(&pr.liq_upload);
                    pr.gpu_prepare_dirty = true;
                    pixels_changed = true;
                }
                if history.price_lines_changed || history.combo_reset {
                    fill_price_upload(
                        &pr.history_buffers.last_points,
                        pane.view.epoch_ms,
                        &mut pr.last_line_upload,
                    );
                    fill_price_upload(
                        &pr.history_buffers.mark_points,
                        pane.view.epoch_ms,
                        &mut pr.mark_line_upload,
                    );
                    crate::diag::bump_by(
                        &crate::diag::CHART_PRICE_LINE_UPLOAD_LEN,
                        (pr.last_line_upload.len() + pr.mark_line_upload.len()) as u64,
                    );
                    pr.layers
                        .set_price_lines(&pr.last_line_upload, &pr.mark_line_upload);
                    pr.gpu_prepare_dirty = true;
                    pixels_changed = true;
                }
                if chart_market_diag_enabled()
                    && chart_market_diag_due(format!("combo:{}:{}:{}", pane.core, pane.market, idx))
                {
                    chart_market_diag(format!(
                        "pane={} core={} market={} provider={} rev={} reset={} ticks={} \
                         price_lines={} clipped={} caught_up={} scan_price={} \
                         window=[{:.1},{:.1}] resident_left={:.1} last_price={:?} bounds={:?}",
                        idx,
                        pane.core,
                        pane.market,
                        history.provider,
                        history.revision,
                        history.combo_reset,
                        pr.history_buffers.ticks.len(),
                        history.price_lines_changed,
                        history.clipped,
                        history.caught_up,
                        scan_price,
                        view_time0,
                        view_time0 + window_ms,
                        pr.resident_left_rel,
                        history.last_price,
                        pr.view.bounds
                    ));
                }
                pr.cached_last_price = last_price;
                last_price
            } else if read_history {
                if pr.resident_left_rel.is_finite() {
                    pr.layers.reset_combo(Vec::new());
                    pr.layers.set_price_lines(&[], &[]);
                    pr.history_cursor.reset();
                    pr.resident_left_rel = f32::NAN;
                    pr.cached_tick_price = None;
                    pr.cached_last_price = None;
                    pr.gpu_prepare_dirty = true;
                    pixels_changed = true;
                }
                if scan_price {
                    pr.cached_tick_price = None;
                    pr.scan_cam_px = cam_px;
                }
                let latest = source.latest_price(pane.core, &pane.market).ok();
                pr.cached_last_price = latest;
                latest
            } else {
                pr.cached_last_price
            };
            let tick_price = pr.cached_tick_price;
            // Якорь авто-фокуса по стакану: лучшие bid/ask. O(1) чтение под коротким
            // read-lock; полную книгу строим ниже уже по выставленному окну.
            let book_top = source.with_orderbook_view(pane.core, &pane.market, |data| {
                data.and_then(|(book, _)| book.best_bid_ask())
            });
            let book_mid = book_top.map(|(bid, ask)| (bid + ask) * 0.5);
            // Если трейдов нет — центрируемся по стакану: середину книги даём как якорь
            // центра (fallback для last_price), а видимую полосу строим так, чтобы она
            // ГАРАНТИРОВАННО включала лучшие bid/ask (широкий спред HIP-3), но была не уже
            // ±BOOK_FOCUS_HALF (иначе на узком спреде — абсурдный зум-ин). Когда трейды
            // есть (tick_price.is_some()) — полосу НЕ добавляем: диапазон ведут реальные тики.
            let book_focus =
                tick_price
                    .is_none()
                    .then_some(book_top)
                    .flatten()
                    .map(|(bid, ask)| {
                        let mid = (bid + ask) * 0.5;
                        let min_half = mid.abs() * BOOK_FOCUS_HALF_FRAC;
                        (bid.min(mid - min_half), ask.max(mid + min_half))
                    });
            let last_price = last_price.or(book_mid);
            // Цена-ориентир для подписи курсора (% выше/ниже): трейды, иначе мид стакана. Без
            // этого на HIP-рынках без трейдов (но со стаканом) подпись % у курсора пропадала.
            pr.cached_last_price = last_price;
            let visible_price = union_range(
                union_range(
                    union_range(tick_price, pr.cached_order_price),
                    last_price.map(|p| (p, p)),
                ),
                book_focus,
            );
            pane.view.update_y(now, plot_h, visible_price, last_price);
            let area_win = Rect {
                x: self.origin.0 + chart_area.x,
                y: self.origin.1 + chart_area.y,
                w: chart_area.w,
                h: chart_area.h,
            };
            let next_view = view::view_gpu(&pane.view, area_win, res, self.last_ppp);
            if pr.view != next_view {
                pr.view = next_view;
                pr.gpu_prepare_dirty = true;
                pixels_changed = true;
            }
            pr.epoch_ms = pane.view.epoch_ms;
            pr.right_margin_frac = pane.view.right_margin_frac;
            pr.follow = pane.view.follow;
            pr.last_edge_px = ((pane.view.right_time_ms - pane.view.epoch_ms)
                * pane.view.px_per_ms.max(1e-9) as f64)
                .round() as i64;
            let (bg_uv_off, bg_uv_scale) = cover_uv(chart_area.w, chart_area.h, 1.0);
            let background_opacity = if CHART_PHOTO_BACKGROUND_ENABLED {
                self.theme.background_opacity.clamp(0.0, 1.0)
            } else {
                0.0
            };
            let next_background_params = BackgroundParams {
                dst: pr.view.bounds,
                resolution: res,
                uv_off: bg_uv_off,
                uv_scale: bg_uv_scale,
                opacity: background_opacity,
                _pad: 0.0,
                bg: rgb4(self.theme.bg),
            };
            if pr.background_params != next_background_params {
                pr.background_params = next_background_params;
                pixels_changed = true;
            }
            let next_grid_params = GridParams {
                bounds: pr.view.bounds,
                resolution: res,
                n_vert: GRID_N_VERT,
                n_horiz: GRID_N_HORIZ,
                _pad0: 0.0,
                _pad1: 0.0,
                grid_alpha: self.theme.grid_alpha,
                bg_alpha: if background_opacity > 0.0 { 0.0 } else { 1.0 },
                bg: rgb4(self.theme.bg),
                grid_col: rgb4(self.theme.grid),
            };
            if pr.grid_params != next_grid_params {
                pr.grid_params = next_grid_params;
                pixels_changed = true;
            }
            let glass_win = Rect {
                x: self.origin.0 + glass_area.x,
                y: self.origin.1 + glass_area.y,
                w: glass_area.w,
                h: glass_area.h,
            };
            let next_orderbook_view = view::view_gpu(&pane.view, glass_win, res, self.last_ppp);
            if pr.orderbook_view != next_orderbook_view {
                pr.orderbook_view = next_orderbook_view;
                pr.gpu_prepare_dirty = true;
                pixels_changed = true;
            }
            let next_book_style = BookStyle {
                book_bg: rgb4(self.theme.book_bg),
                bid: rgb4(self.theme.book_bid),
                ask: rgb4(self.theme.book_ask),
                level: [
                    self.theme.book_level_alpha.clamp(0.0, 1.0),
                    self.theme.book_level_width.max(0.0),
                    0.0,
                    0.0,
                ],
            };
            if pr.book_style != next_book_style {
                pr.book_style = next_book_style;
                pr.gpu_prepare_dirty = true;
                pixels_changed = true;
            }
            // Флаг стакана в pane (для гейта угловой подписи в render_state/text). В режиме
            // «только стакан» стакан принудительно включён (даже если галка «Стакан» снята).
            pr.orderbook_only = self.orderbook_only;
            // Эффективная позиция оси (с учётом форс-Hide в режиме метлы) — для рендера подписей.
            pr.price_axis_pos = axis_pos;
            pr.time_axis_visible = self.time_axis_visible;
            pr.prospective_usd = self.prospective_usd;
            let orderbook_on = self.orderbook_enabled || self.orderbook_only;
            pr.orderbook_enabled = orderbook_on;
            // Стакан выключен (per-окно) → уровни не строим и не грузим (а если были — чистим).
            if !orderbook_on {
                if pr.last_book_rev != u64::MAX {
                    pr.layers.set_orderbook(Vec::new());
                    pr.last_book_rev = u64::MAX;
                    pr.last_book_lo = f32::NAN;
                    pr.last_book_hi = f32::NAN;
                    pr.gpu_prepare_dirty = true;
                    pixels_changed = true;
                }
                pr.orderbook_levels.clear();
            } else {
                source.with_orderbook_view(pane.core, &pane.market, |data| {
                    if let Some((book, book_rev)) = data {
                        let half = pane.view.render_range.max(1e-9) * 0.5;
                        let (lo, hi) = (
                            pane.view.render_center - half,
                            pane.view.render_center + half,
                        );
                        let mut diag_levels_len = None;
                        if pr.last_book_rev != book_rev
                            || pr.last_book_lo != lo
                            || pr.last_book_hi != hi
                        {
                            let mut levels = Vec::new();
                            book.build_instances(lo, hi, &mut levels);
                            diag_levels_len = Some(levels.len());
                            pr.layers.set_orderbook(levels);
                            // CPU-копия видимой книги — для подписей объёма в стакане:
                            // cursor volume и MoonBot-style sell-line depth label.
                            book.collect_visible_depth(lo, hi, &mut pr.orderbook_levels);
                            pr.last_book_rev = book_rev;
                            pr.last_book_lo = lo;
                            pr.last_book_hi = hi;
                            pr.gpu_prepare_dirty = true;
                            pixels_changed = true;
                        }
                        if chart_market_diag_enabled()
                            && chart_market_diag_due(format!(
                                "book:{}:{}:{}",
                                pane.core, pane.market, idx
                            ))
                        {
                            chart_market_diag(format!(
                                "pane={} core={} market={} book_rev={} book_len={} levels={:?} \
                                 y=[{lo:.8},{hi:.8}] center={:.8} range={:.8} book_bounds={:?}",
                                idx,
                                pane.core,
                                pane.market,
                                book_rev,
                                book.len(),
                                diag_levels_len,
                                pane.view.render_center,
                                pane.view.render_range,
                                pr.orderbook_view.bounds
                            ));
                        }
                    } else if pr.last_book_rev != u64::MAX {
                        pr.layers.set_orderbook(Vec::new());
                        pr.orderbook_levels.clear();
                        pr.last_book_rev = u64::MAX;
                        pr.last_book_lo = f32::NAN;
                        pr.last_book_hi = f32::NAN;
                        pr.gpu_prepare_dirty = true;
                        pixels_changed = true;
                    }
                });
            }
            let edge_rel = view_time0 + (chart_area.w + glass_w) / pane.view.px_per_ms.max(1e-6);
            if pr.view.pad != edge_rel {
                pr.view.pad = edge_rel;
                pixels_changed = true;
            }
            pr.last_device_gen = device_gen;
            pr.active = true;
        }
        for (idx, was_active) in was_active.into_iter().enumerate() {
            if was_active && !st.panes.get(idx).is_some_and(|pr| pr.active) {
                pixels_changed = true;
            }
        }
        let prev_cursor_params: Vec<CursorParams> =
            st.panes.iter().map(|pr| pr.cursor_params).collect();
        st.sync_cursor_params();
        let cursor_changed = st.cursor.is_some()
            && st
                .panes
                .iter()
                .zip(prev_cursor_params.iter())
                .any(|(pr, prev)| pr.cursor_params != *prev);
        if pixels_changed {
            st.base_dirty = true;
        }
        if pixels_changed || cursor_changed {
            st.needs_present = true;
        }
        drop(container);
        drop(st);
        self.last_prepared_market_sig =
            prepared_sig.unwrap_or_else(|| self.source_market_signature(source));
        self.view_dirty = false;
    }
}

/// Подписи ордерных линий рынка для слоя текста: размер у buy-линии, % от входа +
/// количество купленного у sell-линии, % стопа у stop-линии. Сторона размещения
/// (над/под линией) зависит от long/short — как в эталоне MoonBot (категория E).
/// Только открытые ордера (закрытые/исполненные не подписываем).
fn build_order_labels(
    out: &mut Vec<OrderLabel>,
    book_out: &mut Vec<OrderBookLabel>,
    store: &moon_core::session::order_lines::OrderLineStore,
    market: &str,
    theme: &ChartTheme,
    quote_usd: Option<f64>,
    drag_preview: Option<(u64, LineKind, f32)>,
) {
    out.clear();
    book_out.clear();
    let mut orders: Vec<_> = store
        .iter_market(market)
        .filter(|o| o.closed_ms.is_none())
        .collect();
    orders.sort_by_key(|o| o.seq);
    for o in orders {
        let preview = drag_preview
            .filter(|(uid, _, price)| *uid == o.uid && price.is_finite() && *price > 0.0);
        let line_price = |kind: LineKind| {
            preview
                .filter(|(_, preview_kind, _)| *preview_kind == kind)
                .map(|(_, _, price)| price)
                .or_else(|| o.lines[kind as usize].current_price())
        };
        let line_forced =
            |kind: LineKind| preview.is_some_and(|(_, preview_kind, _)| preview_kind == kind);
        let mut push =
            |price: f32, text: String, above: bool, color: u32, priority: u8, force: bool| {
                if price.is_finite() && price > 0.0 && !text.is_empty() {
                    out.push(OrderLabel {
                        price,
                        text,
                        above,
                        color,
                        priority,
                        force,
                    });
                }
            };
        let buy = line_price(LineKind::Buy);
        let sell = line_price(LineKind::Sell);
        let stop = line_price(LineKind::Stop);
        let short = o.is_short;
        // Порядковый номер ордера на чарте — на основной подписи каждой линии (buy/sell/stop),
        // чтобы связать линии одного ордера: «$X [10]», «-5% [10]», стоп «-3% [10]».
        let tag = if o.chart_num > 0 {
            format!("[{}]", o.chart_num)
        } else {
            String::new()
        };
        let with_tag = |text: String| {
            if tag.is_empty() {
                text
            } else {
                format!("{text} {tag}")
            }
        };
        // BUY (линия входа): ожидающий ордер (fill=0) → «размер [N]» ОДНОЙ строкой, чтобы номер
        // и размер не наложились друг на друга на одной стороне линии; исполненный → только [N].
        // Размер входа — ВСЕГДА белый (не цвет линии, не по стороне).
        if let Some(bp) = buy {
            let forced = line_forced(LineKind::Buy);
            let text = if o.fill_pct <= 0.0 && o.size > 0.0 {
                let amount = match quote_usd {
                    Some(rate) if rate > 0.0 => fmt_usd(o.size as f64 * bp as f64 * rate),
                    _ => fmt_amount(o.size),
                };
                with_tag(amount)
            } else {
                tag.clone()
            };
            push(bp, text, !short, ORDER_LABEL_NEUTRAL, PRIO_BUY, forced);
        }
        // SELL: профит-% от цены входа (знаковый цвет) + РАЗМЕР на продажу в $-ноционале
        // (remaining·цена_продажи·курс) на противоположной стороне линии — как в MoonBot:
        // процент primary рисуется всегда, остаток caption проходит через YTextFill.
        if let Some(sp) = sell {
            let forced = line_forced(LineKind::Sell);
            if sp.is_finite() && sp > 0.0 {
                book_out.push(OrderBookLabel { price: sp, short });
            }
            if let Some(bp) = buy {
                if bp > 0.0 {
                    let pct = signed_pct(sp, bp, short);
                    push(
                        sp,
                        with_tag(fmt_pct(pct)),
                        short,
                        pct_color(theme, pct),
                        PRIO_SELL_PCT,
                        forced,
                    );
                }
            }
            let remaining = if o.remaining_size > 0.0 {
                o.remaining_size
            } else {
                o.size
            };
            if remaining > 0.0 && sp > 0.0 {
                let amount = match quote_usd {
                    Some(rate) if rate > 0.0 => fmt_usd(remaining as f64 * sp as f64 * rate),
                    _ => fmt_amount(remaining),
                };
                push(
                    sp,
                    amount,
                    !short,
                    side_color(theme, short),
                    PRIO_SELL_SIZE,
                    forced,
                );
            }
        }
        // STOP: % стопа от цены покупки. Шорт → сверху, лонг → снизу. Primary без YTextFill,
        // как Delphi-блок stop-loss label.
        if let (Some(stp), Some(bp)) = (stop, buy) {
            if bp > 0.0 {
                let forced = line_forced(LineKind::Stop);
                let pct = signed_pct(stp, bp, short);
                push(
                    stp,
                    with_tag(fmt_pct(pct)),
                    short,
                    pct_color(theme, pct),
                    PRIO_STOP_PCT,
                    forced,
                );
            }
        }
    }
}

fn rgb_u32(c: [u8; 3]) -> u32 {
    ((c[0] as u32) << 16) | ((c[1] as u32) << 8) | c[2] as u32
}

/// Знаковый процент уровня от цены входа С УЧЁТОМ СТОРОНЫ: лонг — как есть; шорт —
/// инвертирован (выше входа = минус). Так профит у обеих сторон зелёный, лосс красный (как MB):
/// для шорта sell ниже входа → «+», стоп выше входа → «−».
fn signed_pct(level: f32, entry: f32, short: bool) -> f32 {
    let raw = (level - entry) / entry * 100.0;
    if short { -raw } else { raw }
}

/// Цвет подписи размера по стороне ордера: лонг → positive, шорт → negative.
fn side_color(theme: &ChartTheme, short: bool) -> u32 {
    if short {
        rgb_u32(theme.label_negative)
    } else {
        rgb_u32(theme.label_positive)
    }
}

/// Цвет знакового процента: плюс → positive, минус → negative.
fn pct_color(theme: &ChartTheme, v: f32) -> u32 {
    if v >= 0.0 {
        rgb_u32(theme.label_positive)
    } else {
        rgb_u32(theme.label_negative)
    }
}

/// Компактное число (база/штуки) с SI-суффиксом K/M/B/T.
/// Размер ордера: SI-суффикс (K/M/B/T) + ДО 2 знаков дробной части (сотые, если есть),
/// без хвостовых нулей. Не использует общий `compact_si` (тот для десятков даёт до 3 знаков
/// — «49.744»). 50 → «50»; 49.744 → «49.74»; 1234 → «1.23K»; 49744 → «49.74K».
fn fmt_size_2dp(v: f64) -> String {
    let a = v.abs();
    let (n, suffix) = if a >= 1e12 {
        (v / 1e12, "T")
    } else if a >= 1e9 {
        (v / 1e9, "B")
    } else if a >= 1e6 {
        (v / 1e6, "M")
    } else if a >= 1e3 {
        (v / 1e3, "K")
    } else {
        (v, "")
    };
    let s = format!("{n:.2}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    format!("{s}{suffix}")
}

fn fmt_amount(v: f32) -> String {
    fmt_size_2dp(v as f64)
}

/// $-сумма с SI-суффиксом: 1234 → «$1.23K».
fn fmt_usd(v: f64) -> String {
    format!("${}", fmt_size_2dp(v))
}

fn fmt_pct(v: f32) -> String {
    format!("{v:+.2}%")
}
