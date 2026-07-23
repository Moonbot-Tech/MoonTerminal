//! OS-window host view for a detached chart tab (`DetachedChartHost`): a header with market search,
//! scale, the ⚙ layout popup, and "close all charts" above the chart-stack panel. It writes window
//! geometry and per-tab settings to `charts.json` and requests repinning on close. Window creation,
//! restoration, and repinning live in `windows.rs` under `impl ChartTabs`; traits from
//! [`super::common`] provide shared ⚙ popup and market-search logic, implemented below.

use gpui::*;
use moon_ui::{MoonInputEvent, MoonInputState};
use rust_i18n::t;
use std::time::Duration;

use super::common::{
    CoinPopupHost, LayoutPopupHost, LayoutPopupSnapshot, StackSetting, set_stack_setting,
};
use super::{AddChartStack, chart_pane_label, coin_search};
use crate::Backend;
use crate::persistence::chart_persist::{self, StackLayoutMode, StackOrientation};
use moon_core::config::ChartBucket;
use moon_core::session::CoreId;

mod render;

/// Host view for a detached chart-tab window with a header and chart-stack panel.
///
/// The header contains scale and "close all charts" controls. The host writes window geometry to
/// charts.json through `observe_window_bounds` and requests repinning on `on_release` through
/// `chart_repin_request`, which `ChartTabs` drains.
pub(super) struct DetachedChartHost {
    panel: Entity<AddChartStack>,
    backend: Entity<Backend>,
    group: String,
    num: u32,
    bucket: ChartBucket,
    /// Whether `observe_window_bounds` may persist geometry.
    ///
    /// A restored window starts false because GPUI auto-placement on a non-primary DPI can report a
    /// scale-shifted value that MUST NOT overwrite the saved geometry or the position drifts each
    /// launch. It arms after about 1.5 seconds so only real user moves are written. A newly detached
    /// window starts true.
    persist_armed: bool,
    /// Saved logical size used to correct the FIRST render of a restored window.
    ///
    /// GPUI creates the window on the primary display, and `WM_DPICHANGED` rescales its SIZE while
    /// moving to a display with another DPI even though position is already correct. Force the saved
    /// logical size once. Newly detached windows use `None`.
    restore_size: Option<Size<Pixels>>,
    /// Remaining first renders that should call `ITaskbarList::DeleteTab` after the window is shown.
    ///
    /// The window remains normally independent so FancyZones can see it. Several attempts cover the
    /// race where the taskbar button has not appeared yet.
    taskbar_hide_ticks: u8,
    /// In-scene layout settings popup for this tab, opened by ⚙.
    ///
    /// It is not a separate OS window because chart text now lies below the normal GPUI scene.
    layout_popup_open: bool,
    /// Whether the cursor has entered the popup; leaving after first entry closes it and commits input.
    layout_popup_hovered: bool,
    /// In-scene "Candles and Trades" popup opened by ❚ for this window tab's candle settings.
    candle_popup_open: bool,
    candle_popup_hovered: bool,
    /// Last observed `chart_x_sync_rev`; Shift+middle-click in THIS window applies scale to its panel
    /// and persists it in the tab spec exactly once.
    last_x_sync_rev: u64,
    /// Size input for Fit mode.
    layout_fit_input: Entity<MoonInputState>,
    /// Size input for Scroll mode.
    layout_scroll_input: Entity<MoonInputState>,
    /// Custom-tab name input in the ⚙ popup, only when this window holds a detached Custom tab.
    custom_name_input: Entity<MoonInputState>,
    /// Window-header market-search input; its universe depends on this window bucket's cores.
    coin_input: Entity<MoonInputState>,
    /// Current market-search text mirroring `coin_input`.
    coin_query: String,
    /// Whether the market-match list is open.
    coin_popup_open: bool,
    /// Window-root focus handle for receiving `on_key_down` hotkeys when nothing else is focused.
    ///
    /// The root receives focus on creation. Clicking market input moves focus there, but key events
    /// bubble back to the root. This currently covers Scale +/- for the window panel.
    focus: FocusHandle,
}

impl DetachedChartHost {
    pub(super) fn new(
        panel: Entity<AddChartStack>,
        backend: Entity<Backend>,
        group: String,
        num: u32,
        bucket: ChartBucket,
        restored: bool,
        restore_size: Option<Size<Pixels>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        // Persist geometry from causal bounds events to charts.json for restoration in the same place.
        cx.observe_window_bounds(window, |this, window, cx| {
            this.persist_geometry(window, cx);
        })
        .detach();
        // When panel composition changes by closing or adding a market, persist ticker changes if
        // this window holds a detached Custom tab. The helper diffs internally and no-ops otherwise.
        cx.observe(&panel, |this, _panel, cx| {
            this.persist_custom_coins_if_any(cx);
        })
        .detach();
        // A restored window does not write initial bounds immediately: on a non-primary DPI,
        // GPUI/Win32 can report a temporary scale-shifted position or size. Saving it would drift the
        // window each launch. Re-enable normal user move/resize persistence after a short settling
        // period. A newly detached window persists geometry immediately.
        if restored {
            cx.spawn(async move |this, cx| {
                let executor = cx.update(|cx| cx.background_executor().clone());
                executor.timer(Duration::from_millis(1500)).await;
                let _ = cx.update(|cx| {
                    this.update(cx, |this, _cx| {
                        this.persist_armed = true;
                        moon_core::detect_diag::line(&format!(
                            "[geom] n={} bucket={:?} persist armed after restore settle",
                            this.num, this.bucket
                        ));
                    })
                    .is_ok()
                });
            })
            .detach();
        }
        // Closing requests repinning into the strip, drained by `ChartTabs`. During app shutdown the
        // request is not handled, leaving the spec detached so the window restores next launch.
        let (g, n, c) = (group.clone(), num, bucket.clone());
        cx.on_release(move |this, app| {
            this.backend.update(app, |b, cx| {
                b.chart_repin_request.push((g.clone(), n, c.clone()));
                cx.notify();
            });
        })
        .detach();
        let initial_x_sync_rev = backend.read(cx).chart_x_sync_rev;
        // Restore this detached panel's saved per-tab display settings from charts.json.
        let (group2, num2, bucket2) = (group.clone(), num, bucket.clone());
        let saved = backend.read(cx).chart_specs.iter().find_map(|s| {
            s.matches(&group2, num2, &bucket2).then(|| {
                (
                    s.layout_mode,
                    s.layout_height_fit,
                    s.layout_height_scroll,
                    s.orderbook_enabled,
                    s.liquidations_enabled,
                    s.show_zone,
                    s.auto_pin,
                    (s.cancel_buy_pos, s.panic_sell_pos),
                    s.price_axis_pos,
                    s.time_axis_visible,
                    s.line_labels,
                    s.cursor_labels,
                    s.candle_view,
                    s.x_ppm,
                )
            })
        });
        if let Some((
            m,
            hf,
            hs,
            ob,
            liq,
            sz,
            ap,
            action_pos,
            axis_pos,
            time_axis,
            line_labels,
            cursor_labels,
            candle_view,
            saved_x_ppm,
        )) = saved
        {
            if m.is_some() || hf.is_some() || hs.is_some() {
                panel.update(cx, |p, pcx| p.set_layout(m, hf, hs, pcx));
            }
            if ob.is_some() {
                panel.update(cx, |p, pcx| p.set_orderbook_enabled(ob, pcx));
            }
            if liq.is_some() {
                panel.update(cx, |p, pcx| p.set_liquidations_enabled(liq, pcx));
            }
            if candle_view.is_some() {
                panel.update(cx, |p, pcx| p.set_candle_view(candle_view, pcx));
            }
            // Window X scale comes from its spec, falling back to the parent group's scale.
            let x_ppm = saved_x_ppm.or_else(|| {
                backend
                    .read(cx)
                    .layout
                    .chart_x_ppm_by_group
                    .get(&group)
                    .copied()
            });
            if x_ppm.is_some() {
                panel.update(cx, |p, pcx| p.set_x_ppm(x_ppm, false, pcx));
            }
            if sz.is_some() {
                panel.update(cx, |p, pcx| p.set_show_zone(sz, pcx));
            }
            if ap.is_some() {
                panel.update(cx, |p, pcx| p.set_auto_pin(ap, pcx));
            }
            if action_pos.0.is_some() || action_pos.1.is_some() {
                panel.update(cx, |p, pcx| {
                    p.set_action_btn_pos(action_pos.0, action_pos.1, pcx)
                });
            }
            if axis_pos.is_some() {
                panel.update(cx, |p, pcx| p.set_price_axis_pos(axis_pos, pcx));
            }
            if time_axis.is_some() {
                panel.update(cx, |p, pcx| p.set_time_axis_visible(time_axis, pcx));
            }
            if line_labels.is_some() {
                panel.update(cx, |p, pcx| p.set_line_labels(line_labels, pcx));
            }
            if cursor_labels.is_some() {
                panel.update(cx, |p, pcx| p.set_cursor_labels(cursor_labels, pcx));
            }
        }
        let layout_fit_input = cx.new(|cx| MoonInputState::new(window, cx));
        let layout_scroll_input = cx.new(|cx| MoonInputState::new(window, cx));
        cx.subscribe(
            &layout_fit_input,
            |this, _input, ev: &MoonInputEvent, cx| {
                if this.layout_popup_open
                    && matches!(ev, MoonInputEvent::Blur | MoonInputEvent::PressEnter { .. })
                {
                    this.commit_layout_popup(cx);
                }
            },
        )
        .detach();
        cx.subscribe(
            &layout_scroll_input,
            |this, _input, ev: &MoonInputEvent, cx| {
                if this.layout_popup_open
                    && matches!(ev, MoonInputEvent::Blur | MoonInputEvent::PressEnter { .. })
                {
                    this.commit_layout_popup(cx);
                }
            },
        )
        .detach();
        // Custom-tab name input commits renaming on Blur or Enter.
        let custom_name_input = cx.new(|cx| MoonInputState::new(window, cx));
        cx.subscribe(
            &custom_name_input,
            |this, input, ev: &MoonInputEvent, cx| {
                if this.layout_popup_open
                    && matches!(ev, MoonInputEvent::Blur | MoonInputEvent::PressEnter { .. })
                {
                    let name = input.read(cx).value().to_string();
                    this.rename_custom(name, cx);
                }
            },
        )
        .detach();
        let coin_input = cx.new(|cx| {
            MoonInputState::new(window, cx).placeholder(t!("chart.coin.search").to_string())
        });
        // Transliterate Russian keyboard layout to Latin directly in the field, matching `chart_tabs::new`.
        cx.subscribe_in(
            &coin_input,
            window,
            |this, input, ev: &MoonInputEvent, window, cx| {
                if matches!(ev, MoonInputEvent::Change) {
                    let value = input.read(cx).value().to_string();
                    if let std::borrow::Cow::Owned(en) =
                        crate::controls::coin_search::normalize_layout(&value)
                    {
                        input.update(cx, |st, c| st.set_value(en, window, c));
                        return;
                    }
                    if this.coin_query != value {
                        this.coin_popup_open = !value.trim().is_empty();
                        this.coin_query = value;
                        cx.notify();
                    }
                }
            },
        )
        .detach();
        // Focus the root immediately so Scale +/- hotkeys work without first clicking the window body.
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        Self {
            panel,
            backend,
            group,
            num,
            bucket,
            persist_armed: !restored,
            restore_size,
            taskbar_hide_ticks: 8,
            layout_popup_open: false,
            layout_popup_hovered: false,
            candle_popup_open: false,
            candle_popup_hovered: false,
            last_x_sync_rev: initial_x_sync_rev,
            layout_fit_input,
            layout_scroll_input,
            custom_name_input,
            coin_input,
            coin_query: String::new(),
            coin_popup_open: false,
            focus,
        }
    }

    /// Return the detached chart window's unambiguous trading target.
    ///
    /// The target is the locked comparison anchor or the window's sole market. A multi-panel window
    /// without an anchor returns `None`, so trading hotkeys skip an ambiguous target.
    fn window_target(&self, cx: &App) -> Option<(CoreId, String)> {
        let p = self.panel.read(cx);
        if let Some(anchor) = p.compare_anchor() {
            return Some(anchor);
        }
        let mut coins = p.coins(cx);
        if coins.len() == 1 {
            return coins.pop();
        }
        None
    }

    /// Handle a detached chart window hotkey through the single [`crate::hotkeys`] recognizer.
    ///
    /// Scale belongs to this window panel and is applied directly, without group revision routing.
    /// Trading and figure actions use shared `apply` against THIS window's `window_target`; figures
    /// are global state and always work.
    fn on_hotkey(&mut self, ev: &KeyDownEvent, cx: &mut Context<Self>) {
        use crate::hotkeys::HotkeyAction;
        let action = {
            let b = self.backend.read(cx);
            crate::hotkeys::resolve(ev, &b.preview.as_ref().unwrap_or(&b.config).hotkeys)
        };
        let Some(action) = action else {
            return;
        };
        let handled = match action {
            // Built-in Ctrl+Shift+F10 resets every window position.
            HotkeyAction::ResetWindows => {
                crate::windowing::reset_all_windows_onscreen(cx);
                true
            }
            // Built-in Tab/Delete cancels the order under the cursor on the hovered chart.
            HotkeyAction::CancelHoveredOrder => {
                crate::hotkeys::cancel_hovered_order(&self.backend, cx)
            }
            // Delete prioritizes the selected figure. With no selection, fall back to built-in order
            // cancellation under the cursor; otherwise `fig_delete=Delete` would shadow it. See Shell.
            HotkeyAction::FigDelete => {
                self.backend.update(cx, |b, bcx| {
                    // `FigDelete` does not use a target or core.
                    crate::hotkeys::apply(action, b, bcx, None, None)
                }) || crate::hotkeys::cancel_hovered_order(&self.backend, cx)
            }
            // Built-in Shift+Esc closes every group's Main charts.
            HotkeyAction::CloseAllCharts => {
                self.backend.update(cx, |b, _| {
                    b.close_all_charts_rev = b.close_all_charts_rev.wrapping_add(1);
                });
                true
            }
            HotkeyAction::ScalePlus | HotkeyAction::ScaleMinus => {
                let zoom_in = matches!(action, HotkeyAction::ScalePlus);
                let next = crate::controls::step_scale(self.panel.read(cx).scale(), zoom_in);
                self.panel.update(cx, |st, scx| st.set_scale(next, scx));
                cx.notify();
                true
            }
            // Place a manual order at the cursor price through the hovered chart.
            HotkeyAction::NewLong | HotkeyAction::NewShort => {
                let short = matches!(action, HotkeyAction::NewShort);
                let chart = self
                    .backend
                    .read(cx)
                    .hovered_chart
                    .clone()
                    .and_then(|w| w.upgrade());
                match chart {
                    Some(chart) => chart.update(cx, |p, pcx| p.place_order_at_cursor(short, pcx)),
                    None => false,
                }
            }
            other => {
                let target = self.window_target(cx);
                let active_core = target.as_ref().map(|(c, _)| *c);
                self.backend.update(cx, |b, bcx| {
                    crate::hotkeys::apply(other, b, bcx, target.clone(), active_core)
                })
            }
        };
        if handled {
            cx.stop_propagation();
        }
    }

    /// Return whether this window is a detached Custom tab whose spec has `custom_coins`.
    fn is_custom(&self, cx: &App) -> bool {
        let (group, num, bucket) = (&self.group, self.num, &self.bucket);
        self.backend
            .read(cx)
            .chart_specs
            .iter()
            .any(|s| s.matches(group, num, bucket) && s.custom_coins.is_some())
    }

    /// Rename this window's Custom tab from the name field in the ⚙ popup.
    ///
    /// This writes `custom_label` to charts.json. The window title updates through
    /// `chart_pane_label` on the next render.
    fn rename_custom(&mut self, name: String, cx: &mut Context<Self>) {
        let name = name.trim().to_string();
        if name.is_empty() {
            return;
        }
        let (group, num, bucket) = (self.group.clone(), self.num, self.bucket.clone());
        self.backend.update(cx, |b, _| {
            if let Some(s) = b
                .chart_specs
                .iter_mut()
                .find(|s| s.matches(&group, num, &bucket))
            {
                s.custom_label = Some(name);
                b.chart_specs_dirty = true;
            }
        });
        cx.notify();
    }

    /// Return market-search matches for this window from its bucket's cores.
    fn coin_results(&self, cx: &App) -> Vec<(CoreId, String, String)> {
        coin_search::search(
            self.backend.read(cx),
            &self.group,
            Some(&self.bucket),
            &self.coin_query,
        )
    }

    /// Rewrite a Custom tab spec's tickers from the current panel composition only when changed.
    ///
    /// The observer calls this frequently. It applies only when `custom_coins.is_some()` and no-ops
    /// for ordinary AddToChart windows.
    fn persist_custom_coins_if_any(&self, cx: &mut Context<Self>) {
        let (group, num, bucket) = (self.group.clone(), self.num, self.bucket.clone());
        let is_custom = {
            let specs = &self.backend.read(cx).chart_specs;
            specs
                .iter()
                .any(|s| s.matches(&group, num, &bucket) && s.custom_coins.is_some())
        };
        if !is_custom {
            return;
        }
        let (coins, anchor, broom) = {
            let p = self.panel.read(cx);
            (p.coins(cx), p.compare_anchor(), p.compare_orderbook_only())
        };
        self.backend.update(cx, |b, _| {
            if let Some(s) = b
                .chart_specs
                .iter_mut()
                .find(|s| s.matches(&group, num, &bucket))
            {
                if s.custom_coins.as_deref() != Some(coins.as_slice())
                    || s.compare_anchor != anchor
                    || s.compare_orderbook_only != broom
                {
                    s.custom_coins = Some(coins);
                    s.compare_anchor = anchor;
                    s.compare_orderbook_only = broom;
                    b.chart_specs_dirty = true;
                }
            }
        });
    }

    /// Return this window panel's current per-tab layout as `(mode, height_fit, height_scroll)`.
    fn panel_layout(&self, cx: &App) -> (Option<StackLayoutMode>, Option<u16>, Option<u16>) {
        let p = self.panel.read(cx);
        (
            p.layout_mode(),
            p.layout_height_fit(),
            p.layout_height_scroll(),
        )
    }

    fn persist_geometry(&mut self, window: &Window, cx: &mut Context<Self>) {
        // A restored window defers saving until `persist_armed`, preventing initial GPUI/Win32
        // auto-placement from replacing the saved position with DPI-shifted values.
        if !self.persist_armed {
            return;
        }
        let Some((x, y, w, h)) = crate::windowing::window_geom(window) else {
            moon_core::detect_diag::line(&format!(
                "[geom] n={} НЕ Windowed → геометрия не сохранена",
                self.num
            ));
            return;
        };
        let geom = chart_persist::WinGeom { x, y, w, h };
        let (group, num, bucket) = (self.group.clone(), self.num, self.bucket.clone());
        let found = self.backend.update(cx, |bk, _| {
            if let Some(s) = bk
                .chart_specs
                .iter_mut()
                .find(|s| s.matches(&group, num, &bucket))
            {
                let cur = s.detached.map(|g| (g.x, g.y, g.w, g.h));
                if cur != Some((geom.x, geom.y, geom.w, geom.h)) {
                    s.detached = Some(geom);
                    bk.chart_specs_dirty = true;
                }
                true
            } else {
                false
            }
        });
        moon_core::detect_diag::line(&format!(
            "[geom] n={num} bucket={bucket:?} → x={} y={} w={} h={} (spec_found={found})",
            geom.x, geom.y, geom.w, geom.h
        ));
    }
}

/// Host for the "Candles and Trades" ❚ popup targeting THIS window's panel.
///
/// "Apply to all" sends a group request through Backend, drained by the tab strip like
/// `ChartApplyAll`.
impl super::candle_popup::CandlePopupHost for DetachedChartHost {
    fn candle_popup_open(&self) -> bool {
        self.candle_popup_open
    }
    fn set_candle_popup_open(&mut self, open: bool) {
        self.candle_popup_open = open;
    }
    fn candle_popup_hovered(&self) -> bool {
        self.candle_popup_hovered
    }
    fn set_candle_popup_hovered(&mut self, hovered: bool) {
        self.candle_popup_hovered = hovered;
    }
    fn candle_view_override(&self, cx: &App) -> Option<moon_core::market::CandleViewCfg> {
        self.panel.read(cx).candle_view()
    }
    fn apply_candle_view_all(
        &mut self,
        cfg: moon_core::market::CandleViewCfg,
        cx: &mut Context<Self>,
    ) {
        // Apply immediately to this window and queue the rest through Backend for the group strip to
        // drain. Copy this window's X scale with the candle settings.
        self.apply_candle_view(cfg, cx);
        let x_ppm = self.panel.read(cx).x_ppm();
        let group = self.group.clone();
        self.backend.update(cx, |bk, bcx| {
            bk.chart_candle_apply_all.push((group, cfg, x_ppm));
            bcx.notify();
        });
    }
}

/// Detached-window host for the ⚙ popup targeting the window's ONLY panel.
///
/// The window's fixed `(num, bucket)` is the persistence key. Trait default methods provide shared
/// popup and application logic.
impl LayoutPopupHost for DetachedChartHost {
    fn popup_open(&self) -> bool {
        self.layout_popup_open
    }
    fn set_popup_open(&mut self, open: bool) {
        self.layout_popup_open = open;
    }
    fn popup_hovered(&self) -> bool {
        self.layout_popup_hovered
    }
    fn set_popup_hovered(&mut self, hovered: bool) {
        self.layout_popup_hovered = hovered;
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
        (self.num, self.bucket.clone())
    }
    fn current_layout(&self, cx: &App) -> (Option<StackLayoutMode>, Option<u16>, Option<u16>) {
        self.panel_layout(cx)
    }
    fn current_orientation(&self, cx: &App) -> Option<StackOrientation> {
        self.panel.read(cx).layout_orientation()
    }
    fn action_btn_pos_opt(
        &self,
        cx: &App,
    ) -> (
        Option<chart_persist::ChartBtnPos>,
        Option<chart_persist::ChartBtnPos>,
    ) {
        self.panel.read(cx).action_btn_pos()
    }
    fn layout_popup_snapshot(&self, cx: &App) -> LayoutPopupSnapshot {
        let p = self.panel.read(cx);
        let (cancel_pos, panic_pos) = p.action_btn_pos();
        LayoutPopupSnapshot {
            mode: p.layout_mode().unwrap_or(StackLayoutMode::Fit),
            orientation: p.layout_orientation().unwrap_or(StackOrientation::Vertical),
            orderbook: p.orderbook_enabled().unwrap_or(true),
            liquidations: p.liquidations_enabled().unwrap_or(true),
            show_zone: p.show_zone().unwrap_or(true),
            auto_pin: p.auto_pin().unwrap_or(false),
            cancel_pos: cancel_pos.unwrap_or_default(),
            panic_pos: panic_pos.unwrap_or_default(),
            price_axis_pos: p.price_axis_pos().unwrap_or_default(),
            time_axis: p.time_axis_visible().unwrap_or(true),
            line_labels: p.line_labels().unwrap_or(true),
            cursor_labels: p.cursor_labels().unwrap_or(true),
        }
    }
    fn popup_is_custom(&self, cx: &App) -> bool {
        self.is_custom(cx)
    }
    /// Return the Custom tab name for renaming when this window holds a Custom tab.
    fn seed_rename_input(&self, window: &mut Window, cx: &mut Context<Self>) {
        if self.is_custom(cx) {
            let name = chart_pane_label(&self.backend, &self.group, self.num, &self.bucket, cx);
            self.custom_name_input
                .update(cx, |input, c| input.set_value(name, window, c));
        }
    }
    fn set_on_stacks(&mut self, v: StackSetting, cx: &mut Context<Self>) {
        self.panel.update(cx, |s, c| set_stack_setting!(s, c, v));
    }
    /// Apply this window's settings to all by sending `ChartApplyAll` through Backend for the tab
    /// strip to drain, because the host cannot access group stacks directly. Copy ALL window
    /// settings, including scale and order-book toggle, while leaving Main unchanged with
    /// `include_main=false`.
    fn apply_all_from_popup(&mut self, cx: &mut Context<Self>) {
        let (mode, _, _) = self.panel_layout(cx);
        let mode = Some(mode.unwrap_or(StackLayoutMode::Fit));
        let height_fit = self.read_layout_height(StackLayoutMode::Fit, cx);
        let height_scroll = self.read_layout_height(StackLayoutMode::Scroll, cx);
        let group = self.group.clone();
        let scale = self.panel.read(cx).scale();
        let orderbook = Some(self.panel.read(cx).orderbook_enabled().unwrap_or(true));
        let liquidations = Some(self.panel.read(cx).liquidations_enabled().unwrap_or(true));
        let show_zone = Some(self.panel.read(cx).show_zone().unwrap_or(true));
        let auto_pin = Some(self.panel.read(cx).auto_pin().unwrap_or(false));
        let orientation = self.panel.read(cx).layout_orientation();
        let (cancel_pos, panic_pos) = {
            let (c, pp) = self.panel.read(cx).action_btn_pos();
            (Some(c.unwrap_or_default()), Some(pp.unwrap_or_default()))
        };
        let price_axis_pos = Some(self.panel.read(cx).price_axis_pos().unwrap_or_default());
        let time_axis_visible = Some(self.panel.read(cx).time_axis_visible().unwrap_or(true));
        let line_labels = Some(self.panel.read(cx).line_labels().unwrap_or(true));
        let cursor_labels = Some(self.panel.read(cx).cursor_labels().unwrap_or(true));
        self.backend.update(cx, |bk, bcx| {
            bk.chart_apply_all.push(crate::ChartApplyAll {
                group,
                include_main: false,
                mode,
                height_fit,
                height_scroll,
                scale,
                orderbook,
                liquidations,
                show_zone,
                auto_pin,
                orientation,
                cancel_pos,
                panic_pos,
                price_axis_pos,
                time_axis_visible,
                line_labels,
                cursor_labels,
            });
            bcx.notify();
        });
    }
}

/// Window-header market search that opens the selected market in THIS window's stack.
///
/// For a detached Custom tab, the ticker composition is persisted immediately.
impl CoinPopupHost for DetachedChartHost {
    fn clear_coin_search(&mut self, cx: &mut Context<Self>) {
        self.coin_query.clear();
        self.coin_popup_open = false;
        cx.notify();
    }
    fn open_picked_coin(&mut self, core: CoreId, market: String, cx: &mut Context<Self>) {
        self.panel.update(cx, |p, c| {
            p.add_coin(core, &market, coin_search::MANUAL_COIN_TTL_MS, c)
        });
        // If this is a detached Custom tab, keep its charts.json ticker list synchronized so a
        // market added in the window persists across restart.
        self.persist_custom_coins_if_any(cx);
        cx.notify();
    }
}
