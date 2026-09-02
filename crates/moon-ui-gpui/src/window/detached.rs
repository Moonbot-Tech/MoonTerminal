//! Dock panels detached into separate OS windows, ported from egui `app/detached.rs` and
//! `WindowLayout.detached`. Detaching removes a panel from its dock through
//! `TabPanel::remove_panel`. `detached.json` persists the detached state and current window
//! geometry so startup can reopen the panel detached. Closing the detached window requests a repin
//! through `Backend.repin_request`, which Shell drains to return the panel to its owner's dock —
//! but only when the close was the user's. A repin is queued only while
//! `Backend.detached_panel_windows` still names this window, so a deliberate teardown (a group
//! window rebuild, whose OS-owned children die with it) closes and reopens these windows without
//! undoing the detachment. Application exit is the same kind of teardown, handled at two points:
//! closing the last group window unregisters these windows before requesting the quit, and
//! `Backend.quitting` stops `Shell::drain_repin_requests` from acting on any request that still
//! arrives.
//!
//! Each window contains a fresh panel instance backed by the shared `Backend`, so its data remains
//! live. [`DetachedWindow`] renders it, observes window geometry, and requests repinning when
//! released by the user. Detached chart tabs use a separate persistence subsystem because their
//! panel state requires serialization.

use std::collections::HashSet;
use std::path::Path;
use std::rc::Rc;
use std::time::Duration;

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
    /// Display this window was last seen on, when the platform could name one.
    ///
    /// Persisted because `x`/`y` identify a monitor only where window coordinates are global. On
    /// macOS they are relative to the window's own screen, so without this the panel comes back on
    /// the group window's display rather than its own. A monitor that is gone simply fails to
    /// resolve and the previous coordinate/owner routes place the window.
    ///
    /// Decoded leniently: detached.json holds every detachment; a malformed identity must not cost the user all of them.
    #[serde(
        default,
        deserialize_with = "moon_core::config::layout::de_lenient",
        skip_serializing_if = "Option::is_none"
    )]
    pub display_uuid: Option<uuid::Uuid>,
    /// Whether the window was left MAXIMIZED, and its macOS FULLSCREEN counterpart.
    ///
    /// Detached panels reopen from this file rather than from `layout.detached_geom`, so the pair
    /// `GeomRect` carries has to be mirrored here or a maximized panel would come back windowed.
    /// `x`/`y`/`w`/`h` stay the RESTORE rectangle while either is set, exactly as they do there.
    ///
    /// Absent from older `detached.json` files and omitted when false, so an untouched file keeps
    /// its previous shape, and decoded leniently exactly like [`Self::display_uuid`]: the whole
    /// file is one `Vec`, so a single malformed value would otherwise cost the user every
    /// detachment at once.
    #[serde(
        default,
        deserialize_with = "moon_core::config::layout::de_lenient_bool",
        skip_serializing_if = "std::ops::Not::not"
    )]
    pub maximized: bool,
    /// macOS fullscreen counterpart of [`Self::maximized`]; see `GeomRect::fullscreen`.
    #[serde(
        default,
        deserialize_with = "moon_core::config::layout::de_lenient_bool",
        skip_serializing_if = "std::ops::Not::not"
    )]
    pub fullscreen: bool,
    /// Whether `x`/`y` are the first-detach cascade rather than a position this panel actually had.
    ///
    /// Not persisted, and false when read back from `detached.json`: a spec that reached the file
    /// carries a real position by definition. Only a real one may pick a display — the cascade point
    /// lies inside the primary display and would answer "primary" for every first detach, hiding the
    /// owner window's own monitor. `spawn` reads this instead of re-deriving it, so the answer cannot
    /// disagree with the spec it describes.
    #[serde(skip)]
    pub cascade_origin: bool,
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
            maximized: false,
            fullscreen: false,
            display_uuid: None,
            cascade_origin: true,
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
            spec.maximized = g.maximized;
            spec.fullscreen = g.fullscreen;
            spec.display_uuid = g.display_uuid;
            spec.cascade_origin = false;
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

/// Raise the live window of an already-detached panel, if the map still names one.
///
/// The routes that refuse a second detach end here: a person clicking "detach" on a panel that is
/// already detached is asking to SEE it, which is what every singleton tool window answers with.
/// Borrowed keys keep the lookup allocation-free, and `false` means the map named no live window —
/// the caller has nothing to raise and its own refusal stands.
pub(crate) fn raise_existing(
    backend: &Entity<Backend>,
    group: &str,
    panel: &str,
    app: &mut App,
) -> bool {
    let Some(handle) = backend
        .read(app)
        .detached_panel_windows
        .iter()
        .find(|((g, p), _)| g == group && p == panel)
        .map(|(_, handle)| *handle)
    else {
        // The spec says detached but no window answers to it — the gap a group-window rebuild opens
        // between taking the old windows down and `respawn_all` putting them back. The click cannot
        // be served, and silence here is what made the original defect so hard to report.
        log::warn!("panel {group}/{panel} is detached but has no live window to raise");
        return false;
    };
    // `update` doubles as the liveness probe every singleton open() uses: an `Err` is a window that
    // died without clearing its entry. Both failures are logged HERE rather than returned upward,
    // because neither caller can do anything about them — the panel's spec is taken, so a second
    // window is exactly what must not happen — and silence is what made the original defect so hard
    // to report.
    let raised = handle
        .update(app, |_, window, _| window.activate_window())
        .is_ok();
    if !raised {
        log::warn!("panel {group}/{panel} names a window that no longer exists");
    }
    raised
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
        Ok(s) => deduplicated(serde_json::from_str(&s).unwrap_or_else(|e| {
            log::warn!("detached.json битый ({e}) → без откреплённых");
            Vec::new()
        })),
        Err(_) => Vec::new(),
    }
}

/// Retain the first specification for every stable `(group, panel)` identity.
///
/// Args:
///     list: Loaded or in-memory detached specifications in persistence order.
///
/// Returns:
///     Stable deduplicated list so one identity can never spawn multiple native windows.
pub(crate) fn deduplicated(list: Vec<DetachedSpec>) -> Vec<DetachedSpec> {
    let mut seen = HashSet::new();
    list.into_iter()
        .filter(|spec| seen.insert(spec.key()))
        .collect()
}

/// Serialize detached panel specifications and atomically write an exact destination.
///
/// Args:
///     list: Complete Classic detached-window specification list.
///     path: Destination supplied by production or an isolated failure-path regression.
///
/// Returns:
///     `true` only after serialization and the atomic write both succeed.
pub(crate) fn save_all_to_path(list: &[DetachedSpec], path: &Path) -> bool {
    match serde_json::to_string_pretty(list) {
        Ok(s) => {
            if let Err(e) =
                moon_core::config::write_file_atomic(path, s.as_bytes(), "detached.json")
            {
                log::warn!("could not write detached.json: {e}");
                false
            } else {
                true
            }
        }
        Err(e) => {
            log::warn!("could not serialize detached.json: {e}");
            false
        }
    }
}

/// True for panels that can be moved into a detached OS window.
pub fn supports_panel(name: &str) -> bool {
    registry::supports(name)
}

/// Build a fresh dock-panel instance by name for Classic repinning or temporary Auto hosting.
///
/// `None` info means the panel starts from defaults. The detached instance owns its live state;
/// neither Classic repinning nor Auto's temporary attached replacement may copy that payload.
///
/// Args:
///     name: Stable panel registry name.
///     group: Group that owns the new panel instance.
///     backend: Shared application state supplied to the panel factory.
///     window: Window that will host the new docked instance.
///     cx: Application context used by the registry factory.
///
/// Returns:
///     A fresh panel view, or `None` when the registry does not support the name.
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
    /// Whether the user asked to close THIS window, which is what a repin actually requires.
    ///
    /// Ownership of the registry entry is not enough. A detached panel is an OS-owned child of its
    /// group window, so application exit destroys it BEFORE the group window whose close would
    /// unregister it — its release then looks user-driven, docks the panel and deletes the
    /// `DetachedSpec`, which is how a detachment failed to survive a restart.
    ///
    /// The flag is authoritative rather than a guess on both shipping platforms. On Windows every
    /// user-driven close of this window — the frame's ✕ (a `window_control_area` hit-test, which the
    /// platform turns into `WM_CLOSE`), Alt+F4, the system menu, the taskbar — arrives through
    /// `on_window_should_close`, while dying with the owner arrives as a bare `WM_DESTROY` that
    /// never runs it. On macOS the same hook is `windowShouldClose:`, which AppKit sends only for a
    /// user close (the red button, Cmd+W); MoonUI draws no frame buttons there, and both the
    /// programmatic `close` and a child window dying with its parent skip it.
    ///
    /// Linux is the exception: MoonUI's frame button calls `remove_window()` directly and bypasses
    /// the hook (logged in `docs-internal/FORK_BUGS.md`), so it starts from `true` and keeps the
    /// previous behaviour, guarded by `Backend::quitting` and by the shutdown branch that
    /// unregisters these windows.
    user_close: std::rc::Rc<std::cell::Cell<bool>>,
}

impl DetachedWindow {
    /// Construct a detached group-panel host and attach its lifecycle observers.
    ///
    /// Args:
    ///     backend: Shared terminal state used for geometry, ownership, activity, and repin updates.
    ///     group: Group whose workspace and primary window own the panel logically.
    ///     panel: Stable panel name used by persistence and the detached-window registry.
    ///     content: Existing live panel view rendered inside this OS window.
    ///     widths_reset: Optional header action and table state for resetting column widths.
    ///     window: Newly created detached window observed for bounds and native activation.
    ///     cx: View context used to register lifecycle observers.
    ///
    /// Returns:
    ///     Fully initialized host that repins only while it still owns its registry entry.
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
        // Native activation is a deliberate interaction even before the mouse moves. Attribute
        // singleton scope to this panel's group, whichever preset it is in.
        let activation_backend = backend.clone();
        let activation_group = group.clone();
        cx.observe_window_activation(window, move |_this, window, cx| {
            if window.is_window_active() {
                activation_backend.update(cx, |b, bcx| {
                    b.note_main_input(&activation_group);
                    b.focus_singleton_owner(&activation_group, bcx);
                });
            }
        })
        .detach();
        // Record the user's own request to close this window. See `DetachedWindow::user_close`:
        // ownership alone cannot tell a real close from this window being destroyed with its owner,
        // because the child dies FIRST and the owner's close is what would have unregistered it.
        let user_close = std::rc::Rc::new(std::cell::Cell::new(!cfg!(any(
            target_os = "windows",
            target_os = "macos"
        ))));
        {
            let requested = user_close.clone();
            window.on_window_should_close(cx, move |_window, _app| {
                requested.set(true);
                true
            });
        }
        // A user-owned close queues a repin, removes ownership, and wakes Shell in that order.
        // Programmatic teardown first removes ownership through `take_windows`, so its later
        // release remains deliberately silent. During shutdown, the final on_app_quit flush
        // persists detached specifications before windows are released.
        let key = (group.clone(), panel.clone());
        let window_id = window.window_handle().window_id();
        cx.on_release(move |this, app| {
            if !this.user_close.get() {
                return;
            }
            this.backend.update(app, |b, cx| {
                if !owns_panel(b, &key, this.window_id) {
                    return;
                }
                b.repin_request.push(key.clone());
                // Same edge, so the handle map can never outlive the window it points at.
                b.detached_panel_windows.remove(&key);
                cx.notify();
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
            user_close,
        }
    }

    fn persist_geometry(&mut self, window: &Window, cx: &mut Context<Self>) {
        let mut geom = crate::window::windowing::window_geom_rect(window, cx);
        let key = (self.group.clone(), self.panel.clone());
        let window_id = self.window_id;
        self.backend.update(cx, |bk, _| {
            // A window being torn down still emits bounds events; letting a dying twin write them
            // would overwrite the live window's position with a teardown artefact.
            if !owns_panel(bk, &key, window_id) {
                return;
            }
            let (group, panel) = (&key.0, &key.1);
            let geom_memory_key = geom_key(group, panel);
            // An unknown display must not erase a known one; see `GeomRect::keeping_display_of`.
            // The spec is asked FIRST because it is the store that can already hold an identity the
            // geometry memory does not: a first detach records the chosen display on the spec, and
            // `detached_geom` only learns it from the write below.
            let remembered = bk
                .detached
                .iter()
                .find(|s| s.group == *group && s.panel == *panel)
                .and_then(|s| s.display_uuid)
                .or_else(|| {
                    bk.layout
                        .detached_geom
                        .get(&geom_memory_key)
                        .and_then(|previous| previous.display_uuid)
                });
            geom.display_uuid = geom.display_uuid.or(remembered);
            if let Some(s) = bk
                .detached
                .iter_mut()
                .find(|s| s.group == *group && s.panel == *panel)
            {
                // The window exists at these coordinates, so whatever the spec started as, its
                // origin is now a real position — including the first-detach case, where leaving the
                // flag set would make the next reopen discard a position the user chose by dragging.
                s.cascade_origin = false;
                if (
                    s.x,
                    s.y,
                    s.w,
                    s.h,
                    s.maximized,
                    s.fullscreen,
                    s.display_uuid,
                ) != (
                    geom.x,
                    geom.y,
                    geom.w,
                    geom.h,
                    geom.maximized,
                    geom.fullscreen,
                    geom.display_uuid,
                ) {
                    s.x = geom.x;
                    s.y = geom.y;
                    s.w = geom.w;
                    s.h = geom.h;
                    s.maximized = geom.maximized;
                    s.fullscreen = geom.fullscreen;
                    s.display_uuid = geom.display_uuid;
                    bk.detached_dirty = true;
                }
            }
            // Retain geometry independently of the detached specification. Repinning removes the
            // specification but keeps this memory, so the next detachment restores the same bounds.
            let changed = bk.layout.detached_geom.get(&geom_memory_key) != Some(&geom);
            if changed {
                bk.layout.detached_geom.insert(geom_memory_key, geom);
                bk.layout_dirty = true;
            }
        });
    }
}

impl Render for DetachedWindow {
    /// Render the detached panel chrome and attribute active-window interaction to its group.
    ///
    /// Args:
    ///     window: Detached OS window that supplies capture-phase mouse events and active state.
    ///     cx: View context used to update Backend activity and workspace focus.
    ///
    /// Returns:
    ///     Full-window element containing the detached panel titlebar and live content.
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        crate::diag::bump(&crate::diag::DETACHED_RENDER);
        let _render_us = crate::diag::scope(&crate::diag::DETACHED_RENDER_US);
        // Detached panels share Main's group but live in a separate OS window whose mouse movement
        // Shell cannot observe. Record group activity from any widget in this active window so Main's
        // inactivity policy does not close its chart while the user works in a detached panel.
        //
        // Registered through a paint-phase hook rather than called here: `on_mouse_event`
        // belongs to paint and `render` runs a phase earlier (`window::input_hook`).
        let activity_hook = {
            let backend = self.backend.clone();
            let group = self.group.clone();
            // Use the capture phase, as Shell does, so activity is recorded before bubble
            // handlers and cannot be suppressed by their stop_propagation calls.
            crate::window::input_hook::window_mouse_hook(
                move |_e: &MouseMoveEvent, phase, window: &mut Window, cx| {
                    if phase == DispatchPhase::Capture && window.is_window_active() {
                        backend.update(cx, |b, bcx| {
                            b.note_main_input(&group);
                            b.focus_singleton_owner(&group, bcx);
                        });
                    }
                },
            )
        };
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
            // The window-level activity hook, installed when this is painted.
            .child(activity_hook)
    }
}

/// Opens a detached window from a specification, either while restoring each saved specification at
/// startup or after a new detach action. The content is a fresh panel and geometry comes from `spec`.
///
/// `owner_display` is the owner's display captured at the call site with `window.display(cx)`. A
/// detach action runs INSIDE the owner window's update, where its slot in `cx.windows` is taken and
/// the `owner.update()` fallback cannot resolve a display; without it macOS falls back to the
/// primary display and every detached panel opens on the wrong monitor. `respawn_all` is the one
/// route that runs outside that borrow — after the group window is registered — so it passes `None`
/// and lets the fallback do the work.
pub fn spawn(
    app: &mut App,
    backend: &Entity<Backend>,
    spec: &mut DetachedSpec,
    owner: Option<AnyWindowHandle>,
    owner_display: Option<DisplayId>,
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
    //
    // See `DetachedSpec::cascade_origin` for why a fabricated origin must not pick the display.
    let saved_origin = (!spec.cascade_origin).then_some(bounds.origin);
    let display_id = crate::window::windowing::saved_or_owner_display_id(
        spec.display_uuid,
        saved_origin,
        owner,
        owner_display,
        app,
    );
    // A cascade origin now has to be expressed on the display actually chosen, or Windows drops the
    // rectangle — size included — as "not inside that display". Written back onto the spec because
    // the caller persists it: a spec describing a different point than the window opened at would
    // move that window on the next launch, and the point is a real position now, not the cascade.
    let bounds = match saved_origin {
        Some(origin) => Bounds { origin, ..bounds },
        None => {
            let origin =
                crate::window::windowing::cascade_origin_on(bounds.origin, display_id, app);
            spec.x = f32::from(origin.x) as i32;
            spec.y = f32::from(origin.y) as i32;
            spec.cascade_origin = false;
            Bounds { origin, ..bounds }
        }
    };
    // Record the display this window is opening on, so a panel detached and left alone until quit
    // still knows its monitor. A known identity is never replaced by an unknown one — off macOS the
    // lookup always answers `None`, and a spec carried over from a Mac must survive that.
    spec.display_uuid =
        crate::window::windowing::display_identity(display_id, app).or(spec.display_uuid);
    let opts = crate::window::windowing::detached_panel_window_options(
        format!(
            "{} — MoonTerminal",
            crate::persistence::panel_meta::panel_title(&spec.panel)
        ),
        crate::window::windowing::window_bounds_for(spec.maximized, spec.fullscreen, bounds),
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
    auto_groups: &HashSet<&str>,
    open_windows: &HashSet<(&str, &str)>,
    pending_repins: &HashSet<(&str, &str)>,
    docked: &HashSet<(&str, &str)>,
) -> bool {
    let key = spec.key_ref();
    live_groups.contains(spec.group.as_str())
        && !auto_groups.contains(spec.group.as_str())
        && !open_windows.contains(&key)
        && !pending_repins.contains(&key)
        && !docked.contains(&key)
}

/// Revalidate one saved detached specification against current application ownership.
///
/// The caller invokes this only after its timer yield. Building every authority set here ensures
/// a group rebuild, mode transition, repin, dock restore, or competing respawn that happened during
/// the yield wins over the stale initial specification snapshot.
///
/// Args:
///     backend: Current shared application state after the scheduling yield.
///     spec: One stable-order detached specification being considered.
///
/// Returns:
///     `true` only when no current owner accounts for the panel and Classic may reopen it.
fn should_reopen_now(backend: &Backend, spec: &DetachedSpec) -> bool {
    if backend.quitting {
        return false;
    }
    let live_groups: HashSet<&str> = backend.group_windows.keys().map(String::as_str).collect();
    let auto_groups: HashSet<&str> = backend
        .group_windows
        .keys()
        .filter(|group| {
            backend.workspace_mode(group) == moon_core::config::WorkspaceMode::AutoTrading
        })
        .map(String::as_str)
        .collect();
    let open_windows: HashSet<(&str, &str)> = backend
        .detached_panel_windows
        .keys()
        .map(|(group, panel)| (group.as_str(), panel.as_str()))
        .collect();
    let pending_repins: HashSet<(&str, &str)> = backend
        .repin_request
        .iter()
        .map(|(group, panel)| (group.as_str(), panel.as_str()))
        .collect();
    let docked = crate::persistence::dock_persist::docked_panels(&backend.dock_states);
    should_reopen(
        spec,
        &live_groups,
        &auto_groups,
        &open_windows,
        &pending_repins,
        &docked,
    )
}

/// Minimal real delay used to yield native window creation to a later application turn.
const DETACHED_RESPAWN_YIELD: Duration = Duration::from_millis(1);

/// Reopen the detached panel windows that nothing else accounts for, leaving every spec in place.
///
/// Called after group windows are recreated, from both settings paths. A foreground task yields
/// before every synchronous `open_window`, then revalidates exactly that specification so native
/// creation is distributed across later turns without racing current dock or mode ownership.
///
/// Args:
///     backend: Shared application state containing persisted specs and live ownership.
///     cx: Application context used to schedule serialized foreground recreation.
pub(crate) fn respawn_all(backend: &Entity<Backend>, cx: &mut App) {
    let specs = deduplicated(backend.read(cx).detached.clone());
    if specs.is_empty() {
        return;
    }
    let backend = backend.clone();
    cx.spawn(async move |cx| {
        let executor = cx.update(|app| app.background_executor().clone());
        log::info!("reopening {} detached panel window(s)", specs.len());
        for spec in specs {
            executor.timer(DETACHED_RESPAWN_YIELD).await;
            cx.update(|app| {
                if !should_reopen_now(backend.read(app), &spec) {
                    return;
                }
                // A clone, because `spawn`'s origin write-back is meaningless on this route: every
                // spec here comes from `detached.json` or a live detachment, so its origin is real
                // and the cascade branch that writes back cannot be taken.
                let mut spec = spec.clone();
                if let Err(err) = spawn(app, &backend, &mut spec, None, None) {
                    log::warn!(
                        "reopen detached panel failed group={} panel={}: {err:#}",
                        spec.group,
                        spec.panel
                    );
                    // Without a window the panel exists nowhere: its dock tab was removed when it
                    // was detached. Ask Shell to take it back and wake its Backend observer.
                    backend.update(app, |b, cx| {
                        b.repin_request.push(spec.key());
                        cx.notify();
                    });
                }
            });
        }
    })
    .detach();
}
