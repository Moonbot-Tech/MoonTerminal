//! [`Shell::new`] construction: assemble a group window's dock and panels, wire observers and
//! subscriptions for inputs, sliders, dock events, and window activation, and delegate metric
//! wiring to `wire_metric_subscriptions`. Factored out of `shell/mod.rs`.

use std::rc::Rc;
use std::time::Instant;

use gpui::*;
use rust_i18n::t;

use moon_ui::{
    DockArea, DockEvent, DockItem, MoonBackgroundPolicy, MoonInputEvent, MoonInputState,
    MoonSliderEvent, MoonSliderState, PanelView,
};

use moon_core::feed::ClientSettingsEdit;
use moon_core::session::CoreId;

use super::Shell;
use crate::chart_tabs::ChartTabs;
use crate::panels::DetectsPanel;
use crate::persistence::dock_persist::DOCK_VERSION;
use crate::shell::core_settings_popup;
use crate::{Backend, controls};

impl Shell {
    /// Construct a group window shell, restore or create its dock, and wire its long-lived inputs.
    ///
    /// Args:
    ///     backend: Shared backend entity for the group window.
    ///     group: Window group name.
    ///     focus: Optional core and market to focus when constructing the chart area.
    ///     epoch: Initial chart epoch.
    ///     theme: Initial chart theme.
    ///     window: Window used to construct dock and input entities.
    ///     cx: Shell context used to create child entities and subscriptions.
    ///
    /// Returns:
    ///     A fully initialized shell with all controlled popovers closed.
    pub(crate) fn new(
        backend: Entity<Backend>,
        group: String,
        focus: Option<(CoreId, String)>,
        epoch: f64,
        theme: moon_core::config::ChartTheme,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let window_handle = window.window_handle();
        // Use one DockArea per group window. Charts occupy center-left, Detects the right split,
        // and Orders plus the other utility panels the bottom tabs. No-fill background policies
        // let MoonPalette and the chart UnderScene control their own backgrounds.
        let dock = cx.new(|cx| {
            DockArea::new("group-dock", Some(DOCK_VERSION), window, cx)
                .background_policy(MoonBackgroundPolicy::NoFill)
                .tab_background_policy(MoonBackgroundPolicy::NoFill)
        });
        let weak = dock.downgrade();

        // Restore a version-compatible saved layout through `DockArea::load`; PanelRegistry
        // recreates panels from panel name and group. Build the default layout only when no
        // compatible saved state exists. This ports full dock persistence.
        let saved = backend
            .read(cx)
            .dock_states
            .get(&group)
            .filter(|s| s.version == Some(DOCK_VERSION))
            .cloned();

        if let Some(state) = saved {
            dock.update(cx, |area, cx| {
                if let Err(e) = area.load(state, window, cx) {
                    log::warn!("failed to restore dock layout for group {group}: {e}");
                }
            });
        } else {
            // Chart tabs (Main and AddToChart-N) use their own strip in chart_tabs/strip.rs for explicit
            // active-tab and detach control. Detects and bottom tabs are GPUI dock panels.
            let charts = cx.new(|cx| {
                ChartTabs::new(
                    backend.clone(),
                    group.clone(),
                    focus,
                    epoch,
                    theme.clone(),
                    window,
                    cx,
                )
            });
            let detects = cx.new(|cx| DetectsPanel::new(backend.clone(), group.clone(), cx));

            // Build bottom tabs while omitting detached panels, whose windows startup restores.
            // Detaching removes the panel from the dock, so persisted dock state excludes it.
            let detached_set: std::collections::HashSet<String> = backend
                .read(cx)
                .detached
                .iter()
                .filter(|s| s.group == group)
                .map(|s| s.panel.clone())
                .collect();
            // Build the default bottom strip from the panel registry in home-tab order (Orders,
            // Assets, Report, Alerts, Log — the trading tabs first, the diagnostic Log last;
            // CoreStatus is intentionally not a default tab, `home_order: None`). This order is the
            // left-to-right tab order and is the SAME source `shell::docks` uses to re-seat a
            // returning panel, so the two cannot disagree. A panel whose window startup restores
            // detached is skipped so it is not also docked here; `build_docked(None)` starts each
            // panel fresh, matching a first-run layout.
            let mut bottom_tabs: Vec<Rc<dyn PanelView>> = Vec::new();
            for name in crate::panels::registry::home_ordered_names() {
                if detached_set.contains(*name) {
                    continue;
                }
                if let Some(kind) = crate::panels::registry::find(name) {
                    bottom_tabs.push(kind.build_docked(&backend, &group, None, window, cx));
                }
            }

            // Place the entire default layout in the center split: charts on the left, Detects in
            // a roughly 220px right slot, and utility tabs in a roughly 220px bottom slot. Split
            // handles resize panels; tab docking and edge dragging are separate dock behavior.
            // The size/sell/scale toolbar remains a fixed Shell::render row outside the dock.
            let chart_item = DockItem::tab(charts, &weak, window, cx);
            let right = DockItem::tab(detects, &weak, window, cx);
            let top = DockItem::split_with_sizes(
                Axis::Horizontal,
                vec![chart_item, right],
                vec![None, Some(px(220.0))],
                &weak,
                window,
                cx,
            );
            let bottom = DockItem::tabs(bottom_tabs, &weak, window, cx);
            let center = DockItem::split_with_sizes(
                Axis::Vertical,
                vec![top, bottom],
                vec![None, Some(px(220.0))],
                &weak,
                window,
                cx,
            );

            dock.update(cx, |area, cx| area.set_center(center, window, cx));
        }

        // Header and status bar read Backend, but a top-down GPUI repaint also pulls in the heavy
        // Orders view. Throttle ordinary notifications to at most 4 Hz; book/CPU/FPS updates need
        // no faster visual cadence even if their source changes up to 10 Hz.
        cx.observe(&backend, |this, backend, cx| {
            crate::diag::bump(&crate::diag::SHELL_OBS_FIRE);
            this.drain_order_size_edit_request(cx);
            this.drain_sell_edit_request(cx);
            this.drain_repin_requests(cx);
            this.drain_engine_action_toasts(cx);
            let now = Instant::now();
            // User-triggered Follow/Live and scale changes, plus any order-size revision, bypass
            // the 250ms throttle. Autonomous book/CPU/FPS updates remain throttled to 4 Hz.
            let (follow, price_scale, order_size_rev) = {
                let b = backend.read(cx);
                (b.follow, b.price_scale, b.order_size_rev)
            };
            let follow_changed = follow != this.last_follow;
            let scale_changed = price_scale != this.last_price_scale;
            let size_changed = order_size_rev != this.last_order_size_rev;
            this.last_follow = follow;
            this.last_price_scale = price_scale;
            this.last_order_size_rev = order_size_rev;
            let due = follow_changed
                || scale_changed
                || size_changed
                || this
                    .last_notify
                    .map(|t| now.duration_since(t).as_millis() >= 250)
                    .unwrap_or(true);
            if due {
                this.last_notify = Some(now);
                crate::diag::bump(&crate::diag::SHELL_OBS_NOTIFY);
                cx.notify();
            }
        })
        .detach();

        // Wake Shell once per second so the header clock advances while idle; backend notifications
        // are data-gated and would otherwise leave it frozen. One 1 Hz timer per window is below
        // the status bar's 4 Hz cadence and stops when the Shell entity is gone.
        cx.spawn(async move |this, cx| {
            loop {
                let executor = cx.update(|cx| cx.background_executor().clone());
                executor.timer(std::time::Duration::from_secs(1)).await;
                let alive = cx.update(|cx| {
                    this.update(cx, |_this, cx| {
                        crate::diag::bump(&crate::diag::CLOCK_NOTIFY);
                        cx.notify();
                    })
                    .is_ok()
                });
                if !alive {
                    break;
                }
            }
        })
        .detach();

        // Dump every dock event, including drag, split, resize, detach, and close, into Backend.
        // The drain timer debounces persistence to `docks.json`.
        cx.subscribe(&dock, |this, dock, event: &DockEvent, cx| {
            match event {
                DockEvent::DetachRequested { panel_name } => {
                    this.defer_detach_panel(panel_name.to_string(), cx);
                }
                DockEvent::PanelCloseRequested { panel_name } => {
                    this.defer_restore_closed_panel(panel_name.to_string(), cx);
                }
                DockEvent::LayoutChanged => {}
            }
            let state = dock.read(cx).dump(cx);
            let group = this.group.clone();
            this.backend.update(cx, |b, _| {
                b.dock_states.insert(group, state);
                b.dock_dirty = true;
            });
        })
        .detach();

        cx.observe_window_bounds(window, |this, window, cx| {
            this.persist_group_geometry(window, cx);
        })
        .detach();

        // Track window activation for Main's idle auto-close. Mouse movement over an unfocused
        // window must not reset inactivity; gaining focus records activity so charts are not closed
        // immediately after the user returns.
        cx.observe_window_activation(window, |this, window, cx| {
            this.window_active = window.is_window_active();
            if this.window_active {
                let group = this.group.clone();
                this.backend.update(cx, |b, _| b.note_main_input(&group));
            }
        })
        .detach();

        // Inline order-size editor opened by double-clicking an F1-F6 button. Every valid positive
        // Change updates the focused core's `ServerConfig.order_sizes` immediately and schedules a
        // debounced save, so a hotkey or click before Enter uses the text already typed. Blur or
        // Enter performs a direct config save; empty, nonnumeric, or nonpositive input is ignored.
        let size_input = cx.new(|cx| MoonInputState::new(window, cx));
        cx.subscribe(&size_input, |this, inp, ev: &MoonInputEvent, cx| {
            if matches!(ev, MoonInputEvent::Change) {
                let Some((core, ix)) = this.size_edit else {
                    return;
                };
                if let Ok(v) = inp.read(cx).value().trim().replace(',', ".").parse::<f64>() {
                    if v > 0.0 && ix < 6 {
                        this.backend.update(cx, |b, bcx| {
                            b.set_order_size_value(core, ix, v);
                            b.order_size_rev = b.order_size_rev.wrapping_add(1);
                            bcx.notify();
                        });
                    }
                }
                return;
            }
            if !matches!(ev, MoonInputEvent::Blur | MoonInputEvent::PressEnter { .. }) {
                return;
            }
            let Some((core, ix)) = this.size_edit.take() else {
                return;
            };
            let raw = inp.read(cx).value().to_string();
            if let Ok(v) = raw.trim().replace(',', ".").parse::<f64>() {
                if v > 0.0 && ix < 6 {
                    this.backend.update(cx, |b, bcx| {
                        let base = b.session.core_base(core).unwrap_or("").to_string();
                        let mut saved = false;
                        if let Some(s) = b.config.servers.iter_mut().find(|s| s.id == core) {
                            let mut arr = s.order_sizes.unwrap_or_else(|| {
                                moon_core::config::servers::default_order_sizes(&base)
                            });
                            arr[ix] = v;
                            s.order_sizes = Some(arr);
                            saved = true;
                        }
                        if saved {
                            if let Err(e) = b.config.save() {
                                log::warn!("save order size failed: {e}");
                            }
                        }
                        bcx.notify();
                    });
                }
            }
            cx.notify();
        })
        .detach();

        // Inline fixed-sell percentage editor opened by double-clicking an S button. Blur or Enter
        // sends `SetFixedSellPct` to the captured core; empty, nonnumeric, or negative input is
        // ignored.
        let sell_input = cx.new(|cx| MoonInputState::new(window, cx));
        cx.subscribe(&sell_input, |this, inp, ev: &MoonInputEvent, cx| {
            if !matches!(ev, MoonInputEvent::Blur | MoonInputEvent::PressEnter { .. }) {
                return;
            }
            let Some((core, ix)) = this.sell_edit.take() else {
                return;
            };
            if let Ok(v) = inp.read(cx).value().trim().replace(',', ".").parse::<f64>() {
                if v >= 0.0 && ix < 6 {
                    this.backend.update(cx, |b, bcx| {
                        // Update the optimistic display cache before sending the edit to the core.
                        b.set_fixed_sell_pct_local(core, ix, v);
                        b.order_size_rev = b.order_size_rev.wrapping_add(1);
                        bcx.notify();
                        if let Err(error) = b.session.edit_client_settings(
                            core,
                            ClientSettingsEdit::SetFixedSellPct {
                                slot: ix + 1,
                                pct: v,
                            },
                        ) {
                            log::warn!("set fixed-sell pct failed: {error}");
                        }
                    });
                }
            }
            cx.notify();
        })
        .detach();

        // Trading-metric popups pair a slider for quick selection with an input for exact values.
        // Bounds come from `controls`; TP has normal and extended sliders selected by `x_tmode`.
        // Popup opening seeds the current value through `on_open_change`; constructors use defaults.
        let mk_slider = |cx: &mut Context<Self>, (min, max, step): (f32, f32, f32), def: f32| {
            cx.new(|_| {
                MoonSliderState::new()
                    .min(min)
                    .max(max)
                    .step(step)
                    .default_value(def)
            })
        };
        let tp_slider_normal = mk_slider(cx, controls::TP_NORMAL, 1.0);
        let tp_slider_ext = mk_slider(cx, controls::TP_EXT, 100.0);
        // The fixed 0..2 TP fine slider is enabled only when the upper normal TP slider reaches 2.
        let tp_fine_slider = Self::make_tp_fine_slider(cx);
        let sl_slider = mk_slider(cx, controls::SL_BOUNDS, 0.0);
        let lev_slider = mk_slider(cx, controls::LEV_BOUNDS, 1.0);
        let tp_input = cx.new(|cx| MoonInputState::new(window, cx));
        let sl_input = cx.new(|cx| MoonInputState::new(window, cx));
        let lev_input = cx.new(|cx| MoonInputState::new(window, cx));
        let gtp_slider = mk_slider(cx, core_settings_popup::CORE_GTP_BOUNDS, 0.5);
        let trailing_slider = mk_slider(cx, core_settings_popup::CORE_TRAILING_BOUNDS, -0.1);
        let vstop_slider = mk_slider(cx, core_settings_popup::CORE_VSTOP_BOUNDS, 0.0);
        let gtp_input = cx.new(|cx| MoonInputState::new(window, cx));
        let trailing_input = cx.new(|cx| MoonInputState::new(window, cx));
        let vstop_input = cx.new(|cx| MoonInputState::new(window, cx));
        let blacklist_input = cx.new(|cx| MoonInputState::new(window, cx));
        let def_strategy_input = cx.new(|cx| {
            MoonInputState::new(window, cx)
                .placeholder(t!("core_settings.def_strategy_search").to_string())
        });
        // Repaint on strategy-search input so the popup can refilter its list.
        cx.subscribe(&def_strategy_input, |_this, _, ev: &MoonInputEvent, cx| {
            if matches!(ev, MoonInputEvent::Change) {
                cx.notify();
            }
        })
        .detach();
        // Start in multiline mode, but make Enter submit rather than insert a newline.
        let blacklist_area = cx.new(|cx| {
            MoonInputState::new(window, cx)
                .multi_line(true)
                .submit_on_enter(true)
        });
        let ticker_input = cx.new(|cx| MoonInputState::new(window, cx).placeholder("BTC…"));
        // Ticker-search changes only repaint; layer rendering computes the filtered list.
        cx.subscribe(&ticker_input, |_this, _inp, ev: &MoonInputEvent, cx| {
            if matches!(ev, MoonInputEvent::Change) {
                cx.notify();
            }
        })
        .detach();

        // Metric sliders and fields send guarded edits for the popup's core and keep numeric inputs
        // synchronized. `wire_metric_subscriptions` keeps the repeated subscription plumbing out of
        // this constructor.
        Self::wire_metric_subscriptions(
            cx,
            &tp_slider_normal,
            &tp_slider_ext,
            &sl_slider,
            &lev_slider,
            &tp_input,
            &sl_input,
            &lev_input,
        );

        // Core-settings popup fields commit on Blur or Enter. Global TP writes
        // `GlobalTakeProfit { on: true, pct }`, because the field implies that it is enabled;
        // trailing writes `TrailingDrop`. Empty or nonnumeric input is ignored.
        cx.subscribe(&gtp_input, |this, inp, ev: &MoonInputEvent, cx| {
            if !matches!(ev, MoonInputEvent::Blur | MoonInputEvent::PressEnter { .. }) {
                return;
            }
            if let Ok(v) = inp.read(cx).value().trim().replace(',', ".").parse::<f64>() {
                this.commit_client_edit(
                    ClientSettingsEdit::GlobalTakeProfit { on: true, pct: v },
                    cx,
                );
            }
        })
        .detach();
        cx.subscribe(&trailing_input, |this, inp, ev: &MoonInputEvent, cx| {
            if !matches!(ev, MoonInputEvent::Blur | MoonInputEvent::PressEnter { .. }) {
                return;
            }
            if let Ok(v) = inp.read(cx).value().trim().replace(',', ".").parse::<f32>() {
                this.commit_client_edit(ClientSettingsEdit::TrailingDrop(v), cx);
            }
        })
        .detach();

        // Core-settings sliders send edits to the active core and update their fields live, matching
        // the toolbar metric popups.
        cx.subscribe(&gtp_slider, |this, _e, ev: &MoonSliderEvent, cx| {
            if let MoonSliderEvent::Change(v) = ev {
                let v = v.end();
                this.commit_client_edit(
                    ClientSettingsEdit::GlobalTakeProfit {
                        on: true,
                        pct: v as f64,
                    },
                    cx,
                );
                this.live_set_field(this.gtp_input.clone(), controls::fmt_field2(v), cx);
            }
        })
        .detach();
        cx.subscribe(&trailing_slider, |this, _e, ev: &MoonSliderEvent, cx| {
            if let MoonSliderEvent::Change(v) = ev {
                let v = v.end();
                this.commit_client_edit(ClientSettingsEdit::TrailingDrop(v), cx);
                this.live_set_field(
                    this.trailing_input.clone(),
                    controls::fmt_field2_signed(v),
                    cx,
                );
            }
        })
        .detach();
        // V-Stop maps integer-percent `vol_drop_level`: slider changes send an edit and update the
        // integer field.
        cx.subscribe(&vstop_slider, |this, _e, ev: &MoonSliderEvent, cx| {
            if let MoonSliderEvent::Change(v) = ev {
                let n = v.end().round() as i32;
                this.commit_client_edit(ClientSettingsEdit::VolDropLevel(n), cx);
                this.live_set_field(this.vstop_input.clone(), format!("{n}"), cx);
            }
        })
        .detach();
        cx.subscribe(&vstop_input, |this, inp, ev: &MoonInputEvent, cx| {
            if !matches!(ev, MoonInputEvent::Blur | MoonInputEvent::PressEnter { .. }) {
                return;
            }
            if let Ok(n) = inp.read(cx).value().trim().parse::<i32>() {
                this.commit_client_edit(ClientSettingsEdit::VolDropLevel(n), cx);
            }
        })
        .detach();
        // Commit blacklist text on Blur or Enter through one path shared by the single-line and
        // expanded multiline editors. Preserve the active core's current enabled flag.
        let commit_bl = |this: &mut Self,
                         inp: Entity<MoonInputState>,
                         ev: &MoonInputEvent,
                         cx: &mut Context<Self>| {
            if !matches!(ev, MoonInputEvent::Blur | MoonInputEvent::PressEnter { .. }) {
                return;
            }
            let text = inp.read(cx).value().to_string();
            this.commit_blacklist_text(text, cx);
        };
        cx.subscribe(
            &blacklist_input,
            move |this, inp, ev: &MoonInputEvent, cx| commit_bl(this, inp, ev, cx),
        )
        .detach();
        cx.subscribe(
            &blacklist_area,
            move |this, inp, ev: &MoonInputEvent, cx| commit_bl(this, inp, ev, cx),
        )
        .detach();

        // Focus the window root immediately so hotkeys, including F keys, work even when Main is
        // empty and the dock has nothing else to focus; see the `focus` field.
        let focus = cx.focus_handle();
        window.focus(&focus, cx);

        Self {
            backend,
            group,
            dock,
            last_frame: None,
            fps: 0.0,
            last_notify: None,
            last_follow: true,
            last_price_scale: None,
            last_order_size_rev: 0,
            window_handle,
            size_input,
            size_edit: None,
            sell_input,
            sell_edit: None,
            tp_slider_normal,
            tp_slider_ext,
            tp_fine_slider,
            sl_slider,
            lev_slider,
            tp_input,
            sl_input,
            lev_input,
            gtp_slider,
            trailing_slider,
            vstop_slider,
            gtp_input,
            trailing_input,
            vstop_input,
            blacklist_input,
            blacklist_area,
            def_strategy_input,
            open_metric_popup: None,
            focus,
            window_active: true,
            header_core_selector_open: false,
            core_settings_open: false,
            core_settings_cancel_confirm: false,
            core_settings_bl_expanded: false,
            ticker_popup_open: false,
            ticker_popup_hovered: false,
            ticker_input,
        }
    }

    /// Subscribe the TP/SL/leverage popup editors to guarded writes and live field updates.
    ///
    /// Each callback writes only while its metric popup is still open for the core and market from
    /// which it was seeded. The entities are passed in because `new` registers these subscriptions
    /// before `Shell` itself has been assembled.
    fn wire_metric_subscriptions(
        cx: &mut Context<Self>,
        tp_slider_normal: &Entity<MoonSliderState>,
        tp_slider_ext: &Entity<MoonSliderState>,
        sl_slider: &Entity<MoonSliderState>,
        lev_slider: &Entity<MoonSliderState>,
        tp_input: &Entity<MoonInputState>,
        sl_input: &Entity<MoonInputState>,
        lev_input: &Entity<MoonInputState>,
    ) {
        cx.subscribe(tp_slider_normal, |this, _e, ev: &MoonSliderEvent, cx| {
            if let MoonSliderEvent::Change(v) = ev {
                let v = v.end();
                this.commit_metric_edit(
                    controls::TradeMetric::Tp,
                    ClientSettingsEdit::TakeProfit {
                        pct: v as f64,
                        extended: false,
                    },
                    cx,
                );
                this.live_set_field(this.tp_input.clone(), controls::fmt_field2(v), cx);
                // Reaching the upper slider's minimum of 2 enables the fine slider and sets it to 2.
                if v <= controls::TP_FINE_MAX {
                    this.defer_set_slider(this.tp_fine_slider.clone(), controls::TP_FINE_MAX, cx);
                }
            }
        })
        .detach();
        cx.subscribe(tp_slider_ext, |this, _e, ev: &MoonSliderEvent, cx| {
            if let MoonSliderEvent::Change(v) = ev {
                let v = v.end();
                this.commit_metric_edit(
                    controls::TradeMetric::Tp,
                    ClientSettingsEdit::TakeProfit {
                        pct: v as f64,
                        extended: true,
                    },
                    cx,
                );
                this.live_set_field(this.tp_input.clone(), controls::fmt_field2(v), cx);
            }
        })
        .detach();
        cx.subscribe(sl_slider, |this, _e, ev: &MoonSliderEvent, cx| {
            if let MoonSliderEvent::Change(v) = ev {
                let v = v.end();
                this.commit_metric_edit(
                    controls::TradeMetric::Sl,
                    ClientSettingsEdit::StopLossPct(v),
                    cx,
                );
                this.live_set_field(this.sl_input.clone(), controls::fmt_field2_signed(v), cx);
            }
        })
        .detach();
        cx.subscribe(lev_slider, |this, _e, ev: &MoonSliderEvent, cx| {
            // Do not apply leverage while dragging because it is an exchange action. Only mirror
            // the value into the field; the popup's Apply button reads and commits it.
            if let MoonSliderEvent::Change(v) = ev {
                let v = v.end();
                this.live_set_field(this.lev_input.clone(), format!("{}", v as i32), cx);
            }
        })
        .detach();

        // Inputs commit exact values on Blur or Enter and ignore empty or nonnumeric text. TP reads
        // the active core's current `x_tmode` so the edit uses the same range.
        cx.subscribe(tp_input, |this, inp, ev: &MoonInputEvent, cx| {
            if !matches!(ev, MoonInputEvent::Blur | MoonInputEvent::PressEnter { .. }) {
                return;
            }
            if let Ok(v) = inp.read(cx).value().trim().replace(',', ".").parse::<f64>() {
                let extended = this.active_tp_extended(cx);
                this.commit_metric_edit(
                    controls::TradeMetric::Tp,
                    ClientSettingsEdit::TakeProfit { pct: v, extended },
                    cx,
                );
            }
        })
        .detach();
        cx.subscribe(sl_input, |this, inp, ev: &MoonInputEvent, cx| {
            if !matches!(ev, MoonInputEvent::Blur | MoonInputEvent::PressEnter { .. }) {
                return;
            }
            if let Ok(v) = inp.read(cx).value().trim().replace(',', ".").parse::<f32>() {
                this.commit_metric_edit(
                    controls::TradeMetric::Sl,
                    ClientSettingsEdit::StopLossPct(v),
                    cx,
                );
            }
        })
        .detach();
        // The leverage field never commits on Blur or Enter. Leverage is an exchange action sent
        // only by the popup's Apply button; the field and slider merely select its value.
        let _ = lev_input;
    }
}
