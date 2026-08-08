//! Public `ChartEngine` handle for opening, scaling, following, pruning, pinning, layout,
//! presentation control, and panel synchronization. This implementation block was extracted from
//! `mod.rs`, where the `ChartEngine` structure remains declared and visible to this child module.

use super::*;

/// Weak handle for another engine's ghost crosshair in comparison mode. A hovered panel holds one
/// handle per peer chart in the same tab stack and writes the cursor price to each on mouse movement.
/// Each peer draws a horizontal line plus volume and percentage from its own data. Like the real
/// cursor, this bypasses GPUI notification.
#[derive(Clone)]
pub struct ChartGhostCursor {
    state: std::rc::Weak<RefCell<RenderState>>,
}

impl ChartGhostCursor {
    pub fn set_price(&self, price: Option<f64>) {
        if let Some(state) = self.state.upgrade() {
            let mut state = state.borrow_mut();
            // Receiving a price means the mouse is over another chart in the tab, so this chart
            // cannot have a real cursor. Clear a stale crosshair left when a neighbor's fast-path
            // mouse-move stop_propagation consumed panel hover-out; otherwise the ghost cannot appear
            // because the real cursor takes precedence in `render_state.rs::sync_cursor_params`. This is
            // idempotent for panels whose cursor is already None. Do not clear on price=None because
            // the mouse may have just entered this chart, where its real cursor is valid.
            if price.is_some() {
                state.set_cursor(None);
            }
            state.set_ghost_price(price.map(|p| p as f32));
        }
    }
}

fn hex3(rgb: [u8; 3]) -> u32 {
    ((rgb[0] as u32) << 16) | ((rgb[1] as u32) << 8) | rgb[2] as u32
}

fn rgba3(rgb: [u8; 3], alpha: f32) -> [f32; 4] {
    let mut out = rgb4(rgb);
    out[3] = alpha.clamp(0.0, 1.0);
    out
}

fn initial_palette_from_theme(theme: &ChartTheme) -> moon_ui::MoonPalette {
    let base = moon_ui::MoonPalette::default();
    let panel = hex3(theme.panel_bg);
    let chart_bg = hex3(theme.bg);
    let border = hex3(theme.grid);
    let accent = hex3(theme.cross);
    let green = hex3(theme.book_bid);
    let orange = hex3(theme.book_ask);
    moon_ui::MoonPalette {
        shell: panel,
        shell_high: panel,
        window: panel,
        surface: chart_bg,
        panel,
        panel_high: panel,
        chrome: panel,
        tabbar: panel,
        panel_head: panel,
        gutter: panel,
        chart_bg,
        card: panel,
        row_alt: panel,
        head_row: panel,
        border,
        border_soft: border,
        border_card: border,
        border_hover: border,
        row_line: border,
        shadow: base.shadow,
        overlay: base.overlay,
        on_accent: base.on_accent,
        text: accent,
        text_soft: border,
        text_dim: accent,
        text_muted: border,
        text_faint: border,
        table_head: panel,
        table_body: panel,
        table_selected: panel,
        green,
        green_btn: green,
        green_text: green,
        red: orange,
        red_text: orange,
        red_soft_bd: orange,
        orange,
        amber: accent,
        blue: accent,
        accent,
        accent_fg: accent,
        accent_tint_a: base.accent_tint_a,
        yellow: accent,
    }
}

impl ChartEngine {
    pub fn new(epoch: f64, theme: ChartTheme) -> Self {
        Self::new_kind(epoch, theme, ContainerKind::Main)
    }

    pub fn new_kind(epoch: f64, theme: ChartTheme, kind: ContainerKind) -> Self {
        let container = Rc::new(RefCell::new(Container::new(kind)));
        let state = Rc::new(RefCell::new(RenderState {
            panes: Vec::new(),
            needs_present: true,
            base_dirty: true,
            last_present_at: None,
            target_present_interval: Duration::from_secs_f64(1.0 / 60.0),
            camera_shift_window_start: None,
            camera_shift_count: 0,
            camera_shift_hz: 0.0,
            last_gpu_prepare_generation: 0,
            text_runs: Vec::new(),
            text_run_cursor: 0,
            firetest_text_labels: Vec::new(),
            firetest_text_runs: Vec::new(),
            firetest_text_layer: GpuCanvasRetainedTextLayer::default(),
            firetest_text_revision: 0,
            firetest_force_present: false,
            ui_palette: initial_palette_from_theme(&theme),
            slot_origin: [0.0, 0.0],
            cursor: None,
            ghost_price: None,
            compare_ref_price: None,
            arrival_pulse: None,
            arrival_pulse_color: [0.0; 4],
            last_arrival_present_at: None,
            cursor_color: {
                let mut c = rgb4(theme.cross);
                c[3] = theme.cross_alpha;
                c
            },
            cursor_thickness: theme.cross_thickness.max(1.0),
            readout_bg: rgba3(theme.bg, theme.readout_bg_alpha),
            readout_soft_bg: rgba3(theme.bg, theme.readout_soft_bg_alpha),
            readout_order_bg: rgba3(theme.bg, theme.line_label_bg_alpha),
            readout_border: rgba3(theme.bg, theme.readout_border_alpha),
            readout_border_px: theme.readout_border_px.max(0.0),
            label_positive: hex3(theme.label_positive),
            label_negative: hex3(theme.label_negative),
            label_neutral: hex3(theme.label_neutral),
            axis_label: hex3(theme.axis_label),
            caption_label: hex3(theme.caption_label),
            readout_label: hex3(theme.readout_label),
            label_font_delta: theme.label_font_delta,
            line_labels: true,
            cursor_labels: true,
            pixel_scale: 1.0,
            #[cfg(windows)]
            scissor_rs: None,
            #[cfg(windows)]
            scissor_generation: 0,
            #[cfg(windows)]
            window_bg: background::BackgroundLayer::new(background::SPLASH_PNG),
            #[cfg(windows)]
            window_bg_color: rgb4(theme.bg),
            #[cfg(windows)]
            base_cache: base::BaseCache::new(),
        }));
        let data = Rc::new(RefCell::new(ChartDataState::new(
            container.clone(),
            state.clone(),
            theme.clone(),
        )));
        let canvas = GpuCanvasHandle::new(ChartCanvasDriver {
            state: state.clone(),
            data: Rc::downgrade(&data),
        });
        Self {
            container,
            state,
            data,
            canvas,
            epoch,
            theme,
            orders: OrdersStyle::default(),
            scale: None,
            follow: true,
            present_rate_hz: 60.0,
        }
    }

    pub fn data_handle(&self) -> ChartDataHandle {
        ChartDataHandle {
            inner: Rc::downgrade(&self.data),
        }
    }

    pub fn set_market_source(&mut self, source: Option<MarketDataSource>) -> bool {
        self.data.borrow_mut().set_market_source(source)
    }

    /// Returns a normal GPUI element whose bounds, clip, and lifetime are owned by the tree. Unlike
    /// the former window-global pass, it disappears with a hidden tab and moves with `ChartPanel`
    /// when detached.
    pub fn canvas(&self) -> gpui::GpuCanvas {
        gpui::gpu_canvas(self.canvas.clone())
    }

    pub fn slot_geometry(&self) -> Option<(Bounds<Pixels>, f32, (u32, u32))> {
        let data = self.data.borrow();
        Some((data.slot_bounds?, data.last_ppp, (data.w, data.h)))
    }

    pub fn slot_dev_size(&self) -> (u32, u32) {
        let data = self.data.borrow();
        (data.w.max(1), data.h.max(1))
    }

    pub fn slot_dev_width(&self) -> f32 {
        self.data.borrow().w.max(1) as f32
    }

    pub fn chart_local_from_window_pos(
        &self,
        pos: gpui::Point<Pixels>,
    ) -> Option<((f32, f32), bool)> {
        let (bounds, sf, _) = self.slot_geometry()?;
        let lx = f32::from(pos.x) - f32::from(bounds.origin.x);
        let ly = f32::from(pos.y) - f32::from(bounds.origin.y);
        let w = f32::from(bounds.size.width);
        let h = f32::from(bounds.size.height);
        let within = lx >= 0.0 && lx <= w && ly >= 0.0 && ly <= h;
        Some(((lx * sf, ly * sf), within))
    }

    pub fn pane_rects(&self) -> Vec<(usize, Rect)> {
        let (w, h) = self.slot_dev_size();
        let area = Rect {
            x: 0.0,
            y: 0.0,
            w: w as f32,
            h: h as f32,
        };
        self.container.borrow().layout(area)
    }

    pub fn set_present_rate_hz(&mut self, hz: f32) {
        self.present_rate_hz = hz.max(1.0);
        self.data.borrow_mut().present_rate_hz = self.present_rate_hz;
        self.state
            .borrow_mut()
            .set_target_present_rate_hz(self.present_rate_hz);
    }

    pub fn set_ui_palette(&mut self, palette: moon_ui::MoonPalette) {
        let mut state = self.state.borrow_mut();
        if state.ui_palette.panel != palette.panel
            || state.ui_palette.chart_bg != palette.chart_bg
            || state.ui_palette.text_soft != palette.text_soft
            || state.ui_palette.border != palette.border
        {
            state.ui_palette = palette;
            state.needs_present = true;
        }
    }

    pub fn set_cursor(&mut self, cursor: Option<(usize, f32, f32)>) -> bool {
        self.state
            .borrow_mut()
            .set_cursor(cursor.map(|(pane, x, y)| CursorState {
                pane,
                local: [x, y],
            }))
    }

    /// Returns a weak comparison-mode ghost-crosshair handle. A sibling chart in the same tab stack
    /// writes its cursor price through this handle without GPUI notification, and the engine requests
    /// presentation. The handle is weak so peer lists do not extend the lifetime of closed charts.
    pub fn ghost_cursor(&self) -> ChartGhostCursor {
        ChartGhostCursor {
            state: Rc::downgrade(&self.state),
        }
    }

    /// Clears this engine's ghost crosshair when leaving comparison mode.
    pub fn clear_ghost_cursor(&mut self) {
        self.state.borrow_mut().set_ghost_price(None);
    }

    /// Starts the accent border flash for a chart that just appeared in a stack slot, or clears it.
    ///
    /// `accent` is the palette token; the flash never picks its own colour. The own-pass paces and
    /// expires the flash from the stamp, so this schedules no timer and requests no GPUI render.
    pub fn set_arrival_pulse(&mut self, at: Option<Instant>, accent: u32) -> bool {
        // `MoonPalette` stores colours as packed `0xRRGGBB`; the chart layers take sRGB channels.
        let color = rgb4([(accent >> 16) as u8, (accent >> 8) as u8, accent as u8]);
        self.state.borrow_mut().set_arrival_pulse(at, color)
    }

    /// Sets the comparison tab anchor's last price, used for the large delta beneath the corner
    /// label in book-only mode. None means not comparing or that this engine is the anchor. The
    /// stack supplies the value on every observation.
    pub fn set_compare_ref_price(&mut self, price: Option<f64>) -> bool {
        self.state
            .borrow_mut()
            .set_compare_ref_price(price.map(|p| p as f32))
    }

    /// Returns the first active pane's ticker as the corner caption spells it, if it has one yet.
    ///
    /// Read rather than resolved again: `data_state::market` already resolves the label through
    /// the market source, retries it when the catalog generation moves, and caches the answer,
    /// precisely because resolving takes the source lock and a snapshot and must not sit on a
    /// per-frame path. Any other surface naming the same chart takes it from here, so the two can
    /// never spell the instrument differently.
    pub fn pane_ticker(&self) -> Option<String> {
        self.state
            .borrow()
            .panes
            .iter()
            .find(|p| p.active)
            .map(|p| p.ticker.clone())
            .filter(|ticker| !ticker.is_empty())
    }

    /// Returns the first active pane's last price, which the anchor supplies for neighbor deltas.
    pub fn last_price(&self) -> Option<f64> {
        self.state
            .borrow()
            .panes
            .iter()
            .find(|p| p.active)
            .and_then(|p| p.cached_last_price)
            .map(f64::from)
    }

    /// Sync only account/order overlays that still live in `SessionManager`.
    /// Market ticks, price lines and orderbook data are pulled exclusively from
    /// `gpu_canvas.frame()` through `MarketDataSource`.
    pub fn sync_orders_if_visible(&mut self, session: &SessionManager, force: bool) -> bool {
        self.data
            .borrow_mut()
            .sync_orders_if_visible(session, force)
    }

    pub fn notify_signature(&self, session: &SessionManager) -> u64 {
        self.data.borrow().notify_signature(session)
    }

    pub fn set_scene_visible(&mut self, visible: bool) {
        self.data.borrow_mut().scene_visible = visible;
    }

    pub fn set_last_ppp(&mut self, ppp: f32) {
        let ppp = ppp.max(0.1);
        self.data.borrow_mut().last_ppp = ppp;
        self.state.borrow_mut().set_pixel_scale(ppp);
    }

    // ── Settings ported from the former chart.rs::ChartGpu ───────────────────────

    pub fn set_theme(&mut self, theme: ChartTheme) -> bool {
        if self.theme != theme {
            let mut cursor_color = rgb4(theme.cross);
            cursor_color[3] = theme.cross_alpha;
            {
                let mut st = self.state.borrow_mut();
                st.set_cursor_style(cursor_color, theme.cross_thickness);
                st.set_readout_style(
                    rgba3(theme.bg, theme.readout_bg_alpha),
                    rgba3(theme.bg, theme.readout_soft_bg_alpha),
                    rgba3(theme.bg, theme.line_label_bg_alpha),
                    rgba3(theme.bg, theme.readout_border_alpha),
                    theme.readout_border_px,
                );
                st.label_positive = hex3(theme.label_positive);
                st.label_negative = hex3(theme.label_negative);
                st.label_neutral = hex3(theme.label_neutral);
                st.axis_label = hex3(theme.axis_label);
                st.caption_label = hex3(theme.caption_label);
                st.readout_label = hex3(theme.readout_label);
                st.label_font_delta = theme.label_font_delta;
            }
            self.theme = theme;
            let mut data = self.data.borrow_mut();
            data.theme = self.theme.clone();
            data.mark_view_dirty();
            true
        } else {
            false
        }
    }

    pub fn set_orders(&mut self, orders: OrdersStyle) -> bool {
        if self.orders != orders {
            self.orders = orders;
            let mut data = self.data.borrow_mut();
            data.orders = self.orders.clone();
            data.mark_view_dirty();
            drop(data);
            for pr in &mut self.state.borrow_mut().panes {
                pr.last_order_lines_rev = u64::MAX;
            }
            true
        } else {
            false
        }
    }

    pub fn set_order_visual(
        &mut self,
        highlight: Option<(CoreId, u64)>,
        drag_preview: Option<(CoreId, u64, LineKind, f32)>,
    ) -> bool {
        self.data
            .borrow_mut()
            .set_order_visual(highlight, drag_preview)
    }

    /// Attaches the backend's shared user-figure store when the panel is created.
    pub fn set_figures_store(
        &mut self,
        store: std::rc::Rc<RefCell<moon_core::figures::FigureStore>>,
    ) {
        self.data.borrow_mut().set_figures_store(store);
    }

    /// Sets this panel's figure preview, hover, and selection state.
    pub(crate) fn set_figure_visual(&mut self, visual: super::figures_sync::FigureVisual) -> bool {
        self.data.borrow_mut().set_figure_visual(visual)
    }

    /// Sets this panel's news marks and the mark under the cursor. Returns whether anything changed,
    /// so the caller can skip the userdata resync.
    pub(crate) fn set_news_marks(
        &mut self,
        marks: std::rc::Rc<Vec<moon_chart::news_marks::NewsMark>>,
        hovered: Option<usize>,
    ) -> bool {
        self.data.borrow_mut().set_news_marks(marks, hovered)
    }

    /// Sets this panel's warning badges and the one under the cursor. Returns whether anything
    /// changed, so the caller can skip the userdata resync.
    pub(crate) fn set_warn_marks(
        &mut self,
        marks: std::rc::Rc<Vec<moon_chart::news_marks::NewsMark>>,
        hovered: Option<usize>,
    ) -> bool {
        self.data.borrow_mut().set_warn_marks(marks, hovered)
    }

    /// Applies a price-axis scale to every pane and stores it in the container. None selects Auto.
    pub fn set_scale(&mut self, pct: Option<f32>) -> bool {
        if self.scale == pct {
            return false;
        }
        self.scale = pct;
        self.container.borrow_mut().set_scale(pct);
        self.data.borrow_mut().mark_view_dirty();
        true
    }

    /// Returns the first pane's current Y window `(center, range)` as the comparison-mode anchor.
    pub fn y_window(&self) -> Option<(f32, f32)> {
        self.container
            .borrow()
            .panes()
            .first()
            .map(|p| p.view.y_window())
    }

    /// Forces the anchor-locked comparison Y window onto every engine pane. Returns true on change.
    pub fn set_locked_y(&mut self, center: f32, range: f32) -> bool {
        let mut changed = false;
        for p in self.container.borrow_mut().panes_mut() {
            changed |= p.view.set_y_window(center, range);
        }
        if changed {
            self.data.borrow_mut().mark_view_dirty();
        }
        changed
    }

    /// Reapplies the tab scale after leaving comparison lock, bypassing the unchanged `self.scale`
    /// cache that would make a normal `set_scale` call a no-op. None selects Auto.
    pub fn reapply_scale(&mut self, pct: Option<f32>) {
        self.scale = pct;
        self.container.borrow_mut().set_scale(pct);
        self.data.borrow_mut().mark_view_dirty();
    }

    /// Enables or disables the per-window order book for every engine pane. Returns true on change.
    pub fn set_orderbook_enabled(&mut self, enabled: bool) -> bool {
        let mut data = self.data.borrow_mut();
        if data.orderbook_enabled == enabled {
            return false;
        }
        data.orderbook_enabled = enabled;
        data.mark_view_dirty();
        true
    }

    /// Stores the X scale for new panes in pixels per millisecond. None uses the built-in default.
    pub fn set_default_x_ppm(&mut self, ppm: Option<f32>) {
        self.data.borrow_mut().default_x_ppm = ppm;
    }

    /// Returns the X scale in pixels per millisecond for pane `idx`, or the first pane as fallback.
    pub fn pane_x_ppm(&self, idx: Option<usize>) -> Option<f32> {
        let container = self.container.borrow();
        let panes = container.panes();
        let pane = idx.and_then(|i| panes.get(i)).or_else(|| panes.first())?;
        Some(pane.view.px_per_ms)
    }

    /// Forces the X scale onto every engine pane for Shift+middle-click synchronization. Returns
    /// true on change.
    pub fn set_x_ppm_all(&mut self, ppm: f32, now_ms: f64) -> bool {
        let mut changed = false;
        for p in self.container.borrow_mut().panes_mut() {
            changed |= p.view.set_px_per_ms_sync(ppm, now_ms);
        }
        if changed {
            self.data.borrow_mut().mark_view_dirty();
        }
        changed
    }

    /// Applies global candle and trade display settings, including timeframe, mode, zone, and
    /// outline, to every engine pane. Returns true on change and forces history resynchronization.
    pub fn set_candle_view(&mut self, cfg: moon_core::market::CandleViewCfg) -> bool {
        let mut data = self.data.borrow_mut();
        if data.candle_view == cfg {
            return false;
        }
        data.candle_view = cfg;
        data.mark_view_dirty();
        true
    }

    /// Enables or disables liquidation trades for every pane in this window. Returns true on change.
    pub fn set_liquidations_enabled(&mut self, enabled: bool) -> bool {
        let mut data = self.data.borrow_mut();
        if data.liquidations_enabled == enabled {
            return false;
        }
        data.liquidations_enabled = enabled;
        data.mark_view_dirty();
        true
    }

    /// Sets comparison book-only mode, hiding the plot and price axis while expanding the order book
    /// to the full width. Returns true on change.
    pub fn set_orderbook_only(&mut self, only: bool) -> bool {
        let mut data = self.data.borrow_mut();
        if data.orderbook_only == only {
            return false;
        }
        data.orderbook_only = only;
        data.mark_view_dirty();
        true
    }

    /// Sets the per-window price-axis position for every engine pane. Returns true on change.
    pub fn set_price_axis_pos(
        &mut self,
        pos: crate::persistence::chart_persist::PriceAxisPos,
    ) -> bool {
        let mut data = self.data.borrow_mut();
        if data.price_axis_pos == pos {
            return false;
        }
        data.price_axis_pos = pos;
        data.mark_view_dirty();
        true
    }

    /// Sets per-window time-axis visibility for every engine pane. Returns true on change.
    pub fn set_time_axis_visible(&mut self, visible: bool) -> bool {
        let mut data = self.data.borrow_mut();
        if data.time_axis_visible == visible {
            return false;
        }
        data.time_axis_visible = visible;
        data.mark_view_dirty();
        true
    }

    /// Toggles per-tab order-line labels. Returns true on change.
    pub fn set_line_labels(&mut self, show: bool) -> bool {
        let mut st = self.state.borrow_mut();
        if st.line_labels == show {
            return false;
        }
        st.line_labels = show;
        st.needs_present = true;
        true
    }

    /// Toggles crosshair cursor readout labels. Returns true on change.
    pub fn set_cursor_labels(&mut self, show: bool) -> bool {
        let mut st = self.state.borrow_mut();
        if st.cursor_labels == show {
            return false;
        }
        st.cursor_labels = show;
        st.needs_present = true;
        true
    }

    /// Sets the projected manual-order size from s1-s6 in USD for the cursor crosshair label.
    /// Returns true when the change exceeds the anti-jitter threshold. None means no size or rate.
    pub fn set_prospective_usd(&mut self, usd: Option<f64>) -> bool {
        let mut data = self.data.borrow_mut();
        let changed = match (data.prospective_usd, usd) {
            (Some(a), Some(b)) => (a - b).abs() > a.abs().max(1.0) * 1e-3,
            (None, None) => false,
            _ => true,
        };
        if changed {
            data.prospective_usd = usd;
            data.mark_view_dirty();
        }
        changed
    }

    /// Applies the toolbar's global Live/Pause follow state to this `ChartEngine`'s single pane.
    /// Only an explicit change to the global flag has an effect. Per-pane pan and rejoin state lives
    /// in `view.follow`; `sync_follow_from_views` supplies the already consolidated value here.
    pub fn set_follow(&mut self, follow: bool, now_ms: f64) -> bool {
        if self.follow == follow {
            return false;
        }
        self.follow = follow;
        self.data.borrow_mut().follow = follow;
        for p in self.container.borrow_mut().panes_mut() {
            if follow {
                // Explicit toolbar Live resumes only panes that were not following. Leave already
                // live panes untouched so their window and zoom are not reset.
                if !p.view.follow {
                    p.view.resume_live(now_ms);
                    p.view.reset_default_window_on_next_prepare();
                }
            } else {
                // An explicit Live-button disable does not automatically rejoin on a timer.
                p.view.set_manual_persistent();
            }
        }
        self.data.borrow_mut().mark_view_dirty();
        true
    }

    pub fn follow(&self) -> bool {
        self.follow
    }

    /// Returns the nearest pane deadline for automatically rejoining live, used to arm the timer.
    pub fn next_auto_live_deadline_ms(&self) -> Option<f64> {
        self.container
            .borrow()
            .panes()
            .iter()
            .filter_map(|p| p.view.auto_live_deadline_ms())
            .reduce(f64::min)
    }

    /// Processes automatic live rejoin by anchoring panes whose manual hold expired to now. Returns
    /// true if any pane resumed live and therefore requires a frame and notification.
    pub fn tick_auto_live(&mut self, now_ms: f64) -> bool {
        let mut resumed = false;
        for p in self.container.borrow_mut().panes_mut() {
            resumed |= p.view.tick_auto_live(now_ms);
        }
        if resumed {
            self.data.borrow_mut().mark_view_dirty();
            self.sync_follow_from_views();
        }
        resumed
    }

    pub fn sync_follow_from_views(&mut self) -> bool {
        let container = self.container.borrow();
        let follow = if container.is_empty() {
            self.follow
        } else {
            container.panes().iter().all(|p| p.view.follow)
        };
        drop(container);
        if self.follow == follow {
            false
        } else {
            self.follow = follow;
            self.data.borrow_mut().follow = follow;
            true
        }
    }

    /// Opens a market in the full-screen pane.
    pub fn open(&mut self, core: CoreId, market: &str) {
        self.container
            .borrow_mut()
            .open_manual(core, market, self.epoch);
        self.data.borrow_mut().mark_view_dirty();
    }

    /// Opens or extends an AddToChart market in this pane with a TTL.
    pub fn push_auto(&mut self, core: CoreId, market: &str, ttl_ms: f64, now_ms: f64) {
        self.container
            .borrow_mut()
            .push_auto(core, market, now_ms, ttl_ms, self.epoch);
        self.data.borrow_mut().mark_view_dirty();
    }

    /// Removes expired AddToChart panes and returns their markets.
    pub fn prune_ttl(&mut self, now_ms: f64) -> Vec<(CoreId, String)> {
        let removed = self.container.borrow_mut().prune_ttl(now_ms);
        if !removed.is_empty() {
            self.data.borrow_mut().mark_view_dirty();
        }
        removed
    }

    #[allow(dead_code)]
    pub fn has_ttl_panes(&self) -> bool {
        self.container.borrow().has_ttl_panes()
    }

    pub fn next_ttl_deadline_ms(&self) -> Option<f64> {
        self.container.borrow().next_ttl_deadline_ms()
    }

    pub fn with_container_mut<R>(&mut self, f: impl FnOnce(&mut Container) -> R) -> R {
        let out = f(&mut self.container.borrow_mut());
        self.data.borrow_mut().mark_view_dirty();
        out
    }

    pub fn with_container<R>(&self, f: impl FnOnce(&Container) -> R) -> R {
        f(&self.container.borrow())
    }

    pub fn remove_pane(&mut self, idx: usize) -> Option<(CoreId, String)> {
        let removed = self.container.borrow_mut().remove_pane(idx);
        if removed.is_some() {
            self.data.borrow_mut().mark_view_dirty();
        }
        removed
    }

    pub fn uses_market(&self, core: CoreId, market: &str) -> bool {
        self.container.borrow().uses_market(core, market)
    }

    /// Returns whether pane `idx` can be pinned; only AddToChart panes with a TTL qualify.
    pub fn pane_is_pinnable(&self, idx: usize) -> bool {
        self.container.borrow().is_pinnable(idx)
    }

    pub fn pane_pinned(&self, idx: usize) -> bool {
        self.container.borrow().is_pinned(idx)
    }

    /// Toggles pinning for pane `idx`, disabling or restoring TTL auto-close. Returns true on change.
    pub fn toggle_pane_pin(&mut self, idx: usize) -> bool {
        let changed = self.container.borrow_mut().toggle_pin(idx).is_some();
        if changed {
            self.data.borrow_mut().mark_view_dirty();
        }
        changed
    }

    pub fn clear_panes(&mut self) -> Vec<(CoreId, String)> {
        let removed = self.container.borrow_mut().clear_panes();
        if !removed.is_empty() {
            self.data.borrow_mut().mark_view_dirty();
        }
        removed
    }

    /// Returns the active full-screen or first pane's core and market.
    pub fn active_target(&self) -> Option<(CoreId, String)> {
        let container = self.container.borrow();
        container.pane(0).map(|p| (p.core, p.market.clone()))
    }

    /// Returns a pane's core and market by index for chart overlay actions such as Panic Sell and
    /// Cancel Buy that are bound to a specific slot.
    pub fn pane_target(&self, idx: usize) -> Option<(CoreId, String)> {
        self.container
            .borrow()
            .pane(idx)
            .map(|p| (p.core, p.market.clone()))
    }

    /// Returns the active full-screen or first pane's market for the tab label.
    pub fn active_market(&self) -> Option<String> {
        self.active_target().map(|(_, market)| market)
    }

    /// Force the next prepare to rebuild resident GPU history from MoonProto.
    /// Does not change the visible time window: viewport scale and retained
    /// data capacity are independent.
    pub fn force_history_reupload(&mut self) {
        let mut st = self.state.borrow_mut();
        for pr in &mut st.panes {
            pr.history_cursor.reset();
            pr.resident_left_rel = f32::NAN;
            pr.cached_tick_price = None;
            pr.scan_cam_px = i64::MIN;
            pr.gpu_prepare_dirty = true;
        }
        st.needs_present = true;
        st.base_dirty = true;
        self.data.borrow_mut().mark_view_dirty();
    }

    /// Invalidate retained axis and readout text after the selected display zone changes.
    ///
    /// Returns:
    ///     Nothing; the next frame rebuilds display-time text from retained data.
    pub fn invalidate_display_time(&mut self) {
        self.data.borrow_mut().mark_view_dirty();
    }

    pub fn pane_count(&self) -> usize {
        self.container.borrow().pane_count()
    }

    /// Return axis snapshots for visible panes after preparation.
    ///
    /// Returns:
    ///     Visible panes as `(index, device-pixel rectangle, snapshot)` tuples.
    pub fn axis_panes(&self) -> Vec<(usize, Rect, AxisSnapshot)> {
        let container = self.container.borrow();
        container
            .layout({
                let data = self.data.borrow();
                Rect {
                    x: 0.0,
                    y: 0.0,
                    w: data.w.max(1) as f32,
                    h: data.h.max(1) as f32,
                }
            })
            .into_iter()
            .filter_map(|(idx, rect)| {
                let v = &container.pane(idx)?.view;
                Some((
                    idx,
                    rect,
                    AxisSnapshot {
                        px_per_ms: v.px_per_ms,
                        right_margin_frac: v.right_margin_frac,
                        render_center: v.render_center,
                        render_range: v.render_range,
                        epoch_ms: v.epoch_ms,
                        right_time_ms: v.right_time_ms,
                    },
                ))
            })
            .collect()
    }
}
