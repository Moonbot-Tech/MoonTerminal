//! [OrdersPanel] event/focus/panel/render trait impls.

use super::*;

impl EventEmitter<PanelEvent> for OrdersPanel {}
impl Focusable for OrdersPanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}
impl Panel for OrdersPanel {
    fn panel_name(&self) -> &'static str {
        "Orders"
    }
    /// Visible tab caption. `panel_name` is the stable persistence key and stays untouched.
    fn tab_name(&self, _cx: &App) -> Option<SharedString> {
        crate::persistence::panel_meta::tab_label(self.panel_name())
    }
    // Closing returns the panel to the bottom row rather than deleting it; see
    // `Shell::PanelCloseRequested` handling.
    fn closable(&self, _cx: &App) -> bool {
        true
    }
    // A single panel split out on its own keeps its header so it still has a drag handle and close
    // button.
    fn show_dock_header(&self, _cx: &App) -> bool {
        true
    }
    fn title(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        crate::persistence::panel_meta::panel_title(self.panel_name())
    }
    fn dump(&self, cx: &App) -> PanelState {
        // Persist the group for reconstruction and the view's sorting, kind, filter, and column
        // settings. Since schema v11, Core IDs equal stable `ServerConfig.uid` values, but the
        // product intentionally resets the selected-core filter to All instead of persisting it,
        // matching the egui behavior.
        PanelState {
            panel_name: "Orders".to_string(),
            children: Vec::new(),
            info: PanelInfo::panel(serde_json::json!({
                "group": self.group,
                "primary": self.view.primary.to_u8(),
                "kind": self.view.kind.to_u8(),
                "newest_first": self.view.newest_first,
                "only_current": self.view.only_current_market,
                "main_on_top": self.view.main_on_top.to_u8(),
                // Store visible columns as stable keys rather than a mask so enum reordering is
                // harmless. A missing field restores every column as visible.
                "columns": self
                    .view
                    .visible_columns()
                    .iter()
                    .map(|c| c.key())
                    .collect::<Vec<_>>(),
                // Persist header drag order as stable keys. It lives in `table_state`, so read it
                // directly; an empty list leaves the default `ALL` order.
                "column_order": self
                    .table_state
                    .read(cx)
                    .column_order
                    .iter()
                    .map(|k| k.to_string())
                    .collect::<Vec<_>>(),
            })),
        }
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
        Some(vec![
            crate::persistence::table_persist::reset_button(
                "orders-reset-widths",
                &self.table_state,
            ),
            crate::panels::detach_button(
                "Orders",
                self.group.clone(),
                self.backend.clone(),
                self.dock.clone(),
            ),
        ])
    }
}

impl Render for OrdersPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        crate::diag::bump(&crate::diag::ORDERS_RENDER);
        let view = self.view;
        let cores = self.group_cores(self.backend.read(cx));
        let entries = self.cached_entries.clone();
        let p = MoonPalette::active(cx);

        // Control bar.
        let mut controls = h_flex()
            .w_full()
            .flex_none()
            .gap_2()
            .items_center()
            .px_2()
            .py_1()
            .child(self.source_combo(&cores, cx))
            .child(self.kind_combo(cx))
            .child(self.columns_menu(cx))
            .child(self.sort_menu(cx));
        if view.only_current_market {
            controls = controls.child(
                div()
                    .text_size(design::t_body(cx))
                    .text_color(rgb(p.text_muted))
                    .child(format!("· {}", t!("orders.only_current"))),
            );
        }

        // Virtualized table using the reference HTML geometry.
        // Highlight one row per Main-open `(core, market)` pair; see `table::orders_table`.
        // Prune optimistic stop toggles aged three seconds or more, then snapshot the survivors for
        // SL/TS/Vstop cells.
        self.stop_overlay
            .retain(|_, (_, t)| t.elapsed() < Duration::from_secs(3));
        let stop_overlay: Rc<std::collections::HashMap<(CoreId, u64, u8), bool>> = Rc::new(
            self.stop_overlay
                .iter()
                .map(|(k, (v, _))| (*k, *v))
                .collect(),
        );
        let table = table::orders_table(
            entries,
            view.columns,
            &self.table_state,
            self.highlight.clone(),
            stop_overlay,
            cx,
        );

        // Footer with total, real, and emulated open-order counts.
        let total = self.count_real + self.count_emu;
        let footer = h_flex()
            .w_full()
            .flex_none()
            .gap_2()
            .items_center()
            .px_2()
            .py_1()
            .child(
                div()
                    .text_size(design::t_body(cx))
                    .text_color(rgb(p.text_soft))
                    .child(
                        t!(
                            "orders.footer_total",
                            total = total,
                            real = self.count_real,
                            emu = self.count_emu
                        )
                        .to_string(),
                    ),
            );

        v_flex()
            .id("orders-panel")
            .size_full()
            .min_h(px(0.0))
            .overflow_hidden()
            .track_focus(&self.focus)
            .font_family(design::mono())
            .text_size(design::t_body(cx))
            .bg(rgb(p.table_body))
            .child(controls)
            .child(div().w_full().h(px(1.0)).flex_none().bg(rgb(p.border)))
            .child(table)
            .child(div().w_full().h(px(1.0)).flex_none().bg(rgb(p.border)))
            .child(footer)
    }
}
