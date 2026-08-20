//! Detached chart tabs: OS-window lifecycle, including Classic-mode creation, restoration,
//! repinning, geometry and scale persistence, plus the `DetachedChartHost` window view. Auto
//! refuses new detachment before any lifecycle or persistence work. The tab strip interacts with
//! this subsystem through a small set of `pub(super)` methods called from event and observe paths.

use gpui::*;
use moon_ui::{MoonBackgroundPolicy, Root};

use super::detached_host::DetachedChartHost;
use super::{AddChartStack, ChartTabs, Tab, chart_pane_label, coin_search};
use crate::persistence::chart_persist::{self, StackLayoutMode, StackOrientation};
use moon_core::config::{ChartBucket, WorkspaceMode};
use moon_core::session::CoreId;

/// Return whether ChartTabs may create a new independent chart window in this workspace mode.
///
/// Auto permits in-window dock editing but owns all navigation through the shared main window, so
/// its independent chart detach path must stop before geometry lookup, window creation, or spec
/// mutation. Existing detached windows and their persistence remain untouched.
pub(super) fn chart_detach_allowed(mode: WorkspaceMode) -> bool {
    mode != WorkspaceMode::AutoTrading
}

/// Geometry a chart window falls back to the first time its tab is detached.
///
/// Its origin is deliberately NOT a display hint: the point lies inside the primary display, so
/// treating it as a remembered position would answer "primary" for every first detach and hide the
/// monitor the group window is actually on. [`ChartWindowPlacement::geom`] being `None` is what says
/// "no real position", and this is materialized only after that question is answered.
const FIRST_DETACH_GEOM: chart_persist::WinGeom = chart_persist::WinGeom {
    x: 200,
    y: 160,
    w: 900,
    h: 620,
    // A window nobody has opened yet belongs to no display; the owner's is chosen instead.
    display_uuid: None,
};

/// Where and how one detached chart window should be created.
///
/// Grouped rather than passed as loose arguments because two of the three answer one question
/// between them — which monitor this window belongs on — and each is a different partial answer.
pub(super) struct ChartWindowPlacement {
    /// Remembered position and size from `charts.json`, or `None` for a tab detached for the first
    /// time. Only a remembered origin may pick a display; see [`FIRST_DETACH_GEOM`].
    pub geom: Option<chart_persist::WinGeom>,
    /// Whether this window is being restored at startup rather than detached by a gesture.
    /// Restoration must not raise its windows: it opens all of them and would steal focus per window.
    pub restored: bool,
    /// Display of the group window, captured by the caller. Inside that window's own update it is
    /// the ONLY resolvable answer, because its slot in `cx.windows` is taken.
    pub owner_display: Option<DisplayId>,
}

impl ChartTabs {
    /// Gather this group's detached chart windows from the Main window's tab-strip control.
    /// Every platform activates them; Windows additionally restores, shows, and cascades them onto
    /// the primary display, while the position reset is a no-op elsewhere.
    pub(super) fn gather_windows(&mut self, cx: &mut Context<Self>) {
        let group = self.group.clone();
        let handles: Vec<_> = self
            .backend
            .read(cx)
            .detached_chart_windows
            .iter()
            .filter(|(g, _)| *g == group)
            .map(|(_, h)| *h)
            .collect();
        for (i, handle) in handles.into_iter().enumerate() {
            let _ = handle.update(cx, |_, window, _| {
                crate::window::windowing::reset_window_onscreen(window, i);
                window.activate_window();
            });
        }
    }

    /// Detach an AddToChart or Custom tab into a separate OS window and remove it from the strip.
    /// Custom tabs live in `self.custom`, regular tabs in `self.add`, and both use `AddChartStack`.
    ///
    /// Args:
    ///     tab: Attached AddToChart or Custom tab to move into its own window.
    ///     owner_display: Display of the tab strip's own window, captured by the event handler
    ///         because this runs inside that window's update and the owner fallback cannot resolve.
    ///     cx: Parent context used to open the window and synchronize active state.
    ///
    /// Returns:
    ///     Nothing; Auto, Main, missing tabs, and failed window creation are ignored.
    pub(super) fn detach(
        &mut self,
        tab: Tab,
        owner_display: Option<DisplayId>,
        cx: &mut Context<Self>,
    ) {
        if !chart_detach_allowed(self.backend.read(cx).workspace_mode(&self.group)) {
            return;
        }
        let (n, bucket, is_custom) = match tab.clone() {
            Tab::Add(n, b) => (n, b, false),
            Tab::Custom(n, b) => (n, b, true),
            Tab::Main => return,
        };
        let from = if is_custom { &self.custom } else { &self.add };
        let Some(pos) = from
            .iter()
            .position(|(num, c, _)| *num == n && *c == bucket)
        else {
            return;
        };
        let panel = from[pos].2.clone();
        // Previously detached geometry, or `None` so the window falls back to the first-detach cascade.
        let geom = self.spec_geom(cx, n, &bucket);
        // The geometry the window ACTUALLY opened with, which is not the requested one for a first
        // detach: its cascade point is resolved onto the chosen display. Persisting the request
        // instead would move the window on the next launch.
        let Some((geom, window)) = self.open_chart_window(
            n,
            panel.clone(),
            bucket.clone(),
            ChartWindowPlacement {
                geom,
                restored: false,
                owner_display,
            },
            cx,
        ) else {
            return;
        };
        // A visible detached tab retains its own order-book demand, so clear the suspend gate.
        panel.update(cx, |p, pcx| {
            p.set_orderbook_suspended(false, pcx);
            p.set_scene_visible(false, pcx);
        });
        if is_custom {
            self.custom
                .retain(|(num, c, _)| !(*num == n && *c == bucket));
        } else {
            self.add.remove(pos);
        }
        self.detached.push((n, bucket.clone(), panel));
        if self.active == tab {
            self.active = Tab::Main;
            self.sync_seen_for_active(cx);
            self.sync_active_scale(cx);
            self.sync_inactive_chart_visibility(cx);
            self.persist_scales(cx);
            self.sync_main_chart_target(cx);
        }
        // Mark the tab detached in `charts.json` so it restores as a window next launch.
        self.upsert_spec(cx, n, &bucket, |s| s.detached = Some(geom));
        moon_core::detect_diag::line(&format!(
            "[detach] n={n} bucket={bucket:?} → detached=Some({},{},{},{})",
            geom.x, geom.y, geom.w, geom.h
        ));
        // Raised last, once the tab has left the strip and its spec is recorded — the same order
        // both panel routes follow. On macOS a window created on another display otherwise stays
        // hidden until the next application activation.
        crate::window::windowing::activate_new_window(window.into(), cx);
        cx.notify();
    }

    /// Open an OS window for a detached tab during either detachment or startup restoration.
    /// The panel remains in `detached` so ordinary Add tabs receive ingest by number/core, while
    /// Custom tabs seed from their specs and the `gpu_canvas` moves with the window's GPUI scene.
    /// `DetachedChartHost` persists geometry and requests repinning on close; group tracking closes
    /// it with the group window.
    ///
    /// Returns the geometry the window was created with — which differs from the requested one when
    /// a first-detach cascade point had to be resolved onto the chosen display — together with its
    /// handle, or `None` when the window could not be created. The handle goes back to the caller
    /// rather than being raised here, because only the caller knows whether a gesture asked for this
    /// window and whether the move it belongs to has finished.
    fn open_chart_window(
        &mut self,
        n: u32,
        panel: Entity<AddChartStack>,
        bucket: ChartBucket,
        placement: ChartWindowPlacement,
        cx: &mut Context<Self>,
    ) -> Option<(chart_persist::WinGeom, WindowHandle<Root>)> {
        let ChartWindowPlacement {
            geom,
            restored,
            owner_display,
        } = placement;
        let remembered = geom.is_some();
        let geom = geom.unwrap_or(FIRST_DETACH_GEOM);
        // Multi-monitor restoration requires `display_id`; otherwise GPUI creates the window on
        // the primary display and rejects bounds outside it. Non-macOS resolves the display from
        // the saved point — but only a REMEMBERED one, because the first-detach cascade point lies
        // inside the primary display and would answer for every monitor. macOS skips containment
        // entirely (its coordinates are display-relative) and relies on the owner. Both detach and
        // startup restoration run while the group window's slot is borrowed, so `owner_display`
        // captured by the caller is the only display either of them can resolve.
        let origin = point(px(geom.x as f32), px(geom.y as f32));
        let owner = self
            .backend
            .read(cx)
            .group_windows
            .get(&self.group)
            .copied()
            .map(Into::into);
        let display_id = crate::window::windowing::saved_or_owner_display_id(
            geom.display_uuid,
            remembered.then_some(origin),
            owner,
            owner_display,
            cx,
        );
        // The cascade point is relative to a display, and Windows reads window coordinates as
        // global: left as-is against a non-primary `display_id` it falls outside that display, and
        // the platform layer replaces the whole rectangle — 900x620 included — with default bounds.
        let origin = if remembered {
            origin
        } else {
            crate::window::windowing::cascade_origin_on(origin, display_id, cx)
        };
        // The identity of the display finally chosen, so what gets persisted names the monitor the
        // window is on rather than the one it was last remembered on.
        let geom = chart_persist::WinGeom {
            x: f32::from(origin.x) as i32,
            y: f32::from(origin.y) as i32,
            // The display actually chosen, falling back to what was remembered — the one rule the
            // panel and tool windows carry as `keeping_display_of`. An unknown identity is not
            // evidence the window moved: off macOS it is the ordinary answer, so overwriting here
            // would wipe a `charts.json` carried over from a Mac on the first detach.
            display_uuid: crate::window::windowing::display_identity(display_id, cx)
                .or(geom.display_uuid),
            ..geom
        };
        let mut opts = crate::window::windowing::detached_chart_window_options(
            format!(
                "MoonTerminal — {}",
                chart_pane_label(&self.backend, &self.group, n, &bucket, cx)
            ),
            WindowBounds::Windowed(Bounds {
                origin,
                size: size(px(geom.w as f32), px(geom.h as f32)),
            }),
            display_id,
        );
        // Clear with the themed chart background. The transparent window body must not cover the
        // own-pass `UnderScene`, so clear color supplies the background beneath and between charts.
        let bg = self.theme.bg;
        opts.window_clear_color = Some(gpui::rgb(
            ((bg[0] as u32) << 16) | ((bg[1] as u32) << 8) | bg[2] as u32,
        ));
        let backend = self.backend.clone();
        let group = self.group.clone();
        // Give restored windows their saved logical size so first render can correct DPI-change shrinkage.
        let restore_size = restored.then(|| size(px(geom.w as f32), px(geom.h as f32)));
        let host_bucket = bucket.clone();
        let opened = cx.open_window(opts, move |window, cx| {
            crate::window::windowing::configure_chart_clear_color(window, cx);
            let host = cx.new(|cx| {
                DetachedChartHost::new(
                    panel,
                    backend,
                    group,
                    n,
                    host_bucket,
                    restored,
                    restore_size,
                    window,
                    cx,
                )
            });
            cx.new(|cx| Root::new(host, window, cx).background_policy(MoonBackgroundPolicy::NoFill))
        });
        if let Ok(handle) = opened {
            let group = self.group.clone();
            self.backend.update(cx, |b, _| {
                b.detached_chart_windows.push((group, handle));
            });
            Some((geom, handle))
        } else {
            log::warn!(
                "failed to open detached chart window for group={} n={} bucket={:?}",
                self.group,
                n,
                bucket
            );
            None
        }
    }

    /// Return the persisted geometry for a detached tab window, if present in `charts.json`.
    fn spec_geom(
        &self,
        cx: &App,
        num: u32,
        bucket: &ChartBucket,
    ) -> Option<chart_persist::WinGeom> {
        self.backend
            .read(cx)
            .chart_specs
            .iter()
            .find(|s| s.matches(&self.group, num, bucket))
            .and_then(|s| s.detached)
    }

    /// Find or create a tab spec by group, number, and bucket, apply a mutation, and mark it dirty.
    /// This wraps [`super::common::upsert_spec`] so main and detached windows share one path.
    pub(super) fn upsert_spec(
        &self,
        cx: &mut Context<Self>,
        num: u32,
        bucket: &ChartBucket,
        f: impl FnOnce(&mut chart_persist::ChartTabSpec),
    ) {
        super::common::upsert_spec(&self.backend, &self.group, num, bucket, cx, f);
    }

    /// Drain detached-tab repin requests after a user closes a host window.
    /// Move the panel from `detached` to `add` and clear the spec's detached flag. During app quit
    /// requests remain undrained so the detached window restores on the next launch.
    pub(super) fn drain_chart_repin(&mut self, cx: &mut Context<Self>) {
        // Never repin during app quit; closing detached windows must preserve restoration state,
        // and `on_app_quit` has already performed the final save.
        if self.backend.read(cx).quitting {
            return;
        }
        let group = self.group.clone();
        let reqs: Vec<(u32, ChartBucket)> = self.backend.update(cx, |b, _| {
            let mut out = Vec::new();
            b.chart_repin_request.retain(|(g, n, c)| {
                if *g == group {
                    out.push((*n, c.clone()));
                    false
                } else {
                    true
                }
            });
            out
        });
        for (n, bucket) in reqs {
            // `custom_coins` alone classifies the tab as Custom; retrieve its optional label afterward.
            let (is_custom, custom_label) = {
                let specs = &self.backend.read(cx).chart_specs;
                let spec = specs.iter().find(|s| s.matches(&self.group, n, &bucket));
                (
                    spec.is_some_and(|s| s.custom_coins.is_some()),
                    spec.and_then(|s| s.custom_label.clone()),
                )
            };
            if let Some(p) = self
                .detached
                .iter()
                .position(|(num, c, _)| *num == n && *c == bucket)
            {
                let (num, c, pnl) = self.detached.remove(p);
                if is_custom {
                    self.custom.push((num, c, pnl));
                    if let Some(label) = custom_label {
                        self.custom_labels.entry(n).or_insert(label);
                    }
                } else {
                    self.add.push((num, c, pnl));
                    self.add.sort_by_key(|(num, c, _)| (*num, c.clone()));
                }
            }
            self.upsert_spec(cx, n, &bucket, |s| s.detached = None);
            moon_core::detect_diag::line(&format!(
                "[repin] n={n} bucket={bucket:?} custom={is_custom} → detached=None (окно закрыли/репин)"
            ));
            // A repinned Custom tab is inactive in the strip, so start its five-second order-book gate.
            if is_custom {
                self.refresh_orderbook_gates(cx);
            }
            cx.notify();
        }
    }

    /// Persist changed scales for Main, regular Add tabs, and detached tabs to `charts.json`.
    /// Main uses number zero; Custom tabs still in `self.custom` are omitted by this method.
    pub(super) fn persist_scales(&self, cx: &mut Context<Self>) {
        let mut items: Vec<(u32, ChartBucket, Option<f32>)> =
            vec![(0, ChartBucket::Shared, self.main.read(cx).scale())];
        for (n, c, p) in &self.add {
            items.push((*n, c.clone(), p.read(cx).scale()));
        }
        for (n, c, p) in &self.detached {
            items.push((*n, c.clone(), p.read(cx).scale()));
        }
        for (num, bucket, scale) in items {
            let (cur, exists) = {
                let specs = &self.backend.read(cx).chart_specs;
                let found = specs.iter().find(|s| s.matches(&self.group, num, &bucket));
                (found.and_then(|s| s.scale), found.is_some())
            };
            if cur != scale && (scale.is_some() || exists) {
                self.upsert_spec(cx, num, &bucket, move |s| s.scale = scale);
            }
        }
    }

    /// Restore detached windows deferred from `charts.json`.
    /// Opening OS windows during render invalidates GPUI's element arena, so construction queues
    /// the work through `cx.defer` instead.
    ///
    /// Args:
    ///     owner_display: Display of the group window, read by `ChartTabs::new` from the window it
    ///         is being built into. The deferred work below cannot ask for it: `group_windows` is
    ///         filled only after `open_window` returns, so at that point the group window is neither
    ///         registered nor addressable, and every restored chart would fall back to the primary
    ///         display.
    ///     cx: Parent context used to defer window creation and retain stack observers.
    ///
    /// Returns:
    ///     Nothing; pending specifications are drained and restored asynchronously.
    pub(super) fn restore_detached(
        &mut self,
        owner_display: Option<DisplayId>,
        cx: &mut Context<Self>,
    ) {
        if self.restore_pending.is_empty() {
            return;
        }
        let pending = std::mem::take(&mut self.restore_pending);
        let this = cx.entity();
        cx.defer(move |app| {
            this.update(app, |this, cx| {
                let (epoch, theme) = (this.epoch, this.theme.clone());
                // Detached charts are always independent because ownership would raise Main when
                // clicking a chart on another display. Policy plus a Windows fallback hides taskbar entries.
                for (n, bucket, geom, scale) in pending {
                    let backend = this.backend.clone();
                    let workspace_group = this.group.clone();
                    let panel = cx.new(|_| {
                        AddChartStack::new(
                            backend,
                            workspace_group,
                            n,
                            bucket.clone(),
                            epoch,
                            theme.clone(),
                        )
                    });
                    if scale.is_some() {
                        panel.update(cx, |p, pcx| p.set_scale(scale, pcx));
                    }
                    // Detect ingest does not populate a detached custom tab, so seed its markets
                    // and layout, orientation, and pin settings directly from the spec.
                    #[allow(clippy::type_complexity)]
                    let custom: Option<(
                        Vec<(CoreId, String)>,
                        Option<String>,
                        (Option<StackLayoutMode>, Option<u16>, Option<u16>),
                        Option<StackOrientation>,
                        Option<bool>,
                        Option<bool>,
                        Option<bool>,
                        Option<(CoreId, String)>,
                        bool,
                        Option<chart_persist::PriceAxisPos>,
                        Option<bool>,
                        Option<moon_core::config::ChartGraphicsCfg>,
                        Option<moon_core::config::ChartLabelsCfg>,
                    )> = {
                        let specs = &this.backend.read(cx).chart_specs;
                        specs
                            .iter()
                            .find(|s| s.matches(&this.group, n, &bucket))
                            .and_then(|s| {
                                s.custom_coins.clone().map(|coins| {
                                    (
                                        coins,
                                        s.custom_label.clone(),
                                        (
                                            s.layout_mode,
                                            s.layout_height_fit,
                                            s.layout_height_scroll,
                                        ),
                                        s.layout_orientation,
                                        s.orderbook_enabled,
                                        s.show_zone,
                                        s.auto_pin,
                                        s.compare_anchor.clone(),
                                        s.compare_orderbook_only,
                                        s.price_axis_pos,
                                        s.time_axis_visible,
                                        s.chart_graphics,
                                        s.chart_labels,
                                    )
                                })
                            })
                    };
                    if let Some((
                        coins,
                        label,
                        layout,
                        orientation,
                        ob,
                        sz,
                        ap,
                        anchor,
                        broom,
                        axis_pos,
                        time_axis,
                        chart_graphics,
                        chart_labels,
                    )) = custom
                    {
                        panel.update(cx, |s, c| {
                            s.set_hold_vacated(false);
                            s.set_orientation(
                                Some(orientation.unwrap_or(StackOrientation::Horizontal)),
                                c,
                            );
                            s.set_layout(layout.0, layout.1, layout.2, c);
                            if let Some(v) = ob {
                                s.set_orderbook_enabled(Some(v), c);
                            }
                            if let Some(v) = sz {
                                s.set_show_zone(Some(v), c);
                            }
                            if let Some(v) = ap {
                                s.set_auto_pin(Some(v), c);
                            }
                            if axis_pos.is_some() {
                                s.set_price_axis_pos(axis_pos, c);
                            }
                            if time_axis.is_some() {
                                s.set_time_axis_visible(time_axis, c);
                            }
                            // BEFORE the markets: adding one starts this tile's durable history
                            // request, and a tab whose own setting draws no trades at all skips that
                            // read entirely. Applying the override afterwards would pay for a read
                            // whose result is then hidden.
                            if chart_graphics.is_some() {
                                s.set_chart_graphics(chart_graphics, c);
                            }
                            if chart_labels.is_some() {
                                s.set_chart_labels(chart_labels, c);
                            }
                            for (core, market) in &coins {
                                s.add_coin(*core, market, coin_search::MANUAL_COIN_TTL_MS, c);
                            }
                            s.pin_all(c);
                        });
                        if anchor.is_some() || broom {
                            panel.update(cx, |s, c| s.restore_compare(anchor.clone(), broom, c));
                        }
                        if let Some(label) = label {
                            this.custom_labels.insert(n, label);
                        }
                        this.next_custom_num = this.next_custom_num.max(n + 1);
                        // This membership subscription is ineffective while the stack is detached;
                        // the detached host persists it then, and the subscription resumes after repin.
                        this.watch_custom_stack(n, &bucket, &panel, cx);
                    } else {
                        this.watch_regular_stack_target(&panel, cx);
                    }
                    if this
                        .open_chart_window(
                            n,
                            panel.clone(),
                            bucket.clone(),
                            ChartWindowPlacement {
                                // Restored geometry is remembered by definition: it came from `charts.json`.
                                geom: Some(geom),
                                restored: true,
                                owner_display,
                            },
                            cx,
                        )
                        .is_some()
                    {
                        panel.update(cx, |p, pcx| p.set_scene_visible(false, pcx));
                        this.detached.push((n, bucket, panel));
                    }
                }
                cx.notify();
            });
        });
    }
}
