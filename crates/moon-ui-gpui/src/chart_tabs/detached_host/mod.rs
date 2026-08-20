//! OS-window host view for a detached chart tab (`DetachedChartHost`): a header with market search,
//! scale, the ⚙ layout popup, and "close all charts" above the chart-stack panel. It writes window
//! geometry and per-tab settings to `charts.json` and requests repinning on close. Window creation,
//! restoration, and repinning live in `windows.rs` under `impl ChartTabs`; traits from
//! [`super::common`] provide shared ⚙ popup and market-search logic, implemented below.

use gpui::*;
use moon_ui::{MoonInputEvent, MoonInputState};
use rust_i18n::t;
use std::time::Duration;

use super::apply_all::{self, ApplyAll, ApplyAllRequest};
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
    /// This window's own id, compared against `Backend.detached_chart_windows` on release.
    ///
    /// A release means "the user closed this window" only while that registry still lists THIS
    /// window. Every deliberate teardown — a settings rebuild, a reconcile dropping a group, a
    /// group window closing, the debug harness — unregisters first, so its releases stay silent: a
    /// repin nobody asked for would return the tab to the strip and, through `drain_chart_repin`'s
    /// `upsert_spec`, undo the detachment in `charts.json` and re-create specs a rebuild pruned.
    /// Registry identity rather than a queue clear, because effect ordering cannot be relied on —
    /// GPUI dedups `Effect::Notify`, so an unrelated earlier notify can run the observers first.
    window_id: WindowId,
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
    /// Anchored layout settings popup for this tab, opened by ⚙.
    ///
    /// It is not a separate OS window because chart text now lies below the normal GPUI scene.
    layout_popup_open: bool,
    /// Anchored "Candles and Trades" popup opened by the candlestick button for this window tab.
    candle_popup_open: bool,
    /// Anchored "Chart graphics" popup opened by the palette button, for THIS window's charts.
    graphics_popup_open: bool,
    /// Whether the chart-labels popup is open, and which row has its style panel expanded.
    labels_popup_open: bool,
    labels_style_open: Option<usize>,
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
    /// Reads Caps Lock and lone-modifier presses out of this window's modifier-change stream.
    ///
    /// Per window for the same reason the group window keeps its own: a press spans several events.
    modifier_watch: moon_ui::MoonHotkeyModifierWatch,
    /// Exact cancellation authority for this window's current background taskbar-hide burst.
    taskbar_hide: crate::window::windowing::TaskbarHideTask,
}

impl DetachedChartHost {
    /// Construct a detached chart host and attach its window and persistence observers.
    ///
    /// Taskbar suppression is armed at construction and after every activation while the host
    /// remains an independent window. Active-window transitions also attribute singleton scope to
    /// this host's Auto group; repeated activation of the same owner is suppressed by Backend.
    /// [`crate::window::windowing::hide_window_from_taskbar_soon`] owns the platform timing
    /// rationale.
    ///
    /// Args:
    ///     panel: Chart-stack panel rendered by this host.
    ///     backend: Shared terminal state used for persistence and window registration.
    ///     group: Persisted chart group name.
    ///     num: Persisted tab number within the group.
    ///     bucket: Core bucket represented by the detached tab.
    ///     restored: Whether this window is being restored from persisted state.
    ///     restore_size: Saved logical size to enforce on the first render, when needed.
    ///     window: Newly opened independent window.
    ///     cx: Host context used for subscriptions and background work.
    ///
    /// Returns:
    ///     Fully initialized detached chart host.
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
        let window_id = window.window_handle().window_id();
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
            this.taskbar_hide.cancel();
            this.backend.update(app, |b, cx| {
                let mine = b
                    .detached_chart_windows
                    .iter()
                    .any(|(_, h)| h.window_id() == this.window_id);
                if !mine {
                    return;
                }
                b.detached_chart_windows
                    .retain(|(_, h)| h.window_id() != this.window_id);
                b.chart_repin_request.push((g.clone(), n, c.clone()));
                cx.notify();
            });
        })
        .detach();
        // Apply the shared independent-window taskbar policy now and after every activation.
        // Active transitions also own Auto singleton scope independently of idle-chart polling;
        // `hide_window_from_taskbar_soon` owns the delayed retry rationale.
        let taskbar_hide = crate::window::windowing::hide_window_from_taskbar_soon(window);
        cx.observe_window_activation(window, |this, window, cx| {
            this.taskbar_hide.cancel();
            this.taskbar_hide = crate::window::windowing::hide_window_from_taskbar_soon(window);
            if !window.is_window_active() {
                // See the group window: the state a returning window is re-told is not a press.
                this.modifier_watch.forget();
            }
            if window.is_window_active() {
                let group = this.group.clone();
                this.backend
                    .update(cx, |b, bcx| b.focus_auto_workspace(&group, bcx));
            }
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
                    s.chart_graphics,
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
            chart_graphics,
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
            if chart_graphics.is_some() {
                panel.update(cx, |p, pcx| p.set_chart_graphics(chart_graphics, pcx));
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
                // Focus opens the list without typing; see `ChartTabs::new` for why.
                if matches!(ev, MoonInputEvent::Focus) {
                    this.open_coin_popup(cx);
                    return;
                }
                if matches!(ev, MoonInputEvent::Change) {
                    let value = input.read(cx).value().to_string();
                    if let std::borrow::Cow::Owned(en) =
                        crate::controls::coin_search::normalize_layout(&value)
                    {
                        input.update(cx, |st, c| st.set_value(en, window, c));
                        return;
                    }
                    if this.coin_query != value {
                        // Clearing the text falls back to suggestions rather than closing.
                        this.coin_popup_open = true;
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
            window_id,
            persist_armed: !restored,
            restore_size,
            layout_popup_open: false,
            candle_popup_open: false,
            graphics_popup_open: false,
            labels_popup_open: false,
            labels_style_open: None,
            last_x_sync_rev: initial_x_sync_rev,
            layout_fit_input,
            layout_scroll_input,
            custom_name_input,
            coin_input,
            coin_query: String::new(),
            coin_popup_open: false,
            focus,
            modifier_watch: moon_ui::MoonHotkeyModifierWatch::default(),
            taskbar_hide,
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
    ///
    /// Args:
    ///     ev: Key-down event to resolve against the configured hotkeys.
    ///     window: Detached OS window receiving the event and, if requested, the chart shot.
    ///     cx: Host context used to dispatch the resolved action.
    ///
    /// Returns:
    ///     Nothing; a handled action stops propagation.
    fn on_hotkey(&mut self, ev: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        // Same first rule as the group window: Escape leaves the Sells-to-zone mode regardless of
        // the modifier held with it.
        if crate::hotkeys::escape_leaves_sells_zone(ev, &self.backend, cx) {
            cx.stop_propagation();
            return;
        }
        let action = {
            let b = self.backend.read(cx);
            crate::hotkeys::resolve(ev, &b.preview.as_ref().unwrap_or(&b.config).hotkeys)
        };
        let Some(action) = action else {
            return;
        };
        // Action-owned policies first, exactly as in the group window: auto-repeat suppression and
        // the cursor-addressed Split target, whose market-level fallback lives in `apply`.
        if crate::hotkeys::pre_dispatch(action, ev.is_held, &self.backend, cx) {
            cx.stop_propagation();
            return;
        }
        if self.dispatch_hotkey(action, window, cx) {
            cx.stop_propagation();
        }
    }

    /// Route a hotkey bound to Caps Lock or to a lone modifier, which arrive as a modifier change.
    ///
    /// The group window carries the same pair; see `shell::actions::on_modifier_hotkey` for why the
    /// two paths differ only in what the event can tell us.
    fn on_modifier_hotkey(
        &mut self,
        ev: &ModifiersChangedEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let typing = window.is_text_input_active();
        let action = {
            let b = self.backend.read(cx);
            let hk = &b.preview.as_ref().unwrap_or(&b.config).hotkeys;
            crate::hotkeys::resolve_modifiers(&mut self.modifier_watch, ev, hk, typing)
        };
        let Some(action) = action else {
            return;
        };
        if crate::hotkeys::pre_dispatch(action, false, &self.backend, cx) {
            cx.stop_propagation();
            return;
        }
        if self.dispatch_hotkey(action, window, cx) {
            cx.stop_propagation();
        }
    }

    /// Execute one resolved action against this detached window.
    ///
    /// Args:
    ///     action: The action a binding resolved to, whichever event carried it.
    ///     window: The window the binding arrived at.
    ///     cx: Host context used to route the action.
    ///
    /// Returns:
    ///     Whether the action was handled here, which is what decides propagation.
    fn dispatch_hotkey(
        &mut self,
        action: crate::hotkeys::HotkeyAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        use crate::hotkeys::HotkeyAction;
        let handled = match action {
            // Resolved against THIS window's own hover trail, exactly as in the group window: a
            // detached host owns a stack rather than a single chart, so it names no panel either.
            HotkeyAction::ChartShot => {
                crate::panels::shot::copy_active_chart(&self.backend, window, cx)
            }
            // Built-in Ctrl+Shift+F10 resets every window position.
            HotkeyAction::ResetWindows => {
                crate::window::windowing::reset_all_windows_onscreen(cx);
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
                    crate::hotkeys::apply(action, b, bcx, &self.group, None, None)
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
                    crate::hotkeys::apply(other, b, bcx, &self.group, target.clone(), active_core)
                })
            }
        };
        handled
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

    /// What this window's dropdown shows: query matches, or empty-field suggestions.
    ///
    /// Mirrors `ChartTabs::coin_results`, scoped to this window's own bucket. The empty-field
    /// branch reads only the cached suggestions; the scan filling that cache runs on open.
    fn coin_results(&self, cx: &App) -> crate::controls::coin_search::CoinResults {
        use crate::controls::coin_search::{CoinResults, suggestions};

        let b = self.backend.read(cx);
        if !self.coin_query.trim().is_empty() {
            return CoinResults::Query(coin_search::search(
                b,
                &self.group,
                Some(&self.bucket),
                &self.coin_query,
            ));
        }
        let (recent, volatile) = suggestions(
            b,
            &self.group,
            Some(&self.bucket),
            b.coin_suggest_markets(&self.group, Some(&self.bucket)),
        );
        CoinResults::Suggest { recent, volatile }
    }

    /// Open this window's coin dropdown, refreshing the suggestions it reads.
    pub(super) fn open_coin_popup(&mut self, cx: &mut Context<Self>) {
        // Resync the query mirror with the field; see `ChartTabs::open_coin_popup`.
        self.coin_query = self.coin_input.read(cx).value().to_string();
        let (group, bucket) = (self.group.clone(), self.bucket.clone());
        self.backend
            .update(cx, |b, _| b.refresh_coin_suggest(&group, Some(&bucket)));
        self.coin_popup_open = true;
        cx.notify();
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
        let Some((x, y, w, h)) = crate::window::windowing::window_geom(window) else {
            moon_core::detect_diag::line(&format!(
                "[geom] n={} НЕ Windowed → геометрия не сохранена",
                self.num
            ));
            return;
        };
        let mut geom = chart_persist::WinGeom {
            x,
            y,
            w,
            h,
            display_uuid: crate::window::windowing::window_display_uuid(window, cx),
        };
        let (group, num, bucket) = (self.group.clone(), self.num, self.bucket.clone());
        let found = self.backend.update(cx, |bk, _| {
            if let Some(s) = bk
                .chart_specs
                .iter_mut()
                .find(|s| s.matches(&group, num, &bucket))
            {
                geom = geom.keeping_display_of(s.detached);
                if s.detached != Some(geom) {
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

/// Host for the "Chart graphics" palette popup targeting THIS window's panel.
///
/// "Apply to all" sends a group request through Backend, drained by the tab strip, exactly like the
/// candle popup's below.
impl super::graphics_popup::GraphicsPopupHost for DetachedChartHost {
    fn graphics_popup_open(&self) -> bool {
        self.graphics_popup_open
    }
    fn set_graphics_popup_open(&mut self, open: bool) {
        self.graphics_popup_open = open;
    }
    fn graphics_override(&self, cx: &App) -> Option<moon_core::config::ChartGraphicsCfg> {
        self.panel.read(cx).chart_graphics()
    }
    fn apply_graphics_all(
        &mut self,
        cfg: moon_core::config::ChartGraphicsCfg,
        cx: &mut Context<Self>,
    ) {
        self.apply_graphics(cfg, cx);
        self.queue_apply_all(
            ApplyAll {
                values: vec![StackSetting::Graphics(cfg)],
                x_ppm: None,
            },
            cx,
        );
    }
}

/// Host for the "Chart labels" popup targeting THIS window's panel.
///
/// "Apply to all" sends a group request through Backend, drained by the tab strip, exactly like the
/// graphics popup's above.
impl super::labels_popup::LabelsPopupHost for DetachedChartHost {
    fn labels_popup_open(&self) -> bool {
        self.labels_popup_open
    }
    fn set_labels_popup_open(&mut self, open: bool) {
        self.labels_popup_open = open;
    }
    fn labels_style_open(&self) -> Option<usize> {
        self.labels_style_open
    }
    fn set_labels_style_open(&mut self, ix: Option<usize>) {
        self.labels_style_open = ix;
    }
    fn labels_override(&self, cx: &App) -> Option<moon_core::config::ChartLabelsCfg> {
        self.panel.read(cx).chart_labels()
    }
    fn apply_labels_all(
        &mut self,
        cfg: moon_core::config::ChartLabelsCfg,
        cx: &mut Context<Self>,
    ) {
        self.apply_labels(cfg, cx);
        self.queue_apply_all(
            ApplyAll {
                values: vec![StackSetting::Labels(cfg)],
                x_ppm: None,
            },
            cx,
        );
    }
}

/// Host for the "Candles and Trades" candlestick popup targeting THIS window's panel.
///
/// "Apply to all" sends a group request through Backend, drained by the tab strip.
impl super::candle_popup::CandlePopupHost for DetachedChartHost {
    fn candle_popup_open(&self) -> bool {
        self.candle_popup_open
    }
    fn set_candle_popup_open(&mut self, open: bool) {
        self.candle_popup_open = open;
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
        self.queue_apply_all(
            ApplyAll {
                values: vec![StackSetting::CandleView(cfg)],
                x_ppm,
            },
            cx,
        );
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
    /// Apply this window's settings to all by queueing them through Backend for the tab strip,
    /// because the host cannot access group stacks directly. Copy ALL window settings, including
    /// scale and order-book toggle, while leaving Main unchanged.
    fn apply_all_from_popup(&mut self, cx: &mut Context<Self>) {
        let snap = self.layout_popup_snapshot(cx);
        let height_fit = self.read_layout_height(StackLayoutMode::Fit, cx);
        let height_scroll = self.read_layout_height(StackLayoutMode::Scroll, cx);
        let scale = self.panel.read(cx).scale();
        let orientation = self.panel.read(cx).layout_orientation();
        let values = apply_all::layout_values(&snap, height_fit, height_scroll, scale, orientation);
        self.queue_apply_all(
            ApplyAll {
                values,
                x_ppm: None,
            },
            cx,
        );
    }
}

impl DetachedChartHost {
    /// Queue one ⧉ press for this window's group, for its tab strip to perform.
    fn queue_apply_all(&mut self, apply: ApplyAll, cx: &mut Context<Self>) {
        let group = self.group.clone();
        self.backend.update(cx, |bk, bcx| {
            bk.chart_apply_all.push(ApplyAllRequest { group, apply });
            bcx.notify();
        });
    }
}

/// Window-header market search that opens the selected market in THIS window's stack.
///
/// For a detached Custom tab, the ticker composition is persisted immediately.
impl CoinPopupHost for DetachedChartHost {
    /// Return the shared backend that supplies search state and persisted recents.
    fn coin_backend(&self) -> Entity<crate::Backend> {
        self.backend.clone()
    }

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
