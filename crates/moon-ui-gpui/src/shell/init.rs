//! [`Shell::new`] construction: assemble a group window's dock and panels, wire observers and
//! subscriptions for inputs, sliders, dock events, and window activation, and delegate metric
//! wiring to `wire_metric_subscriptions`. Factored out of `shell/mod.rs`.

use std::rc::Rc;
use std::time::Instant;

use gpui::*;

use moon_ui::{
    DockArea, DockEvent, DockItem, MoonBackgroundPolicy, MoonInputEvent, MoonInputState,
    MoonSliderEvent, MoonSliderState, PanelView,
};

use moon_core::feed::ClientSettingsEdit;
use moon_core::session::CoreId;

use super::Shell;
use crate::chart_tabs::ChartTabs;
use crate::panels::DetectsPanel;
use crate::persistence::dock_persist::{DOCK_VERSION, is_compatible_version};
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
        let updater = backend.read(cx).updater.clone();
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
            .filter(|s| is_compatible_version(s.version))
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
            // Build the default bottom strip from the panel registry in home-tab order: Orders,
            // Assets, Report, Alerts, News, CoreStatus, Log. `shell::docks` uses that same registry
            // order to re-seat a returning panel, so the two cannot disagree. A panel restored as a
            // detached window during startup is skipped so it is not also docked here;
            // `build_docked(None)` starts each panel fresh, matching a first-run layout.
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

        // Seed Shell's surface cursor from this group's latest existing request. A request produced
        // before this group window existed must not steal the Auto tab after startup or rebuild.
        let (initial_workspace_mode, last_auto_surface_revision) = {
            let backend = backend.read(cx);
            (
                backend.workspace_mode(&group),
                backend
                    .auto_workspace_surface_request(&group)
                    .map(|(revision, _)| revision)
                    .unwrap_or(0),
            )
        };
        let workspace_resize_state = cx.new(|_| moon_ui::MoonResizableState::default());
        let initial_auto_rail_width = backend.read(cx).auto_workspace_rail_width();

        // Header and status bar read Backend, but a top-down GPUI repaint also pulls in the heavy
        // Orders view. Throttle ordinary notifications to at most 4 Hz; book/CPU/FPS updates need
        // no faster visual cadence even if their source changes up to 10 Hz.
        cx.observe(&backend, |this, backend, cx| {
            crate::diag::bump(&crate::diag::SHELL_OBS_FIRE);
            this.drain_order_size_edit_request(cx);
            this.drain_sell_edit_request(cx);
            // Repins first. Both defer, so whichever is queued first runs first — and a detach
            // requested while a repin of the same panel is still pending would hit the "already
            // detached" guard and be swallowed, because the pending repin has not yet cleared the
            // spec. Draining repins first makes detach-after-close work in a single tick.
            this.drain_repin_requests(cx);
            this.drain_panel_detach_requests(cx);
            this.drain_engine_action_toasts(cx);
            this.drain_strategy_edit_toasts(cx);
            // Main-open requests live on Backend, while DockArea activation needs this native
            // window. Coalesce the bridge through the stored handle after this observer returns.
            this.defer_workspace_window_sync(cx);
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

        // Update availability and progress are rare process-wide edges and must repaint every
        // group header immediately, without riding the backend's ordinary 4 Hz throttle.
        cx.observe(&updater, |_this, _updater, cx| cx.notify())
            .detach();

        // Workspace mode, selected core, singleton owner, and window lifecycle publish on a
        // dedicated channel so an otherwise idle Shell redraws and applies a cross-group switch.
        let workspace_revision = backend.read(cx).workspace_revision();
        cx.observe(&workspace_revision, |this, _, cx| {
            this.defer_workspace_window_sync(cx);
            cx.notify();
        })
        .detach();

        // Shared Auto topology and rail width have a separate channel: edits in one group must
        // update every other open Auto Shell without waking all scoped panel queries.
        let auto_layout_revision = backend.read(cx).auto_workspace_layout_revision();
        cx.observe(&auto_layout_revision, |this, _, cx| {
            this.defer_workspace_window_sync(cx);
            cx.notify();
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

        // Route one dock's events to its current mode authority. Classic dumps retain panel payload
        // and group-local state in `docks.json`; Auto publishes only normalized name topology to
        // the process-wide `auto_dock.json` authority.
        cx.subscribe_in(
            &dock,
            window,
            |this, dock, event: &DockEvent, window, cx| {
                let auto = this.backend.read(cx).workspace_mode(&this.group)
                    == moon_core::config::WorkspaceMode::AutoTrading;
                match event {
                    DockEvent::DetachRequested { panel_name } => {
                        if auto {
                            return;
                        }
                        this.defer_detach_panel(panel_name.to_string(), true, cx);
                    }
                    DockEvent::PanelCloseRequested { panel_name } => {
                        if auto {
                            return;
                        }
                        this.defer_restore_closed_panel(panel_name.to_string(), cx);
                    }
                    DockEvent::PanelActivated { panel_name } => {
                        if let Some(panel_name) = super::workspace::auto_workspace_tab_to_persist(
                            auto,
                            this.applying_auto_topology,
                            panel_name,
                        ) {
                            let group = this.group.clone();
                            this.backend.update(cx, |backend, _| {
                                backend.set_auto_workspace_tab(&group, panel_name);
                            });
                        }
                        return;
                    }
                    DockEvent::TabContextMenu {
                        panel_name,
                        position,
                    } => {
                        // The dock carries no menu of its own; display policy for a tab lives here.
                        // Returns early: a right-click moved no panel, so re-dumping the dock tree and
                        // rewriting `docks.json` below would be work for nothing.
                        crate::panels::tab_menu::open(
                            panel_name,
                            *position,
                            &this.backend.clone(),
                            window,
                            cx,
                        );
                        return;
                    }
                    DockEvent::LayoutChanged => {}
                }
                if auto {
                    if !super::workspace::auto_workspace_topology_is_persistable(
                        auto,
                        this.applying_auto_topology,
                    ) {
                        return;
                    }
                    let topology = dock.read(cx).topology_by_name(cx);
                    this.backend.update(cx, |backend, backend_cx| {
                        backend.set_auto_dock_topology(topology, backend_cx);
                    });
                    return;
                }
                crate::diag::bump(&crate::diag::DOCK_DUMP);
                let _dump_us = crate::diag::scope(&crate::diag::DOCK_DUMP_US);
                let state = dock.read(cx).dump(cx);
                let group = this.group.clone();
                this.backend.update(cx, |b, _| {
                    b.store_classic_dock_state(group, state);
                });
            },
        )
        .detach();

        cx.observe_window_bounds(window, |this, window, cx| {
            this.persist_group_geometry(window, cx);
            // MoonUI proportionally repairs live panel sizes when the native viewport changes.
            // Reapply the global preference with this window's fit clamp after that local repair.
            this.sync_auto_rail_width(window, cx);
        })
        .detach();

        // Track window activation for Main's idle auto-close. Mouse movement over an unfocused
        // window must not reset inactivity; gaining focus records activity so charts are not closed
        // immediately after the user returns.
        cx.observe_window_activation(window, |this, window, cx| {
            this.window_active = window.is_window_active();
            if !this.window_active {
                // A window without focus is not told about key-ups, and the platform re-announces
                // the whole modifier state when it comes back. Forgetting here — never on the way
                // in, which arrives AFTER that re-announcement — keeps the snapshot from reading as
                // a Caps Lock press or as a tap of the modifier that Alt+Tab was still holding.
                // A platform that sends no such re-announcement simply costs the first press after
                // the window comes back, which is the safe side of this trade.
                this.modifier_watch.forget();
            }
            if this.window_active {
                let group = this.group.clone();
                this.backend.update(cx, |b, bcx| {
                    b.note_main_input(&group);
                    b.focus_auto_workspace(&group, bcx);
                });
            }
        })
        .detach();

        // Inline order-size editor opened by double-clicking an F1-F6 button. Every valid positive
        // Change updates the group's USD-equivalent preset immediately and schedules a
        // debounced save, so a hotkey or click before Enter uses the text already typed. Blur or
        // Enter performs a direct config save; empty, nonnumeric, or nonpositive input is ignored.
        let size_input = cx.new(|cx| MoonInputState::new(window, cx));
        cx.subscribe(&size_input, |this, inp, ev: &MoonInputEvent, cx| {
            if matches!(ev, MoonInputEvent::Change) {
                let Some((group, ix)) = this.size_edit.as_ref() else {
                    return;
                };
                if let Ok(v) = inp.read(cx).value().trim().replace(',', ".").parse::<f64>() {
                    if v > 0.0 && *ix < 6 {
                        this.backend.update(cx, |b, bcx| {
                            b.set_order_size_value(group, *ix, v);
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
            let Some((group, ix)) = this.size_edit.take() else {
                return;
            };
            let raw = inp.read(cx).value().to_string();
            if let Ok(v) = raw.trim().replace(',', ".").parse::<f64>() {
                if v > 0.0 && ix < 6 {
                    this.backend.update(cx, |b, bcx| {
                        b.set_order_size_value(&group, ix, v);
                        if let Err(error) = b.config.save() {
                            log::warn!("save order size failed: {error}");
                        } else {
                            b.config_dirty = false;
                        }
                        bcx.notify();
                    });
                }
            }
            cx.notify();
        })
        .detach();

        // Inline fixed-sell percentage editor opened by double-clicking an S button. Blur or Enter
        // updates the captured group; empty, nonnumeric, or negative input is
        // ignored.
        let sell_input = cx.new(|cx| MoonInputState::new(window, cx));
        cx.subscribe(&sell_input, |this, inp, ev: &MoonInputEvent, cx| {
            if !matches!(ev, MoonInputEvent::Blur | MoonInputEvent::PressEnter { .. }) {
                return;
            }
            let Some((group, ix)) = this.sell_edit.take() else {
                return;
            };
            if let Ok(v) = inp.read(cx).value().trim().replace(',', ".").parse::<f64>()
                && v.is_finite()
            {
                if v >= 0.0 && ix < 6 {
                    this.backend.update(cx, |b, bcx| {
                        b.edit_group_exit(
                            &group,
                            ClientSettingsEdit::SetFixedSellPct {
                                slot: ix + 1,
                                pct: v,
                            },
                        );
                        b.order_size_rev = b.order_size_rev.wrapping_add(1);
                        bcx.notify();
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
        let blacklist_input = cx.new(|cx| MoonInputState::new(window, cx));
        // Start in multiline mode, but make Enter submit rather than insert a newline.
        let blacklist_area = cx.new(|cx| {
            MoonInputState::new(window, cx)
                .multi_line(true)
                .submit_on_enter(true)
        });
        // Quiet-mode schedule and bypass editors, committing the WHOLE popup (see `shell::quiet`).
        // NOT on Change: every keystroke of an `HH:MM` field is a valid time of its own ("2" is
        // 02:00), so a per-keystroke commit would make half-typed hours the live schedule and could
        // silence real detects mid-edit. Enter and Blur cover the ordinary paths, and closing the
        // popup commits once more — which is what catches a value typed and then dismissed with the
        // ✕ or a click on a checkbox, neither of which blurs the field.
        let quiet_from_input = cx.new(|cx| MoonInputState::new(window, cx));
        let quiet_to_input = cx.new(|cx| MoonInputState::new(window, cx));
        let quiet_charts_input = cx.new(|cx| MoonInputState::new(window, cx));
        for input in [&quiet_from_input, &quiet_to_input, &quiet_charts_input] {
            cx.subscribe(input, |this, _inp, ev: &MoonInputEvent, cx| {
                if matches!(ev, MoonInputEvent::Blur | MoonInputEvent::PressEnter { .. }) {
                    this.commit_quiet_editors(cx);
                }
            })
            .detach();
        }
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

        // Both blacklist editors STAGE into the popup's draft on Blur or Enter; nothing here writes
        // to the core, because everything below the popup's tab strip commits under its OK.
        let stage_bl = |this: &mut Self, ev: &MoonInputEvent, cx: &mut Context<Self>| {
            if matches!(ev, MoonInputEvent::Blur | MoonInputEvent::PressEnter { .. }) {
                this.stage_blacklist_text(cx);
            }
        };
        cx.subscribe(
            &blacklist_input,
            move |this, _inp, ev: &MoonInputEvent, cx| stage_bl(this, ev, cx),
        )
        .detach();
        cx.subscribe(
            &blacklist_area,
            move |this, _inp, ev: &MoonInputEvent, cx| stage_bl(this, ev, cx),
        )
        .detach();

        // Focus the window root immediately so hotkeys, including F keys, work even when Main is
        // empty and the dock has nothing else to focus; see the `focus` field.
        let focus = cx.focus_handle();
        window.focus(&focus, cx);

        // Armed HERE and never from `render`: a view that armed its own repaint chain while
        // rendering would keep itself awake through its own repaints forever (`pulse::arm`).
        let settings_hint_at =
            (!backend.read(cx).config.core_ever_configured()).then(std::time::Instant::now);
        let mut shell = Self {
            settings_hint_at,
            settings_hint_armed: false,
            backend,
            updater,
            group,
            dock,
            classic_dock_layout: None,
            classic_only_panels: Vec::new(),
            auto_only_panels: Vec::new(),
            exchange_logo_prewarm_started: false,
            exchange_logos_ready: false,
            applying_auto_topology: false,
            auto_topology_guard_generation: 0,
            workspace_resize_state,
            applied_auto_rail_width: initial_auto_rail_width,
            applied_workspace_mode: moon_core::config::WorkspaceMode::Classic,
            last_auto_surface_revision,
            workspace_sync_pending: false,
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
            strat_slot_menu: None,
            strat_slots_open: false,
            tp_slider_normal,
            tp_slider_ext,
            tp_fine_slider,
            sl_slider,
            lev_slider,
            tp_input,
            sl_input,
            lev_input,
            core_settings_tab: core_settings_popup::CoreSettingsTab::default(),
            core_settings_draft: None,
            core_settings_seed: None,
            core_settings_inputs: std::collections::HashMap::new(),
            core_settings_sliders: std::collections::HashMap::new(),
            core_settings_seed_gen: 0,
            blacklist_input,
            blacklist_area,
            open_metric_popup: None,
            focus,
            modifier_watch: moon_ui::MoonHotkeyModifierWatch::default(),
            window_active: true,
            header_core_selector_open: false,
            core_settings_open: false,
            core_settings_target: None,
            core_settings_cancel_confirm: false,
            core_settings_bl_expanded: false,
            quiet_settings_open: false,
            quiet_from_input,
            quiet_to_input,
            quiet_charts_input,
            ticker_popup_open: false,
            ticker_popup_hovered: false,
            ticker_input,
        };
        // Every Shell is constructed from its saved/default Classic dock first. Persisted Auto then
        // captures that local named layout and applies the process-wide topology before first frame.
        shell.apply_workspace_mode(initial_workspace_mode, window, cx);
        // Re-stamp the hint AFTER the workspace is applied. The stamp above has to happen during
        // construction because it reads the config, but `pulse::ATTENTION` is measured from it, and
        // on a fresh profile the layout, fonts and dock all settle between the two — spending the
        // window before the user's eyes reach the screen. `map` preserves the `None` decision, so
        // the `core_ever_configured` gate is untouched and a configured user still sees nothing.
        shell.settings_hint_at = shell.settings_hint_at.map(|_| std::time::Instant::now());
        crate::pulse::arm(
            &mut shell,
            cx,
            |s| &mut s.settings_hint_armed,
            |s| {
                s.settings_hint_at
                    .is_some_and(|at| at.elapsed() < crate::pulse::ATTENTION)
            },
        );
        shell
    }

    /// When the first-run Settings-gear hint was armed, for the toolbar that draws it.
    ///
    /// Returns:
    ///     The arming instant while a hint exists, else `None`.
    pub(crate) fn settings_hint_at(&self) -> Option<std::time::Instant> {
        self.settings_hint_at
    }

    /// Subscribe the TP/SL/leverage popup editors to guarded writes and live field updates.
    ///
    /// Each callback writes only while its metric popup is still open for the group-local exit or
    /// leverage core and market from which it was seeded. The entities are passed in because `new`
    /// registers these subscriptions before `Shell` itself has been assembled.
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
        // the group's exact mode so the edit uses the same range.
        cx.subscribe(tp_input, |this, inp, ev: &MoonInputEvent, cx| {
            if !matches!(ev, MoonInputEvent::Blur | MoonInputEvent::PressEnter { .. }) {
                return;
            }
            if let Ok(v) = inp.read(cx).value().trim().replace(',', ".").parse::<f64>()
                && v.is_finite()
            {
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
            if let Ok(v) = inp.read(cx).value().trim().replace(',', ".").parse::<f32>()
                && v.is_finite()
            {
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
