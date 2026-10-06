//! Core Status dock panel hosting and toolbar integration.

use super::{CoreStatusMode, CoreStatusView};
use crate::design;
use gpui::prelude::FluentBuilder;
use gpui::*;
use moon_ui::{DockArea, MoonPalette, Panel, PanelEvent, PanelState, h_flex};

impl EventEmitter<PanelEvent> for CoreStatusView {}
impl Focusable for CoreStatusView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Panel for CoreStatusView {
    fn panel_name(&self) -> &'static str {
        "CoreStatus"
    }
    /// Visible tab caption. `panel_name` is the stable persistence key and stays untouched.
    fn tab_name(&self, _cx: &App) -> Option<SharedString> {
        crate::persistence::panel_meta::tab_label(self.panel_name())
    }
    fn closable(&self, _cx: &App) -> bool {
        true
    }
    fn show_dock_header(&self, _cx: &App) -> bool {
        true
    }
    fn title(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        crate::persistence::panel_meta::panel_title(self.panel_name())
    }
    /// Draw what the tab has to announce while it is hidden behind a sibling: a warning triangle
    /// while any server warns, and a count of core findings nobody has looked at.
    ///
    /// TWO markers, not one merged glyph. They mean different things — the triangle is this
    /// terminal measuring a threshold, the count is the cores' own confirmed conclusions — and a
    /// reader who cannot tell them apart cannot tell whether to look at our numbers or at the
    /// core's words.
    fn title_suffix(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let p = MoonPalette::active(cx);
        let warn = self.has_warn;
        let unseen = self.unseen_problems;
        (warn || unseen > 0).then(|| {
            h_flex()
                .gap(design::ui_px(cx, 3.0))
                .items_center()
                .when(warn, |row| {
                    row.child(
                        svg()
                            .path("icons/triangle-alert.svg")
                            .size(px(13.0))
                            .flex_none()
                            .text_color(rgb(p.amber)),
                    )
                })
                .when(unseen > 0, |row| {
                    row.child(crate::panels::common::count_badge(
                        unseen,
                        design::danger_color(p),
                    ))
                })
                .into_any_element()
        })
    }
    fn dump(&self, _cx: &App) -> PanelState {
        crate::persistence::dock_persist::panel_state_with_group("CoreStatus", &self.group)
    }
    fn on_added_to(
        &mut self,
        dock_area: WeakEntity<DockArea>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) {
        self.dock = Some(dock_area);
    }
    fn toolbar_buttons(
        &mut self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Vec<AnyElement>> {
        // ONE button, pointed at whatever grid the panel is currently showing: a user sees one set
        // of columns and expects one reset. By IP has its own width bag because it is a tree, not
        // the flat table.
        //
        // The DETACHED window does not come through here. `panels/registry.rs` resolves
        // `table_state()` ONCE at window construction into `DetachedContent.widths_reset`, so its
        // header button keeps resetting the flat table whatever the mode; there, By-IP reset is
        // reachable by double-clicking a divider (Shift+double-click for all of them). Fixing that
        // means giving `widths_reset` a closure instead of an entity, which is a change to two
        // files outside this panel.
        let state = match self.mode {
            CoreStatusMode::ByIp => &self.by_ip_col_widths,
            CoreStatusMode::Flat => &self.table_state,
            // Warnings is its OWN table with its own widths (`warnings_table` is handed
            // `warn_table_state`), so it must not be folded in with Flat: doing that resets the
            // hidden grid and leaves the visible one untouched.
            CoreStatusMode::Warnings => &self.warn_table_state,
            // Same reasoning as Warnings: Updates is its own table with its own widths.
            CoreStatusMode::Updates => &self.updates_table_state,
            // And again for Problems, which is a third independent grid.
            CoreStatusMode::Problems => &self.problems_table_state,
        };
        Some(vec![crate::persistence::table_persist::reset_button(
            "core-status-reset-widths",
            state,
        )])
    }
}
