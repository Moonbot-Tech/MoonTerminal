//! Generic fallback panel for an unavailable panel implementation.
//!
//! The detached-window factory currently constructs it for unrecognized persisted panel types.
//! `name` supplies the [`Panel::panel_name`] and layout persistence key, while `title` supplies the
//! visible heading. The detach toolbar action can open another detached window and, when a dock
//! owner exists, removes the source only after that window opens successfully.

use gpui::*;
use moon_ui::{DockArea, MoonPalette, Panel, PanelEvent, PanelState};
use rust_i18n::t;

use crate::Backend;

/// Generic placeholder implementing the panel lifecycle for unavailable content.
pub struct StubPanel {
    name: &'static str,
    title: SharedString,
    /// Owning window group used by layout persistence and detached-window specifications.
    group: String,
    /// Shared backend used by the detach action to persist its detached-window specification.
    backend: Entity<Backend>,
    /// Owning dock area, when present, used to remove the source after successful detachment.
    dock: Option<WeakEntity<DockArea>>,
    focus: FocusHandle,
}

impl StubPanel {
    pub fn new(
        name: &'static str,
        title: impl Into<SharedString>,
        group: String,
        backend: Entity<Backend>,
        cx: &mut Context<Self>,
    ) -> Self {
        Self {
            name,
            title: title.into(),
            group,
            backend,
            dock: None,
            focus: cx.focus_handle(),
        }
    }
}

impl EventEmitter<PanelEvent> for StubPanel {}
impl Focusable for StubPanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}
impl Panel for StubPanel {
    fn closable(&self, _cx: &App) -> bool {
        true
    }
    fn show_dock_header(&self, _cx: &App) -> bool {
        true
    }
    fn panel_name(&self) -> &'static str {
        self.name
    }
    fn title(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        self.title.clone()
    }
    fn dump(&self, _cx: &App) -> PanelState {
        crate::persistence::dock_persist::panel_state_with_group(self.name, &self.group)
    }
    /// Retain the owning dock so the detach action can remove this source panel after spawning.
    fn on_added_to(
        &mut self,
        dock_area: WeakEntity<DockArea>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) {
        self.dock = Some(dock_area);
    }
    /// Return the shared detach toolbar action.
    ///
    /// It opens a detached window, removes the source from its dock only after a successful spawn,
    /// and records a `DetachedSpec` for startup restoration. This ports egui's `open_detached` flow.
    fn toolbar_buttons(
        &mut self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Vec<AnyElement>> {
        Some(vec![super::detach_button(
            self.name,
            self.group.clone(),
            self.backend.clone(),
            self.dock.clone(),
        )])
    }
}
impl Render for StubPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = MoonPalette::active(cx);
        div()
            .id(self.name)
            .size_full()
            .p_4()
            .track_focus(&self.focus)
            .text_color(rgb(p.text_soft))
            .child(t!("dock.stub_soon", name = self.title).to_string())
    }
}
