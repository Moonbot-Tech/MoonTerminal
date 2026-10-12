//! Core-tree pieces the Cores page and the Storage core-data tree share: group and server row
//! metadata, the group header parts, the group transport writer, and the group icon picker.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use gpui::*;
use moon_ui::{MoonButton, MoonPalette, StyledExt, h_flex, v_flex};
use rust_i18n::t;

use super::SettingsView;
use super::columns::CONN_TABLE_INSET;
use crate::design;
use moon_core::config::{GroupConfig, ServerConfig, TransportVersion};
use moon_core::session::CoreId;
use moon_core::venue::CoreVenue;

/// Draft server metadata needed to assemble the Connections hierarchy.
pub(in crate::settings) type ServerRowMeta = (CoreId, u64, bool, String, Option<CoreVenue>);

/// Visible group metadata needed to render one Connections branch.
pub(in crate::settings) type GroupRowMeta = (String, bool, u32);

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

/// The visible groups in the order every core tree draws them: by name.
///
/// ONE place for the order, because two tabs draw this tree — Connections and Settings →
/// Storage's "Data by core" — and a sort re-typed at each call site would let them drift apart.
pub(in crate::settings) fn sorted_group_rows(
    servers: &[ServerRowMeta],
    groups: &[GroupConfig],
) -> Vec<GroupRowMeta> {
    let mut rows = visible_group_rows(servers, groups);
    rows.sort_by(|a, b| a.0.cmp(&b.0));
    rows
}

/// The pill a window-group branch sits on, shared by both core trees.
///
/// The right inset clears the overlay scrollbar of a virtual list: a row there ending in live
/// controls would otherwise have their right edge painted over and its clicks swallowed.
pub(in crate::settings) fn group_pill(p: MoonPalette, cx: &App) -> Div {
    h_flex()
        .w_full()
        .gap_1()
        .items_center()
        .px_1()
        .pr(design::ui_px(cx, design::MOON_SCROLLBAR_OVERLAY_W))
        .py_0p5()
        .rounded(design::r_button(cx))
        .bg(rgb(p.panel_high))
}

/// A group's icon, or an empty box of the same size while it has none.
pub(in crate::settings) fn group_icon(tex: Option<Arc<RenderImage>>, cx: &App) -> AnyElement {
    let side = design::ui_px(cx, 20.0);
    match tex {
        Some(arc) => img(arc).w(side).h(side).into_any_element(),
        None => div().w(side).h(side).into_any_element(),
    }
}

/// A group's name on its pill.
///
/// It SHRINKS but never GROWS: `flex_1` here handed it every spare pixel of a 1791px row, which
/// pushed the count away from the name it counts and left the controls scattered across the gap
/// instead of reading as one cluster. `min_w_0` keeps the truncation, so a long name still
/// ellipsises rather than pushing the cluster off-row.
pub(in crate::settings) fn group_name(name: &str) -> Div {
    div()
        .min_w_0()
        .truncate()
        .font_bold()
        .child(name.to_string())
}

/// "N cores" beside a group's name.
pub(in crate::settings) fn group_count(member_count: usize, p: MoonPalette, cx: &App) -> Div {
    div()
        .flex_shrink_0()
        .text_size(design::t_body(cx))
        .text_color(rgb(p.text_soft))
        .child(super::super::common::count_label(
            "conn.member_count",
            member_count as u64,
        ))
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
pub(in crate::settings) fn subsection_header_row(
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
        .pl(px(CONN_TABLE_INSET))
        // A row of the same virtual list as the core rows, so it clears the overlay scrollbar the
        // same way; `pr_1` alone left the member count under the track once the list overflowed.
        .pr(design::ui_px(cx, design::MOON_SCROLLBAR_OVERLAY_W))
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
                .child(super::super::common::count_label(
                    "conn.member_count",
                    member_count as u64,
                )),
        )
}

/// Set every draft server row in one group to a single transport mode.
///
/// Pure and GPUI-free so it is directly testable (`moon-ui-gpui` is a binary crate with no
/// `[lib]`): the bulk dropdown below is a thin GPUI shell around this one loop.
///
/// Args:
///     servers: Every draft server row.
///     group: The target group's name; only rows whose `group` matches are touched.
///     mode: The mode to set on every matching row.
///
/// Returns:
///     Whether at least one row's mode actually changed.
pub(super) fn apply_group_transport(
    servers: &mut [ServerConfig],
    group: &str,
    mode: TransportVersion,
) -> bool {
    let mut changed = false;
    for s in servers.iter_mut().filter(|s| s.group == group) {
        if s.transport != Some(mode) {
            s.transport = Some(mode);
            changed = true;
        }
    }
    changed
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
pub(super) fn icon_picker_grid(
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
                            if let Some(p) = b.preview.as_mut()
                                && let Some(g) = p.groups.iter_mut().find(|g| g.name == n)
                            {
                                g.icon = id;
                                bcx.notify();
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

impl SettingsView {}
