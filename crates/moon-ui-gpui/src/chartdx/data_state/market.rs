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

/// Wall-clock span an opened chart asks back for, before the bar band clamps it.
const HISTORY_FLOOR_SPAN_MS: f64 = 2.0 * 86_400_000.0;
/// Fewest base candles the floor may resolve to. Keeps the coarse timeframes honest: two days is
/// two candles on a daily chart, which is the case where the symptom is worst.
const HISTORY_FLOOR_MIN_BARS: f64 = 120.0;
/// Most base candles the floor may resolve to. Keeps a 1-minute chart from rebuilding a
/// multi-thousand-bar series every time a bucket rolls over.
const HISTORY_FLOOR_MAX_BARS: f64 = 1500.0;

/// Minimum history span an open chart requests, regardless of camera zoom.
///
/// A chart opened at a few hours of zoom used to ask for only those few hours, so it started near
/// now and showed nothing of the history the local kline cache and the core's retained rings were
/// already holding; the user had to pan left before anything was fetched. The floor is really a BAR
/// COUNT — two days is only the tie-breaker inside the band — because a wall-clock span alone is
/// meaningless at both ends of the timeframe range.
///
/// It costs no exchange API weight. `from_rel_ms` reaches only the in-memory trade and candle
/// rings, the local SQLite prefix read and the series clip; every outbound `request_coin_card` is
/// gated on staleness, the effective-kind change, the per-panel backoff and the 30-second global
/// dedup, none of which reads it. A wider ask is the same one request.
///
/// Zero in pure tick mode, where there is no series to fill.
fn chart_history_floor_ms(cfg: moon_core::market::CandleViewCfg) -> f32 {
    if candles_disabled() || cfg.mode == moon_core::market::candles::CANDLE_MODE_OFF {
        return 0.0;
    }
    let tf = cfg.tf_ms() as f64;
    HISTORY_FLOOR_SPAN_MS
        .max(HISTORY_FLOOR_MIN_BARS * tf)
        .min(HISTORY_FLOOR_MAX_BARS * tf) as f32
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
        // Sell-line depth labels live in the TEXT layer, so a re-measure needs a present but not
        // the base re-bake `pixels_changed` promotes to.
        let mut text_changed = false;
        // Read the two theme-driven uniform fields once for the whole sync: `view_gpu` is called
        // for the plot AND the order-book glass, and the cull margin has to be computed from the
        // same marker scale the shader will use, or trades disappear a frame before their glyph
        // reaches the edge.
        let theme_marker_scale = self.theme.marker_scale;
        let view_style = view::ViewStyle {
            marker_scale: theme_marker_scale,
            volume_alpha: self.theme.trade_volume_alpha,
        };
        // Price-line colours and thickness. `price_line_px` is LOGICAL, and the shaders offset by
        // a HALF width either side of the centre line, so the device scale and the halving both
        // happen exactly once, here. Hoisted out of the per-pane loop below for the same reason
        // `view_style` is: every input is theme- or DPI-derived, so a multi-pane chart would
        // otherwise rebuild an identical struct once per pane per sync.
        let next_price_style = PriceStyleGpu {
            last: rgba3(self.theme.price_line, self.theme.price_line_alpha),
            mark: rgba3(self.theme.mark_line, self.theme.mark_line_alpha),
            m: [
                (self.theme.price_line_px * self.last_ppp * 0.5).max(0.25),
                0.0,
                0.0,
                0.0,
            ],
        };
        // Likewise theme-only: the band's style id is clamped once, not per pane. The band's
        // remaining fields stay in the loop because they fold in per-pane `volume_stats`.
        let volume_style_id = self
            .theme
            .candle_volume_style
            .min(moon_core::market::candles::VOLUME_STYLE_MAX);
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
            let (axis_pos, price_axis_w, glass_w, chart_w) = horizontal_chart_layout(
                rect.w,
                self.orderbook_only,
                self.orderbook_enabled,
                self.price_axis_pos,
                self.last_ppp,
            );
            // A hidden time axis reserves no label gutter, allowing the plot to use the full height.
            let time_axis_h = if self.time_axis_visible {
                moon_chart::TIME_AXIS_H * self.last_ppp
            } else {
                0.0
            };
            let plot_h = (rect.h - time_axis_h).max(1.0);
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
            // Prepare is the only place that knows the anchor, the scale AND the width at once, so
            // the future ceiling is re-applied here rather than in each mutator that can break it.
            // Not while the pane shows only its order book: `chart_w` is floored at 1 px there, and
            // a ceiling computed from a one-pixel window would drag a view parked six hours ahead
            // down to six seconds ahead and lose the drawing position on a mode toggle.
            if !pr.orderbook_only {
                pane.view.clamp_future_anchor(now, chart_area.w);
            }
            pane.view.follow_edge(now, now);
            let (view_time0, window_ms) = pane.view.visible_x(chart_area.w);
            let cam_px = ((pane.view.right_time_ms - pane.view.epoch_ms)
                * pane.view.px_per_ms.max(1e-9) as f64)
                .round() as i64;
            let marker_margin =
                view::cross_cull_margin_physical_px(&pane.view, self.last_ppp, theme_marker_scale)
                    / pane.view.px_per_ms.max(moon_chart::view::MIN_PX_PER_MS);
            let history_prefetch = (window_ms * 0.20).max(marker_margin);
            // The LEFT edge gets a floor; the right edge and the prefetch itself do not. Widening
            // the prefetch would also widen `pan_budget_px` below, which is a different decision
            // about when to reset; the right edge keeps its existing normal prefetch.
            let history_from =
                view_time0 - history_prefetch.max(chart_history_floor_ms(self.candle_view));
            let history_to = view_time0 + window_ms + history_prefetch;
            let scan_price = device_lost || cam_px != pr.scan_cam_px;
            let source_revs = source.market_revisions(pane.core, &pane.market);
            // The corner caption's ticker, resolved HERE rather than while drawing: the draw runs
            // per frame and this takes the source lock and a snapshot.
            //
            // The retry key mixes ONLY provider, generation and meta — deliberately not
            // `combined_signature()`, which also folds `history` and `book`. Those bump on every
            // trade and every book tick, so keying on them would re-resolve the label on every
            // sync of a live market: the source lock and a snapshot clone back in the hot loop,
            // which is what moving this out of the draw was for. `meta` alone is not enough
            // either: its counters are per provider and `set_provider_map` drops them wholesale,
            // so a provider election could hand back the number the pane already cached.
            let catalog_key = source_revs.map(|revs| {
                let mut key = mix_sig(0xcbf29ce4_84222325u64, revs.provider);
                key = mix_sig(key, revs.generation);
                mix_sig(key, revs.meta)
            });
            if !pr.ticker_resolved || catalog_key.is_some_and(|key| pr.ticker_catalog_key != key) {
                // No provider yet: read what the NAME supports so the caption is never blank, and
                // stay unresolved so the catalog still gets its turn.
                let ticker = match catalog_key {
                    Some(key) => {
                        pr.ticker_catalog_key = key;
                        pr.ticker_resolved = true;
                        source.market_label(pane.core, &pane.market).pair()
                    }
                    None => MarketLabel::from_name(&pane.market, Exchange::Unknown).pair(),
                };
                if pr.ticker != ticker {
                    pr.ticker = ticker;
                    // The caption is part of the frame, so a corrected ticker has to reach one:
                    // without this it waits for an unrelated repaint, which on a quiet market can
                    // be a long time.
                    pixels_changed = true;
                }
            }
            let source_generation = source_revs.map(|revs| revs.generation).unwrap_or(0);
            let source_generation_changed = source_generation != pr.source_generation;
            // The core's chart archive was merged, prepending history OLDER than every cursor this
            // pane holds. A wake is not enough: an incremental drain starts at the cursor and can
            // never reach behind it, so this forces a full window re-read exactly once per archive.
            let source_archive = source_revs.map(|revs| revs.archive).unwrap_or(0);
            let source_archive_changed = source_archive != pr.source_archive;
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
            // Only the fields the read actually consumes may buy a reset; the popup's style
            // checkboxes and the MoonShot corridor must not. See `CandleViewCfg::history_inputs`,
            // which owns that policy next to the struct it governs. The pane stores the already
            // neutralized value, so the comparison costs nothing extra per sync.
            let candle_history_inputs = candle_cfg.history_inputs();
            let candle_cfg_changed = pr.applied_candle_cfg != candle_history_inputs;
            pr.applied_candle_cfg = candle_history_inputs;
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
            // A pane panned off the live edge needs its coverage re-established, and only a reset
            // does it. Three invariants ride on it, and none of them survives dropping it:
            //   * the trade ring is left with a HOLE. A reset copies `[from, to]` and then parks
            //     the cursor at `cursor_from_now()`, so rows between the window's right edge and
            //     now reach neither path. While following the two coincide; a pane in the past
            //     carries a hole exactly as wide as it scrolled, and panning back sweeps it.
            //   * a candle-series rebuild re-clips the series to the then-current window, so its
            //     left edge can move RIGHT of `resident_left_rel` between resets.
            //   * a pane parked in the past keeps appending live trades into a fixed-capacity ring,
            //     evicting the historical crosses it is displaying.
            // What it does NOT need is a reset per camera pixel, which is what a drag used to cost:
            // ~100 a second, each re-copying the window, rebuilding the series, re-draining both
            // price lines and re-uploading the whole combo ring. Every read already fetches
            // `history_prefetch` beyond both edges, so the pane stays covered until the camera has
            // panned further than that slack — which is exactly the condition below, in the units
            // the invariant is written in. `history_from` cannot express it: as an f32 offset from
            // the process epoch its ULP reaches ~8 ms within a day, so a one-pixel pan need not
            // change the value at all. `cam_px` is exact, and it also folds in zoom, which moves
            // the camera without moving time.
            // Spend the prefetch, but not the marker margin inside it: a cross whose glyph straddles
            // the visible edge has to stay in the buffer, so the budget stops one margin short. A
            // pane too narrow for any slack (an order-book-only pane floors `chart_w` at 1 px) gets
            // a zero budget and the old per-pixel behaviour, which is the correct degradation.
            let panned_off_edge = !pane.view.follow && scan_price;
            let pan_budget_px = (history_prefetch - marker_margin).max(0.0) as f64
                * pane.view.px_per_ms.max(1e-9) as f64;
            let pan_reset_due = panned_off_edge
                && (cam_px.saturating_sub(pr.pan_reset_cam_px).unsigned_abs() as f64)
                    >= pan_budget_px;
            let force_history_reset = device_lost
                || source_generation_changed
                || source_archive_changed
                || liq_toggle_changed
                || candle_cfg_changed
                || zone_bucket_changed
                || pr.resident_left_rel.is_nan()
                // Coverage runs out when the VISIBLE left edge leaves the fetched range, not when
                // the requested one moves at all. `resident_left_rel` is stamped to `history_from`,
                // which already carries a whole prefetch, so comparing `history_from` against it —
                // as this did — fired on every single pixel of a pan into the past and left that
                // drag direction paying the full per-pixel reset. The margin keeps the glyph
                // overhang, exactly as in the pan budget above.
                || view_time0 - marker_margin < pr.resident_left_rel
                || pan_reset_due;
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
                let hide_rel = (hide_open - pane.view.epoch_ms) as f32;
                // Never blank a bucket that has no trades to show instead. The setting means "draw
                // ticks here rather than candles", so where there are no ticks it has to mean
                // nothing at all — otherwise a freshly opened market, whose trade ring the core has
                // not streamed yet, renders the whole zone as empty space. NaN means no crosses are
                // resident at all, which suppresses the zone entirely rather than clamping it.
                if pr.combo_left_rel.is_nan() {
                    f32::MAX
                } else {
                    hide_rel.max(pr.combo_left_rel)
                }
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
            // Automatic Y refits on every camera pixel, so a panning pane reads even when the pan
            // budget has not run out: the price scan keeps its own windowed buffer and follows
            // `scan_price` alone, independently of `force_reset`. Reading without resetting is what
            // makes the budget affordable — those pixels still get a fitted Y and an incremental
            // drain, they just do not re-upload the world.
            let read_history = history_source_changed || force_history_reset || panned_off_edge;
            let mut history = if read_history {
                let read_timer = crate::diag::timer();
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
                crate::diag::record_us(&crate::diag::CHART_HISTORY_READ_US, read_timer);
                if force_history_reset && let Some(started) = read_timer {
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
                // Only once the read actually HAPPENED. `market_revisions` answers from the
                // provider map alone, while the read bails when the client, snapshot or readers
                // are momentarily absent — committing there would consume the one-shot archive
                // revision without ever reading the rows it announced, and nothing re-raises it.
                if history.is_some() {
                    pr.source_archive = source_archive;
                }
            }
            let capacity_changed = history.as_ref().is_some_and(|h| {
                (h.combo_capacity > 0 && h.combo_capacity != pr.combo_cross_capacity)
                    || (h.price_line_capacity > 0
                        && h.price_line_capacity != pr.combo_price_line_capacity)
            });
            if capacity_changed && history.as_ref().is_some_and(|h| !h.combo_reset) {
                let read_timer = crate::diag::timer();
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
                crate::diag::record_us(&crate::diag::CHART_HISTORY_READ_US, read_timer);
                if let Some(started) = read_timer {
                    crate::diag::bump_by(
                        &crate::diag::CHART_HISTORY_RESET_MS,
                        started.elapsed().as_millis().max(1) as u64,
                    );
                }
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
                    // Where the trade crosses ACTUALLY begin, as opposed to where the read asked
                    // them to. A freshly connected market has no trade history at all until the
                    // core streams some, and the hide-candles zone below refuses to blank a bucket
                    // that has no crosses to replace it. Stamped only on a full range read, since
                    // an incremental drain returns just the live edge.
                    pr.combo_left_rel = history
                        .combo_left_rel_ms
                        .map(|v| v as f32)
                        .unwrap_or(f32::NAN);
                    // Restart the pan budget from the reset that ACTUALLY happened, whatever raised
                    // it. Stamping back where the decision was made would also credit a frame whose
                    // read returned nothing, and would miss the capacity-driven re-read that resets
                    // without the pane having asked for it.
                    pr.pan_reset_cam_px = cam_px;
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
                    // Retain a compact copy for the bottom band's visible-range statistics.
                    // `history_buffers` is cleared on entry to every read and refilled only when
                    // the series revision moved, so this block is the only place it is populated -
                    // and a plain pan, which must rescale the band, does not reach it.
                    moon_chart::collect_samples(
                        &pr.history_buffers.candles,
                        &pr.history_buffers.candle_tf_ms,
                        candle_tf_ms as f64,
                        &mut pr.volume_samples,
                    );
                    pr.layers.set_candles(std::mem::take(&mut pr.candle_upload));
                    pr.last_candle_rev = history.candles_revision;
                    pr.gpu_prepare_dirty = true;
                    pixels_changed = true;
                }
                if history.price_lines_changed || history.combo_reset {
                    // Each price line follows its OWN toggle, so one can be drawn without the
                    // other. Flipping either forces a history reset through candle_cfg_changed and
                    // reaches this branch; a disabled line uploads an empty buffer rather than a
                    // stale one, because the layer keeps whatever it was last given.
                    let last_points: &[_] = if candle_cfg.last_price_line {
                        &pr.history_buffers.last_points
                    } else {
                        &[]
                    };
                    fill_price_upload(last_points, pane.view.epoch_ms, &mut pr.last_line_upload);
                    let mark_points: &[_] = if candle_cfg.mark_price_line {
                        &pr.history_buffers.mark_points
                    } else {
                        &[]
                    };
                    fill_price_upload(mark_points, pane.view.epoch_ms, &mut pr.mark_line_upload);
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
                    pr.layers.set_candles(Vec::new());
                    pr.last_candle_rev = u64::MAX;
                    pr.history_cursor.reset();
                    pr.resident_left_rel = f32::NAN;
                    pr.pan_reset_cam_px = i64::MIN;
                    pr.cached_tick_price = None;
                    pr.cached_last_price = None;
                    // A different market is a different price entirely: the new one has had no data
                    // in this pane yet and must fit its reference until it does.
                    pr.saw_window_data = false;
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
            // Split by MEANING, not by convenience: ticks and order lines are drawn inside the
            // window, while the last price and the book band only have to stay on screen. Unioning
            // the two left the fit unable to tell "no data here" from "data here", which is what
            // made a view panned off the data rescale to a reference that is not in it.
            // BOUNDED, not unioned: an order carrying a distant target would otherwise drag the whole
            // pane to itself and squeeze the candles into a corner. `admit_order_band` fits the
            // candles first and admits the order span only inside a bounded expansion of them; the
            // rule and the reason it is a fraction of the CANDLE span live with the function.
            let window_data = moon_chart::view::admit_order_band(tick_price, pr.cached_order_price);
            let reference = union_range(last_price.map(|p| (p, p)), book_focus);
            // A pane that has NEVER had data of its own has no scale to keep, so it goes on fitting
            // the reference until something real arrives — otherwise a chart opened while the
            // toolbar's Live is already off, or a market with a book and no trades, would sit on the
            // constructor's zero centre forever. MONOTONE on purpose: the test cannot be "does the
            // view know a price", because the very first fit gives it one and would switch the
            // fallback off after a single frame, latching the pane onto that first hairline band.
            pr.saw_window_data |= window_data.is_some();
            let use_reference = pane.view.follow || !pr.saw_window_data;
            let visible_price = moon_chart::view::fit_band(window_data, reference, use_reference);
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
            // `view_gpu` cannot fill `pad`: the live-edge extent spans the plot AND the order-book
            // glass, which is layout it does not see. It has to be stamped HERE, before the
            // comparison — it used to be written in a separate block further down, so the field
            // ping-ponged between `view_gpu`'s 0.0 and this value on every single sync. That made
            // `pr.view != next_view` permanently true, so every active pane set `pixels_changed`
            // unconditionally and the base texture was rebuilt on every sync forever. Measured at
            // idle on one chart: 25 full base rebuilds a second with nothing on screen moving.
            let mut next_view =
                view::view_gpu(&pane.view, area_win, res, self.last_ppp, view_style);
            next_view.pad = view_time0
                + (chart_area.w + glass_w)
                    / pane.view.px_per_ms.max(moon_chart::view::MIN_PX_PER_MS);
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
            let next_orderbook_view =
                view::view_gpu(&pane.view, glass_win, res, self.last_ppp, view_style);
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
            if pr.price_style != next_price_style {
                pr.price_style = next_price_style;
                pr.layers.set_price_style(next_price_style);
                pr.gpu_prepare_dirty = true;
                pixels_changed = true;
            }
            // Bottom volume band. The window is the VISIBLE one, not the prefetched
            // `history_from`/`history_to`: those reach past both edges, and a max taken over them
            // would scale the band against candles the user cannot see.
            //
            // `view_time0` is relative to the chart epoch while `ChartCandle::t_open_ms` is
            // absolute, hence the epoch added back here.
            let vol_from = view_time0 as f64 + pane.view.epoch_ms;
            let vol_to = vol_from + window_ms as f64;
            pr.volume_stats =
                moon_chart::visible_volume_stats(&pr.volume_samples, vol_from, vol_to);
            let next_volume_style = match pr.volume_stats {
                // Nothing visible, or every visible bucket empty: draw no band rather than
                // normalise against a zero maximum.
                None => VolumeStyleGpu::default(),
                Some(stats) => VolumeStyleGpu {
                    up: rgba3(self.theme.candle_up, self.theme.candle_volume_alpha),
                    down: rgba3(self.theme.candle_down, self.theme.candle_volume_alpha),
                    scale: rgba3(
                        self.theme.candle_volume_scale,
                        self.theme.candle_volume_alpha,
                    ),
                    m: [
                        volume_style_id as f32,
                        moon_chart::volume_bars::clamp_band_fraction(
                            self.theme.candle_volume_height,
                        ),
                        // BOTH quantized, and both for the same reason: the live-edge bucket's
                        // volume grows with every print, so exact values here differ on every
                        // frame. The band lives in the CACHED base texture and this struct is the
                        // diff gate, so one raw field is enough to rebake that texture
                        // continuously while the chart sits still.
                        moon_chart::volume_bars::quantize_inv_max(1.0 / stats.max),
                        moon_chart::volume_bars::quantize_ratio(stats.avg / stats.max),
                    ],
                    // Logical pixels scaled by the device ratio exactly once, here.
                    m2: [
                        // Once the band-height cap; the height fraction alone decides that now.
                        0.0,
                        moon_chart::volume_bars::VOLUME_BAR_W_PX * self.last_ppp,
                        moon_chart::volume_bars::VOLUME_SCALE_LINE_PX * self.last_ppp,
                        0.0,
                    ],
                },
            };
            if pr.volume_style != next_volume_style {
                pr.volume_style = next_volume_style;
                pr.layers.set_volume_style(next_volume_style);
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
                pr.forget_book_figures();
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
                            // Keep a CPU copy of the visible book for cursor-volume labels, whose
                            // level always sits at the cursor and is therefore on screen anyway.
                            book.collect_visible_depth(lo, hi, &mut pr.orderbook_levels);
                            pr.last_book_rev = book_rev;
                            pr.last_book_lo = lo;
                            pr.last_book_hi = hi;
                            pr.gpu_prepare_dirty = true;
                            pixels_changed = true;
                        }
                        // The sell-line depth label reads the WHOLE book, so its figure spans
                        // price to the line and must not shrink when part of that span leaves the
                        // viewport. That also means panning cannot change it: gate the re-sum on
                        // the book revision alone. The order path asks for a re-measure of labels
                        // it rebuilt by resetting the revision to `u64::MAX`.
                        //
                        // Measuring an empty book is safe: `side_notional_toward` answers `None`
                        // for a side it does not carry, so the labels stay unmeasured instead of
                        // claiming zero glass. `MarketStore::reset` installs an empty view the
                        // moment a market opens, so `Some(book)` alone does not mean data arrived.
                        if !pr.orderbook_labels.is_empty() && pr.last_label_book_rev != book_rev {
                            refresh_orderbook_label_notionals(&mut pr.orderbook_labels, book);
                            pr.last_label_book_rev = book_rev;
                            text_changed = true;
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
                        pr.forget_book_figures();
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
            // No `orderbook_on` filter: `forget_book_figures` clears `book_best` when the book
            // goes away, so the field never outlives the book it came from.
            let book_edges = pr.book_best;
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
        if pixels_changed || cursor_changed || text_changed {
            st.needs_present = true;
        }
        drop(container);
        drop(st);
        self.last_prepared_market_sig =
            prepared_sig.unwrap_or_else(|| self.source_market_signature(source));
        self.view_dirty = false;
    }
}

#[cfg(test)]
mod tests;
