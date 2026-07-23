//! Synchronizes market history, the order book, and automatic Y scaling.

use super::orders::refresh_orderbook_label_notionals;
use super::*;

/// Emergency candle kill switch. The presence of `MOON_CANDLES_OFF`, regardless of its value,
/// restores pure tick mode with crosses across the full window and an empty candle layer. Intended
/// for GPU/CPU A/B measurements.
fn candles_disabled() -> bool {
    static OFF: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *OFF.get_or_init(|| std::env::var_os("MOON_CANDLES_OFF").is_some())
}

impl ChartDataState {
    pub(crate) fn sync_from_market_source(
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
            // Price-axis position is per window. Order-book-only mode forcibly hides the axis,
            // overriding the per-tab setting. Hide removes the gutter and returns its space to the plot.
            let axis_pos = if self.orderbook_only {
                crate::persistence::chart_persist::PriceAxisPos::Hide
            } else {
                self.price_axis_pos
            };
            let price_axis_w = if matches!(
                axis_pos,
                crate::persistence::chart_persist::PriceAxisPos::Hide
            ) {
                0.0
            } else {
                moon_chart::PRICE_AXIS_W * self.last_ppp
            };
            // A hidden time axis reserves no label gutter, allowing the plot to use the full height.
            let time_axis_h = if self.time_axis_visible {
                moon_chart::TIME_AXIS_H * self.last_ppp
            } else {
                0.0
            };
            let plot_h = (rect.h - time_axis_h).max(1.0);
            // On a narrow plot, the order book must not consume half the width. Start from
            // GLASS_ZONE_PX, capped at half the slot. If the remaining plot would be narrower than
            // twice the base order-book width, shrink the book to 80% of the zone and return space
            // to the plot. Wide layouts keep the normal order-book width.
            let glass_cap = rect.w * 0.5;
            let glass_base = moon_chart::GLASS_ZONE_PX.min(glass_cap);
            let chart_w_base = rect.w - price_axis_w - glass_base;
            // Book-only mode uses the full width, disabled mode uses zero, otherwise adapt the zone.
            let glass_w = if self.orderbook_only {
                (rect.w - price_axis_w).max(1.0)
            } else if !self.orderbook_enabled {
                0.0
            } else if chart_w_base < glass_base * 2.0 {
                (moon_chart::GLASS_ZONE_PX * 0.8).min(glass_cap)
            } else {
                glass_base
            };
            // Left places the axis gutter on the left, shifts the plot right, and keeps the book at
            // the right edge. Right starts the plot at the left edge, then places the book and the
            // axis gutter to its right. Hide removes the axis, starts the plot at the left edge,
            // and keeps the book at the right edge.
            let axis_on_left = matches!(
                axis_pos,
                crate::persistence::chart_persist::PriceAxisPos::Left
            );
            let chart_x = if axis_on_left {
                rect.x + price_axis_w
            } else {
                rect.x
            };
            let chart_w = (rect.w - price_axis_w - glass_w).max(1.0);
            let glass_x = if matches!(
                axis_pos,
                crate::persistence::chart_persist::PriceAxisPos::Right
            ) {
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
                .ensure_default_window(chart_area.w, self.present_rate_hz, self.default_x_ppm);
            pane.view.follow_edge(now, now);
            let (view_time0, window_ms) = pane.view.visible_x(chart_area.w);
            let cam_px = ((pane.view.right_time_ms - pane.view.epoch_ms)
                * pane.view.px_per_ms.max(1e-9) as f64)
                .round() as i64;
            let marker_margin = view::cross_cull_margin_physical_px(&pane.view, self.last_ppp)
                / pane.view.px_per_ms.max(moon_chart::view::MIN_PX_PER_MS);
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
            // Changing the Liquidations toggle reuploads combo to add or remove liquidation crosses.
            let liq_toggle_changed = pr.liquidations_enabled != self.liquidations_enabled;
            pr.liquidations_enabled = self.liquidations_enabled;
            // Candle/trade-zone configuration changes, including timeframe, K, or limit, require a
            // history reset. Moving the current bucket does too because the last-K-candle zone has
            // advanced and old crosses must be removed. The bucket advances only once per timeframe,
            // measured in minutes, so resets are infrequent.
            let candle_cfg = self.candle_view;
            let candle_tf_ms = candle_cfg.tf_ms();
            let candle_cfg_changed = pr.applied_candle_cfg != candle_cfg;
            pr.applied_candle_cfg = candle_cfg;
            // Mode None is a pure tick chart: do not build or draw candles, and do not restrict
            // crosses to a trade zone. Passing params=None below keeps trades across the full window.
            let candles_off = candles_disabled()
                || candle_cfg.mode == moon_core::market::candles::CANDLE_MODE_OFF;
            let now_zone_bucket = (now / candle_tf_ms as f64).floor() as i64;
            let zone_bucket_changed = !candles_off
                && candle_cfg.trade_candles > 0
                && pr.last_zone_bucket != now_zone_bucket;
            pr.last_zone_bucket = now_zone_bucket;
            if device_lost {
                // A new device has an empty candle layer, invalidating the delivered revision.
                pr.last_candle_rev = u64::MAX;
            }
            let force_history_reset = device_lost
                || source_generation_changed
                || liq_toggle_changed
                || candle_cfg_changed
                || zone_bucket_changed
                || pr.resident_left_rel.is_nan()
                || history_from < pr.resident_left_rel
                || (!pane.view.follow && scan_price);
            // The lower displayed-trade boundary in relative milliseconds is the opening of bucket
            // N-K+1. K=0 yields infinity, suppressing all crosses and leaving only candles.
            let trades_zone_rel = if candle_cfg.trade_candles == 0 {
                f32::INFINITY
            } else {
                let zone_open = moon_core::market::candles::bucket_open_ms(now, candle_tf_ms)
                    - (candle_cfg.trade_candles as f64 - 1.0) * candle_tf_ms as f64;
                (zone_open - pane.view.epoch_ms) as f32
            };
            // The hide-candles zone makes the last N buckets trade-only. This shader boundary does
            // not alter data and moves once per bucket; the style update below picks it up on the
            // next synchronization.
            let hide_start_rel = if candle_cfg.hide_candles == 0 {
                f32::MAX
            } else {
                let hide_open = moon_core::market::candles::bucket_open_ms(now, candle_tf_ms)
                    - (candle_cfg.hide_candles as f64 - 1.0) * candle_tf_ms as f64;
                (hide_open - pane.view.epoch_ms) as f32
            };
            // Diagnose X geometry for gaps between the plot and order book after zooming out. Once
            // per second per panel, log the window, anchor, and latest data to distinguish a camera
            // whose right edge drifted from now from data whose ticks or candles legitimately end earlier.
            if chart_market_diag_enabled()
                && chart_market_diag_due(format!("xgeom:{}:{}:{}", pane.core, pane.market, idx))
            {
                let epoch = pane.view.epoch_ms;
                let last_tick_rel = pr
                    .history_buffers
                    .ticks
                    .last()
                    .map(|t| t.time_ms - epoch)
                    .unwrap_or(f64::NAN);
                let last_candle_rel = pr
                    .history_buffers
                    .candles
                    .last()
                    .map(|c| c.t_open_ms - epoch)
                    .unwrap_or(f64::NAN);
                chart_market_diag(format!(
                    "xgeom pane={} market={} now_rel={:.0} right_rel={:.0} follow={} \
                     ppm={:.6} window_ms={:.0} view_time0={:.0} chart_w={:.0} \
                     right_edge_rel={:.0} now_frac={:.2} last_tick_rel={:.0} \
                     last_candle_rel={:.0} zone_rel={:.0} hide_rel={:.0}",
                    idx,
                    pane.market,
                    now - epoch,
                    pane.view.right_time_ms - epoch,
                    pane.view.follow,
                    pane.view.px_per_ms,
                    window_ms,
                    view_time0,
                    chart_area.w,
                    view_time0 as f64 + window_ms as f64,
                    ((now - epoch) - view_time0 as f64) / window_ms.max(1.0) as f64,
                    last_tick_rel,
                    last_candle_rel,
                    trades_zone_rel,
                    hide_start_rel,
                ));
            }
            let candle_params = moon_core::market::CandleReadParams {
                tf_ms: candle_tf_ms,
                trades_from_rel_ms: trades_zone_rel,
                // The hard trade limit was removed at the user's request; ring capacity is the actual bound.
                // Keep the field in the read protocol for future use.
                trades_limit: usize::MAX,
                shipped_revision: pr.last_candle_rev,
            };
            if candles_off && pr.last_candle_rev != u64::MAX {
                pr.layers.set_candles(Vec::new());
                pr.last_candle_rev = u64::MAX;
                pr.gpu_prepare_dirty = true;
                pixels_changed = true;
            }
            let candle_params_opt = (!candles_off).then_some(&candle_params);
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
                    candle_params_opt,
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
                    candle_params_opt,
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
                // Append liquidation-trade crosses with side=2 to the same combo ring. Ring order
                // does not affect placement because the shader uses time_rel. On combo_reset the
                // source supplies the full visible range; otherwise it supplies only the new live
                // edge. The per-panel toggle suppresses appends, and changing it forces the reset
                // above to remove existing liquidation crosses.
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
                // A candle-series change from a rebuild or live trade batch fully reuploads the
                // layer's instance buffer. It contains only hundreds of rows, so this is inexpensive.
                if history.candles_changed {
                    fill_candle_upload(
                        &pr.history_buffers.candles,
                        &pr.history_buffers.candle_tf_ms,
                        pane.view.epoch_ms,
                        &mut pr.candle_upload,
                    );
                    crate::diag::bump_by(
                        &crate::diag::CHART_CANDLE_UPLOAD_LEN,
                        pr.candle_upload.len() as u64,
                    );
                    pr.layers.set_candles(std::mem::take(&mut pr.candle_upload));
                    pr.last_candle_rev = history.candles_revision;
                    pr.gpu_prepare_dirty = true;
                    pixels_changed = true;
                }
                if history.price_lines_changed || history.combo_reset {
                    // When Price Lines is disabled, omit last and mark lines. Changing the toggle
                    // forces a history reset through candle_cfg_changed and reaches this branch.
                    if candle_cfg.price_lines {
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
                    } else {
                        pr.last_line_upload.clear();
                        pr.mark_line_upload.clear();
                        pr.layers.set_price_lines(&[], &[]);
                    }
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
                    pr.layers.set_candles(Vec::new());
                    pr.last_candle_rev = u64::MAX;
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
            // Use best bid and ask as the order-book autofocus anchor. This is an O(1) read under a
            // short lock; build the full book below after the visible window is established.
            let book_top = source.with_orderbook_view(pane.core, &pane.market, |data| {
                data.and_then(|(book, _)| book.best_bid_ask())
            });
            let book_mid = book_top.map(|(bid, ask)| (bid + ask) * 0.5);
            // With no trades, center on the order book: use its midpoint as the center anchor and
            // last-price fallback, and construct a visible band guaranteed to include best bid and
            // ask for wide HIP-3 spreads. Keep it at least +/-BOOK_FOCUS_HALF to prevent excessive
            // zoom on narrow spreads. When tick_price is present, omit this band and let real ticks
            // determine the range.
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
            // Use trades, or otherwise the order-book midpoint, as the cursor label's percentage
            // reference. Without this fallback, HIP markets with a book but no trades lost the label.
            pr.cached_last_price = last_price;
            let visible_price = union_range(
                union_range(
                    union_range(tick_price, pr.cached_order_price),
                    last_price.map(|p| (p, p)),
                ),
                book_focus,
            );
            pane.view.update_y(now, plot_h, visible_price, last_price);
            // Show the current Y-scale badge beside the corner label always in Auto mode. For manual
            // drag, right-button zoom, or comparison lock, show it when the whole percentage differs
            // from the selected step.
            let next_badge = scale_badge_pct(&pane.view);
            if pr.scale_badge != next_badge {
                pr.scale_badge = next_badge;
                pixels_changed = true;
            }
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
            // Candle-layer colors come from the theme; mode, zone, and outline come from the config.
            // The relative-millisecond zone changes once per timeframe bucket, as tracked by
            // zone_bucket_changed above, and the style updates at the same time.
            let next_candle_style = CandleStyleGpu {
                up: rgb4(self.theme.candle_up),
                down: rgb4(self.theme.candle_down),
                neutral: rgb4(self.theme.candle_neutral),
                tf_rel_ms: candle_tf_ms as f32,
                zone_start_rel: if trades_zone_rel.is_finite() {
                    trades_zone_rel
                } else {
                    f32::MAX
                },
                mode: candle_cfg.mode.min(2) as f32,
                outline_px: (candle_cfg.outline_px * self.last_ppp).max(1.0),
                wicks_in_zone: candle_cfg.wicks_in_zone as u8 as f32,
                neutral_in_zone: candle_cfg.neutral_in_zone as u8 as f32,
                fill_alpha: self.theme.candle_fill_alpha.clamp(0.05, 1.0),
                hide_start_rel,
            };
            if pr.candle_style != next_candle_style {
                pr.candle_style = next_candle_style;
                pr.layers.set_candle_style(next_candle_style);
                pr.gpu_prepare_dirty = true;
                pixels_changed = true;
            }
            // Store the pane's order-book-only flag for gating the corner label in render_state/text.
            // Order-book-only mode forces the book on even when the Order Book toggle is cleared.
            pr.orderbook_only = self.orderbook_only;
            // Store the effective axis position, including forced hiding in book-only mode, for labels.
            pr.price_axis_pos = axis_pos;
            pr.time_axis_visible = self.time_axis_visible;
            pr.prospective_usd = self.prospective_usd;
            let orderbook_on = self.orderbook_enabled || self.orderbook_only;
            pr.orderbook_enabled = orderbook_on;
            // When the order book is disabled for this window, neither build nor upload levels and
            // clear any that already exist.
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
                        // Retain live book boundaries for the zone's ask/spread/bid three-color background.
                        pr.book_best = book.best_bid_ask();
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
                            // Keep a CPU copy of the visible book for cursor-volume labels and the
                            // Moonbot-style sell-line depth label.
                            book.collect_visible_depth(lo, hi, &mut pr.orderbook_levels);
                            refresh_orderbook_label_notionals(
                                &mut pr.orderbook_labels,
                                &pr.orderbook_levels,
                            );
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
                    } else {
                        pr.book_best = None;
                        if pr.last_book_rev != u64::MAX {
                            pr.layers.set_orderbook(Vec::new());
                            pr.orderbook_levels.clear();
                            pr.last_book_rev = u64::MAX;
                            pr.last_book_lo = f32::NAN;
                            pr.last_book_hi = f32::NAN;
                            pr.gpu_prepare_dirty = true;
                            pixels_changed = true;
                        }
                    }
                });
            }
            // Build the order-book style after reading the book so it carries live bid/ask boundaries
            // for the three-color background above ask, inside the spread, and below bid.
            let book_edges = pr.book_best.filter(|_| orderbook_on);
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
                bg_ask: rgb4(self.theme.book_bg_ask),
                bg_bid: rgb4(self.theme.book_bg_bid),
                edges: match book_edges {
                    Some((bid, ask)) => [ask, bid, 1.0, 0.0],
                    None => [0.0; 4],
                },
            };
            if pr.book_style != next_book_style {
                pr.book_style = next_book_style;
                pr.gpu_prepare_dirty = true;
                pixels_changed = true;
            }
            let edge_rel = view_time0
                + (chart_area.w + glass_w)
                    / pane.view.px_per_ms.max(moon_chart::view::MIN_PX_PER_MS);
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
        let cursor_changed = (st.cursor.is_some() || st.ghost_price.is_some())
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
