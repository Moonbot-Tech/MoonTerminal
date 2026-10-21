//! Main chart stack prefs operations.

use super::*;
use crate::chart_tabs::layout_popup;

impl MainChartStack {
    /// Step time zoom on every chart in this stack.
    pub(crate) fn super_zoom(&mut self, zoom_in: bool, cx: &mut Context<Self>) {
        for entry in &self.charts {
            entry.panel.update(cx, |p, pcx| p.super_zoom(zoom_in, pcx));
        }
    }

    pub(crate) fn scale(&self) -> Option<f32> {
        self.scale
    }

    pub(crate) fn set_scale(&mut self, pct: Option<f32>, cx: &mut Context<Self>) {
        apply_setting(&mut self.scale, pct, &self.charts, cx, |c, cx| {
            set_panels_scale(c, pct, cx)
        });
    }

    /// Apply a price scale as a COMMAND: reaches every panel even when the stored choice is what
    /// it already was. The dropdown and the Scale hotkeys go through here; `set_scale` stays the
    /// setter for restores, where an unchanged value really is nothing to do.
    pub(crate) fn force_scale(&mut self, pct: Option<f32>, cx: &mut Context<Self>) {
        self.scale = pct;
        force_panels_scale(&self.charts, pct, cx);
        cx.notify();
    }

    /// Moonbot's "Center chart" on every chart in this stack.
    pub(crate) fn center_on_price(&mut self, cx: &mut Context<Self>) {
        for entry in &self.charts {
            entry.panel.update(cx, |p, pcx| p.center_on_price(pcx));
        }
    }

    pub(crate) fn layout_mode(&self) -> Option<StackLayoutMode> {
        self.layout_mode
    }

    pub(crate) fn layout_height_fit(&self) -> Option<u16> {
        self.layout_height_fit
    }

    pub(crate) fn layout_height_scroll(&self) -> Option<u16> {
        self.layout_height_scroll
    }

    /// Apply a per-tab layout mode and separate Fit/Scroll heights to this stack.
    pub(crate) fn set_layout(
        &mut self,
        mode: Option<StackLayoutMode>,
        height_fit: Option<u16>,
        height_scroll: Option<u16>,
        cx: &mut Context<Self>,
    ) {
        if self.layout_mode == mode
            && self.layout_height_fit == height_fit
            && self.layout_height_scroll == height_scroll
        {
            return;
        }
        self.layout_mode = mode;
        self.layout_height_fit = height_fit;
        self.layout_height_scroll = height_scroll;
        cx.notify();
    }

    pub(crate) fn orderbook_enabled(&self) -> Option<bool> {
        self.orderbook_enabled
    }

    /// Enable or disable the order book for every chart in this stack and window.
    pub(crate) fn set_orderbook_enabled(&mut self, enabled: Option<bool>, cx: &mut Context<Self>) {
        apply_setting(
            &mut self.orderbook_enabled,
            enabled,
            &self.charts,
            cx,
            |c, cx| set_panels_orderbook_enabled(c, enabled.unwrap_or(true), cx),
        );
    }

    pub(crate) fn candle_view(&self) -> Option<moon_core::market::CandleViewCfg> {
        self.candle_view
    }

    /// Store the window X scale for new charts and, when `apply` is true, apply it to all open charts.
    /// Startup seeding stores the scale before any charts are open; Shift+middle-click synchronizes it.
    pub(crate) fn set_x_ppm(&mut self, ppm: Option<f32>, apply: bool, cx: &mut Context<Self>) {
        self.x_ppm = ppm;
        for e in &self.charts {
            e.panel.update(cx, |p, pcx| {
                p.set_default_x_ppm(ppm);
                if apply && let Some(v) = ppm {
                    p.apply_x_ppm(v, pcx);
                }
            });
        }
        if apply {
            cx.notify();
        }
    }

    /// Apply per-tab candle and trade display settings to every chart in the stack.
    pub(crate) fn set_candle_view(
        &mut self,
        cfg: Option<moon_core::market::CandleViewCfg>,
        cx: &mut Context<Self>,
    ) {
        apply_setting(&mut self.candle_view, cfg, &self.charts, cx, |c, cx| {
            set_panels_candle_view(c, cfg, cx)
        });
    }

    pub(crate) fn chart_graphics(&self) -> Option<moon_core::config::ChartGraphicsCfg> {
        self.chart_graphics
    }

    /// Take the captions a panel's right-click menu produced, for the host to persist.
    pub(crate) fn take_pending_labels(&mut self) -> Option<moon_core::config::ChartLabelsCfg> {
        self.pending_labels.take()
    }

    pub(crate) fn chart_labels(&self) -> Option<moon_core::config::ChartLabelsCfg> {
        self.chart_labels.clone()
    }

    /// Set chart captions for every chart in this stack and window.
    pub(crate) fn set_chart_labels(
        &mut self,
        cfg: Option<moon_core::config::ChartLabelsCfg>,
        cx: &mut Context<Self>,
    ) {
        apply_setting(
            &mut self.chart_labels,
            cfg.clone(),
            &self.charts,
            cx,
            |c, cx| set_panels_chart_labels(c, cfg, cx),
        );
    }

    /// Set chart-drawing settings for every chart in this stack and window.
    pub(crate) fn set_chart_graphics(
        &mut self,
        cfg: Option<moon_core::config::ChartGraphicsCfg>,
        cx: &mut Context<Self>,
    ) {
        apply_setting(&mut self.chart_graphics, cfg, &self.charts, cx, |c, cx| {
            set_panels_chart_graphics(c, cfg, cx)
        });
    }

    pub(crate) fn show_zone(&self) -> Option<bool> {
        self.show_zone
    }

    /// Enable or disable the control-zone fill for every chart in this stack and window.
    pub(crate) fn set_show_zone(&mut self, show: Option<bool>, cx: &mut Context<Self>) {
        apply_setting(&mut self.show_zone, show, &self.charts, cx, |c, cx| {
            set_panels_show_zone(c, show.unwrap_or(true), cx)
        });
    }

    pub(crate) fn auto_pin(&self) -> Option<bool> {
        self.auto_pin
    }

    pub(crate) fn price_axis_pos(&self) -> Option<PriceAxisPos> {
        self.price_axis_pos
    }

    /// Set the price-axis position for every chart in this stack and window.
    pub(crate) fn set_price_axis_pos(&mut self, pos: Option<PriceAxisPos>, cx: &mut Context<Self>) {
        apply_setting(&mut self.price_axis_pos, pos, &self.charts, cx, |c, cx| {
            set_panels_price_axis_pos(c, pos.unwrap_or_default(), cx)
        });
    }

    pub(crate) fn time_axis_visible(&self) -> Option<bool> {
        self.time_axis_visible
    }

    /// Set time-axis visibility for every chart in this stack and window.
    pub(crate) fn set_time_axis_visible(&mut self, visible: Option<bool>, cx: &mut Context<Self>) {
        apply_setting(
            &mut self.time_axis_visible,
            visible,
            &self.charts,
            cx,
            |c, cx| set_panels_time_axis_visible(c, visible.unwrap_or(true), cx),
        );
    }

    pub(crate) fn line_labels(&self) -> Option<bool> {
        self.line_labels
    }

    /// Set line-label visibility for every chart in this stack and window.
    pub(crate) fn set_line_labels(&mut self, show: Option<bool>, cx: &mut Context<Self>) {
        apply_setting(&mut self.line_labels, show, &self.charts, cx, |c, cx| {
            set_panels_line_labels(c, show.unwrap_or(true), cx)
        });
    }

    pub(crate) fn cursor_labels(&self) -> Option<bool> {
        self.cursor_labels
    }

    /// Set crosshair-label visibility for every chart in this stack and window.
    pub(crate) fn set_cursor_labels(&mut self, show: Option<bool>, cx: &mut Context<Self>) {
        apply_setting(&mut self.cursor_labels, show, &self.charts, cx, |c, cx| {
            set_panels_cursor_labels(c, show.unwrap_or(true), cx)
        });
    }

    /// Enable or disable automatic pinning on order placement for this stack and window.
    pub(crate) fn set_auto_pin(&mut self, on: Option<bool>, cx: &mut Context<Self>) {
        apply_setting(&mut self.auto_pin, on, &self.charts, cx, |c, cx| {
            set_panels_auto_pin(c, on.unwrap_or(false), cx)
        });
    }

    /// Whether comparison is in broom mode here, where the stack is one row by construction.
    pub(crate) fn compare_orderbook_only(&self) -> bool {
        self.compare_orderbook_only
    }

    pub(crate) fn layout_columns(&self) -> (Option<u8>, Option<bool>, Option<u16>) {
        (
            self.layout_columns,
            self.layout_columns_exact,
            self.layout_min_slot,
        )
    }

    /// Set the screen divider for the expanded stack; see `AddChartStack::set_layout_columns`.
    pub(crate) fn set_layout_columns(
        &mut self,
        columns: Option<u8>,
        exact: Option<bool>,
        min_slot: Option<u16>,
        cx: &mut Context<Self>,
    ) {
        let columns = columns.map(|c| c.clamp(1, grid::MAX_COLUMNS));
        let min_slot = min_slot.map(|m| m.clamp(layout_popup::MIN_H, layout_popup::MAX_H));
        if self.layout_columns == columns
            && self.layout_columns_exact == exact
            && self.layout_min_slot == min_slot
        {
            return;
        }
        self.layout_columns = columns;
        self.layout_columns_exact = exact;
        self.layout_min_slot = min_slot;
        cx.notify();
    }

    /// This stack's divider settings, as one `Copy` value the size probe can carry into paint.
    pub(super) fn grid_cfg(&self) -> grid::GridCfg {
        grid::GridCfg {
            columns: self.layout_columns,
            exact: self.layout_columns_exact,
            min_slot: self.layout_min_slot,
            broom: self.compare_orderbook_only,
            orderbook: self.orderbook_enabled.unwrap_or(true),
        }
    }

    /// How many columns the expanded stack lays out in right now; see the AddToChart twin.
    pub(super) fn effective_columns(&self, count: usize, horizontal: bool, cfg_h: f32) -> usize {
        let measured = self.measured.get();
        grid::columns_for(
            self.grid_cfg(),
            (f32::from(measured.width), f32::from(measured.height)),
            count,
            horizontal,
            cfg_h,
        )
    }

    // --- The three settings Main does not have ---
    //
    // Main draws no arrival flash — it has no `flash_arrival` — and detects never reach it: ingest
    // routes them to numbered AddToChart stacks only. `StackSetting::applies_to` keeps both values
    // away from Main, so these six exist purely to satisfy the one macro that dispatches every
    // setting to either stack type. They report "not set" and store nothing: holding a value that
    // nothing can read would be worse than not holding it.

    pub(crate) fn arrival_flash(&self) -> Option<bool> {
        None
    }

    pub(crate) fn set_arrival_flash(&mut self, _on: Option<bool>, _cx: &mut Context<Self>) {}

    pub(crate) fn arrival_frame(&self) -> (Option<bool>, Option<bool>) {
        (None, None)
    }

    pub(crate) fn set_arrival_frame(
        &mut self,
        _core_color: Option<bool>,
        _hold: Option<bool>,
        _cx: &mut Context<Self>,
    ) {
    }

    pub(crate) fn max_charts(&self) -> Option<u16> {
        None
    }

    pub(crate) fn max_charts_evict(&self) -> Option<bool> {
        None
    }

    pub(crate) fn set_max_charts(
        &mut self,
        _max: Option<u16>,
        _evict: Option<bool>,
        _cx: &mut Context<Self>,
    ) {
    }

    pub(crate) fn layout_orientation(&self) -> Option<StackOrientation> {
        self.layout_orientation
    }

    /// Change the per-window stack orientation and rebuild the current display.
    pub(crate) fn set_orientation(
        &mut self,
        orientation: Option<StackOrientation>,
        cx: &mut Context<Self>,
    ) {
        if self.layout_orientation == orientation {
            return;
        }
        self.layout_orientation = orientation;
        self.sync_compare(cx);
        cx.notify();
    }
}
