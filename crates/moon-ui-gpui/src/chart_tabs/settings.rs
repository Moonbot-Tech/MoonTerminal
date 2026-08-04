//! `ChartTabs` per-tab settings controller. It provides active-tab setting getters (`active_*` for
//! layout, orientation, order book, zone, auto-pin, and scale), applies settings to every group
//! stack and window through `apply_layout_to_all` plus detached-window request draining, and
//! implements [`LayoutPopupHost`]. Shared ⚙ popup and single-setting application logic through
//! `apply_tab_setting` lives in [`super::common`]; popup rendering lives in [`super::layout_popup`].

use gpui::*;

use super::common::{LayoutPopupHost, LayoutPopupSnapshot, StackSetting, set_stack_setting};
use super::{AddChartStack, ChartTabs, Tab};
use crate::Backend;
use crate::persistence::chart_persist::{ChartBtnPos, StackLayoutMode, StackOrientation};
use moon_core::config::ChartBucket;
use moon_ui::MoonInputState;

impl ChartTabs {
    /// Return the active tab's persistence key: Main is `(0, Shared)`; AddToChart and Custom use
    /// `(num, bucket)`. Persistence still skips Custom tabs; see `persist_active`.
    pub(super) fn active_stack_key(&self) -> (u32, ChartBucket) {
        match &self.active {
            Tab::Main => (0, ChartBucket::Shared),
            Tab::Add(n, b) | Tab::Custom(n, b) => (*n, b.clone()),
        }
    }

    /// Return whether a custom multi-market tab is active.
    ///
    /// This selects all group cores as the market-search universe and gates order-book subscriptions
    /// by focus.
    pub(super) fn active_is_custom(&self) -> bool {
        matches!(self.active, Tab::Custom(..))
    }

    /// Return the active Add or Custom stack, or `None` for Main or a missing stack.
    pub(super) fn active_stack(&self) -> Option<Entity<AddChartStack>> {
        match &self.active {
            Tab::Main => None,
            Tab::Add(n, b) | Tab::Custom(n, b) => self.add_stack(*n, b),
        }
    }

    /// Return the active tab's per-tab layout mode, with `None` meaning the Fit default.
    pub(super) fn active_layout_mode(&self, cx: &App) -> Option<StackLayoutMode> {
        match &self.active {
            Tab::Main => self.main.read(cx).layout_mode(),
            Tab::Add(n, b) | Tab::Custom(n, b) => {
                self.add_stack(*n, b).and_then(|p| p.read(cx).layout_mode())
            }
        }
    }

    /// Return the active tab's per-tab Fit size.
    pub(super) fn active_layout_height_fit(&self, cx: &App) -> Option<u16> {
        match &self.active {
            Tab::Main => self.main.read(cx).layout_height_fit(),
            Tab::Add(n, b) | Tab::Custom(n, b) => self
                .add_stack(*n, b)
                .and_then(|p| p.read(cx).layout_height_fit()),
        }
    }

    /// Return the active tab's per-tab Scroll size.
    pub(super) fn active_layout_height_scroll(&self, cx: &App) -> Option<u16> {
        match &self.active {
            Tab::Main => self.main.read(cx).layout_height_scroll(),
            Tab::Add(n, b) | Tab::Custom(n, b) => self
                .add_stack(*n, b)
                .and_then(|p| p.read(cx).layout_height_scroll()),
        }
    }

    /// Return whether the active tab enables the order book, defaulting to enabled for `None`.
    pub(super) fn active_orderbook_enabled(&self, cx: &App) -> bool {
        let v = match &self.active {
            Tab::Main => self.main.read(cx).orderbook_enabled(),
            Tab::Add(n, b) | Tab::Custom(n, b) => self
                .add_stack(*n, b)
                .and_then(|p| p.read(cx).orderbook_enabled()),
        };
        v.unwrap_or(true)
    }

    /// Return whether the active tab draws liquidation trades, defaulting to enabled for `None`.
    pub(super) fn active_liquidations_enabled(&self, cx: &App) -> bool {
        let v = match &self.active {
            Tab::Main => self.main.read(cx).liquidations_enabled(),
            Tab::Add(n, b) | Tab::Custom(n, b) => self
                .add_stack(*n, b)
                .and_then(|p| p.read(cx).liquidations_enabled()),
        };
        v.unwrap_or(true)
    }

    /// Return whether the active tab fills the control zone, defaulting to enabled for `None`.
    pub(super) fn active_show_zone(&self, cx: &App) -> bool {
        let v = match &self.active {
            Tab::Main => self.main.read(cx).show_zone(),
            Tab::Add(n, b) | Tab::Custom(n, b) => {
                self.add_stack(*n, b).and_then(|p| p.read(cx).show_zone())
            }
        };
        v.unwrap_or(true)
    }

    /// Return whether the active tab auto-pins on an order, defaulting to disabled for `None`.
    pub(super) fn active_auto_pin(&self, cx: &App) -> bool {
        let v = match &self.active {
            Tab::Main => self.main.read(cx).auto_pin(),
            Tab::Add(n, b) | Tab::Custom(n, b) => {
                self.add_stack(*n, b).and_then(|p| p.read(cx).auto_pin())
            }
        };
        v.unwrap_or(false)
    }

    /// Return the active tab's Cancel Buy and Panic Sell button positions, defaulting to Right.
    pub(super) fn active_action_btn_pos(&self, cx: &App) -> (ChartBtnPos, ChartBtnPos) {
        let (c, pp) = self.active_action_btn_pos_opt(cx);
        (c.unwrap_or_default(), pp.unwrap_or_default())
    }

    fn active_action_btn_pos_opt(&self, cx: &App) -> (Option<ChartBtnPos>, Option<ChartBtnPos>) {
        match &self.active {
            Tab::Main => self.main.read(cx).action_btn_pos(),
            Tab::Add(n, b) | Tab::Custom(n, b) => self
                .add_stack(*n, b)
                .map(|p| p.read(cx).action_btn_pos())
                .unwrap_or((None, None)),
        }
    }

    /// Return the active tab's price-axis position, defaulting to Left for `None`.
    pub(super) fn active_price_axis_pos(
        &self,
        cx: &App,
    ) -> crate::persistence::chart_persist::PriceAxisPos {
        let v = match &self.active {
            Tab::Main => self.main.read(cx).price_axis_pos(),
            Tab::Add(n, b) | Tab::Custom(n, b) => self
                .add_stack(*n, b)
                .and_then(|p| p.read(cx).price_axis_pos()),
        };
        v.unwrap_or_default()
    }

    /// Return the active tab's time-axis visibility, defaulting to enabled for `None`.
    pub(super) fn active_time_axis_visible(&self, cx: &App) -> bool {
        let v = match &self.active {
            Tab::Main => self.main.read(cx).time_axis_visible(),
            Tab::Add(n, b) | Tab::Custom(n, b) => self
                .add_stack(*n, b)
                .and_then(|p| p.read(cx).time_axis_visible()),
        };
        v.unwrap_or(true)
    }

    /// Return the active tab's line-label visibility, defaulting to enabled for `None`.
    pub(super) fn active_line_labels(&self, cx: &App) -> bool {
        let v = match &self.active {
            Tab::Main => self.main.read(cx).line_labels(),
            Tab::Add(n, b) | Tab::Custom(n, b) => {
                self.add_stack(*n, b).and_then(|p| p.read(cx).line_labels())
            }
        };
        v.unwrap_or(true)
    }

    /// Return the active tab's crosshair-label visibility, defaulting to enabled for `None`.
    pub(super) fn active_cursor_labels(&self, cx: &App) -> bool {
        let v = match &self.active {
            Tab::Main => self.main.read(cx).cursor_labels(),
            Tab::Add(n, b) | Tab::Custom(n, b) => self
                .add_stack(*n, b)
                .and_then(|p| p.read(cx).cursor_labels()),
        };
        v.unwrap_or(true)
    }

    /// Return the active tab's optional stack orientation unchanged.
    ///
    /// Popup and layout consumers resolve `None` to the Vertical default.
    pub(super) fn active_layout_orientation(&self, cx: &App) -> Option<StackOrientation> {
        match &self.active {
            Tab::Main => self.main.read(cx).layout_orientation(),
            Tab::Add(n, b) | Tab::Custom(n, b) => self
                .add_stack(*n, b)
                .and_then(|p| p.read(cx).layout_orientation()),
        }
    }

    /// Return the active tab's price scale, with `None` meaning Auto.
    pub(super) fn active_scale_value(&self, cx: &App) -> Option<f32> {
        match &self.active {
            Tab::Main => self.main.read(cx).scale(),
            Tab::Add(n, b) | Tab::Custom(n, b) => {
                self.add_stack(*n, b).and_then(|p| p.read(cx).scale())
            }
        }
    }

    /// Apply all layout-popup settings plus price scale from a source tab to every group stack.
    ///
    /// This deliberately does not copy candle view or X scale. `include_main` controls Main: `true`
    /// from Main's popup includes it, while `false` from charts leaves it unchanged. Each tab is
    /// persisted.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn apply_layout_to_all(
        &mut self,
        include_main: bool,
        mode: Option<StackLayoutMode>,
        height_fit: Option<u16>,
        height_scroll: Option<u16>,
        scale: Option<f32>,
        orderbook: Option<bool>,
        liquidations: Option<bool>,
        show_zone: Option<bool>,
        auto_pin: Option<bool>,
        orientation: Option<StackOrientation>,
        cancel_pos: Option<ChartBtnPos>,
        panic_pos: Option<ChartBtnPos>,
        price_axis_pos: Option<crate::persistence::chart_persist::PriceAxisPos>,
        time_axis_visible: Option<bool>,
        line_labels: Option<bool>,
        cursor_labels: Option<bool>,
        cx: &mut Context<Self>,
    ) {
        let ob = orderbook.unwrap_or(true);
        let liq = liquidations.unwrap_or(true);
        let sz = show_zone.unwrap_or(true);
        let ap = auto_pin.unwrap_or(false);
        let axis = price_axis_pos.unwrap_or_default();
        let time_axis = time_axis_visible.unwrap_or(true);
        let lbl = line_labels.unwrap_or(true);
        let curl = cursor_labels.unwrap_or(true);
        if include_main {
            self.main.update(cx, |s, c| {
                s.set_layout(mode, height_fit, height_scroll, c);
                s.set_scale(scale, c);
                s.set_orderbook_enabled(Some(ob), c);
                s.set_liquidations_enabled(Some(liq), c);
                s.set_show_zone(Some(sz), c);
                s.set_auto_pin(Some(ap), c);
                s.set_orientation(orientation, c);
                s.set_action_btn_pos(cancel_pos, panic_pos, c);
                s.set_price_axis_pos(Some(axis), c);
                s.set_time_axis_visible(Some(time_axis), c);
                s.set_line_labels(Some(lbl), c);
                s.set_cursor_labels(Some(curl), c);
            });
            self.upsert_spec(cx, 0, &ChartBucket::Shared, |s| {
                s.layout_mode = mode;
                s.layout_height_fit = height_fit;
                s.layout_height_scroll = height_scroll;
                s.scale = scale;
                s.orderbook_enabled = Some(ob);
                s.liquidations_enabled = Some(liq);
                s.show_zone = Some(sz);
                s.auto_pin = Some(ap);
                s.layout_orientation = orientation;
                s.cancel_buy_pos = cancel_pos;
                s.panic_sell_pos = panic_pos;
                s.price_axis_pos = Some(axis);
                s.time_axis_visible = Some(time_axis);
                s.line_labels = Some(lbl);
                s.cursor_labels = Some(curl);
            });
        }
        // "Charts" includes Add tabs in the strip, Custom tabs, and window-detached stacks in `self.detached`.
        let targets: Vec<(u32, ChartBucket, Entity<AddChartStack>)> = self
            .add
            .iter()
            .chain(self.custom.iter())
            .chain(self.detached.iter())
            .map(|(n, b, p)| (*n, b.clone(), p.clone()))
            .collect();
        for (num, bucket, panel) in targets {
            panel.update(cx, |s, c| {
                s.set_layout(mode, height_fit, height_scroll, c);
                s.set_scale(scale, c);
                s.set_orderbook_enabled(Some(ob), c);
                s.set_liquidations_enabled(Some(liq), c);
                s.set_show_zone(Some(sz), c);
                s.set_auto_pin(Some(ap), c);
                s.set_orientation(orientation, c);
                s.set_action_btn_pos(cancel_pos, panic_pos, c);
                s.set_price_axis_pos(Some(axis), c);
                s.set_time_axis_visible(Some(time_axis), c);
                s.set_line_labels(Some(lbl), c);
                s.set_cursor_labels(Some(curl), c);
            });
            self.upsert_spec(cx, num, &bucket, |s| {
                s.layout_mode = mode;
                s.layout_height_fit = height_fit;
                s.layout_height_scroll = height_scroll;
                s.scale = scale;
                s.orderbook_enabled = Some(ob);
                s.liquidations_enabled = Some(liq);
                s.show_zone = Some(sz);
                s.auto_pin = Some(ap);
                s.layout_orientation = orientation;
                s.cancel_buy_pos = cancel_pos;
                s.panic_sell_pos = panic_pos;
                s.price_axis_pos = Some(axis);
                s.time_axis_visible = Some(time_axis);
                s.line_labels = Some(lbl);
                s.cursor_labels = Some(curl);
            });
        }
        self.backend.update(cx, |b, _| b.rebuild_orderbook_wanted());
        cx.notify();
    }

    /// Apply candle rendering settings, plus the source window's X scale when set, to ALL group
    /// tabs and windows through the candlestick popup's ⧉ button, then update the defaults inherited by new
    /// tabs and windows. As with ⚙, `include_main` changes Main ONLY when its own popup is open;
    /// otherwise Main is pinned to its current view before the global fallback changes.
    pub(super) fn apply_candle_view_to_all(
        &mut self,
        cfg: moon_core::market::CandleViewCfg,
        x_ppm: Option<f32>,
        include_main: bool,
        cx: &mut Context<Self>,
    ) {
        if include_main {
            self.main.update(cx, |s, c| {
                s.set_candle_view(Some(cfg), c);
                if x_ppm.is_some() {
                    s.set_x_ppm(x_ppm, true, c);
                }
            });
            self.upsert_spec(cx, 0, &ChartBucket::Shared, |s| s.candle_view = Some(cfg));
        } else if self.main.read(cx).candle_view().is_none() {
            let pin = self.backend.read(cx).layout.candle_view;
            self.main.update(cx, |s, c| s.set_candle_view(Some(pin), c));
            self.upsert_spec(cx, 0, &ChartBucket::Shared, |s| s.candle_view = Some(pin));
        }
        let targets: Vec<(u32, ChartBucket, Entity<AddChartStack>)> = self
            .add
            .iter()
            .chain(self.custom.iter())
            .chain(self.detached.iter())
            .map(|(n, b, p)| (*n, b.clone(), p.clone()))
            .collect();
        for (num, bucket, panel) in targets {
            panel.update(cx, |s, c| {
                s.set_candle_view(Some(cfg), c);
                if x_ppm.is_some() {
                    s.set_x_ppm(x_ppm, true, c);
                }
            });
            self.upsert_spec(cx, num, &bucket, |s| {
                s.candle_view = Some(cfg);
                if x_ppm.is_some() {
                    s.x_ppm = x_ppm;
                }
            });
        }
        let group = self.group.clone();
        self.backend.update(cx, |b, _| {
            b.layout.candle_view = cfg;
            if let Some(ppm) = x_ppm {
                // Store scale for THIS group and every known group window so their new charts
                // inherit it. Live windows from other groups update their own charts separately.
                b.layout.chart_x_ppm_by_group.insert(group, ppm);
                let groups: Vec<String> = b.layout.groups.keys().cloned().collect();
                for g in groups {
                    b.layout.chart_x_ppm_by_group.insert(g, ppm);
                }
            }
            b.layout_dirty = true;
        });
        cx.notify();
    }

    /// Apply X scale from Shift+middle-click on OUR window's chart to every stack in that window.
    ///
    /// This covers Main and tabs but not the group's detached windows, which have their own scope,
    /// and persists to `layout.chart_x_ppm_by_group` for new charts to inherit.
    pub(super) fn drain_x_sync(&mut self, cx: &mut Context<Self>) {
        let (rev, req) = {
            let b = self.backend.read(cx);
            (b.chart_x_sync_rev, b.chart_x_sync)
        };
        if rev == self.last_x_sync_rev {
            return;
        }
        self.last_x_sync_rev = rev;
        let Some((handle, ppm)) = req else {
            return;
        };
        if handle != self.window_handle {
            return;
        }
        self.apply_x_ppm_to_window(ppm, cx);
    }

    /// Apply X scale to every stack in THIS window and persist it per group.
    pub(super) fn apply_x_ppm_to_window(&mut self, ppm: f32, cx: &mut Context<Self>) {
        self.main.update(cx, |s, c| s.set_x_ppm(Some(ppm), true, c));
        let stacks: Vec<Entity<AddChartStack>> = self
            .add
            .iter()
            .chain(self.custom.iter())
            .map(|(_, _, p)| p.clone())
            .collect();
        for panel in stacks {
            panel.update(cx, |s, c| s.set_x_ppm(Some(ppm), true, c));
        }
        let group = self.group.clone();
        self.backend.update(cx, |b, _| {
            b.layout.chart_x_ppm_by_group.insert(group, ppm);
            b.layout_dirty = true;
        });
        cx.notify();
    }

    /// Drain "apply to all" requests from detached chart windows in THIS group.
    ///
    /// They send requests through Backend because they cannot access the group's stacks directly.
    pub(super) fn drain_apply_all(&mut self, cx: &mut Context<Self>) {
        let group = self.group.clone();
        let candle_reqs: Vec<(moon_core::market::CandleViewCfg, Option<f32>)> =
            self.backend.update(cx, |b, _| {
                let (mine, rest): (Vec<_>, Vec<_>) = b
                    .chart_candle_apply_all
                    .drain(..)
                    .partition(|(g, _, _)| *g == group);
                b.chart_candle_apply_all = rest;
                mine.into_iter().map(|(_, c, x)| (c, x)).collect()
            });
        for (cfg, x_ppm) in candle_reqs {
            // A detached-window request leaves Main unchanged, like ⚙ with `include_main=false`.
            self.apply_candle_view_to_all(cfg, x_ppm, false, cx);
        }
        let reqs: Vec<crate::ChartApplyAll> = self.backend.update(cx, |b, _| {
            let (mine, rest): (Vec<_>, Vec<_>) =
                b.chart_apply_all.drain(..).partition(|r| r.group == group);
            b.chart_apply_all = rest;
            mine
        });
        for r in reqs {
            self.apply_layout_to_all(
                r.include_main,
                r.mode,
                r.height_fit,
                r.height_scroll,
                r.scale,
                r.orderbook,
                r.liquidations,
                r.show_zone,
                r.auto_pin,
                r.orientation,
                r.cancel_pos,
                r.panic_pos,
                r.price_axis_pos,
                r.time_axis_visible,
                r.line_labels,
                r.cursor_labels,
                cx,
            );
        }
    }
}

/// Host for the "Candles and Trades" candlestick popup targeting the ACTIVE tab, like ⚙.
///
/// Application and persistence use `apply_tab_setting(StackSetting::CandleView)`.
impl super::candle_popup::CandlePopupHost for ChartTabs {
    fn candle_popup_open(&self) -> bool {
        self.candle_popup_open
    }
    fn set_candle_popup_open(&mut self, open: bool) {
        self.candle_popup_open = open;
    }
    fn candle_view_override(&self, cx: &App) -> Option<moon_core::market::CandleViewCfg> {
        match &self.active {
            Tab::Main => self.main.read(cx).candle_view(),
            Tab::Add(n, b) | Tab::Custom(n, b) => {
                self.add_stack(*n, b).and_then(|p| p.read(cx).candle_view())
            }
        }
    }
    fn apply_candle_view_all(
        &mut self,
        cfg: moon_core::market::CandleViewCfg,
        cx: &mut Context<Self>,
    ) {
        // Copy this window's X scale with the candle settings when one is set.
        let x_ppm = self
            .backend
            .read(cx)
            .layout
            .chart_x_ppm_by_group
            .get(&self.group)
            .copied();
        // Main receives a copy only when its own popup is open, matching ⚙ behavior.
        let include_main = matches!(self.active, Tab::Main);
        self.apply_candle_view_to_all(cfg, x_ppm, include_main, cx);
    }
}

/// Tab-strip host for the ⚙ popup targeting the ACTIVE Main, Add, or Custom stack.
///
/// `active_stack_key` supplies the persistence key; trait default methods provide shared popup and
/// application logic.
impl LayoutPopupHost for ChartTabs {
    fn popup_open(&self) -> bool {
        self.layout_popup_open
    }
    fn set_popup_open(&mut self, open: bool) {
        self.layout_popup_open = open;
    }
    fn fit_input(&self) -> &Entity<MoonInputState> {
        &self.layout_fit_input
    }
    fn scroll_input(&self) -> &Entity<MoonInputState> {
        &self.layout_scroll_input
    }
    fn rename_input(&self) -> &Entity<MoonInputState> {
        &self.custom_name_input
    }
    fn backend(&self) -> &Entity<Backend> {
        &self.backend
    }
    fn spec_group(&self) -> &str {
        &self.group
    }
    fn spec_key(&self) -> (u32, ChartBucket) {
        self.active_stack_key()
    }
    fn current_layout(&self, cx: &App) -> (Option<StackLayoutMode>, Option<u16>, Option<u16>) {
        (
            self.active_layout_mode(cx),
            self.active_layout_height_fit(cx),
            self.active_layout_height_scroll(cx),
        )
    }
    fn current_orientation(&self, cx: &App) -> Option<StackOrientation> {
        self.active_layout_orientation(cx)
    }
    fn action_btn_pos_opt(&self, cx: &App) -> (Option<ChartBtnPos>, Option<ChartBtnPos>) {
        self.active_action_btn_pos_opt(cx)
    }
    fn layout_popup_snapshot(&self, cx: &App) -> LayoutPopupSnapshot {
        let (cancel_pos, panic_pos) = self.active_action_btn_pos(cx);
        LayoutPopupSnapshot {
            mode: self.active_layout_mode(cx).unwrap_or(StackLayoutMode::Fit),
            orientation: self
                .active_layout_orientation(cx)
                .unwrap_or(StackOrientation::Vertical),
            orderbook: self.active_orderbook_enabled(cx),
            liquidations: self.active_liquidations_enabled(cx),
            show_zone: self.active_show_zone(cx),
            auto_pin: self.active_auto_pin(cx),
            cancel_pos,
            panic_pos,
            price_axis_pos: self.active_price_axis_pos(cx),
            time_axis: self.active_time_axis_visible(cx),
            line_labels: self.active_line_labels(cx),
            cursor_labels: self.active_cursor_labels(cx),
        }
    }
    fn popup_is_custom(&self, _cx: &App) -> bool {
        self.active_is_custom()
    }
    /// Return the custom tab name for the popup's rename field, available only for Custom tabs.
    fn seed_rename_input(&self, window: &mut Window, cx: &mut Context<Self>) {
        if let Tab::Custom(n, _) = &self.active {
            let name = self.custom_label(*n);
            self.custom_name_input
                .update(cx, |input, c| input.set_value(name, window, c));
        }
    }
    fn set_on_stacks(&mut self, v: StackSetting, cx: &mut Context<Self>) {
        match self.active.clone() {
            Tab::Main => self.main.update(cx, |s, c| set_stack_setting!(s, c, v)),
            Tab::Add(..) | Tab::Custom(..) => {
                if let Some(p) = self.active_stack() {
                    p.update(cx, |s, c| set_stack_setting!(s, c, v));
                }
            }
        }
    }
    /// Apply all active-tab layout-popup settings plus price scale directly to every group stack.
    ///
    /// Candle view and X scale are not copied. `include_main` indicates that the popup is open on Main.
    fn apply_all_from_popup(&mut self, cx: &mut Context<Self>) {
        let include_main = matches!(self.active, Tab::Main);
        let hf = self.read_layout_height(StackLayoutMode::Fit, cx);
        let hs = self.read_layout_height(StackLayoutMode::Scroll, cx);
        let mode = Some(self.active_layout_mode(cx).unwrap_or(StackLayoutMode::Fit));
        let scale = self.active_scale_value(cx);
        let ob = Some(self.active_orderbook_enabled(cx));
        let liq = Some(self.active_liquidations_enabled(cx));
        let sz = Some(self.active_show_zone(cx));
        let ap = Some(self.active_auto_pin(cx));
        let or = self.active_layout_orientation(cx);
        let (cp, pp) = self.active_action_btn_pos(cx);
        let pax = self.active_price_axis_pos(cx);
        let tax = self.active_time_axis_visible(cx);
        let ll = self.active_line_labels(cx);
        let cl = self.active_cursor_labels(cx);
        self.apply_layout_to_all(
            include_main,
            mode,
            hf,
            hs,
            scale,
            ob,
            liq,
            sz,
            ap,
            or,
            Some(cp),
            Some(pp),
            Some(pax),
            Some(tax),
            Some(ll),
            Some(cl),
            cx,
        );
    }
}
