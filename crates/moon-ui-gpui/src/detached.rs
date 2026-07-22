//! Dock panels detached into separate OS windows, ported from egui `app/detached.rs` and
//! `WindowLayout.detached`. Detaching removes a panel from its dock through
//! `TabPanel::remove_panel`. `detached.json` persists the detached state and current window
//! geometry so startup can reopen the panel detached. Closing the detached window requests a repin
//! through `Backend.repin_request`, which Shell drains to return the panel to its owner's dock.
//!
//! Each window contains a fresh panel instance backed by the shared `Backend`, so its data remains
//! live. [`DetachedWindow`] renders it, observes window geometry, and requests repinning when
//! released. Detached chart tabs use a separate persistence subsystem because their panel state
//! requires serialization.

use std::rc::Rc;

use gpui::prelude::FluentBuilder;
use gpui::*;
use moon_ui::{
    MoonBackgroundPolicy, MoonPalette, MoonWindowFrame, PanelView, Root, h_flex, v_flex,
};
use serde::{Deserialize, Serialize};

use rust_i18n::t;

use crate::Backend;
use crate::panels::{AssetsView, CoreStatusView, LogPanel, OrdersPanel, ReportPanel, StubPanel};
use moon_core::config::paths;

/// Persisted description of one detached panel window: panel name, source group, and geometry.
#[derive(Clone, Serialize, Deserialize)]
pub struct DetachedSpec {
    pub group: String,
    /// Stable panel name: Orders, Assets, Log, Report, Alerts, or CoreStatus.
    pub panel: String,
    pub x: i32,
    pub y: i32,
    pub w: u32,
    pub h: u32,
}

impl DetachedSpec {
    /// Creates a specification with the default geometry for a panel's first detachment.
    pub fn new(group: String, panel: String) -> Self {
        Self {
            group,
            panel,
            x: 200,
            y: 160,
            w: 1100,
            h: 520,
        }
    }

    /// Creates a specification using this panel's last geometry from `layout.detached_geom`. That
    /// memory survives repinning, so detaching again restores the previous position and size. A
    /// panel without saved geometry uses the default.
    pub fn with_saved_geom(
        backend: &Entity<crate::Backend>,
        app: &App,
        group: String,
        panel: String,
    ) -> Self {
        let mut spec = Self::new(group, panel);
        if let Some(g) = backend
            .read(app)
            .layout
            .detached_geom
            .get(&geom_key(&spec.group, &spec.panel))
        {
            spec.x = g.x;
            spec.y = g.y;
            spec.w = g.w;
            spec.h = g.h;
        }
        spec
    }
}

/// Builds a detached-geometry key for `layout.detached_geom`. The `panel:` prefix separates GPUI
/// dock-panel keys from legacy egui keys such as `g:<idx>` and `o:<idx>:<group>`.
fn geom_key(group: &str, panel: &str) -> String {
    format!("panel:{group}/{panel}")
}

/// Loads detached panel specifications from `detached.json`, returning an empty list if absent or invalid.
pub fn load_all() -> Vec<DetachedSpec> {
    match std::fs::read_to_string(paths::detached_path()) {
        Ok(s) => serde_json::from_str(&s).unwrap_or_else(|e| {
            log::warn!("detached.json битый ({e}) → без откреплённых");
            Vec::new()
        }),
        Err(_) => Vec::new(),
    }
}

/// Saves detached panel specifications to `detached.json`; failures are logged but nonfatal.
pub fn save_all(list: &[DetachedSpec]) {
    match serde_json::to_string_pretty(list) {
        Ok(s) => {
            if let Err(e) = moon_core::config::write_file_atomic(
                &paths::detached_path(),
                s.as_bytes(),
                "detached.json",
            ) {
                log::warn!("не записал detached.json: {e}");
            }
        }
        Err(e) => log::warn!("не сериализовал detached.json: {e}"),
    }
}

/// True for panels that can be moved into a detached OS window.
pub fn supports_panel(name: &str) -> bool {
    matches!(
        name,
        "Orders" | "Assets" | "Log" | "Report" | "Alerts" | "CoreStatus"
    )
}

/// Builds a fresh dock-panel instance by name as `Rc<dyn PanelView>` for detached-window content or
/// repinning into a dock.
pub fn build_panel(
    name: &str,
    group: &str,
    backend: &Entity<Backend>,
    window: &mut Window,
    cx: &mut App,
) -> Option<Rc<dyn PanelView>> {
    let panel: Rc<dyn PanelView> =
        match name {
            "Orders" => Rc::new(
                cx.new(|cx| OrdersPanel::new(backend.clone(), group.to_string(), window, cx)),
            ),
            "Log" => {
                Rc::new(cx.new(|cx| LogPanel::new(backend.clone(), group.to_string(), window, cx)))
            }
            "Report" => Rc::new(
                cx.new(|cx| ReportPanel::new(backend.clone(), group.to_string(), window, cx)),
            ),
            "Assets" => Rc::new(cx.new(|cx| {
                AssetsView::restored_group(backend.clone(), group.to_string(), window, cx)
            })),
            "Alerts" => Rc::new(cx.new(|cx| {
                crate::panels::AlertsPanel::new(backend.clone(), group.to_string(), window, cx)
            })),
            "CoreStatus" => Rc::new(cx.new(|cx| {
                CoreStatusView::restored_group(backend.clone(), group.to_string(), window, cx)
            })),
            _ => return None,
        };
    Some(panel)
}

/// Detached-window wrapper that renders a panel, observes geometry, updates `Backend.detached`, and
/// requests repinning on release. The backend drain performs the debounced save.
pub struct DetachedWindow {
    backend: Entity<Backend>,
    group: String,
    panel: String,
    content: AnyView,
    /// ID and state for a configured window-header auto-width reset button. An active dock tab
    /// supplies this through `Panel::toolbar_buttons`; a detached window has its own header, so
    /// configured branches pass table state explicitly. None means this detached branch exposes no
    /// reset callback; for example, Assets has table state but does not configure this button here.
    widths_reset: Option<(&'static str, Entity<moon_ui::MoonDataTableState>)>,
}

impl DetachedWindow {
    fn new(
        backend: Entity<Backend>,
        group: String,
        panel: String,
        content: AnyView,
        widths_reset: Option<(&'static str, Entity<moon_ui::MoonDataTableState>)>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        // Persist geometry from causal bounds events instead of polling during render or backend pulses.
        cx.observe_window_bounds(window, |this, window, cx| {
            this.persist_geometry(window, cx);
        })
        .detach();
        // Closing requests a repin into the owner's dock. During shutdown, the final on_app_quit
        // flush persists the detached specifications before windows are released, so release-time
        // repin requests cannot erase the saved detachment for the next launch.
        let (g, p) = (group.clone(), panel.clone());
        cx.on_release(move |this, app| {
            this.backend.update(app, |b, _| {
                b.repin_request.push((g.clone(), p.clone()));
            });
        })
        .detach();
        Self {
            backend,
            group,
            panel,
            content,
            widths_reset,
        }
    }

    fn persist_geometry(&mut self, window: &Window, cx: &mut Context<Self>) {
        let Some(geom) = crate::windowing::window_geom(window) else {
            return;
        };
        let (group, panel) = (self.group.clone(), self.panel.clone());
        self.backend.update(cx, |bk, _| {
            if let Some(s) = bk
                .detached
                .iter_mut()
                .find(|s| s.group == group && s.panel == panel)
            {
                if (s.x, s.y, s.w, s.h) != geom {
                    s.x = geom.0;
                    s.y = geom.1;
                    s.w = geom.2;
                    s.h = geom.3;
                    bk.detached_dirty = true;
                }
            }
            // Retain geometry independently of the detached specification. Repinning removes the
            // specification but keeps this memory, so the next detachment restores the same bounds.
            let key = geom_key(&group, &panel);
            let changed = bk
                .layout
                .detached_geom
                .get(&key)
                .map(|g| (g.x, g.y, g.w, g.h) != geom)
                .unwrap_or(true);
            if changed {
                bk.layout.detached_geom.insert(
                    key,
                    moon_core::config::layout::GeomRect {
                        x: geom.0,
                        y: geom.1,
                        w: geom.2,
                        h: geom.3,
                    },
                );
                bk.layout_dirty = true;
            }
        });
    }
}

impl Render for DetachedWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        crate::diag::bump(&crate::diag::DETACHED_RENDER);
        // Detached panels share Main's group but live in a separate OS window whose mouse movement
        // Shell cannot observe. Record group activity from any widget in this active window so Main's
        // inactivity policy does not close its chart while the user works in a detached panel.
        {
            let backend = self.backend.clone();
            let group = self.group.clone();
            // Use the capture phase, as Shell does, so activity is recorded before bubble handlers
            // and cannot be suppressed by their stop_propagation calls.
            window.on_mouse_event::<MouseMoveEvent>(move |_e, phase, window, cx| {
                if phase == DispatchPhase::Capture && window.is_window_active() {
                    backend.update(cx, |b, _| b.note_main_input(&group));
                }
            });
        }
        let p = MoonPalette::active(cx);
        let title = format!(
            "{} · {}",
            crate::panel_meta::panel_title(&self.panel),
            self.group
        );
        v_flex()
            .size_full()
            .bg(rgb(p.shell))
            .text_color(rgb(p.text))
            .child(
                h_flex()
                    .h(crate::design::fit_h_px(cx, 34.0, 13.0, 10.5))
                    .w_full()
                    .items_center()
                    .gap(crate::design::ui_px(cx, 8.0))
                    .pl(crate::design::ui_px(
                        cx,
                        crate::design::titlebar_leading_inset(),
                    ))
                    .pr(crate::design::ui_px(cx, 6.0))
                    .border_b_1()
                    .border_color(rgb(p.border))
                    .bg(rgb(p.shell_high))
                    .child(
                        MoonWindowFrame::detached_panel("detached-panel-title-drag", 0.0)
                            .title_cluster(title, cx)
                            .h_full()
                            .flex_1()
                            .min_w_0()
                            .items_center(),
                    )
                    // Recalculate panel table widths when this button is clicked, matching the
                    // active dock tab action.
                    .when_some(self.widths_reset.clone(), |this, (id, state)| {
                        this.child(crate::table_persist::reset_button(id, &state))
                    })
                    .when(crate::design::show_custom_window_controls(), |this| {
                        this.child(
                            MoonWindowFrame::detached_panel("detached-panel-window-controls", 0.0)
                                .header_height(34.0)
                                .show_controls(true)
                                .visual_controls(cx),
                        )
                    }),
            )
            .child(
                div()
                    .flex_1()
                    .w_full()
                    .overflow_hidden()
                    .child(self.content.clone()),
            )
    }
}

/// Opens a detached window from a specification, either while restoring each saved specification at
/// startup or after a new detach action. The content is a fresh panel and geometry comes from `spec`.
pub fn spawn(
    app: &mut App,
    backend: &Entity<Backend>,
    spec: &DetachedSpec,
    owner: Option<AnyWindowHandle>,
) -> anyhow::Result<WindowHandle<Root>> {
    let owner = owner.or_else(|| {
        backend
            .read(app)
            .group_windows
            .get(&spec.group)
            .copied()
            .map(Into::into)
    });
    let bounds = Bounds {
        origin: point(px(spec.x as f32), px(spec.y as f32)),
        size: size(px(spec.w as f32), px(spec.h as f32)),
    };
    // On multiple displays, choose by saved position outside macOS or fall back to the owner window.
    // Otherwise the window opens on the primary display, especially on macOS where x/y are display-relative.
    let display_id =
        crate::windowing::saved_or_owner_display_id(Some(bounds.origin), owner, None, app);
    let opts = crate::windowing::detached_panel_window_options(
        format!(
            "{} — MoonTerminal",
            crate::panel_meta::panel_title(&spec.panel)
        ),
        WindowBounds::Windowed(bounds),
        display_id,
        owner,
    );
    let backend = backend.clone();
    let spec = spec.clone();
    app.open_window(opts, move |window, cx| {
        crate::windowing::configure_shell_clear_color(window, cx);
        // Configure the window-header auto-width reset button only for the branches below that
        // expose an explicit reset ID and table state.
        let mut widths_reset: Option<(&'static str, Entity<moon_ui::MoonDataTableState>)> = None;
        let content: AnyView = match spec.panel.as_str() {
            "Orders" => {
                let p =
                    cx.new(|cx| OrdersPanel::new(backend.clone(), spec.group.clone(), window, cx));
                // Detached tables use the shared `:win` detached-mode width context and keys across
                // windows, separate from the docked context but not unique per OS window.
                p.update(cx, |this, cx| this.mark_table_detached(cx));
                widths_reset = Some(("orders-reset-widths-win", p.read(cx).table_state()));
                p.into()
            }
            "Log" => cx
                .new(|cx| LogPanel::new(backend.clone(), spec.group.clone(), window, cx))
                .into(),
            "Report" => {
                let p =
                    cx.new(|cx| ReportPanel::new(backend.clone(), spec.group.clone(), window, cx));
                p.update(cx, |this, cx| this.mark_table_detached(cx));
                widths_reset = Some(("report-reset-widths-win", p.read(cx).table_state()));
                p.into()
            }
            "Assets" => cx
                .new(|cx| {
                    AssetsView::detached_group(backend.clone(), spec.group.clone(), window, cx)
                })
                .into(),
            "CoreStatus" => {
                let p = cx.new(|cx| {
                    CoreStatusView::detached_group(backend.clone(), spec.group.clone(), window, cx)
                });
                widths_reset = Some(("core-status-reset-widths-win", p.read(cx).table_state()));
                p.into()
            }
            "Alerts" => cx
                .new(|cx| {
                    crate::panels::AlertsPanel::new(backend.clone(), spec.group.clone(), window, cx)
                })
                .into(),
            _ => cx
                .new(|cx| {
                    StubPanel::new(
                        "?",
                        t!("dock.tab.generic").to_string(),
                        spec.group.clone(),
                        backend.clone(),
                        cx,
                    )
                })
                .into(),
        };
        let dw = cx.new(|cx| {
            DetachedWindow::new(
                backend.clone(),
                spec.group.clone(),
                spec.panel.clone(),
                content,
                widths_reset,
                window,
                cx,
            )
        });
        cx.new(|cx| Root::new(dw, window, cx).background_policy(MoonBackgroundPolicy::Opaque))
    })
}
