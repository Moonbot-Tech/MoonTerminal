//! Dock panels detached into separate OS windows, ported from egui `app/detached.rs` and
//! `WindowLayout.detached`. Detaching removes a panel from its dock through
//! `TabPanel::remove_panel`. `detached.json` persists the detached state and current window
//! geometry so startup can reopen the panel detached. Closing the detached window requests a repin
//! through `Backend.repin_request`, which Shell drains to return the panel to its owner's dock —
//! but only when the close was the user's. A repin is queued only while
//! `Backend.detached_panel_windows` still names this window, so a deliberate teardown (a group
//! window rebuild, whose OS-owned children die with it) closes and reopens these windows without
//! undoing the detachment.
//!
//! Each window contains a fresh panel instance backed by the shared `Backend`, so its data remains
//! live. [`DetachedWindow`] renders it, observes window geometry, and requests repinning when
//! released by the user. Detached chart tabs use a separate persistence subsystem because their
//! panel state requires serialization.

use std::collections::HashSet;
use std::rc::Rc;

#[cfg(test)]
mod tests;

use gpui::prelude::FluentBuilder;
use gpui::*;
use moon_ui::{
    MoonBackgroundPolicy, MoonPalette, MoonWindowFrame, PanelView, Root, h_flex, v_flex,
};
use serde::{Deserialize, Serialize};

use rust_i18n::t;

use crate::Backend;
use crate::panels::StubPanel;
use crate::panels::registry;
use moon_core::config::paths;

/// Persisted description of one detached panel window: panel name, source group, and geometry.
#[derive(Clone, Serialize, Deserialize)]
pub struct DetachedSpec {
    pub group: String,
    /// Stable panel name. Detachable panels are enumerated once in [`crate::panels::registry`];
    /// an unknown name restores as a [`StubPanel`].
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

    /// The `(group, panel)` identity this spec shares with `Backend.detached_panel_windows`,
    /// `repin_request` and `panel_detach_request`.
    pub fn key(&self) -> (String, String) {
        (self.group.clone(), self.panel.clone())
    }

    /// The same identity without allocating, for lookups in borrowed key sets.
    pub fn key_ref(&self) -> (&str, &str) {
        (self.group.as_str(), self.panel.as_str())
    }
}

/// Builds a detached-geometry key for `layout.detached_geom`. The `panel:` prefix separates GPUI
/// dock-panel keys from legacy egui keys such as `g:<idx>` and `o:<idx>:<group>`.
fn geom_key(group: &str, panel: &str) -> String {
    format!("panel:{group}/{panel}")
}

/// Remove the detached panel windows of the groups `doomed` accepts, returning their handles.
///
/// Removal is the point: the map is the authority for "this window may repin", so taking an entry
/// out BEFORE the window closes is what keeps its release silent. Callers that close a panel window
/// any other way undo the user's detachment — see [`Backend::detached_panel_windows`].
///
/// The caller closes the returned handles; this runs inside a `Backend` update, where it cannot.
pub(crate) fn take_windows(
    b: &mut Backend,
    doomed: impl Fn(&str) -> bool,
) -> Vec<WindowHandle<Root>> {
    let mut taken = Vec::new();
    b.detached_panel_windows.retain(|(group, _), handle| {
        if doomed(group) {
            taken.push(*handle);
            false
        } else {
            true
        }
    });
    taken
}

/// Drop queued repin and detach requests addressed to groups `gone` accepts.
///
/// A request outlives the window that made it. One addressed to a departed group would sit in the
/// queue until some later window for that group NAME appears and replays it out of context —
/// repinning a panel nobody asked to repin, and deleting its spec.
pub(crate) fn prune_requests(b: &mut Backend, gone: impl Fn(&str) -> bool) {
    b.repin_request.retain(|(group, _)| !gone(group));
    b.panel_detach_request.retain(|(group, _)| !gone(group));
}

/// Whether `window_id` is the window the map currently calls the one for `key`.
///
/// The single expression of the rule stated on [`Backend::detached_panel_windows`]: a window acts
/// on the panel's behalf — repinning it, saving its geometry — only while it IS the panel's window.
fn owns_panel(b: &Backend, key: &(String, String), window_id: WindowId) -> bool {
    b.detached_panel_windows
        .get(key)
        .is_some_and(|h| h.window_id() == window_id)
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
    registry::supports(name)
}

/// Builds a fresh dock-panel instance by name as `Rc<dyn PanelView>` for repinning into a dock.
///
/// `None` info means the panel starts from defaults, which is correct for a repin: the panel's
/// view state was not persisted in the dock JSON while it lived in a detached window.
pub fn build_panel(
    name: &str,
    group: &str,
    backend: &Entity<Backend>,
    window: &mut Window,
    cx: &mut App,
) -> Option<Rc<dyn PanelView>> {
    let kind = registry::find(name)?;
    Some(kind.build_docked(backend, group, None, window, cx))
}

/// Detached-window wrapper that renders a panel, observes geometry, updates `Backend.detached`, and
/// requests repinning when the user closes it. The backend drain performs the debounced save.
pub struct DetachedWindow {
    backend: Entity<Backend>,
    group: String,
    panel: String,
    content: AnyView,
    /// This window's own id, compared against `Backend.detached_panel_windows` on release.
    ///
    /// A release means "the user closed this window" only while that map still points at THIS
    /// window. A programmatic teardown removes the entry first and then closes the window, so the
    /// release must stay silent; and if a replacement window for the same `(group, panel)` has
    /// already registered, this window's late release must not repin it or clear its entry.
    window_id: WindowId,
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
        let key = (group.clone(), panel.clone());
        let window_id = window.window_handle().window_id();
        cx.on_release(move |this, app| {
            this.backend.update(app, |b, _| {
                if !owns_panel(b, &key, this.window_id) {
                    return;
                }
                b.repin_request.push(key.clone());
                // Same edge, so the handle map can never outlive the window it points at.
                b.detached_panel_windows.remove(&key);
            });
        })
        .detach();
        Self {
            backend,
            group,
            panel,
            content,
            window_id,
            widths_reset,
        }
    }

    fn persist_geometry(&mut self, window: &Window, cx: &mut Context<Self>) {
        let Some(geom) = crate::window::windowing::window_geom(window) else {
            return;
        };
        let key = (self.group.clone(), self.panel.clone());
        let window_id = self.window_id;
        self.backend.update(cx, |bk, _| {
            // A window being torn down still emits bounds events; letting a dying twin write them
            // would overwrite the live window's position with a teardown artefact.
            if !owns_panel(bk, &key, window_id) {
                return;
            }
            let (group, panel) = (&key.0, &key.1);
            if let Some(s) = bk
                .detached
                .iter_mut()
                .find(|s| s.group == *group && s.panel == *panel)
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
            let geom_key = geom_key(group, panel);
            let changed = bk
                .layout
                .detached_geom
                .get(&geom_key)
                .map(|g| (g.x, g.y, g.w, g.h) != geom)
                .unwrap_or(true);
            if changed {
                bk.layout.detached_geom.insert(
                    geom_key,
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
            crate::persistence::panel_meta::panel_title(&self.panel),
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
                        this.child(crate::persistence::table_persist::reset_button(id, &state))
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
        crate::window::windowing::saved_or_owner_display_id(Some(bounds.origin), owner, None, app);
    let opts = crate::window::windowing::detached_panel_window_options(
        format!(
            "{} — MoonTerminal",
            crate::persistence::panel_meta::panel_title(&spec.panel)
        ),
        WindowBounds::Windowed(bounds),
        display_id,
        owner,
    );
    let backend = backend.clone();
    let spec = spec.clone();
    let key = spec.key();
    let record = backend.clone();
    let handle = app.open_window(opts, move |window, cx| {
        crate::window::windowing::configure_shell_clear_color(window, cx);
        // Build the panel in detached-window mode from the registry. Known panels supply their
        // view and any header auto-width reset binding; an unknown name falls back to a stub.
        let mut widths_reset: Option<(&'static str, Entity<moon_ui::MoonDataTableState>)> = None;
        let content: AnyView = match registry::find(spec.panel.as_str()) {
            Some(kind) => {
                let detached = kind.build_detached(&backend, &spec.group, window, cx);
                widths_reset = detached.widths_reset;
                detached.view
            }
            None => cx
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
    })?;
    // Recorded HERE rather than at the call sites, because there are four of them — the startup
    // restore, the dock's detach action, the panel toolbar button and the settings-driven reopen
    // after a group-window rebuild — and a map filled by only some of them is worse than no map: it
    // reads as "every live detached window" while quietly missing the ones nobody remembered. The
    // entry is cleared either by this window's own `on_release` or by a deliberate teardown, which
    // removes it first precisely to keep that release silent.
    record.update(app, |b, _| {
        b.detached_panel_windows.insert(key, handle);
    });
    Ok(handle)
}

/// Whether this spec should be given a window right now.
///
/// Each exclusion is a state the spec must SURVIVE rather than a reason to forget it — a detachment
/// is the user's decision and outlives any window, while the panel is absent from `dock_states`, so
/// dropping the spec would leave the panel in neither the dock nor a window with no UI to bring it
/// back:
///
/// * its group must have a LIVE window — otherwise `spawn` finds no owner and produces a top-level
///   window that no longer dies with its group;
/// * no window for it may be open already — a second one would orphan the first, which can then
///   never repin;
/// * no repin may be queued for it — the user just closed that window, and the pending repin is a
///   request to put the panel back in the dock;
/// * the panel must not be in the restorable dock — a stale spec would otherwise appear twice, once
///   as a tab and once as a window.
pub(crate) fn should_reopen(
    spec: &DetachedSpec,
    live_groups: &HashSet<&str>,
    open_windows: &HashSet<(&str, &str)>,
    pending_repins: &HashSet<(&str, &str)>,
    docked: &HashSet<(&str, &str)>,
) -> bool {
    let key = spec.key_ref();
    live_groups.contains(spec.group.as_str())
        && !open_windows.contains(&key)
        && !pending_repins.contains(&key)
        && !docked.contains(&key)
}

/// Reopen the detached panel windows that nothing else accounts for, leaving every spec in place.
///
/// Called after group windows are recreated, from both settings paths. Deferred because
/// `open_window` draws synchronously and the callers run while their own entity is still leased.
pub(crate) fn respawn_all(backend: &Entity<Backend>, cx: &mut App) {
    let backend = backend.clone();
    cx.defer(move |app| {
        let specs: Vec<DetachedSpec> = {
            let b = backend.read(app);
            if b.detached.is_empty() {
                return;
            }
            let live_groups: HashSet<&str> = b.group_windows.keys().map(String::as_str).collect();
            let open_windows: HashSet<(&str, &str)> = b
                .detached_panel_windows
                .keys()
                .map(|(g, p)| (g.as_str(), p.as_str()))
                .collect();
            let pending_repins: HashSet<(&str, &str)> = b
                .repin_request
                .iter()
                .map(|(g, p)| (g.as_str(), p.as_str()))
                .collect();
            let docked = crate::persistence::dock_persist::docked_panels(&b.dock_states);
            b.detached
                .iter()
                .filter(|s| should_reopen(s, &live_groups, &open_windows, &pending_repins, &docked))
                .cloned()
                .collect()
        };
        if specs.is_empty() {
            return;
        }
        log::info!("reopening {} detached panel window(s)", specs.len());
        for spec in specs {
            if let Err(err) = spawn(app, &backend, &spec, None) {
                log::warn!(
                    "reopen detached panel failed group={} panel={}: {err:#}",
                    spec.group,
                    spec.panel
                );
                // Without a window the panel exists nowhere: its dock tab was removed when it was
                // detached. Ask its Shell to take it back rather than lose it until restart, and
                // notify — the drain runs from a Backend observer, so an unannounced request would
                // wait for an unrelated update to carry it.
                backend.update(app, |b, cx| {
                    b.repin_request.push(spec.key());
                    cx.notify();
                });
            }
        }
    });
}
