//! Canvas geometry, cursor placement, present pacing and palette updates.

use super::*;

impl ChartEngine {
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

    /// The window's content zoom at the last frame: content pixels times this are the chart's own
    /// logical pixels, which the overlays over chart geometry need to convert both ways.
    pub fn slot_content_zoom(&self) -> f32 {
        self.data.borrow().content_zoom
    }

    /// Device pixels per content pixel at the last frame: the window's effective factor, which is
    /// what a content-space position (a pointer, a GPUI element) crosses by to reach the chart's
    /// device pixels. Distinct from [`Self::slot_geometry`]'s factor, which is device pixels per
    /// chart-design pixel and excludes the UI zoom.
    pub fn slot_scale_factor(&self) -> f32 {
        let data = self.data.borrow();
        data.last_ppp * data.content_zoom
    }

    pub fn slot_dev_size(&self) -> (u32, u32) {
        let data = self.data.borrow();
        (data.w.max(1), data.h.max(1))
    }

    pub fn slot_dev_width(&self) -> f32 {
        self.data.borrow().w.max(1) as f32
    }

    /// Map a content-space window position onto the chart's device pixels, and say whether it
    /// is inside the slot.
    ///
    /// Both the position and the slot bounds are content pixels, so the crossing uses the
    /// window's effective factor ([`Self::slot_scale_factor`]); the chart-design factor would land
    /// the pointer at `1 / zoom` of where it is under UI zoom.
    pub fn chart_local_from_window_pos(
        &self,
        pos: gpui::Point<Pixels>,
    ) -> Option<((f32, f32), bool)> {
        let (bounds, _, _) = self.slot_geometry()?;
        let sf = self.slot_scale_factor();
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

    /// Publish a bootstrap present rate when the requested value changes.
    ///
    /// The panel calls this on every render. The rate stored here is that request,
    /// not the cadence learned from frame callbacks. Writing the learned rate back
    /// on each render would schedule a full source sync every time.
    ///
    /// Args:
    ///     hz: Requested frames per second. Values below 1 become 1.
    pub fn set_present_rate_hz(&mut self, hz: f32) {
        let hz = hz.max(1.0);
        if hz == self.present_rate_hz {
            return;
        }
        self.present_rate_hz = hz;
        self.data.borrow_mut().present_rate_hz = hz;
        self.state.borrow_mut().set_target_present_rate_hz(hz);
    }

    /// Uploads the live Moon palette for chart chrome that follows the UI theme.
    ///
    /// Compared on the fields the GPU text pass actually reads — panel chrome plus `accent`,
    /// which paints the Sells-to-zone cursor badge — so a theme swap that keeps the panel
    /// colours still reaches the mode marker. Returns nothing; a change dirties the present.
    pub fn set_ui_palette(&mut self, palette: moon_ui::MoonPalette) {
        let mut state = self.state.borrow_mut();
        if state.ui_palette.panel != palette.panel
            || state.ui_palette.chart_bg != palette.chart_bg
            || state.ui_palette.text_soft != palette.text_soft
            || state.ui_palette.border != palette.border
            || state.ui_palette.accent != palette.accent
        {
            state.ui_palette = palette;
            state.needs_present = true;
        }
    }

    /// Re-read what the measuring captions show after the pointer moved.
    ///
    /// Separate from [`Self::set_cursor`] because it needs the market source, which the cursor path
    /// does not otherwise touch. Cheap to call on every mouse move: it returns immediately unless a
    /// drawn caption is anchored to the pointer AND the moment under it actually changed.
    ///
    /// Returns:
    ///     Whether anything changed, so the caller can repaint only when it did.
    pub fn sync_cursor_volumes(&mut self, source: &moon_core::market::MarketDataSource) -> bool {
        let changed = self.data.borrow_mut().sync_cursor_volumes(source);
        if changed {
            self.state.borrow_mut().needs_present = true;
        }
        changed
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
    /// `accent` is the palette token; the flash never picks its own colour. `hold` keeps the border
    /// on as a steady stroke after the pulses instead of letting it expire. The own-pass paces and
    /// expires the flash from the stamp, so this schedules no timer and requests no GPUI render.
    pub fn set_arrival_pulse(&mut self, at: Option<Instant>, accent: u32, hold: bool) -> bool {
        self.state
            .borrow_mut()
            .set_arrival_pulse(at, types::accent_rgb4(accent), hold)
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

    /// Returns the first active pane's Y-scale badge, as a whole percentage of the visible range.
    ///
    /// Read rather than recomputed, for the same reason [`Self::pane_ticker`] is: the value is
    /// decided by `scale_badge_pct` and cached during `sync_from_market_source`. A second
    /// derivation elsewhere would be free to disagree, and the surface that reads this is the chart
    /// SHOT, where the symptom would be a burnt-in figure contradicting the badge visible in the
    /// very same picture.
    ///
    /// `None` means the chart is showing no badge, which is not the same fact as a zero.
    ///
    /// The CONFIGURATION is consulted before the cache, and that order is the whole point. Hiding
    /// the badge's row — or the caption itself — removes it from the chart without clearing the
    /// value cached behind it, because `sync_from_market_source` caches what the view is doing and
    /// not what the captions print. A reader that took the cache alone would burn a scale into a
    /// picture that visibly has none, which is the exact disagreement this accessor exists to make
    /// impossible.
    pub fn scale_badge(&self) -> Option<i32> {
        let state = self.state.borrow();
        if !state
            .chart_labels
            .any_drawn(|field| field == moon_core::config::ChartLabelField::ScaleBadge)
        {
            return None;
        }
        state
            .panes
            .iter()
            .find(|p| p.active)
            .and_then(|p| p.scale_badge)
    }

    /// Returns the first active pane's time-scale badge: the whole seconds the plot spans.
    ///
    /// Same contract as [`Self::scale_badge`]: read from the value the sync cached, and `None`
    /// whenever no drawn caption shows it, so the chart shot never burns in a figure the picture
    /// itself does not carry.
    pub fn time_scale_secs(&self) -> Option<i64> {
        let state = self.state.borrow();
        if !state
            .chart_labels
            .any_drawn(|field| field == moon_core::config::ChartLabelField::TimeScaleBadge)
        {
            return None;
        }
        state
            .panes
            .iter()
            .find(|p| p.active)
            .and_then(|p| p.time_scale_s)
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

    /// Record whether this chart's scene is on screen.
    ///
    /// An unchanged flag does nothing. A real change drops cadence samples so a
    /// hidden gap cannot train the present rate, including when no frame callback
    /// runs while the chart is hidden. Revealing marks the view dirty so the next
    /// frame pulls the latest source. Render dirtiness is left as it was.
    ///
    /// Args:
    ///     visible: Whether the panel is showing this engine.
    pub fn set_scene_visible(&mut self, visible: bool) {
        let mut data = self.data.borrow_mut();
        if data.scene_visible == visible {
            return;
        }
        data.scene_visible = visible;
        data.last_frame_tick_at = None;
        data.present_rate_candidate_hits = 0;
        data.present_rate_candidate_hz = 0.0;
        if visible {
            data.mark_view_dirty();
        }
    }

    /// Adopt a new device pixel scale, invalidating the userdata layer when it actually moved.
    ///
    /// Marker geometry is baked in PHYSICAL pixels, so a DPI change — dragging a window to a
    /// monitor with a different scale factor — alters every gem, arrow, badge and connector
    /// without touching a single record. `news_sig`/`trade_history_sig` already fold `last_ppp` in,
    /// but `sync_orders_if_visible` short-circuits on the ORDER signature before it ever compares
    /// them, so on an otherwise idle chart those inner signatures are never reached and the layer
    /// keeps the geometry baked at the old scale. Dropping the outer gate here is what lets them
    /// do their job. Guarded on a real change because render calls this every frame.
    ///
    /// Args:
    ///     ppp: Current device pixels per chart-design pixel (the platform factor, excluding UI zoom).
    pub fn set_last_ppp(&mut self, ppp: f32) {
        let ppp = ppp.max(0.1);
        {
            let mut data = self.data.borrow_mut();
            if data.last_ppp != ppp {
                data.last_ppp = ppp;
                data.last_order_sig = u64::MAX;
            }
        }
        self.state.borrow_mut().set_pixel_scale(ppp);
    }

    // ── Settings ported from the former chart.rs::ChartGpu ───────────────────────

    /// Apply the global width to geometry and text, waking an otherwise idle chart on change.
    pub fn set_order_book_width(&mut self, width: f32) -> bool {
        let width = moon_core::config::book_width::normalize(width);
        let mut data = self.data.borrow_mut();
        if data.order_book_width_px == width {
            return false;
        }
        data.order_book_width_px = width;
        data.render.borrow_mut().order_book_width_px = width;
        data.last_order_sig = u64::MAX;
        data.mark_view_dirty();
        true
    }

    /// Apply the active theme to the retained chart render state.
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
}
