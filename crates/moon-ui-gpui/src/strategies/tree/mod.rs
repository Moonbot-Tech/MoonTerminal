//! Left pane of the Strategies window: an optionally grouped core -> folder -> strategy tree with
//! search/kind/direction filters, persisted display settings, staged checkboxes, and start/stop
//! (Apply) buttons. These methods extend `StrategiesView`; state and pure helpers live in [`super`]
//! and [`super::logic`].

pub(crate) mod cache;
pub(crate) mod dialogs;
pub(crate) mod dnd;
pub(crate) mod menu;
pub(crate) mod moon;
pub(crate) mod ops;
pub(in crate::strategies) mod pane_cache;
pub(crate) mod ui;

use super::*;

use moon_ui::{MoonButtonIconSlot, MoonDisclosureDirection};
use rust_i18n::t;

/// Floor for the search field, in design units. Below it the row wraps its two trailing controls
/// onto their own line rather than squeezing the field toward nothing.
const SEARCH_MIN_W: f32 = 90.0;

impl StrategiesView {
    /// Render the Strategies tree pane, its responsive filter row, and atomic action footer.
    ///
    /// Args:
    ///     store: Current strategy and order snapshot.
    ///     cores: Visible cores in canonical order.
    ///     node_data: Row data keyed by the tree adapter's stable ids.
    ///     pane: This frame's cached kinds list, Start/Stop plan and footer label width.
    ///     cx: View context used by the settings popover and callbacks.
    ///
    /// Returns:
    ///     Complete left pane as one type-erased element.
    pub(super) fn tree_panel(
        &self,
        store: &CoreStore,
        cores: &crate::core_order::OrderedCores,
        node_data: std::rc::Rc<std::collections::HashMap<SharedString, moon::NodeData>>,
        pane: &LeftPaneFrame,
        cx: &Context<Self>,
    ) -> AnyElement {
        let p = MoonPalette::active(cx);
        let border = moon(p.border);

        // MoonTree itself is headless and owns flattening, virtualization, and drag-and-drop.
        // The `CoreStore -> MoonTreeItem` adapter plus rows, DnD, and menus live in `moon`.
        let tree_el = self.moon_tree_el(node_data, cx);

        // Search, strategy-kind filter, and direction filter.
        let kind_text = self
            .filter
            .kind
            .and_then(|k| pane.kinds.iter().find(|(o, _)| *o == k))
            .map(|(_, n)| n.clone())
            .unwrap_or_else(|| t!("strat.all_kinds").to_string());
        let dir_text = match self.filter.dir {
            None => t!("strat.all_dirs").to_string(),
            Some(true) => "SHORT".to_string(),
            Some(false) => "LONG".to_string(),
        };

        let collapsed = self.expanded_cores.is_empty() && self.expanded_folders.is_empty();
        let settings =
            self.settings_popover(super::settings::settings_trigger(self.settings_open), p, cx);

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
                    // Search row: the input takes the slack, then the active-only toggle and the
                    // settings gear ride flush right. It wraps like the filter row below it, and
                    // the input keeps a floor so a narrow pane at a raised font scale drops the
                    // trailing controls onto their own line — the gear first — instead of squeezing
                    // the field to nothing.
                    .child(
                        h_flex()
                            .w_full()
                            .flex_wrap()
                            .items_center()
                            .gap_x(design::ui_px(cx, 7.0))
                            .gap_y(design::ui_px(cx, 5.0))
                            .child(
                                div().flex_1().min_w(design::ui_px(cx, SEARCH_MIN_W)).child(
                                    MoonInput::new("strat-search")
                                        .state(&self.search)
                                        .small()
                                        .cleanable(true),
                                ),
                            )
                            .child(self.active_only_toggle(p, cx))
                            .child(settings),
                    )
                    .child(
                        h_flex()
                            .w_full()
                            .flex_wrap()
                            .gap_x(design::ui_px(cx, 7.0))
                            .gap_y(design::ui_px(cx, 5.0))
                            .items_center()
                            .child(self.combo_kind(kind_text, &pane.kinds, cx))
                            .child(self.combo_dir(dir_text, cx))
                            .child({
                                let (cc, ct) = self.default_target(store, cores);
                                self.create_dropdown(cc, ct, cx)
                            })
                            .child(
                                // Wrapper, not a bare button: the caret is right-aligned by
                                // `ml_auto` and must not be shrunk by the wrapping row.
                                h_flex().ml_auto().flex_none().items_center().child(
                                    MoonButton::new("expand-all")
                                        .ghost()
                                        .size(MoonButtonSize::Micro)
                                        .leading_icon(MoonButtonIconSlot::caret(
                                            MoonDisclosureDirection::DownUp,
                                            !collapsed,
                                        ))
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            // Resolved at click time, like the paste handler in
                                            // `ui.rs`: capturing the frame's list instead cost
                                            // a deep copy of every core name on every repaint.
                                            let backend = this.backend.read(cx);
                                            let cores = visible_strategy_cores(this, backend);
                                            let store = backend.session.store();
                                            let coll = this.expanded_cores.is_empty()
                                                && this.expanded_folders.is_empty();
                                            this.expand_collapse_toggle(&cores, store, coll);
                                            cx.notify();
                                        }))
                                        .render(),
                                ),
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
            .child(self.action_bar(cores, store, pane, cx))
            .into_any_element()
    }

    /// Render the kind-filter combo box with "all kinds" and the kinds currently present.
    fn combo_kind(
        &self,
        current: String,
        kinds: &[(u8, String)],
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
            let ord = *ord;
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
            .fit_trigger_width(96.0, 116.0)
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
            .fit_trigger_width(96.0, 128.0)
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
    ///     cores: Canonical visible roots the paste target resolves against.
    ///     store: Strategy snapshot used for selection-dependent enablement.
    ///     pane: This frame's cached Start/Stop plan and measured footer label width.
    ///     cx: View context used to build callbacks and scaled widths.
    ///
    /// Returns:
    ///     Bottom action bar whose delayed callbacks refuse stale target plans atomically.
    fn action_bar(
        &self,
        cores: &crate::core_order::OrderedCores,
        store: &CoreStore,
        pane: &LeftPaneFrame,
        cx: &Context<Self>,
    ) -> AnyElement {
        let start_label = t!("strat.start_checked").to_string();
        let stop_label = t!("strat.stop_checked").to_string();
        // The same count the cached width was measured against, so the rendered label and the
        // density decision cannot describe different states.
        let staged = pane.staged;
        let staged_label = (staged > 0).then(|| t!("strat.staged", n = staged).to_string());

        // Measured behind the pane cache: one full set of labels is ~40 uncached per-glyph
        // advances, and this row is rebuilt on every hover repaint.
        let measured_label_width = pane.footer_label_width;
        // Five native leading icons remain in both densities. The full state additionally reserves
        // their label gaps, six group gaps, outer padding, and the hairline divider.
        let action_icon_width =
            (design::font_value(cx, design::ACTION_LABEL_BASE) + 1.0).clamp(10.0, 14.0);
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
        // The plan comes from the pane cache, which keys it on the workspace generation among the
        // rest — so a captured plan still describes the frame it was rendered on.
        let plan = pane.plan.clone();
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
            .child(self.selection_toolbar(store, show_labels, !cores.is_empty(), cx))
            .child(design::chrome_divider(cx, MoonPalette::active(cx)))
            .child(staged_slot)
            .child(right)
            .into_any_element()
    }

    // Section-pane rendering remains in the parent module.
}
