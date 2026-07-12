//! Жизненный цикл/сигнатуры/frame `ChartDataState` (вынос из data_state.rs, verbatim).

use super::*;

impl ChartDataState {
    pub(crate) fn new(
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
            candle_view: moon_core::market::CandleViewCfg::default(),
            default_x_ppm: None,
            prospective_usd: None,
            order_highlight: None,
            order_drag_preview: None,
            figures: None,
            figure_visual: figures_sync::FigureVisual::default(),
            figure_visual_rev: 0,
            market_source: None,
            last_frame_tick_at: None,
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

    pub(crate) fn notify_signature(&self, session: &SessionManager) -> u64 {
        let mut sig = 0u64;
        if let Some(source) = &self.market_source {
            sig = self.market_signature(source);
        }
        sig.wrapping_mul(31)
            .wrapping_add(self.order_signature(session))
    }

    pub(crate) fn order_signature(&self, session: &SessionManager) -> u64 {
        let mut sig = 0u64;
        if let Some((core, _market)) = self.container.borrow().target_ref(0) {
            if let Some(core_st) = session.store().core(core) {
                sig = sig.wrapping_add(core_st.order_lines_rev);
            }
        }
        sig
    }

    pub(crate) fn sync_orders_if_visible(&mut self, session: &SessionManager, force: bool) -> bool {
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

    pub(crate) fn market_signature(&self, source: &MarketDataSource) -> u64 {
        self.source_market_signature(source)
    }

    pub(crate) fn source_market_signature(&self, source: &MarketDataSource) -> u64 {
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

    pub(crate) fn refresh_visible_markets(&self, source: &MarketDataSource) -> bool {
        let container = self.container.borrow();
        let Some((core, market)) = container.target_ref(0) else {
            return false;
        };
        source.refresh_market(core, market)
    }

    pub(crate) fn mark_view_dirty(&mut self) {
        self.view_dirty = true;
    }

    pub(crate) fn set_order_visual(
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

    pub(crate) fn set_market_source(&mut self, source: Option<MarketDataSource>) -> bool {
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

    pub(crate) fn frame(&mut self, info: GpuFrameInfo) -> GpuFrameDecision {
        // Геометрия слота — СИНХРОННО из info.bounds (форк отдаёт реальные bounds канваса этого
        // кадра, ДО present). Применяем до pull/sync, чтобы own-pass рисовал в текущем слоте без
        // лага probe→notify→render→present (1–2 кадра): иначе при рефлоу стека освободившийся/
        // сдвинутый слот кадр-два мигал clear'ом окна.
        self.apply_slot_geometry(&info);
        if !info.presentable || info.bounds.is_empty() {
            return self.render.borrow_mut().frame(info);
        }
        if self.observe_present_rate(Instant::now()) {
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

    pub(crate) fn observe_present_rate(&mut self, now: Instant) -> bool {
        let Some(prev_tick) = self.last_frame_tick_at.replace(now) else {
            return false;
        };
        let dt_ms = now.duration_since(prev_tick).as_secs_f64() * 1000.0;
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

    pub(crate) fn pull_market_source_if_visible(&mut self) -> bool {
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
}
