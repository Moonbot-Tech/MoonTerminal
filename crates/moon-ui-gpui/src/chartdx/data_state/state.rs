//! `ChartDataState` lifecycle, signatures, and frame handling.

use super::*;

/// How often the countdown clock is consulted, at most.
///
/// A quarter of the finest quantum any countdown asks for, so a second-by-second caption is never
/// more than a quarter second late — below what a reader can see on a clock — while the check
/// itself runs four times a second instead of up to three hundred and sixty.
const COUNTDOWN_CHECK: Duration = Duration::from_millis(250);

impl ChartDataState {
    /// The horizontal-volume zone this engine lays out, or `None`: the tab's own switch and width
    /// gated by [`Self::hvol_allowed`], in one place so the layout and the read cannot disagree.
    /// The panel publishes the same answer to its input hit test from its own copy of the
    /// settings (`ChartPanel::hvol_zone_spec`).
    pub(super) fn hvol_zone_spec(&self) -> Option<moon_chart::hvol::HvolZoneSpec> {
        moon_chart::hvol::zone_spec(&self.chart_graphics).filter(|_| self.hvol_allowed)
    }

    /// Create chart state with a core-local identity report axis until the backend supplies measurements.
    ///
    /// Args:
    ///     container: Chart panes and their shared navigation state.
    ///     render: Per-pane retained render state.
    ///     theme: Initial chart theme.
    ///
    /// Returns:
    ///     Fresh chart data state ready for the first backend synchronization.
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
            orderbook_only: false,
            price_axis_pos: crate::persistence::chart_persist::PriceAxisPos::Left,
            time_axis_visible: true,
            hvol_allowed: true,
            compare_lock_shown: false,
            candle_view: moon_core::market::CandleViewCfg::default(),
            chart_graphics: moon_core::config::ChartGraphicsCfg::default(),
            chart_labels: std::rc::Rc::new(moon_core::config::ChartLabelsCfg::default()),
            trade_labels: None,
            historical: false,
            arb_view: std::rc::Rc::new(moon_core::config::ArbViewCfg::default()),
            default_x_ppm: None,
            prospective_usd: None,
            order_highlight: None,
            order_drag_preview: None,
            figures: None,
            figure_visual: figures_sync::FigureVisual::default(),
            figure_visual_rev: 0,
            news_marks: std::rc::Rc::new(Vec::new()),
            news_hovered: None,
            trade_history: std::rc::Rc::new(Vec::new()),
            trade_history_cores: None,
            report_axis: moon_core::db::ReportAxis::identity_core_local(),
            trade_history_revision: 0,
            archived_lines: Rc::new(HashMap::new()),
            archived_lines_rev: 0,
            trade_hovered: None,
            warn_marks: std::rc::Rc::new(Vec::new()),
            warn_hovered: None,
            market_source: None,
            trade_replay: None,
            frozen_orders: None,
            frozen_fit_range: None,
            frozen_overlay: None,
            frozen_overlay_rev: 0,
            last_frame_tick_at: None,
            present_rate_candidate_hz: 0.0,
            present_rate_candidate_hits: 0,
            last_ppp: 1.0,
            content_zoom: 1.0,
            slot_bounds: None,
            last_order_sig: u64::MAX,
            last_prepared_market_sig: u64::MAX,
            last_source_market_sig: u64::MAX,
            last_countdown_check: None,
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

    /// Wakes order synchronization for each pane's order and confirmed strategy streams.
    pub(crate) fn order_signature(&self, session: &SessionManager) -> u64 {
        let mut sig = 0u64;
        // A detect caption is refreshed on this SAME sync — it is read where the session is in hand
        // — so the detect ring has to be part of what wakes it. Folded in only while such a caption
        // is drawn: detects arrive on their own stream, and mixing them unconditionally would run
        // an order sync on every detect for every chart that prints none.
        // `draws_live_market` for the reason `orders::sync` refuses to FILL these captions on a
        // frozen engine: waking a sync for a value that path will not resolve is work with no
        // possible effect on the screen.
        let wants_detect = self.draws_live_market()
            && self.chart_labels.any_drawn(|f| {
                matches!(
                    f,
                    moon_core::config::ChartLabelField::DetectStrategy
                        | moon_core::config::ChartLabelField::DetectMsg
                )
            });
        let wants_filters = self.draws_live_market()
            && self
                .chart_labels
                .any_drawn(|f| f == moon_core::config::ChartLabelField::StrategyFilters);
        let shared_sigs = (self.figures_sig(), self.news_sig(), self.warn_sig());
        let container = self.container.borrow();
        for ix in 0..container.pane_count() {
            let Some((core, _market)) = container.target_ref(ix) else {
                continue;
            };
            // Trade arrows draw with or without a live core, and a camera-only sync is not forced,
            // so their own signature (scale bucket, built span) has to be able to wake it.
            if let Some(pane) = container.pane(ix) {
                sig = sig
                    .wrapping_mul(31)
                    .wrapping_add(self.trade_history_sig(&pane.view));
            }
            // Everything else the per-pane rebuild gate in `orders::sync` compares, so an unforced
            // sync is woken by at least what that gate would rebuild for.
            let frozen = !self.draws_live_market();
            let highlight_uid = self
                .order_highlight
                .and_then(|(c, uid)| (c == core && !frozen).then_some(uid));
            let drag_preview_sig = self.order_drag_preview.and_then(|(c, uid, kind, price)| {
                (c == core && !frozen).then_some((
                    uid,
                    std::mem::discriminant(&kind),
                    price.to_bits(),
                ))
            });
            let mut pane_sig = std::collections::hash_map::DefaultHasher::new();
            std::hash::Hash::hash(
                &(
                    shared_sigs.0,
                    shared_sigs.1,
                    shared_sigs.2,
                    highlight_uid,
                    drag_preview_sig,
                ),
                &mut pane_sig,
            );
            sig = sig
                .wrapping_mul(31)
                .wrapping_add(std::hash::Hasher::finish(&pane_sig));
            if let Some(core_st) = session.store().core(core) {
                // A frozen viewer draws its archived store, so THAT revision is what wakes it; the
                // live store's changes are exactly what such a viewer must not react to.
                let order_lines_rev = match self.draws_live_market() {
                    true => core_st.order_lines_rev,
                    false => self.frozen_orders.as_ref().map_or(0, |store| store.rev),
                };
                sig = sig.wrapping_mul(31).wrapping_add(order_lines_rev);
                if !self.draws_live_market() {
                    sig = sig.wrapping_mul(31).wrapping_add(self.frozen_overlay_rev);
                }
                if self.draws_live_market() {
                    sig = sig.wrapping_mul(31).wrapping_add(core_st.strategies_rev);
                    sig = sig.wrapping_mul(31).wrapping_add(core_st.schema_rev);
                    // The closed trades' archived lines ride the same buffer as the live orders.
                    sig = sig.wrapping_mul(31).wrapping_add(self.archived_lines_rev);
                }
            }
        }
        if wants_filters {
            for ix in 0..container.pane_count() {
                let Some((core, _)) = container.target_ref(ix) else {
                    continue;
                };
                if let Some(core_st) = session.store().core(core) {
                    sig = sig.wrapping_mul(31).wrapping_add(core_st.chart_text_rev);
                }
            }
        }
        // A detect caption is resolved PER PANE, from that pane's own core — so every pane's core
        // has to be able to wake this sync, not just the first one's. A stack of four coins on four
        // cores would otherwise refresh only the first pane's detect line, and only when its orders
        // happened to move.
        if wants_detect {
            for ix in 0..container.pane_count() {
                let Some((core, _)) = container.target_ref(ix) else {
                    continue;
                };
                if let Some(core_st) = session.store().core(core) {
                    sig = sig.wrapping_mul(31).wrapping_add(core_st.detects_rev);
                }
            }
        }
        sig
    }

    /// Whether this engine's captions may read the LIVE market at all.
    ///
    /// One predicate for every caption gate — the position and detect ones in `orders`, the market
    /// snapshots in `market` — because they answer the same question and a rule copied into each
    /// of them is a rule that drifts. A frozen chart's captions describe the trade it was handed;
    /// what funding costs right now, what the book looks like and what traded in the last minute
    /// are facts about a different picture than the one on the screen.
    ///
    /// Returns:
    ///     `true` while this engine draws the live market.
    pub(in crate::chartdx) fn draws_live_market(&self) -> bool {
        !self.historical && self.trade_replay.is_none()
    }

    /// Mark this engine a historical viewer, mirroring the engine's own flag.
    ///
    /// Args:
    ///     historical: Whether the engine draws a finished interval rather than the live edge.
    pub(crate) fn set_historical(&mut self, historical: bool) {
        if self.historical == historical {
            return;
        }
        self.historical = historical;
        // The caption gates read this, and BOTH sync paths short-circuit on an unchanged signature:
        // without these resets, a viewer marked historical after its first sync would keep whatever
        // live figures that sync had already resolved until its market happened to move.
        self.last_order_sig = u64::MAX;
        self.last_prepared_market_sig = u64::MAX;
        self.last_source_market_sig = u64::MAX;
        self.mark_view_dirty();
    }

    /// Draw a frozen replay instead of the live market source, or go back to the live one.
    ///
    /// Every pane's `resident_left_rel` is reset to NaN, which is the SAME mechanism a device loss
    /// and a source-generation change already use to force a full history re-read on the next
    /// frame. Without it the pane would keep its previous coverage mark, decide nothing had
    /// changed, and never read the series that just arrived — which is exactly what an
    /// asynchronously fetched replay does: the window opens empty and the rows land seconds later.
    ///
    /// Args:
    ///     series: The frozen series to draw, or `None` to return this engine to the live source.
    pub(crate) fn set_trade_replay(
        &mut self,
        series: Option<std::rc::Rc<moon_core::market::trade_replay::TradeReplaySeries>>,
    ) {
        self.trade_replay = series;
        let mut render = self.render.borrow_mut();
        for pane in &mut render.panes {
            pane.resident_left_rel = f32::NAN;
            pane.gpu_prepare_dirty = true;
        }
        render.needs_present = true;
        drop(render);
        self.view_dirty = true;
    }

    /// Hand a frozen viewer the archived order lines it draws, or take them away.
    ///
    /// The order sync is gated on a signature that folds in this store's revision, so the next
    /// pass rebuilds the geometry; nothing else is reset because, unlike a replay, an order store
    /// touches no resident history window.
    ///
    /// Args:
    ///     store: The archived lines, or `None` to draw no orders at all.
    ///     fit_range: The price band the auto-Y fit must include, or `None` for the prices alone.
    pub(crate) fn set_frozen_orders(
        &mut self,
        store: Option<std::rc::Rc<moon_core::session::order_lines::OrderLineStore>>,
        fit_range: Option<(f32, f32)>,
    ) {
        self.frozen_orders = store;
        self.frozen_fit_range = fit_range;
        self.view_dirty = true;
    }

    /// Hand a frozen viewer what it draws beside its store, or take it away.
    ///
    /// Stamps a fresh revision, which both the order signature and the per-pane rebuild gate
    /// read, so the next sync rebuilds the geometry.
    ///
    /// Args:
    ///     overlay: The corridor and modelled trades, or `None` to draw neither.
    pub(crate) fn set_frozen_overlay(
        &mut self,
        overlay: Option<std::rc::Rc<moon_chart::frozen_overlay::FrozenOverlay>>,
    ) {
        self.frozen_overlay = overlay.filter(|o| !o.is_empty());
        self.frozen_overlay_rev = self.frozen_overlay_rev.wrapping_add(1);
        self.view_dirty = true;
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
        sig = mix_sig(sig, str_sig(market));
        if let Some(revs) = source.market_revisions(core, market) {
            // Every revision this market has, including the chart archive. The inner gate in
            // `market.rs` deliberately mixes a SUBSET by hand; this one wants the lot, so it asks
            // for the lot rather than re-listing the fields and drifting from them.
            sig = mix_sig(sig, revs.combined_signature());
        }
        // Only this chart's own cache replies wake it.
        let cache_done = self.render.borrow().panes.iter().fold(0u64, |acc, pr| {
            acc.wrapping_add(pr.history_cursor.cache_completions())
        });
        sig = mix_sig(sig, cache_done);
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

    /// Applies slot geometry from logical-pixel canvas bounds to the engine's size, origin, and
    /// pixel scale. `frame()` synchronously obtains it from the fork's `GpuFrameInfo`, keeping the
    /// own pass in the current slot.
    ///
    /// Two factors, on purpose. The slot is `bounds` content pixels times the frame's effective
    /// factor in device pixels, so the target and its origin use that. The chart's own geometry —
    /// line widths, candle outlines, axis gutters, captions — is sized by the platform factor
    /// alone, so that UI zoom scales the chrome around the chart while the chart keeps the
    /// monitor's density inside whatever slot remains.
    fn apply_slot_geometry(&mut self, info: &GpuFrameInfo) {
        if info.bounds.is_empty() {
            return;
        }
        let sf = info.scale_factor.max(0.1);
        let zoom = info.content_zoom.max(0.1);
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
        let ppp = (sf / zoom).max(0.1);
        self.last_ppp = ppp;
        self.content_zoom = zoom;
        self.slot_bounds = Some(info.bounds);
        let mut st = self.render.borrow_mut();
        st.set_slot_origin(ox, oy); // The setter dirties and presents only when the value changes.
        st.set_pixel_scale(ppp);
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

    /// Prepare one canvas frame, or skip it before any chart work.
    ///
    /// Hidden, not-presentable, and empty-bounds frames return [`GpuFrameDecision::Skip`]
    /// before slot geometry, present-rate sampling, market sync, countdown captions, or
    /// [`RenderState::frame`]. `view_dirty` and `RenderState::needs_present` stay pending.
    /// `last_frame_tick_at` is cleared so the gap is not a cadence sample. The next eligible
    /// frame applies the current slot, then one market pull performs the only source sync.
    /// A present-rate change only marks the view dirty and rides that same pull. With no
    /// source the dirty flag stays set.
    ///
    /// Args:
    ///     info: Frame input from the GPU canvas. `info.now` is the monotonic clock for
    ///         present-rate and countdown checks.
    ///
    /// Returns:
    ///     `Skip` when this chart must not prepare, otherwise the render state's decision.
    pub(crate) fn frame(&mut self, info: GpuFrameInfo) -> GpuFrameDecision {
        if !self.scene_visible || !info.presentable || info.bounds.is_empty() {
            self.last_frame_tick_at = None;
            // Same counter `RenderState::frame` bumps for a direct caller. This path no
            // longer reaches that guard.
            if !info.presentable || info.bounds.is_empty() {
                crate::diag::bump(&crate::diag::CHART_FRAME_SKIP_NOT_PRESENTABLE);
            }
            return GpuFrameDecision::Skip;
        }
        // Geometry before pull/sync: the fork already has this frame's slot, so the own
        // pass draws it without a one- or two-frame reflow flash. A reveal applies it in
        // the same call, before anything is drawn.
        self.apply_slot_geometry(&info);
        let now = info.now;
        if self.observe_present_rate(now) {
            self.mark_view_dirty();
        }
        if self.pull_market_source_if_visible() {
            crate::diag::bump(&crate::diag::CHART_PREPARE);
        }
        self.tick_countdown_captions(now);
        self.render.borrow_mut().frame(info)
    }

    /// Advance the countdown captions when their quantized clock moves, WITHOUT a market sync.
    ///
    /// A countdown is the one caption whose value changes with nothing but the clock, and the sync
    /// that normally re-formats captions runs only when the view is dirty or a market revision
    /// moved. On a quiet market neither happens for minutes, and the countdown would sit frozen —
    /// which is the one failure a clock on screen must not have.
    ///
    /// Deliberately NOT done by marking the view dirty: that would drag a full
    /// `sync_from_market_source` — per-pane history reads, readouts, geometry — through the frame
    /// loop once a second on every chart carrying this caption, to change one string. This
    /// re-resolves the captions and asks for a present, and touches nothing else.
    ///
    /// Throttled to [`COUNTDOWN_CHECK`] because the question itself is not free: answering it reads
    /// the system clock and walks the caption configuration, and the finest quantum any countdown
    /// asks for is a second. Per vblank that would be up to 360 answers to a question that can
    /// change once. The monotonic `now` is the one the frame already took, so this costs one
    /// `Instant` comparison on the frames it skips — on every chart, not only on those that print
    /// no countdown.
    fn tick_countdown_captions(&mut self, now: Instant) {
        // Gated exactly like `pull_market_source_if_visible`: a scene nobody is looking at must not
        // reshape text once a second, and the tick below catches its captions up the moment it
        // comes back — the clock it compares against is absolute, not a step counter.
        if !self.scene_visible {
            return;
        }
        if self
            .last_countdown_check
            .is_some_and(|last| now.duration_since(last) < COUNTDOWN_CHECK)
        {
            return;
        }
        self.last_countdown_check = Some(now);
        let tf_ms = self.candle_view.tf_ms();
        let mut st = self.render.borrow_mut();
        let Some(clock) = st
            .chart_labels
            .countdown_clock_ms(tf_ms, moon_core::util::time::now_unix_ms_i64())
        else {
            return;
        };
        for idx in 0..st.panes.len() {
            // A pane the text pass skips must not be re-formatted for it, and must not raise a
            // present: `prepare_text` draws captions for ACTIVE panes only, so a vacated slot held
            // through its grace period would otherwise repaint the scene once a second for a
            // picture nobody is drawing.
            if !st.panes[idx].active || st.panes[idx].label_now_ms == clock {
                continue;
            }
            st.panes[idx].label_now_ms = clock;
            crate::diag::bump(&crate::diag::CHART_COUNTDOWN_TICK);
            if st.refresh_pane_labels(idx) {
                st.needs_present = true;
            }
        }
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
