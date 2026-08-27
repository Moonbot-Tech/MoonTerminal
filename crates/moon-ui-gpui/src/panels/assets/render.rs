//! [AssetsView] event/focus/panel/render trait impls.

use super::*;

impl EventEmitter<PanelEvent> for AssetsView {}
impl Focusable for AssetsView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Panel for AssetsView {
    fn panel_name(&self) -> &'static str {
        "Assets"
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
    fn dump(&self, _cx: &App) -> PanelState {
        let group = match &self.scope {
            AssetsScope::Group(g) => g.clone(),
            AssetsScope::All => String::new(),
        };
        crate::persistence::dock_persist::panel_state_with_group("Assets", &group)
    }
    fn on_added_to(
        &mut self,
        dock_area: WeakEntity<DockArea>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) {
        self.dock = Some(dock_area);
    }
    /// Builds the toolbar action that opens the singleton global Assets window for all cores. Unlike
    /// Orders detachment, this is not scoped to the current group. Auto hides it: navigation stays
    /// inside the group window, and the docked tab already shows the window-form wallets.
    fn toolbar_buttons(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Vec<AnyElement>> {
        let mut buttons = vec![crate::persistence::table_persist::reset_button(
            "assets-reset-widths",
            &self.table_state,
        )];
        let auto = match &self.scope {
            AssetsScope::Group(group) => {
                self.backend.read(cx).workspace_mode(group)
                    == moon_core::config::WorkspaceMode::AutoTrading
            }
            AssetsScope::All => false,
        };
        if !auto {
            let backend = self.backend.clone();
            buttons.push(
                MoonButton::new("assets-open-global")
                    .ghost()
                    .size(MoonButtonSize::Action)
                    .label("⧉")
                    .tooltip(t!("assets.open_global_hint").to_string())
                    .on_click(move |_, window, app| {
                        let owner_display = window.display(app).map(|d| d.id());
                        open(
                            backend.clone(),
                            Some(window.window_handle()),
                            owner_display,
                            app,
                        );
                    })
                    .render()
                    .into_any_element(),
            );
        }
        Some(buttons)
    }
}

impl Render for AssetsView {
    /// Render the always-present table and footer plus the optional Wallets section.
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        crate::diag::bump(&crate::diag::ASSETS_RENDER);
        let _render_us = crate::diag::scope(&crate::diag::ASSETS_RENDER_US);
        // Keep the shared Assets-view activity marker fresh. While any view renders at least once
        // per second through RenderGate, feed snapshots may publish after a one-second minimum;
        // without a visible view, the minimum interval rises to five seconds after domain events.
        moon_core::feed::note_assets_view_render();
        let cores = self.scope_cores(self.backend.read(cx));
        let entries = self.cached_entries.clone();
        let p = MoonPalette::active(cx);
        let windowed = self.windowed;
        let show_wallets = self.wallets_visible(cx);

        let count = entries.len();
        // Natural table height is its header plus rows, or zero when empty. This lets the table grow
        // with content instead of stretching across a standalone window above the wallet section.
        let table_natural_h = if count == 0 {
            0.0
        } else {
            design::table_head_h(cx) + count as f32 * design::table_row_h(cx)
        };

        // The table and the footer are always present. Separate windows and Auto dock tabs also
        // render the collapsible Wallets section, whose core list breaks the same balances down
        // per core. Classic dock tabs leave it out and give the asset table the full area.
        let aggs = self.cached_aggs.clone();
        // The top bar owns filtering; the footer owns every summary figure the panel produces.
        let core_bar = self.core_bar(&cores, cx);
        let footer = self.footer(cx);
        let wallets = self.cached_wallets.clone();
        let tree_section =
            show_wallets.then(|| self.bottom(&aggs, &wallets, cx).into_any_element());
        // Built only when it will actually be shown — a non-empty table is the common case, and
        // the message is pure dead work there. Use the position-specific copy only for a fully
        // loaded futures-only scope while the dust threshold is active; every other state keeps
        // the generic Assets copy.
        let empty_msg = if count > 0 {
            String::new()
        } else if self.cached_all_futures && self.min_value_usd > 0.0 {
            t!("assets.empty_no_positions").to_string()
        } else {
            t!("assets.empty").to_string()
        };
        let table = table::assets_table(
            "assets-table",
            entries,
            self.sell_marked.clone(),
            Rc::new(self.visible_cols()),
            &self.table_state,
            empty_msg,
            cx,
        );
        // Supply the current width to the title-bar hit overlay for dragging, resizing, and controls.
        let chrome_width = crate::window::windowing::responsive_width(window);

        let mut root = v_flex()
            .id("assets-panel")
            .size_full()
            .relative()
            .min_h(px(0.0))
            .overflow_hidden()
            .track_focus(&self.focus)
            .font_family(design::mono())
            .text_size(design::t_body(cx))
            .bg(rgb(p.table_body))
            .when(windowed, |this| this.child(assets_header(p, cx)))
            .child(core_bar)
            .child(div().w_full().h(px(1.0)).flex_none().bg(rgb(p.border)));
        // The asset table is always present. With wallets it uses its natural content height so the
        // lower section can expand; in a dock tab it fills the space above the footer.
        let table_wrap = v_flex()
            .w_full()
            .min_h(px(0.0))
            .overflow_hidden()
            .child(table);
        root = root.child(if show_wallets {
            table_wrap.h(px(table_natural_h))
        } else {
            table_wrap.flex_1()
        });
        root = root.child(div().w_full().h(px(1.0)).flex_none().bg(rgb(p.border)));
        // In standalone views, let the wallet section consume the flexible space below the table.
        if let Some(tree) = tree_section {
            root = root
                .child(tree)
                .child(div().w_full().h(px(1.0)).flex_none().bg(rgb(p.border)));
        }
        // Footer: visible-row count and Σ on the left, scope account equity on the right.
        root = root.child(footer);
        if windowed {
            root = root.child(
                MoonWindowFrame::tool("assets-window-frame-hit", chrome_width)
                    .header_height(ASSETS_HEADER_H)
                    .leading_inset(design::titlebar_leading_inset())
                    .show_controls(design::show_custom_window_controls())
                    .hit_overlay(),
            );
        }
        root
    }
}
