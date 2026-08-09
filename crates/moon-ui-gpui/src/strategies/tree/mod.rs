//! Left pane of the Strategies window: a core -> folder -> strategy tree with search/kind/direction
//! filters, staged checkboxes, and start/stop (Apply) buttons. These methods extend
//! `StrategiesView`; state and pure helpers live in [`super`] and [`super::logic`].

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
                            .child(self.combo_kind(kind_text, kinds, cx))
                            .child(self.combo_dir(dir_text, cx))
                            .child({
                                let (cc, ct) = self.default_target(store, cores);
                                self.create_dropdown(cc, ct, cx)
                            }),
                    )
                    .child(
                        h_flex()
                            .w_full()
                            .items_center()
                            .justify_between()
                            .child(
                                MoonCheckbox::new("flt-active")
                                    .label(t!("strat.only_active").to_string())
                                    .checked(self.filter.only_active)
                                    .size(MoonCheckboxSize::Compact)
                                    .on_change(cx.listener(|this, ch: &bool, _, cx| {
                                        if this.filter.only_active != *ch {
                                            this.filter.only_active = *ch;
                                            cx.notify();
                                        }
                                    })),
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
            .trigger_width_scaled(80.0)
            .menu_width_scaled(120.0)
            .menu_size(MoonMenuSize::Compact)
            .items(items)
            .into_any_element()
    }

    /// Render the bottom action bar.
    ///
    /// The selection group is on the LEFT with copy/paste and a full-width delete button below;
    /// stacked Start Checked/Stop Checked actions are on the RIGHT, with the staged count in the center.
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
        // Each callback owns the exact plan this rendered button described. A workspace change
        // before dispatch cannot silently reduce that old multi-core action to a surviving subset.
        let plan = Arc::new(self.start_stop_plan(cores.as_ref(), store, cx));
        // Right-align the stacked Start Checked/Stop Checked action group.
        let right = v_flex()
            .gap_1()
            .items_end()
            .child(
                MoonButton::new("start-checked")
                    .primary()
                    .size(MoonButtonSize::Micro)
                    .label(format!("▶ {}", t!("strat.start_checked")))
                    .on_click({
                        let plan = plan.clone();
                        cx.listener(move |this, _, _, cx| {
                            this.apply_start_stop(plan.as_ref(), true, cx);
                        })
                    })
                    .render(),
            )
            .child(
                MoonButton::new("stop-checked")
                    .outline()
                    .size(MoonButtonSize::Micro)
                    .label(format!("■ {}", t!("strat.stop_checked")))
                    .on_click({
                        let plan = plan.clone();
                        cx.listener(move |this, _, _, cx| {
                            this.apply_start_stop(plan.as_ref(), false, cx);
                        })
                    })
                    .render(),
            );

        let mut bar = h_flex()
            .w_full()
            .p_2()
            .gap_2()
            .items_start()
            .justify_between()
            .child(self.selection_toolbar(store, cx));
        let staged = staged_count(self);
        if staged > 0 {
            bar = bar.child(
                div()
                    .text_size(design::t_body(cx))
                    .text_color(rgb(MoonPalette::active(cx).amber))
                    .child(t!("strat.staged", n = staged).to_string()),
            );
        }
        bar.child(right).into_any_element()
    }

    // Section-pane rendering remains in the parent module.
}
