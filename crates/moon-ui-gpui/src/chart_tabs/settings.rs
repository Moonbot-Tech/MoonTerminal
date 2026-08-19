//! `ChartTabs` per-tab settings controller. It provides active-tab setting getters (`active_*` for
//! layout, orientation, order book, zone, auto-pin, and scale), drains detached-window ⧉ requests,
//! and implements [`LayoutPopupHost`] together with the candle and graphics popup hosts. Each ⧉
//! press builds its value set here and hands it to the one shared walk in [`super::apply_all`];
//! shared ⚙ popup and single-setting application through `apply_tab_setting` lives in
//! [`super::common`]; popup rendering lives in [`super::layout_popup`].

use gpui::*;

use super::apply_all::{self, ApplyAll};
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
    /// Every popup's ⧉ travels in the same queue and runs the same walk; a detached source always
    /// leaves Main unchanged (`include_main = false`), like ⚙.
    pub(super) fn drain_apply_all(&mut self, cx: &mut Context<Self>) {
        if self.backend.read(cx).chart_apply_all.is_empty() {
            // The common case by a wide margin: this drain is called from the backend observer, so
            // anything below this line runs on every notification of every group window.
            return;
        }
        let group = self.group.clone();
        let reqs: Vec<crate::chart_tabs::apply_all::ApplyAllRequest> =
            self.backend.update(cx, |b, _| {
                let (mine, rest): (Vec<_>, Vec<_>) =
                    b.chart_apply_all.drain(..).partition(|r| r.group == group);
                b.chart_apply_all = rest;
                mine
            });
        for r in reqs {
            // A detached-window request leaves Main unchanged, like ⚙ with `include_main = false`.
            self.apply_all(r.apply, false, cx);
        }
    }
}

/// Host for the "Chart graphics" palette popup targeting the ACTIVE tab, like ⚙ and the candle
/// popup beside it.
///
/// Application and persistence use `apply_tab_setting(StackSetting::Graphics)`.
impl super::graphics_popup::GraphicsPopupHost for ChartTabs {
    fn graphics_popup_open(&self) -> bool {
        self.graphics_popup_open
    }
    fn set_graphics_popup_open(&mut self, open: bool) {
        self.graphics_popup_open = open;
    }
    fn graphics_override(&self, cx: &App) -> Option<moon_core::config::ChartGraphicsCfg> {
        match &self.active {
            Tab::Main => self.main.read(cx).chart_graphics(),
            Tab::Add(n, b) | Tab::Custom(n, b) => self
                .add_stack(*n, b)
                .and_then(|p| p.read(cx).chart_graphics()),
        }
    }
    fn apply_graphics_all(
        &mut self,
        cfg: moon_core::config::ChartGraphicsCfg,
        cx: &mut Context<Self>,
    ) {
        // Main receives a copy only when its own popup is open, matching ⚙ and the candle popup.
        let include_main = matches!(self.active, Tab::Main);
        self.apply_all(
            ApplyAll {
                values: vec![StackSetting::Graphics(cfg)],
                x_ppm: None,
            },
            include_main,
            cx,
        );
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
        self.apply_all(
            ApplyAll {
                values: vec![StackSetting::CandleView(cfg)],
                x_ppm,
            },
            include_main,
            cx,
        );
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
    /// Candle view, chart graphics and X scale are not copied: each has its own popup and its own ⧉.
    /// `include_main` indicates that the popup is open on Main.
    fn apply_all_from_popup(&mut self, cx: &mut Context<Self>) {
        let include_main = matches!(self.active, Tab::Main);
        let hf = self.read_layout_height(StackLayoutMode::Fit, cx);
        let hs = self.read_layout_height(StackLayoutMode::Scroll, cx);
        let snap = self.layout_popup_snapshot(cx);
        let values = apply_all::layout_values(
            &snap,
            hf,
            hs,
            self.active_scale_value(cx),
            self.active_layout_orientation(cx),
        );
        self.apply_all(
            ApplyAll {
                values,
                x_ppm: None,
            },
            include_main,
            cx,
        );
    }
}
