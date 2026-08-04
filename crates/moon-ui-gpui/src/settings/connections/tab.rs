//! Connections tab assembly: pending-core, group, and exchange branch headers; icon picker;
//! market-data and core-order selectors; and the editable hierarchy with its top add button.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use gpui::*;
use moon_ui::{
    MoonButton, MoonButtonSize, MoonCheckbox, MoonCheckboxSize, MoonMenuSize, MoonPalette,
    MoonSelect, MoonTooltipView, StyledExt, h_flex, v_flex,
};
use rust_i18n::t;

use super::SettingsView;
use crate::design;
use moon_core::config::GroupConfig;
use moon_core::session::CoreId;

/// Draft server metadata needed to assemble the Connections hierarchy.
pub(super) type ServerRowMeta = (CoreId, u64, bool, String, Option<String>);

/// Visible group metadata needed to render one Connections branch.
pub(super) type GroupRowMeta = (String, bool, u32);

/// Project retained draft groups onto names referenced by the current server rows.
///
/// Settings keeps orphaned `GroupConfig` rows until Save so erasing and retyping a name preserves
/// its local trading controls. Those historical rows must not become visible branches while the
/// user is still typing. Pending rows participate because their group name is editable too.
///
/// Args:
///     servers: Every current draft server row, including rows whose uid is still zero.
///     groups: Retained draft group configurations, including intermediate names.
///
/// Returns:
///     Referenced groups in their stored order with their active and icon metadata intact.
pub(super) fn visible_group_rows(
    servers: &[ServerRowMeta],
    groups: &[GroupConfig],
) -> Vec<GroupRowMeta> {
    let referenced: HashSet<&str> = servers
        .iter()
        .map(|(_, _, _, group, _)| group.as_str())
        .collect();
    groups
        .iter()
        .filter(|group| referenced.contains(group.name.as_str()))
        .map(|group| (group.name.clone(), group.active, group.icon))
        .collect()
}

/// Return draft indices for unsaved cores that must stay in the top pending section.
pub(super) fn pending_server_indices(servers: &[ServerRowMeta]) -> Vec<usize> {
    servers
        .iter()
        .enumerate()
        .filter_map(|(index, (_, uid, _, _, _))| (*uid == 0).then_some(index))
        .collect()
}

/// Group one window group's server indices into an unknown-first exchange hierarchy.
///
/// Known exchange names use stable alphabetical order. Member indices retain draft order here and
/// are ranked through `CoreOrder` by the renderer, keeping membership and user ordering separate.
///
/// Args:
///     servers: Draft and saved server rows from Connections settings.
///     group: Window group whose saved server rows should be included.
///
/// Returns:
///     Exchange names and their source indices, with the unidentified section first.
pub(super) fn exchange_sections<'a>(
    servers: &'a [ServerRowMeta],
    group: &str,
) -> Vec<(Option<&'a str>, Vec<usize>)> {
    crate::core_order::exchange_sections(
        servers
            .iter()
            .enumerate()
            .filter(|(_, (_, uid, _, server_group, _))| *uid != 0 && server_group == group)
            .map(|(index, (_, _, _, _, exchange))| (index, exchange.as_deref())),
    )
}

impl SettingsView {
    /// Render a group branch header with active state, icon, name, member count, window action,
    /// icon-picker action, and add-core action.
    ///
    /// `ico_el` is prepared by the caller because texture loading needs `&mut self.icons`.
    fn group_header_row(
        &mut self,
        name: &str,
        active: bool,
        ico_el: AnyElement,
        member_count: usize,
        p: MoonPalette,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let nm_act = name.to_string();
        let nm_eye = name.to_string();
        let nm_pick = name.to_string();
        let nm_add = name.to_string();
        h_flex()
            .w_full()
            .gap_1()
            .items_center()
            .px_1()
            .py_0p5()
            .rounded(design::r_button(cx))
            .bg(rgb(p.panel_high))
            .child(
                MoonCheckbox::new(SharedString::from(format!("grp-{name}")))
                    .checked(active)
                    .size(MoonCheckboxSize::Compact)
                    .on_change(cx.listener(move |this, ch: &bool, _w, cx| {
                        let v = *ch;
                        let n = nm_act.clone();
                        this.backend.update(cx, |b, bcx| {
                            if let Some(p) = b.preview.as_mut() {
                                if let Some(gc) = p.groups.iter_mut().find(|g| g.name == n) {
                                    gc.active = v;
                                    bcx.notify();
                                }
                            }
                        });
                        cx.notify();
                    })),
            )
            .child(ico_el)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .font_bold()
                    .child(name.to_string()),
            )
            .child(
                div()
                    .text_size(design::t_body(cx))
                    .text_color(rgb(p.text_soft))
                    .child(t!("conn.member_count", n = member_count).to_string()),
            )
            .child(
                div()
                    .id(SharedString::from(format!("eye-tip-{name}")))
                    .tooltip(|_window, cx| {
                        cx.new(|_| MoonTooltipView::new(t!("conn.show_group").to_string()))
                            .into()
                    })
                    .child(
                        MoonButton::new(SharedString::from(format!("eye-{name}")))
                            .ghost()
                            .size(MoonButtonSize::Micro)
                            .width(34.0)
                            .label("win")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                let n = nm_eye.clone();
                                this.backend.update(cx, |b, bcx| {
                                    b.show_group_request.push(n);
                                    bcx.notify();
                                });
                            }))
                            .render(),
                    ),
            )
            .child(
                MoonButton::new(SharedString::from(format!("pick-{name}")))
                    .outline()
                    .size(MoonButtonSize::Micro)
                    .width(54.0)
                    .label(t!("conn.icon_btn").to_string())
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.picking = Some(nm_pick.clone());
                        cx.notify();
                    }))
                    .render(),
            )
            .child(
                MoonButton::new(SharedString::from(format!("addgrp-{name}")))
                    .outline()
                    .size(MoonButtonSize::Micro)
                    .width(56.0)
                    .label(format!("+ {}", t!("conn.add_core_short")))
                    .on_click(
                        cx.listener(move |this, _, w, cx| this.add_server(nm_add.clone(), w, cx)),
                    )
                    .render(),
            )
    }

    /// Render a compact pending-core or exchange subsection above its editable rows.
    fn subsection_header_row(
        id: SharedString,
        name: String,
        member_count: usize,
        highlighted: bool,
        p: MoonPalette,
        cx: &App,
    ) -> impl IntoElement {
        h_flex()
            .id(id)
            .w_full()
            .gap_2()
            .items_center()
            .pl(px(20.0))
            .pr_1()
            .py_0p5()
            .child(
                div()
                    .w(px(6.0))
                    .h(px(6.0))
                    .rounded_full()
                    .bg(rgb(if highlighted { p.accent } else { p.text_soft })),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .font_bold()
                    .text_size(design::t_body(cx))
                    .text_color(rgb(if highlighted { p.text } else { p.text_soft }))
                    .child(name),
            )
            .child(
                div()
                    .text_size(design::t_body(cx))
                    .text_color(rgb(p.text_soft))
                    .child(t!("conn.member_count", n = member_count).to_string()),
            )
    }

    /// Build the selected group's icon-picker header and scrollable icon grid.
    ///
    /// Clicking an icon assigns it and closes the picker. The two rows are returned separately so
    /// they can be inserted into the tree without a wrapper container.
    fn icon_picker_rows(
        &mut self,
        name: &str,
        pick_ids: &[u32],
        icon_tex: &HashMap<u32, Option<Arc<RenderImage>>>,
        p: MoonPalette,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let mut grid = h_flex().w_full().flex_wrap().gap_1();
        for id in pick_ids.iter().copied() {
            let cell: AnyElement = match icon_tex.get(&id).and_then(|t| t.clone()) {
                Some(arc) => img(arc)
                    .w(design::ui_px(cx, 22.0))
                    .h(design::ui_px(cx, 22.0))
                    .into_any_element(),
                None => continue,
            };
            let nm = name.to_string();
            grid = grid.child(
                div()
                    .id(SharedString::from(format!("ico-{id}")))
                    .p_0p5()
                    .cursor_pointer()
                    .rounded(design::r_button(cx))
                    .hover(move |s| s.bg(rgb(p.panel_high)))
                    .child(cell)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        let n = nm.clone();
                        this.backend.update(cx, |b, bcx| {
                            if let Some(p) = b.preview.as_mut() {
                                if let Some(g) = p.groups.iter_mut().find(|g| g.name == n) {
                                    g.icon = id;
                                    bcx.notify();
                                }
                            }
                        });
                        this.picking = None;
                        cx.notify();
                    })),
            );
        }
        vec![
            h_flex()
                .w_full()
                .items_center()
                .gap_1()
                .pl(px(20.0))
                .child(
                    div()
                        .flex_1()
                        .text_size(design::t_body(cx))
                        .text_color(rgb(p.text_soft))
                        .child(t!("conn.icon_for", name = name).to_string()),
                )
                .child(
                    MoonButton::new("pick-close")
                        .ghost()
                        .size(MoonButtonSize::Micro)
                        .width(24.0)
                        .label("x")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.picking = None;
                            cx.notify();
                        }))
                        .render(),
                )
                .into_any_element(),
            div()
                .id("icon-picker")
                .pl(px(20.0))
                .max_h(px(220.0))
                .overflow_y_scroll()
                .child(grid)
                .into_any_element(),
        ]
    }

    /// Render the market-data source dropdown ported from the egui ComboBox.
    fn market_src_selector(&self, cx: &App) -> impl IntoElement {
        h_flex()
            .gap_2()
            .items_center()
            .child(
                div()
                    .id("market-src-lbl")
                    .font_bold()
                    .child(t!("conn.market_src").to_string())
                    .tooltip(|_window, cx| {
                        cx.new(|_| {
                            MoonTooltipView::new(t!("conn.market_src_tip").to_string())
                                .max_width(420.0)
                        })
                        .into()
                    }),
            )
            .child(
                div().w(px(260.0)).child(
                    MoonSelect::new(&self.mode)
                        .trigger_size(MoonButtonSize::Action)
                        .menu_width(design::font_w(cx, 260.0))
                        .menu_size(MoonMenuSize::Compact),
                ),
            )
    }

    /// Render the global core-order selector used by every core list.
    fn core_sort_selector(&self, cx: &App) -> impl IntoElement {
        h_flex()
            .gap_2()
            .items_center()
            .child(
                div()
                    .id("core-sort-lbl")
                    .font_bold()
                    .child(t!("conn.core_sort").to_string())
                    .tooltip(|_window, cx| {
                        cx.new(|_| {
                            MoonTooltipView::new(t!("conn.core_sort_tip").to_string())
                                .max_width(420.0)
                        })
                        .into()
                    }),
            )
            .child(
                div().w(px(260.0)).child(
                    MoonSelect::new(&self.core_sort)
                        .trigger_size(MoonButtonSize::Action)
                        .menu_width(design::font_w(cx, 260.0))
                        .menu_size(MoonMenuSize::Compact),
                ),
            )
    }

    /// Render the Connections tab with selectors, pending cores, and the group/exchange tree.
    ///
    /// Args:
    ///     cx: Settings context used to read the draft, live status, and active theme.
    ///
    /// Returns:
    ///     The complete Connections tab.
    pub(in crate::settings) fn connections_tab(
        &mut self,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let p = MoonPalette::active(cx);
        // Snapshot live core status for the status dots.
        let status = self.backend.read(cx).session.status_map();
        // Snapshot server row metadata and groups as (name, active, icon).
        // Rank from the draft so a pending sort-mode change is visible before it is applied.
        let (order, servers, mut groups) = {
            let b = self.backend.read(cx);
            let d = b.preview.as_ref().unwrap_or(&b.config);
            let exchange_names = b.session.market_source().core_exchange_names();
            let servers = d
                .servers
                .iter()
                .map(|s| {
                    (
                        s.id,
                        s.uid,
                        s.active,
                        s.group.clone(),
                        exchange_names.get(&s.id).cloned(),
                    )
                })
                .collect::<Vec<_>>();
            let groups = visible_group_rows(&servers, &d.groups);
            (crate::core_order::CoreOrder::new(d), servers, groups)
        };
        // Keep group branches in stable name order.
        groups.sort_by(|a, b| a.0.cmp(&b.0));
        // Preload group icons and, while the picker is open, every picker icon. `texture()` needs
        // `&mut self.icons`, so load before building UI and then read from the map.
        if self
            .picking
            .as_ref()
            .is_some_and(|selected| !groups.iter().any(|group| group.0 == *selected))
        {
            self.picking = None;
        }
        let picking = self.picking.clone();
        let mut icon_tex: HashMap<u32, Option<Arc<RenderImage>>> = HashMap::new();
        for (_, _, icon) in &groups {
            icon_tex
                .entry(*icon)
                .or_insert_with(|| self.icons.texture(*icon));
        }
        let pick_ids: Vec<u32> = if picking.is_some() {
            self.icons.ids.clone()
        } else {
            Vec::new()
        };
        for id in &pick_ids {
            icon_tex
                .entry(*id)
                .or_insert_with(|| self.icons.texture(*id));
        }

        // Build one tree-like list: pending rows come first, then each group and exchange branch.
        // It is not expandable; indentation expresses hierarchy. The column header uses the same
        // left inset as leaves so core columns stay aligned.
        let mut list_col = v_flex()
            .w_full()
            .min_w_0()
            .gap_1()
            .child(Self::hint_label(
                "h-section",
                t!("conn.groups_panel_heading").to_string(),
                t!("conn.groups_panel_tip").to_string().into(),
                p,
            ))
            .child(Self::conn_col_head_row(p, cx));

        // Keep every newly added row directly below the column header until Save assigns its uid.
        // Ranking still goes through CoreOrder, but no persisted group can precede this section.
        let mut pending = pending_server_indices(&servers);
        order.sort_by(&mut pending, |index| servers[*index].0);
        if !pending.is_empty() {
            list_col = list_col.child(Self::subsection_header_row(
                "pending-cores".into(),
                t!("conn.pending_cores").to_string(),
                pending.len(),
                true,
                p,
                cx,
            ));
            for i in pending {
                let (id, _, srv_active, _, _) = &servers[i];
                if let Some(row) = self.conn.get(i) {
                    let st = status.get(id).cloned();
                    list_col = list_col.child(
                        div()
                            .ml(px(8.0))
                            .pl(px(11.0))
                            .border_l_1()
                            .border_color(rgb(p.border))
                            .child(self.server_row(cx, i, row, *id, *srv_active, st)),
                    );
                }
            }
        }

        // Explain the empty state when no server contributes a group.
        if groups.is_empty() {
            list_col = list_col.child(
                div()
                    .text_color(rgb(p.text_soft))
                    .child(t!("conn.no_groups").to_string()),
            );
        }

        for (group_index, (name, active, icon)) in groups.iter().enumerate() {
            let member_count = servers
                .iter()
                .filter(|(_, _, _, group, _)| group == name)
                .count();
            let ico_el: AnyElement = match icon_tex.get(icon).and_then(|t| t.clone()) {
                Some(arc) => img(arc)
                    .w(design::ui_px(cx, 20.0))
                    .h(design::ui_px(cx, 20.0))
                    .into_any_element(),
                None => div()
                    .w(design::ui_px(cx, 20.0))
                    .h(design::ui_px(cx, 20.0))
                    .into_any_element(),
            };
            list_col =
                list_col.child(self.group_header_row(name, *active, ico_el, member_count, p, cx));
            // Render exchange branches with their core leaves. Keep inactive cores at their
            // canonical position; the status dot shows state. Each member index is also the index
            // in `preview.servers` used by row mutations.
            for (exchange_index, (exchange, mut members)) in
                exchange_sections(&servers, name).into_iter().enumerate()
            {
                let identified = exchange.is_some();
                let exchange_name = exchange
                    .map(str::to_string)
                    .unwrap_or_else(|| t!("common.exchange_unknown").to_string());
                list_col = list_col.child(Self::subsection_header_row(
                    SharedString::from(format!("exchange-{group_index}-{exchange_index}")),
                    exchange_name,
                    members.len(),
                    identified,
                    p,
                    cx,
                ));
                order.sort_by(&mut members, |index| servers[*index].0);
                for i in members {
                    let (id, _, srv_active, _, _) = &servers[i];
                    if let Some(row) = self.conn.get(i) {
                        let st = status.get(id).cloned();
                        list_col = list_col.child(
                            div()
                                .ml(px(8.0))
                                .pl(px(11.0))
                                .border_l_1()
                                .border_color(rgb(p.border))
                                .child(self.server_row(cx, i, row, *id, *srv_active, st)),
                        );
                    }
                }
            }
            // Insert the icon picker directly below its selected group.
            if picking.as_deref() == Some(name.as_str()) {
                list_col =
                    list_col.children(self.icon_picker_rows(name, &pick_ids, &icon_tex, p, cx));
            }
        }

        v_flex()
            .w_full()
            .gap_2()
            // Market-data source dropdown ported from the egui ComboBox.
            .child(self.market_src_selector(cx))
            .child(
                h_flex()
                    .w_full()
                    .gap_2()
                    .items_center()
                    // The selected order applies to every core list.
                    .child(div().flex_1().min_w_0().child(self.core_sort_selector(cx)))
                    // New rows remain in the pending section above every persisted group until Save.
                    .child(
                        MoonButton::new("add-srv")
                            .outline()
                            .small()
                            // Keep the localized label semantic; MoonButton owns the scaled inset.
                            .label(format!("+ {}", t!("conn.add_core")))
                            .padding_x(7.0)
                            .on_click(cx.listener(|this, _, w, cx| {
                                this.add_server("default".into(), w, cx)
                            }))
                            .render(),
                    ),
            )
            .child(list_col)
    }
}
