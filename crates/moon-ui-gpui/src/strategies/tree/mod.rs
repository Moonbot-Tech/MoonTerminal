//! Left pane of the Strategies window: an exchange -> core -> folder -> strategy tree with
//! search/kind/direction filters, staged checkboxes, and start/stop (Apply) buttons. These methods
//! extend `StrategiesView`; state and pure helpers live in [`super`] and [`super::logic`].

pub(crate) mod cache;
pub(crate) mod dialogs;
pub(crate) mod dnd;
pub(crate) mod menu;
pub(crate) mod moon;
pub(crate) mod ops;
pub(crate) mod ui;

use super::*;

use moon_ui::{MoonButtonIconSlot, MoonDisclosureDirection};
use rust_i18n::t;

impl StrategiesView {
    pub(super) fn tree_panel(
        &self,
        store: &CoreStore,
        cores: &crate::core_order::OrderedCores,
        node_data: std::rc::Rc<std::collections::HashMap<SharedString, moon::NodeData>>,
        cx: &Context<Self>,
    ) -> AnyElement {
        let p = MoonPalette::active(cx);
        let border = moon(p.border);

        // MoonTree itself is headless and owns flattening, virtualization, and drag-and-drop.
        // The `CoreStore -> MoonTreeItem` adapter plus rows, DnD, and menus live in `moon`.
        let tree_el = self.moon_tree_el(node_data, cx);

        // Search, strategy-kind filter, and direction filter.
        let kinds = kinds_present(cores, store);
        let kind_text = self
            .filter
            .kind
            .and_then(|k| kinds.iter().find(|(o, _)| *o == k))
            .map(|(_, n)| n.clone())
            .unwrap_or_else(|| t!("strat.all_kinds").to_string());
        let dir_text = match self.filter.dir {
            None => t!("strat.all_dirs").to_string(),
            Some(true) => "SHORT".to_string(),
            Some(false) => "LONG".to_string(),
        };

        let collapsed = self.expanded_cores.is_empty() && self.expanded_folders.is_empty();
        let cores_owned: Arc<Vec<(CoreId, String)>> = Arc::new(cores.to_vec());

        v_flex()
            // The splitter resizes this width, persisted in layout.strategies_panels.
            .w(px(self.panels.tree_w))
            .flex_none()
            .h_full()
            .bg(moon(p.shell_high))
            .font_family(design::mono())
            .text_size(design::t_body(cx))
            .line_height(design::line_px(cx, 14.0))
            .border_r_1()
            .border_color(border)
            // Top filters.
            .child(
                v_flex()
                    .w_full()
                    .px(design::ui_px(cx, 10.0))
                    .py(design::ui_px(cx, 10.0))
                    .gap(design::ui_px(cx, 7.0))
                    .child(
                        div().w_full().child(
                            MoonInput::new("strat-search")
                                .state(&self.search)
                                .small()
                                .cleanable(true),
                        ),
                    )
                    .child(
                        h_flex()
                            .w_full()
                            .gap(design::ui_px(cx, 7.0))
                            .items_center()
                            .justify_between()
                            .child(
                                h_flex()
                                    .min_w_0()
                                    .gap(design::ui_px(cx, 7.0))
                                    .items_center()
                                    .child(self.combo_kind(kind_text, kinds, cx))
                                    .child(self.combo_dir(dir_text, cx))
                                    .child({
                                        let (cc, ct) = self.default_target(store, cores);
                                        self.create_dropdown(cc, ct, cx)
                                    }),
                            )
                            .child(
                                MoonButton::new("expand-all")
                                    .ghost()
                                    .size(MoonButtonSize::Micro)
                                    .leading_icon(MoonButtonIconSlot::caret(
                                        MoonDisclosureDirection::DownUp,
                                        !collapsed,
                                    ))
                                    .on_click({
                                        let cores = cores_owned.clone();
                                        cx.listener(move |this, _, _, cx| {
                                            let store = this.backend.read(cx).session.store();
                                            let coll = this.expanded_cores.is_empty()
                                                && this.expanded_folders.is_empty();
                                            // store borrow tied to cx; clone cores for &-call.
                                            let cores_v = cores.as_ref().clone();
                                            this.expand_collapse_toggle(&cores_v, store, coll);
                                            cx.notify();
                                        })
                                    })
                                    .render(),
                            ),
                    ),
            )
            .child(div().w_full().h(px(1.0)).bg(border))
            // Tree; MoonTree handles its own virtualization and scrolling.
            .child(
                div()
                    .id("strat-tree-scroll")
                    .flex_1()
                    .w_full()
                    .min_h_0()
                    .p(px(8.0))
                    .child(tree_el),
            )
            // Bottom action bar.
            .child(div().w_full().h(px(1.0)).bg(border))
            .child(self.action_bar(cores_owned, store, cx))
            .into_any_element()
    }

    /// Render the kind-filter combo box with "all kinds" and the kinds currently present.
    fn combo_kind(
        &self,
        current: String,
        kinds: Vec<(u8, String)>,
        cx: &Context<Self>,
    ) -> AnyElement {
        let view = cx.entity();
        let selected_kind = self.filter.kind;
        let mut items = vec![
            MoonMenuItem::with_key("kind-all", t!("strat.all_kinds").to_string())
                .selected(selected_kind.is_none())
                .on_click({
                    let view = view.clone();
                    move |_, _, app| {
                        view.update(app, |this, c| {
                            if this.filter.kind.is_some() {
                                this.filter.kind = None;
                                c.notify();
                            }
                        });
                    }
                }),
        ];
        for (ord, name) in kinds {
            let view = view.clone();
            items.push(
                MoonMenuItem::with_key(format!("kind-{ord}"), name.clone())
                    .selected(selected_kind == Some(ord))
                    .on_click({
                        let name_ord = ord;
                        move |_, _, app| {
                            view.update(app, |this, c| {
                                if this.filter.kind != Some(name_ord) {
                                    this.filter.kind = Some(name_ord);
                                    c.notify();
                                }
                            });
                        }
                    }),
            );
        }
        MoonDropdown::new("strat-kind-filter")
            .label(current)
            .trigger_caret(true)
            .trigger_variant(MoonButtonVariant::Soft)
            .trigger_size(MoonButtonSize::Action)
            .trigger_width_scaled(116.0)
            .menu_width_scaled(180.0)
            .menu_size(MoonMenuSize::Compact)
            .menu_max_height_ui(240.0)
            .items(items)
            .into_any_element()
    }

    /// Render the direction-filter combo box with all, LONG, and SHORT options.
    fn combo_dir(&self, current: String, cx: &Context<Self>) -> AnyElement {
        let view = cx.entity();
        let opts: [(&str, String, Option<bool>); 3] = [
            ("all", t!("strat.all_dirs").to_string(), None),
            ("LONG", "LONG".to_string(), Some(false)),
            ("SHORT", "SHORT".to_string(), Some(true)),
        ];
        let mut items = Vec::with_capacity(opts.len());
        for (id, label, val) in opts {
            let view = view.clone();
            items.push(
                MoonMenuItem::with_key(format!("dir-{id}"), label)
                    .selected(self.filter.dir == val)
                    .on_click(move |_, _, app| {
                        view.update(app, |this, c| {
                            if this.filter.dir != val {
                                this.filter.dir = val;
                                c.notify();
                            }
                        });
                    }),
            );
        }
        MoonDropdown::new("strat-dir-filter")
            .label(current)
            .trigger_caret(true)
            .trigger_variant(MoonButtonVariant::Soft)
            .trigger_size(MoonButtonSize::Action)
            .trigger_width_scaled(128.0)
            .menu_width_scaled(140.0)
            .menu_size(MoonMenuSize::Compact)
            .items(items)
            .into_any_element()
    }

    /// Render the bottom action bar.
    ///
    /// Copy/Paste/Delete and Start/Stop stay on one row around a centered staged count. Every
    /// localized label collapses atomically to its icon when the tree pane cannot fit the full set.
    ///
    /// Args:
    ///     cores: Canonical visible roots represented by the rendered Start/Stop buttons.
    ///     store: Strategy snapshot used to capture their exact target plan.
    ///     cx: View context used to build callbacks and capture workspace authority.
    ///
    /// Returns:
    ///     Bottom action bar whose delayed callbacks refuse stale target plans atomically.
    fn action_bar(
        &self,
        cores: Arc<Vec<(CoreId, String)>>,
        store: &CoreStore,
        cx: &Context<Self>,
    ) -> AnyElement {
        let copy_label = t!("strat.action_copy").to_string();
        let paste_label = t!("strat.action_paste").to_string();
        let delete_label = t!("strat.action_delete").to_string();
        let start_label = t!("strat.start_checked").to_string();
        let stop_label = t!("strat.stop_checked").to_string();
        let staged = staged_count(self);
        let staged_label = (staged > 0).then(|| t!("strat.staged", n = staged).to_string());

        let measured_label_width = [
            copy_label.as_str(),
            paste_label.as_str(),
            delete_label.as_str(),
            start_label.as_str(),
            stop_label.as_str(),
        ]
        .into_iter()
        .map(|label| design::ui_text_width(cx, label, 10.5, 400.0, true))
        .sum::<f32>()
            + staged_label.as_deref().map_or(0.0, |label| {
                design::ui_text_width(cx, label, 10.5, 400.0, true)
            });
        // Five native leading icons remain in both densities. The full state additionally reserves
        // their label gaps, six group gaps, outer padding, and the hairline divider.
        let action_icon_width = (design::font_value(cx, 10.5) + 1.0).clamp(10.0, 14.0);
        // Action size ships with pad_x = 0. Labeled footer buttons opt into the same 7-unit
        // inset used by other Action labels so text does not sit on the border.
        let labeled_pad = design::ui_value(cx, 7.0) * 2.0 * 5.0;
        let fixed_width = 5.0 * action_icon_width
            + design::ui_value(cx, 5.0 * 6.0 + 6.0 * design::CHROME_GAP + 16.0)
            + 1.0
            + labeled_pad;
        let show_labels =
            ui::footer_labels_fit(self.panels.tree_w, fixed_width, measured_label_width);

        // Each callback owns the exact plan this rendered button described. A workspace change
        // before dispatch cannot silently reduce that old multi-core action to a surviving subset.
        let plan = Arc::new(self.start_stop_plan(cores.as_ref(), store, cx));
        let icon_width = design::glyph_btn_w(cx);
        let mut start = MoonButton::new("start-checked")
            .primary()
            .size(MoonButtonSize::Action)
            .leading_icon(MoonButtonIconSlot::new("icons/play.svg"))
            .tooltip(format!("▶ {start_label}"))
            .on_click({
                let plan = plan.clone();
                cx.listener(move |this, _, _, cx| {
                    this.apply_start_stop(plan.as_ref(), true, cx);
                })
            });
        let mut stop = MoonButton::new("stop-checked")
            .outline()
            .size(MoonButtonSize::Action)
            .leading_icon(MoonButtonIconSlot::new("icons/pause.svg"))
            .tooltip(format!("■ {stop_label}"))
            .on_click({
                let plan = plan.clone();
                cx.listener(move |this, _, _, cx| {
                    this.apply_start_stop(plan.as_ref(), false, cx);
                })
            });
        if show_labels {
            start = start.padding_x(7.0).label(start_label);
            stop = stop.padding_x(7.0).label(stop_label);
        } else {
            start = start.width(icon_width);
            stop = stop.width(icon_width);
        }

        let group_gap = if show_labels {
            design::CHROME_GAP
        } else {
            design::CHROME_GAP / 2.0
        };
        let right = h_flex()
            .flex_none()
            .items_center()
            .gap(design::ui_px(cx, group_gap))
            .child(start.render())
            .child(stop.render());
        let horizontal_pad = if show_labels { 8.0 } else { 4.0 };
        let staged_slot = div()
            .flex_1()
            .min_w_0()
            .truncate()
            .text_center()
            .text_size(design::t_body(cx))
            .text_color(rgb(MoonPalette::active(cx).amber))
            .child(staged_label.unwrap_or_default());

        h_flex()
            .w_full()
            .px(design::ui_px(cx, horizontal_pad))
            .py(design::ui_px(cx, 8.0))
            .gap(design::ui_px(cx, group_gap))
            .items_center()
            .child(self.selection_toolbar(store, show_labels, cx))
            .child(design::chrome_divider(cx, MoonPalette::active(cx)))
            .child(staged_slot)
            .child(right)
            .into_any_element()
    }

    // Section-pane rendering remains in the parent module.
}
