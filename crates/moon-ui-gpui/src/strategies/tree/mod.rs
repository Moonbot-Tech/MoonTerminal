//! Left pane of the Strategies window: an optionally grouped core -> folder -> strategy tree with
//! search/kind/direction/exchange filters, persisted display settings, staged checkboxes, and start/stop
//! (Apply) buttons. These methods extend `StrategiesView`; state and pure helpers live in [`super`]
//! and [`super::logic`].

pub(crate) mod cache;
pub(crate) mod checks;
pub(crate) mod dialogs;
pub(crate) mod dnd;
mod empty_state;
pub(crate) mod menu;
pub(crate) mod moon;
pub(crate) mod ops;
pub(in crate::strategies) mod pane_cache;
pub(crate) mod ui;

#[cfg(test)]
mod tests;

use super::*;

use moon_ui::{MoonButtonIconSlot, MoonDisclosureDirection};
use rust_i18n::t;

use ui::FolderDrag;

/// Floor for the search field, in design units. Below it the row wraps its two trailing controls
/// onto their own line rather than squeezing the field toward nothing.
const SEARCH_MIN_W: f32 = 90.0;

/// Cancel a folder drag as soon as its pointer leaves the Strategies tree field.
///
/// GPUI keeps drag payloads application-global, while folder destinations belong only to this
/// tree. Binding the typed listener to the field preserves strategy drags and every row-local
/// folder drop rule while preventing an escaped folder payload from remaining active.
fn constrain_folder_drag_to_tree(tree: Div) -> Div {
    tree.on_drag_move(
        |event: &DragMoveEvent<FolderDrag>, window: &mut Window, cx: &mut App| {
            if !event.bounds.contains(&event.event.position) {
                cx.stop_active_drag(window);
            }
        },
    )
}

/// Expand/collapse treats only currently visible cores as the tree the button controls.
///
/// A process snapshot can hold expansion for cores that left the Auto rail while the window was
/// closed. Counting those leftover ids as "expanded" would make a visibly collapsed tree run the
/// collapse branch on the first click.
fn visible_tree_collapsed(
    expanded_cores: &HashSet<CoreId>,
    expanded_folders: &HashSet<(CoreId, String)>,
    cores: &[(CoreId, String)],
) -> bool {
    cores.iter().all(|(core, _)| {
        !expanded_cores.contains(core) && expanded_folders.iter().all(|(c, _)| c != core)
    })
}

impl StrategiesView {
    /// Render the Strategies tree pane, its responsive filter row, and atomic action footer.
    ///
    /// Args:
    ///     store: Current strategy and order snapshot.
    ///     cores: Visible cores in canonical order.
    ///     node_data: Row data keyed by the tree adapter's stable ids.
    ///     pane: This frame's cached kind and exchange lists, Start/Stop plan, and footer label width.
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

        // Resolved before `node_data` moves into the adapter below: `nodes` is the ladder's
        // first-priority signal, and a build with at least one row never reaches the rest.
        let nodes = node_data.len();
        // A connected core enters `CoreData` with `strategies_rev == 0` before its first snapshot
        // arrives — a genuinely empty one included, which is what makes this the loading/empty
        // split rather than a redundant "no cores" check. Read-only against the live store, same
        // field `tree::cache::data_sig` already hashes.
        //
        // ANY, not ALL, and the difference is the whole point of the state: with two visible cores
        // where one has already answered "no strategies" and the other has not answered at all, an
        // ALL test reads false and the ladder falls through to "no strategies yet" while the second
        // core may still deliver some. One unanswered core is enough to owe the user a wait.
        let awaiting_snapshot = cores
            .iter()
            .any(|(core, _)| store.core(*core).is_none_or(|cd| cd.strategies_rev == 0));
        let empty = empty_state::tree_empty_state(
            cores.len(),
            nodes,
            awaiting_snapshot,
            self.filter.narrows(),
            self.scope_marker.as_ref(),
        );

        // MoonTree itself is headless and owns flattening, virtualization, and drag-and-drop.
        // The `CoreStore -> MoonTreeItem` adapter plus rows, DnD, and menus live in `moon`.
        let tree_el = self.moon_tree_el(node_data, cx);

        // Search and the strategy-kind, direction, and exchange filters.
        // A restored ordinal whose rows have all left the visible tree must still name itself.
        // Falling back to "all kinds" would leave an empty tree beside a trigger claiming nothing
        // is filtered, the same trap the exchange caption documents below.
        let kind_text = match self.filter.kind {
            None => t!("strat.all_kinds").to_string(),
            Some(k) => pane
                .kinds
                .iter()
                .find(|(o, _)| *o == k)
                .map(|(_, n)| n.clone())
                .unwrap_or_else(|| k.to_string()),
        };
        let dir_text = match self.filter.dir {
            None => t!("strat.all_dirs").to_string(),
            Some(true) => "SHORT".to_string(),
            Some(false) => "LONG".to_string(),
        };

        // A selection whose cores have all disconnected still names itself, from the identity alone
        // (`venue_id_label`) or as the shared unidentified caption. Falling back to "all exchanges"
        // instead would leave an empty tree beside a trigger claiming nothing is filtered.
        let exchange_text = match self.filter.exchange {
            None => t!("strat.all_exchanges").to_string(),
            Some(selected) => pane
                .exchanges
                .iter()
                .find(|(section, _)| *section == selected)
                .map(|(_, label)| label.clone())
                .unwrap_or_else(|| match selected {
                    crate::core_order::ExchangeSection::Venue(id) => {
                        crate::controls::venue_id_label(id)
                    }
                    crate::core_order::ExchangeSection::Unidentified => {
                        crate::controls::venue_section_label(None)
                    }
                }),
        };

        let collapsed = visible_tree_collapsed(&self.expanded_cores, &self.expanded_folders, cores);
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
                            .child(self.combo_exchange(exchange_text, &pane.exchanges, cx))
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
                                            let coll = visible_tree_collapsed(
                                                &this.expanded_cores,
                                                &this.expanded_folders,
                                                &cores,
                                            );
                                            this.expand_collapse_toggle(&cores, store, coll);
                                            this.persist_session(cx);
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
                constrain_folder_drag_to_tree(div())
                    .id("strat-tree-scroll")
                    .flex_1()
                    .w_full()
                    .min_h_0()
                    .p(px(8.0))
                    .relative()
                    .child({
                        let bounds_cell = self.tree_field_bounds.clone();
                        canvas(
                            move |bounds, _window, _app| {
                                bounds_cell.set(Some(bounds));
                            },
                            |_, _, _, _| {},
                        )
                        .absolute()
                        .size_full()
                    })
                    .child(tree_el)
                    // Sibling AFTER `tree_el`, never in its place: dropping the tree would drop
                    // `MoonTreeState` and every expansion the user has open. Absolute + `inset_0`
                    // overlays it instead, so nothing hidden ⇒ no overlay at all and today's tree
                    // renders byte-identical.
                    .when_some(empty, |el, state| {
                        el.child(
                            v_flex()
                                .id("strat-tree-empty")
                                .absolute()
                                .inset_0()
                                .items_center()
                                .justify_center()
                                .gap(design::ui_px(cx, 4.0))
                                .text_color(moon(p.text_muted))
                                .child(
                                    div()
                                        .text_size(design::t_body(cx))
                                        .child(empty_state::headline(state)),
                                )
                                .when(state == empty_state::TreeEmptyState::HiddenByPreset, |el| {
                                    let Some(marker) = self.scope_marker.as_ref() else {
                                        return el;
                                    };
                                    // `line`, not `facts`: this caption is centred under the
                                    // headline with nothing to its left, so the footer tail's
                                    // leading separator would open it with a stray bullet.
                                    let caption = marker.line();
                                    if caption.is_empty() {
                                        return el;
                                    }
                                    let tip = marker.tooltip(std::slice::from_ref(&caption));
                                    let mut caption_el = div()
                                        .id("strat-tree-empty-caption")
                                        .text_size(design::t_caption(cx))
                                        .child(caption);
                                    if !tip.is_empty() {
                                        caption_el = caption_el.tooltip(
                                            crate::panels::common::text_tooltip(
                                                SharedString::from(tip),
                                            ),
                                        );
                                    }
                                    el.child(caption_el)
                                }),
                        )
                    })
                    .on_drag_move(cx.listener(
                        |this, event: &DragMoveEvent<ui::StratDrag>, window, cx| {
                            let event_window = window.window_handle().window_id();
                            if ui::strat_drag_event_should_stop(
                                event.drag(cx),
                                event_window,
                                event.event.position,
                                this.tree_field_bounds.get(),
                            ) {
                                cx.stop_active_drag(window);
                            }
                        },
                    )),
            )
            // The scope marker for a PARTIAL hide, where the tree has rows and the empty overlay
            // above never renders. Without it a three-core tree out of fifty-six looks like the
            // whole fleet, and this window has no footer of its own to carry the fact — every
            // other filtering surface states it, so silence here reads as "nothing is hidden".
            // Withheld when the overlay is already saying it, or the sentence would appear twice.
            .children(
                self.scope_marker
                    .as_ref()
                    .filter(|marker| {
                        marker.hides_anything()
                            && empty != Some(empty_state::TreeEmptyState::HiddenByPreset)
                    })
                    .map(|marker| {
                        let caption = marker.line();
                        let tip = marker.tooltip(std::slice::from_ref(&caption));
                        div()
                            .id("strat-tree-scope-marker")
                            .w_full()
                            .flex_none()
                            .px(px(8.0))
                            .py(px(2.0))
                            .text_size(design::t_caption(cx))
                            .text_color(moon(p.text_muted))
                            .tooltip(crate::panels::common::text_tooltip(SharedString::from(tip)))
                            .child(caption)
                    }),
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
                                this.persist_session(c);
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
                                    this.persist_session(c);
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
                                this.persist_session(c);
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

    /// Render the exchange-filter combo box with "all exchanges" and the sections currently present.
    ///
    /// Single selection, like its two neighbours: one exchange narrows the tree to the cores of that
    /// section. The entries are DERIVED — the list is whatever the connected cores partition into,
    /// so a build never carries a roster of exchange names to fall behind the directory.
    ///
    /// Args:
    ///     current: Caption already resolved for the trigger, including the vanished-selection case.
    ///     exchanges: This frame's cached sections in canonical order.
    ///     cx: View context used to wire the selection callbacks.
    ///
    /// Returns:
    ///     Complete dropdown as one type-erased element.
    fn combo_exchange(
        &self,
        current: String,
        exchanges: &[(crate::core_order::ExchangeSection, String)],
        cx: &Context<Self>,
    ) -> AnyElement {
        use crate::core_order::ExchangeSection;

        let view = cx.entity();
        let selected_exchange = self.filter.exchange;
        let mut items = Vec::with_capacity(exchanges.len() + 1);
        items.push(
            MoonMenuItem::with_key("exch-all", t!("strat.all_exchanges").to_string())
                .selected(selected_exchange.is_none())
                .on_click({
                    let view = view.clone();
                    move |_, _, app| {
                        view.update(app, |this, c| {
                            if this.filter.exchange.is_some() {
                                this.filter.exchange = None;
                                this.persist_session(c);
                                c.notify();
                            }
                        });
                    }
                }),
        );
        for (section, label) in exchanges {
            let section = *section;
            let view = view.clone();
            // Keyed by IDENTITY, not by the caption: an element id built from a localized or
            // wire-reported name would change under a language switch and under a core rename.
            let key = match section {
                ExchangeSection::Unidentified => "exch-unknown".to_string(),
                ExchangeSection::Venue(id) => format!("exch-{}-{}", id.code, id.dex),
            };
            items.push(
                MoonMenuItem::with_key(key, label.clone())
                    .selected(selected_exchange == Some(section))
                    .on_click(move |_, _, app| {
                        view.update(app, |this, c| {
                            if this.filter.exchange != Some(section) {
                                this.filter.exchange = Some(section);
                                this.persist_session(c);
                                c.notify();
                            }
                        });
                    }),
            );
        }
        MoonDropdown::new("strat-exchange-filter")
            .label(current)
            .trigger_caret(true)
            .trigger_variant(MoonButtonVariant::Soft)
            .trigger_size(MoonButtonSize::Action)
            // Roomier than its neighbours at both ends: a venue caption is a brand plus a market
            // kind ("Binance Quarterly"), and a HIP-3 core appends its DEX name on top of that.
            .fit_trigger_width(96.0, 150.0)
            .fit_menu_width(160.0, 320.0)
            .menu_size(MoonMenuSize::Compact)
            .menu_max_height_ui(240.0)
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
