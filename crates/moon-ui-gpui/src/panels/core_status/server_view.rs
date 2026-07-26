//! Expandable server-by-IP rendering for Core Status.
//!
//! `MoonTree` owns expansion, keyboard navigation, and virtual row layout. Its renderer captures
//! only an immutable frame snapshot and a weak view handle because MoonUI stores the closure in
//! the long-lived tree state.

use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use gpui::prelude::FluentBuilder;
use gpui::*;
use moon_ui::{
    MoonBadge, MoonButton, MoonListItem, MoonPalette, MoonTone, MoonTree, MoonTreeItem,
    MoonTreeState, h_flex, v_flex,
};
use rust_i18n::t;

use super::CoreStatusView;
use super::model::{
    CoreStatusRow, ServerConnectivity, ServerKey, ServerStatusGroup, normal_core_boundary,
    normal_server_boundary,
};
use super::presentation::{
    connection_presentation, memory_u16, memory_u64, normal_section_rule, optional_u8, percent,
};

/// Build hierarchical items while preserving MoonTree's existing expanded-id set.
///
/// Args:
///     groups: Current attention-first server snapshots.
///     hidden: Servers whose process children are locally suppressed.
///     expand_first: Whether the first visible server starts expanded.
///
/// Returns:
///     Root server items with one leaf per visible core process.
pub(super) fn tree_items(
    groups: &[ServerStatusGroup],
    hidden: &HashSet<ServerKey>,
    expand_first: bool,
) -> Vec<MoonTreeItem> {
    groups
        .iter()
        .enumerate()
        .map(|(index, group)| {
            let is_hidden = hidden.contains(&group.key);
            let children = if is_hidden {
                Vec::new()
            } else {
                group
                    .cores
                    .iter()
                    .map(|core| {
                        MoonTreeItem::new(format!("core:{}", core.id), core.name.clone())
                            .folder(false)
                    })
                    .collect()
            };
            MoonTreeItem::new(group.key.tree_id(), server_label(group))
                .children(children)
                .folder(!is_hidden && !group.cores.is_empty())
                .expanded(expand_first && index == 0 && !is_hidden)
        })
        .collect()
}

/// Render the server tree or the localized empty state.
///
/// Args:
///     groups: Immutable frame snapshot used by virtual row callbacks.
///     hidden: Immutable visibility snapshot used for row styling.
///     state: MoonTree state that owns expansion and scrolling.
///     cx: Panel context used to create a weak visibility callback.
///
/// Returns:
///     A full-size grouped status element.
pub(super) fn grouped_server_view(
    groups: Rc<Vec<ServerStatusGroup>>,
    hidden: Rc<HashSet<ServerKey>>,
    state: &Entity<MoonTreeState>,
    cx: &Context<CoreStatusView>,
) -> AnyElement {
    if groups.is_empty() {
        let p = MoonPalette::active(cx);
        return div()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .text_color(rgb(p.text_muted))
            .child(t!("core_status.empty").to_string())
            .into_any_element();
    }

    let weak_view = cx.entity().downgrade();
    let normal_server_start = normal_server_boundary(&groups);
    let server_positions = Rc::new(
        groups
            .iter()
            .enumerate()
            .map(|(index, group)| (SharedString::from(group.key.tree_id()), index))
            .collect::<HashMap<_, _>>(),
    );
    let core_positions = Rc::new(
        groups
            .iter()
            .enumerate()
            .flat_map(|(server_index, group)| {
                let normal_core_start = normal_core_boundary(&group.cores);
                group
                    .cores
                    .iter()
                    .enumerate()
                    .map(move |(core_index, core)| {
                        (
                            SharedString::from(format!("core:{}", core.id)),
                            (
                                server_index,
                                core_index,
                                normal_core_start == Some(core_index),
                            ),
                        )
                    })
            })
            .collect::<HashMap<_, _>>(),
    );
    MoonTree::new(state, move |index, entry, selected, _window, app| {
        let p = MoonPalette::active(app);
        if entry.is_root() {
            if let Some(server_index) = server_positions.get(entry.item().id()).copied() {
                if let Some(group) = groups.get(server_index) {
                    return MoonListItem::new(index)
                        .selected(selected)
                        .child(server_row(
                            group,
                            hidden.contains(&group.key),
                            entry.is_expanded(),
                            normal_server_start == Some(server_index),
                            &weak_view,
                            p,
                        ));
                }
            }
        } else if let Some(&(server_index, core_index, normal_section_start)) =
            core_positions.get(entry.item().id())
        {
            if let Some(core) = groups
                .get(server_index)
                .and_then(|group| group.cores.get(core_index))
            {
                return MoonListItem::new(index).selected(selected).child(core_row(
                    core,
                    entry.depth(),
                    normal_section_start,
                    p,
                    app,
                ));
            }
        }
        MoonListItem::new(index)
            .selected(selected)
            .child(entry.item().label().clone())
    })
    .size_full()
    .into_any_element()
}

/// Render one two-line server summary with its visibility action.
///
/// Args:
///     group: Aggregated server snapshot.
///     hidden: Whether process children are locally suppressed.
///     expanded: Current MoonTree expansion state.
///     normal_section_start: Whether this root starts the normal server section.
///     weak_view: Non-owning panel handle for the visibility callback.
///     p: Active Moon palette.
///
/// Returns:
///     A responsive row whose secondary metrics shrink before the endpoint.
fn server_row(
    group: &ServerStatusGroup,
    hidden: bool,
    expanded: bool,
    normal_section_start: bool,
    weak_view: &WeakEntity<CoreStatusView>,
    p: MoonPalette,
) -> impl IntoElement {
    let key = group.key;
    let weak_view = weak_view.clone();
    let marker = if hidden {
        "-"
    } else if expanded {
        "v"
    } else {
        ">"
    };

    v_flex()
        .relative()
        .w_full()
        .min_w_0()
        .overflow_hidden()
        .gap(px(2.0))
        .opacity(if hidden { 0.58 } else { 1.0 })
        .children(normal_section_start.then(|| normal_section_rule(p)))
        .child(
            h_flex()
                .w_full()
                .min_w_0()
                .items_center()
                .gap_2()
                .child(
                    div()
                        .w(px(12.0))
                        .flex_none()
                        .text_color(rgb(p.text_muted))
                        .child(marker),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(rgb(p.text))
                        .child(server_label(group)),
                )
                .child(connectivity_badge(group.connectivity))
                .child(
                    MoonBadge::new(t!(
                        "core_status.ready_count",
                        ready = group.ready_count,
                        total = group.cores.len()
                    ))
                    .tone(MoonTone::Muted),
                )
                .child(
                    // MoonTree toggles a folder on the row's mouse-down. Stop that event at the
                    // action wrapper so changing visibility does not also collapse the server.
                    div()
                        .on_mouse_down(MouseButton::Left, |_, _, app| app.stop_propagation())
                        .child(
                            MoonButton::new(SharedString::from(format!(
                                "core-status-eye-{}",
                                key.tree_id()
                            )))
                            .xsmall()
                            .ghost()
                            .icon(if hidden {
                                "icons/eye-off.svg"
                            } else {
                                "icons/eye.svg"
                            })
                            .tooltip(if hidden {
                                t!("core_status.show_server").to_string()
                            } else {
                                t!("core_status.hide_server").to_string()
                            })
                            .on_click(move |_, _, app| {
                                let Some(view) = weak_view.upgrade() else {
                                    return;
                                };
                                view.update(app, |this, cx| this.toggle_server(key, cx));
                            }),
                        ),
                ),
        )
        .child(
            h_flex()
                .w_full()
                .min_w_0()
                .overflow_hidden()
                .pl(px(20.0))
                .gap_2()
                .text_size(px(10.0))
                .text_color(rgb(p.text_soft))
                .child(
                    div()
                        .flex_shrink_1()
                        .min_w_0()
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .child(server_metrics(group)),
                )
                .children(group.exchanges.iter().map(|(exchange, count)| {
                    MoonBadge::new(if *count > 1 {
                        format!("{exchange} x{count}")
                    } else {
                        exchange.clone()
                    })
                    .tone(MoonTone::Info)
                })),
        )
}

/// Render one core-process child row.
///
/// Args:
///     core: Per-process snapshot.
///     depth: MoonTree nesting depth.
///     normal_section_start: Whether this child starts the Ready section of a mixed server.
///     p: Active Moon palette.
///     cx: Application context used to scale the shared status dot.
///
/// Returns:
///     A two-line row matching server-root height for MoonTree's uniform virtualization.
fn core_row(
    core: &CoreStatusRow,
    depth: usize,
    normal_section_start: bool,
    p: MoonPalette,
    cx: &App,
) -> impl IntoElement {
    let status = connection_presentation(&core.status, p);
    v_flex()
        .relative()
        .w_full()
        .min_w_0()
        .overflow_hidden()
        .gap(px(2.0))
        .pl(px(12.0 * depth as f32))
        .children(normal_section_start.then(|| normal_section_rule(p)))
        .child(
            h_flex()
                .w_full()
                .min_w_0()
                .gap_2()
                .items_center()
                .child(crate::design::status_dot(status.color, cx))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_color(rgb(p.text_soft))
                        .child(core.name.clone()),
                )
                .when_some(core.exchange.clone(), |row, exchange| {
                    row.child(MoonBadge::new(exchange).tone(MoonTone::Info))
                })
                .child(MoonBadge::new(status.label).tone(status.tone)),
        )
        .child(
            h_flex()
                .w_full()
                .min_w_0()
                .overflow_hidden()
                .pl(px(18.0))
                .whitespace_nowrap()
                .text_size(px(10.0))
                .text_color(rgb(p.text_muted))
                .child(format!(
                    "{}  {}",
                    t!(
                        "core_status.process_cpu",
                        value = percent(core.sys.process_cpu_percent)
                    ),
                    t!(
                        "core_status.process_memory",
                        value = memory_u16(core.sys.used_memory_mb)
                    )
                )),
        )
}

/// Build the localized endpoint label.
///
/// Args:
///     group: Server snapshot whose address is displayed.
///
/// Returns:
///     Address text, or a core-qualified unknown label.
fn server_label(group: &ServerStatusGroup) -> String {
    group
        .address
        .map(|address| address.to_string())
        .unwrap_or_else(|| {
            let core = group
                .cores
                .first()
                .map(|core| core.name.as_str())
                .unwrap_or("-");
            t!("core_status.unknown_server", core = core).to_string()
        })
}

/// Build the compact localized machine/process metric line.
///
/// Args:
///     group: Aggregated server snapshot.
///
/// Returns:
///     One text line ordered by operational priority.
fn server_metrics(group: &ServerStatusGroup) -> String {
    format!(
        "{}  {}  {}  {}",
        t!(
            "core_status.system_cpu",
            value = percent(group.system_cpu_percent)
        ),
        t!(
            "core_status.process_memory_total",
            value = memory_u64(group.process_memory_mb)
        ),
        t!(
            "core_status.free_memory",
            value = memory_u16(group.free_physical_memory_mb)
        ),
        t!(
            "core_status.logical_cpus",
            value = optional_u8(group.logical_cpu_count)
        )
    )
}

/// Render the localized aggregate connectivity badge.
///
/// Args:
///     connectivity: Derived server connection state.
///
/// Returns:
///     A color-coded MoonBadge.
fn connectivity_badge(connectivity: ServerConnectivity) -> MoonBadge {
    match connectivity {
        ServerConnectivity::Online => {
            MoonBadge::new(t!("core_status.online").to_string()).tone(MoonTone::Positive)
        }
        ServerConnectivity::Degraded => {
            MoonBadge::new(t!("core_status.degraded").to_string()).tone(MoonTone::Warning)
        }
        ServerConnectivity::Offline => {
            MoonBadge::new(t!("core_status.offline").to_string()).tone(MoonTone::Danger)
        }
    }
}

#[cfg(test)]
mod tests;
