//! Connections tab assembly: pending-core, group, and exchange branch headers; icon picker;
//! market-data and core-order selectors; and the virtualized editable hierarchy with its top add
//! button.

use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::rc::Rc;
use std::sync::Arc;

use gpui::*;
use moon_ui::{
    MoonButton, MoonButtonSize, MoonCheckbox, MoonCheckboxSize, MoonMenuSize, MoonPalette,
    MoonPopover, MoonPopoverPlacement, MoonScrollbarVisibility, MoonSelect, MoonTooltipView,
    MoonVirtualList, StyledExt, h_flex, v_flex,
};
use rust_i18n::t;

use super::SettingsView;
use super::entries::{ConnEntry, EntryLabels, flatten_entries};
use super::table::{conn_row_h_value, server_row};
use crate::design;
use moon_core::config::GroupConfig;
use moon_core::session::CoreId;
use moon_core::venue::CoreVenue;

/// Draft server metadata needed to assemble the Connections hierarchy.
pub(super) type ServerRowMeta = (CoreId, u64, bool, String, Option<CoreVenue>);

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

/// Count how many draft rows name each group, in one pass over the servers.
///
/// The branch header prints this beside the group name. Counting it inside the group loop instead
/// rescans every server once per group, which is quadratic in the core count and is paid on every
/// frame the tab is rebuilt -- and a wheel notch rebuilds it.
///
/// Pending rows (uid 0) participate, matching the header the user sees: a newly added, unsaved
/// row is still a member of the group whose name is typed in it.
///
/// Args:
///     servers: Every current draft server row.
///
/// Returns:
///     A count per referenced group name; a group no row names is absent, which reads as zero.
pub(super) fn member_counts(servers: &[ServerRowMeta]) -> HashMap<&str, usize> {
    let mut counts: HashMap<&str, usize> = HashMap::new();
    for (_, _, _, group, _) in servers {
        *counts.entry(group.as_str()).or_insert(0) += 1;
    }
    counts
}

/// Return draft indices for unsaved cores that must stay in the top pending section.
pub(super) fn pending_server_indices(servers: &[ServerRowMeta]) -> Vec<usize> {
    servers
        .iter()
        .enumerate()
        .filter_map(|(index, (_, uid, _, _, _))| (*uid == 0).then_some(index))
        .collect()
}

/// Render a compact pending-core or exchange subsection above its editable rows.
///
/// Args:
///     id: Stable element identity for the heading.
///     name: Localized subsection caption.
///     member_count: Number of core rows in the subsection.
///     highlighted: Whether to use the active heading colours.
///     p: Active palette.
///     cx: Application context.
///
/// Returns:
///     A compact heading row with an indicator, caption, and member count.
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

/// Render a group branch header with active state, icon, name, member count, window action,
/// icon-picker popover, and add-core action.
///
/// A free function taking a WEAK handle: it is built inside the virtual list's row factory, which
/// only ever hands out `&mut App` -- see `table.rs`'s module doc for why a strong handle here would
/// leak the window.
///
/// Args:
///     weak: Weak callback owner every listener below closes over.
///     name: The group's name, also its identity for the icon picker.
///     active: Whether the group is enabled.
///     ico_el: The group's icon, already resolved from the preloaded texture cache.
///     member_count: How many draft rows name this group.
///     pick_ids: Every pickable icon id, non-empty only while this group's picker is open.
///     icon_tex: Preloaded icon textures, keyed by id.
///     picking_open: Whether this group's icon picker is the one currently open.
///     p: Active palette.
///     cx: Application context.
///
/// Returns:
///     A group heading row with its controls and, when open, its icon-picker popover.
#[allow(clippy::too_many_arguments)]
fn group_header_row(
    weak: &WeakEntity<SettingsView>,
    name: &str,
    active: bool,
    ico_el: AnyElement,
    member_count: usize,
    pick_ids: &[u32],
    icon_tex: &HashMap<u32, Option<Arc<RenderImage>>>,
    picking_open: bool,
    p: MoonPalette,
    cx: &App,
) -> impl IntoElement {
    let nm_act = name.to_string();
    let nm_eye = name.to_string();
    let nm_add = name.to_string();

    let popover_body =
        picking_open.then(|| icon_picker_grid(weak, name, pick_ids, icon_tex, p, cx));
    let mut popover = MoonPopover::new(SharedString::from(format!("pick-pop-{name}")))
        .placement(MoonPopoverPlacement::BottomStart)
        .content_width_ui(240.0)
        .close_on_content_click(false)
        .overlay_closable(true)
        .open(picking_open)
        .on_open_change({
            let weak = weak.clone();
            let name = name.to_string();
            move |open, _window, app| {
                let _ = weak.update(app, |this, cx| {
                    this.picking = open.then(|| name.clone());
                    cx.notify();
                });
            }
        })
        .trigger(
            MoonButton::new(SharedString::from(format!("pick-{name}")))
                .outline()
                .size(MoonButtonSize::Micro)
                .width(54.0)
                .label(t!("conn.icon_btn").to_string())
                .render(),
        );
    if let Some(body) = popover_body {
        popover = popover.content(body);
    }

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
                .on_change({
                    let weak = weak.clone();
                    move |ch: &bool, _window, cx| {
                        let v = *ch;
                        let n = nm_act.clone();
                        let _ = weak.update(cx, |this, ctx| {
                            this.backend.update(ctx, |b, bcx| {
                                if let Some(p) = b.preview.as_mut() {
                                    if let Some(gc) = p.groups.iter_mut().find(|g| g.name == n) {
                                        gc.active = v;
                                        bcx.notify();
                                    }
                                }
                            });
                            ctx.notify();
                        });
                    }
                }),
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
                        .on_click({
                            let weak = weak.clone();
                            move |_, _, cx| {
                                let n = nm_eye.clone();
                                let _ = weak.update(cx, |this, ctx| {
                                    this.backend.update(ctx, |b, bcx| {
                                        b.show_group_request.push(n);
                                        bcx.notify();
                                    });
                                });
                            }
                        })
                        .render(),
                ),
        )
        .child(popover)
        .child(
            MoonButton::new(SharedString::from(format!("addgrp-{name}")))
                .outline()
                .size(MoonButtonSize::Micro)
                .width(56.0)
                .label(format!("+ {}", t!("conn.add_core_short")))
                .on_click({
                    let weak = weak.clone();
                    move |_, window, cx| {
                        let n = nm_add.clone();
                        let _ = weak.update(cx, |this, ctx| this.add_server(n, window, ctx));
                    }
                })
                .render(),
        )
}

/// Build one group's icon-picker grid, as the content of its anchored [`MoonPopover`].
///
/// Built only while its group's picker is open (the caller gates this): a 220px scrolling grid of
/// every icon is real per-frame cost that a closed picker must not pay, the same mistake the feed
/// dropdown's items made before Phase A.
///
/// Args:
///     weak: Weak callback owner the icon click closures close over.
///     name: The owning group's name, the click target for an icon assignment.
///     pick_ids: Every pickable icon id.
///     icon_tex: Preloaded icon textures, keyed by id.
///     p: Active palette.
///     cx: Application context.
///
/// Returns:
///     The popover body containing the icon selector and its close control.
fn icon_picker_grid(
    weak: &WeakEntity<SettingsView>,
    name: &str,
    pick_ids: &[u32],
    icon_tex: &HashMap<u32, Option<Arc<RenderImage>>>,
    p: MoonPalette,
    cx: &App,
) -> AnyElement {
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
        let weak_ico = weak.clone();
        grid = grid.child(
            div()
                .id(SharedString::from(format!("ico-{id}")))
                .p_0p5()
                .cursor_pointer()
                .rounded(design::r_button(cx))
                .hover(move |s| s.bg(rgb(p.panel_high)))
                .child(cell)
                .on_click(move |_, _, cx| {
                    let n = nm.clone();
                    let _ = weak_ico.update(cx, |this, ctx| {
                        this.backend.update(ctx, |b, bcx| {
                            if let Some(p) = b.preview.as_mut() {
                                if let Some(g) = p.groups.iter_mut().find(|g| g.name == n) {
                                    g.icon = id;
                                    bcx.notify();
                                }
                            }
                        });
                        this.picking = None;
                        ctx.notify();
                    });
                }),
        );
    }
    let weak_close = weak.clone();
    v_flex()
        .id("icon-picker")
        .gap_1()
        .child(
            h_flex()
                .w_full()
                .items_center()
                .gap_1()
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
                        .on_click(move |_, _, cx| {
                            let _ = weak_close.update(cx, |this, ctx| {
                                this.picking = None;
                                ctx.notify();
                            });
                        })
                        .render(),
                ),
        )
        .child(
            div()
                .id("icon-picker-grid")
                .max_h(px(220.0))
                .overflow_y_scroll()
                .child(grid),
        )
        .into_any_element()
}

impl SettingsView {
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

    /// Render the Connections tab with selectors, pending cores, and the virtualized group/exchange
    /// tree.
    ///
    /// Args:
    ///     cx: Settings context used to read the draft, live status, and active theme.
    ///
    /// Returns:
    ///     The complete Connections tab: fixed selectors and headers above a virtualized core list
    ///     that owns the tab's only scroll (`settings/render.rs` gives Connections a non-scrolling
    ///     bounded body for exactly this reason).
    pub(in crate::settings) fn connections_tab(
        &mut self,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        crate::diag::bump(&crate::diag::SETTINGS_CONN_TAB_BUILD);
        let p = MoonPalette::active(cx);
        // Snapshot live core status for the status dots. The dot's COLOUR is the only thing every
        // row needs on every frame, so this one map is the only one built here. The tooltip's
        // VERDICT needs the retained fault and the startup snapshot too, but only for the single
        // dot under the pointer -- so it reads those from the live store at hover time instead.
        let status = self.backend.read(cx).session.status_map();
        // Snapshot server row metadata and groups as (name, active, icon).
        // Rank from the draft so a pending sort-mode change is visible before it is applied.
        let (order, servers, mut groups) = {
            let b = self.backend.read(cx);
            let d = b.preview.as_ref().unwrap_or(&b.config);
            let venues = b.session.core_venues();
            let servers = d
                .servers
                .iter()
                .map(|s| {
                    (
                        s.id,
                        s.uid,
                        s.active,
                        s.group.clone(),
                        venues.get(&s.id).cloned(),
                    )
                })
                .collect::<Vec<_>>();
            let groups = visible_group_rows(&servers, &d.groups);
            (crate::core_order::CoreOrder::new(d), servers, groups)
        };
        // Keep group branches in stable name order.
        groups.sort_by(|a, b| a.0.cmp(&b.0));
        // Preload group icons and, while the picker is open, every picker icon. `texture()` needs
        // `&mut self.icons`, so load before building UI and then read from the map -- the row
        // factory below only ever gets `&App` and could not call it itself.
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
        let icon_tex = Rc::new(icon_tex);
        let pick_ids = Rc::new(pick_ids);

        // Flatten pending rows and every group's exchange sections into the sequence the virtual
        // list draws. Pure and GPUI-free (`entries.rs`); localized captions are resolved here,
        // where `t!` is available, and threaded in.
        let pending_caption = t!("conn.pending_cores").to_string();
        let labels = EntryLabels {
            pending: &pending_caption,
            exchange: &|venue| crate::controls::venue_section_label(venue),
        };
        let entries = Rc::new(flatten_entries(&servers, &groups, &order, labels));
        self.conn_entries = entries.clone();
        // A row the user is EDITING must stay on screen: a keystroke can re-rank the list and carry
        // that row out of the mounted range, where the eviction policy would blur it and the field
        // would accept one character and refuse the rest. Scrolling here, before the list lays out,
        // means the range the eviction handler later sees already contains the row.
        self.follow_edited_conn_row();

        let row_count = entries.len();
        let row_h = conn_row_h_value(cx);

        let list: AnyElement = if row_count == 0 {
            // No servers at all: nothing to virtualize, and no group or pending heading to draw.
            div()
                .text_color(rgb(p.text_soft))
                .child(t!("conn.no_groups").to_string())
                .into_any_element()
        } else {
            let factory_weak = cx.entity().downgrade();
            let factory_entries = entries;
            let factory_icon_tex = icon_tex;
            let factory_pick_ids = pick_ids;
            let factory_picking = picking;
            let factory_status = status;
            // WEAK, and deliberately NOT `cx.processor`: that helper captures `self.entity()` --
            // a STRONG handle (`moon-gpui/src/app/context.rs:268`) -- and `MoonVirtualList` stores
            // this callback inside the rendered element. A strong handle there closes the same
            // `SettingsView -> element -> closure -> SettingsView` cycle the row factory above is
            // written weakly to avoid, and the cost is not merely a leaked allocation: the window
            // never drops, so `on_release` never runs, the unsaved draft is never discarded, and
            // the previewed theme is never restored.
            let range_weak = cx.entity().downgrade();
            let on_visible_range =
                move |range: Range<usize>, window: &mut Window, app: &mut App| {
                    let _ = range_weak.update(app, |this, cx| {
                        this.on_conn_visible_range(range, window, cx);
                    });
                };
            MoonVirtualList::new(
                "conn-core-list",
                row_count,
                row_h,
                move |ix, _window, app| {
                    let Some(entry) = factory_entries.get(ix) else {
                        return div().into_any_element();
                    };
                    match entry {
                        ConnEntry::PendingHeader {
                            caption,
                            member_count,
                        } => subsection_header_row(
                            "pending-cores".into(),
                            caption.clone(),
                            *member_count,
                            true,
                            p,
                            app,
                        )
                        .into_any_element(),
                        ConnEntry::GroupHeader {
                            name,
                            active,
                            icon,
                            member_count,
                        } => {
                            let ico_el: AnyElement =
                                match factory_icon_tex.get(icon).and_then(|t| t.clone()) {
                                    Some(arc) => img(arc)
                                        .w(design::ui_px(app, 20.0))
                                        .h(design::ui_px(app, 20.0))
                                        .into_any_element(),
                                    None => div()
                                        .w(design::ui_px(app, 20.0))
                                        .h(design::ui_px(app, 20.0))
                                        .into_any_element(),
                                };
                            let open = factory_picking.as_deref() == Some(name.as_str());
                            group_header_row(
                                &factory_weak,
                                name,
                                *active,
                                ico_el,
                                *member_count,
                                &factory_pick_ids,
                                &factory_icon_tex,
                                open,
                                p,
                                app,
                            )
                            .into_any_element()
                        }
                        ConnEntry::ExchangeHeader {
                            group_index,
                            exchange_index,
                            caption,
                            member_count,
                            identified,
                        } => subsection_header_row(
                            SharedString::from(format!("exchange-{group_index}-{exchange_index}")),
                            caption.clone(),
                            *member_count,
                            *identified,
                            p,
                            app,
                        )
                        .into_any_element(),
                        ConnEntry::CoreRow {
                            draft_index,
                            core_id,
                            active,
                            indented,
                            ..
                        } => {
                            let Some(view) = factory_weak.upgrade() else {
                                return div().into_any_element();
                            };
                            let view_ref = view.read(app);
                            let Some(row) = view_ref.conn.get(*draft_index) else {
                                return div().into_any_element();
                            };
                            let st = factory_status.get(core_id).cloned();
                            let built = server_row(
                                &view_ref,
                                &factory_weak,
                                row,
                                *draft_index,
                                *core_id,
                                *active,
                                st,
                                app,
                            );
                            if *indented {
                                div()
                                    .ml(px(8.0))
                                    .pl(px(11.0))
                                    .border_l_1()
                                    .border_color(rgb(p.border))
                                    .child(built)
                                    .into_any_element()
                            } else {
                                built
                            }
                        }
                    }
                },
            )
            .track_scroll(&self.conn_scroll)
            .on_visible_range(on_visible_range)
            .surface(false)
            .border(false)
            .radius(0.0)
            .scrollbar_visibility(MoonScrollbarVisibility::Always)
            .into_any_element()
        };

        v_flex()
            .flex_1()
            .min_h(px(0.0))
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
                        // The first-run hint starts HERE and only while the draft has no rows at
                        // all: with nothing configured there is no key field on screen yet, so the
                        // control that creates one is the only honest thing to point at. Adding a
                        // row hands the ring to that field (`add_server` re-arms).
                        div()
                            .relative()
                            .child(
                                MoonButton::new("add-srv")
                                    .outline()
                                    .small()
                                    // Keep the localized label semantic; MoonButton owns the
                                    // scaled inset.
                                    .label(format!("+ {}", t!("conn.add_core")))
                                    .padding_x(7.0)
                                    .on_click(cx.listener(|this, _, w, cx| {
                                        this.add_server("default".into(), w, cx)
                                    }))
                                    .render(),
                            )
                            .children(
                                self.conn_hint(cx)
                                    .filter(|_| servers.is_empty())
                                    .and_then(|at| crate::pulse::attention_ring(p.accent, at)),
                            ),
                    ),
            )
            .child(Self::hint_label(
                "h-section",
                t!("conn.groups_panel_heading").to_string(),
                t!("conn.groups_panel_tip").to_string().into(),
                p,
            ))
            .child(Self::conn_col_head_row(p, cx))
            .child(div().flex_1().min_h(px(0.0)).w_full().child(list))
    }
}
