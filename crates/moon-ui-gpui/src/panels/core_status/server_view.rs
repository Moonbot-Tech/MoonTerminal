//! By-IP rendering for Core Status: a one-line server summary that expands to one row per core.
//!
//! Servers start collapsed. `MoonTree` owns expansion, keyboard navigation, and virtual row
//! layout; the renderer distinguishes a server header (root) from a core row (child) by item id.

use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use gpui::prelude::FluentBuilder;
use gpui::*;
use moon_ui::{
    MoonButton, MoonInput, MoonInputState, MoonListItem, MoonPalette, MoonTree, MoonTreeItem,
    MoonTreeState, h_flex,
};
use rust_i18n::t;

use moon_core::feed::ConnStatus;
use moon_core::session::CoreId;

use super::CoreStatusView;
use super::model::{CoreStatusRow, ServerConnectivity, ServerKey, ServerStatusGroup};
use super::presentation::{
    LoadLevel, cpu_level, cpu_load, free_mem_level, level_color, memory_free, memory_u16,
    metric_icon, order_level, percent, ping, ping_level,
};

/// IP mask shown until the eye reveals the address; a fixed run avoids leaking the address length.
const IP_MASK: &str = "************";
/// Fixed value-box widths so a changing number never reflows the row.
const CPU_W: f32 = 100.0;
const MEM_W: f32 = 116.0;
const PING_W: f32 = 64.0;

/// Build server roots, each with one collapsed folder of core children.
///
/// Args:
///     groups: Current attention-first server snapshots.
///
/// Returns:
///     Server roots that start collapsed; expansion is restored separately by the panel.
pub(super) fn tree_items(groups: &[ServerStatusGroup]) -> Vec<MoonTreeItem> {
    groups
        .iter()
        .map(|group| {
            let children: Vec<MoonTreeItem> = group
                .cores
                .iter()
                .map(|core| {
                    MoonTreeItem::new(format!("core:{}", core.id), core.name.clone()).folder(false)
                })
                .collect();
            MoonTreeItem::new(group.key.tree_id(), group.display_name.clone())
                .children(children)
                .folder(!group.cores.is_empty())
                .expanded(false)
        })
        .collect()
}

/// Render the flat server/core list or the localized empty state.
///
/// Args:
///     groups: Immutable frame snapshot used by virtual row callbacks.
///     revealed: Servers whose IP is momentarily shown by the eye control.
///     editing: The server whose name is being renamed inline, if any.
///     edit_input: Shared input state backing the inline rename field.
///     chart_selected: The server highlighted by a body click (the chart target), if any.
///     chart_core: The core highlighted by a core-row click (charts that core), if any.
///     state: MoonTree state that owns scrolling and selection.
///     cx: Panel context used to create a weak action callback.
///
/// Returns:
///     A full-size list element.
#[allow(clippy::too_many_arguments)]
pub(super) fn grouped_server_view(
    groups: Rc<Vec<ServerStatusGroup>>,
    revealed: Rc<HashSet<ServerKey>>,
    editing: Option<ServerKey>,
    edit_input: Option<Entity<MoonInputState>>,
    chart_selected: Option<ServerKey>,
    chart_core: Option<CoreId>,
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
                group
                    .cores
                    .iter()
                    .enumerate()
                    .map(move |(core_index, core)| {
                        (
                            SharedString::from(format!("core:{}", core.id)),
                            (server_index, core_index),
                        )
                    })
            })
            .collect::<HashMap<_, _>>(),
    );
    // Headless tree: it installs no row click/expand handlers and does not override `.selected`, so
    // the panel drives selection (body click) and expansion (chevron click) itself.
    MoonTree::custom(state, move |entry, meta, _window, app| {
        let p = MoonPalette::active(app);
        if entry.is_root() {
            if let Some(server_index) = server_positions.get(entry.item().id()).copied() {
                if let Some(group) = groups.get(server_index) {
                    let editing_input = (editing == Some(group.key))
                        .then(|| edit_input.clone())
                        .flatten();
                    // Highlight follows the body-click selection (chart target).
                    return MoonListItem::new(meta.index)
                        .selected(chart_selected == Some(group.key))
                        .child(server_row(
                            group,
                            revealed.contains(&group.key),
                            entry.is_expanded(),
                            editing_input,
                            &weak_view,
                            p,
                            app,
                        ));
                }
            }
        } else if let Some(&(server_index, core_index)) = core_positions.get(entry.item().id()) {
            if let Some(core) = groups
                .get(server_index)
                .and_then(|group| group.cores.get(core_index))
            {
                // A core row highlights when it is the charted core, and clicking it charts that core.
                return MoonListItem::new(meta.index)
                    .selected(chart_core == Some(core.id))
                    .child(core_row(core, &weak_view, p, app));
            }
        }
        MoonListItem::new(meta.index)
            .selected(false)
            .child(entry.item().label().clone())
    })
    .size_full()
    .into_any_element()
}

/// Render one server summary header (name, masked IP, machine CPU/memory, status).
///
/// Args:
///     group: Aggregated server snapshot.
///     revealed: Whether the IP is currently shown.
///     edit_input: Present only while this server's name is being renamed inline.
///     weak_view: Non-owning panel handle for the row actions.
///     p: Active Moon palette.
///     app: Application context used to scale the status dot.
///
/// Returns:
///     A single header row; the name stays fixed while a spacer pushes metrics right.
fn server_row(
    group: &ServerStatusGroup,
    revealed: bool,
    expanded: bool,
    edit_input: Option<Entity<MoonInputState>>,
    weak_view: &WeakEntity<CoreStatusView>,
    p: MoonPalette,
    app: &App,
) -> impl IntoElement {
    let has_ip = group.address.is_some();
    // Chevron: down when expanded, right when collapsed.
    let marker = if expanded { "\u{25BE}" } else { "\u{25B8}" };
    let dot_color = match group.connectivity {
        ServerConnectivity::Online => p.green,
        ServerConnectivity::Degraded => p.amber,
        ServerConnectivity::Offline => p.red,
    };
    let ip_text = if revealed {
        group
            .address
            .map(|address| address.to_string())
            .unwrap_or_default()
    } else {
        IP_MASK.to_string()
    };

    h_flex()
        .w_full()
        .min_w_0()
        .items_center()
        .gap_2()
        .overflow_hidden()
        // The chevron is the ONLY expand trigger and toggles expansion directly (headless tree).
        .child({
            let weak_view = weak_view.clone();
            let key = group.key;
            div()
                .w(px(12.0))
                .flex_none()
                .cursor_pointer()
                .text_color(rgb(p.text_muted))
                .on_mouse_down(MouseButton::Left, move |_, _, app| {
                    if let Some(view) = weak_view.upgrade() {
                        view.update(app, |this, cx| this.toggle_server_expand(key, cx));
                    }
                })
                .child(marker)
        })
        // Clicking the body selects this server for the chart AND blocks the row's expand toggle
        // (only the chevron expands). Eye/pencil stop the event earlier, so they keep their own act.
        .child(
            h_flex()
                .flex_1()
                .min_w_0()
                .items_center()
                .gap_2()
                .overflow_hidden()
                .on_mouse_down(MouseButton::Left, {
                    let weak_view = weak_view.clone();
                    let key = group.key;
                    move |_, _, app| {
                        app.stop_propagation();
                        if let Some(view) = weak_view.upgrade() {
                            view.update(app, |this, cx| this.select_chart_server(key, cx));
                        }
                    }
                })
                .child(server_identity(group, edit_input, weak_view, p))
                .when(has_ip, |row| {
                    row.child(
                        div()
                            .flex_none()
                            .text_color(rgb(if revealed { p.text_soft } else { p.text_muted }))
                            .child(ip_text),
                    )
                    .child(eye_action(group.key, revealed, weak_view))
                })
                .child(div().flex_1())
                .child(metric_cell(
                    "icons/cpu.svg",
                    cpu_load(group.system_cpu_percent, group.logical_cpu_count),
                    CPU_W,
                    level_color(cpu_level(group.system_cpu_percent), p),
                    group.cpu_warn,
                    p,
                ))
                .child(metric_cell(
                    "icons/memory-stick.svg",
                    memory_free(group.process_memory_mb, group.free_physical_memory_mb),
                    MEM_W,
                    level_color(
                        free_mem_level(group.process_memory_mb, group.free_physical_memory_mb),
                        p,
                    ),
                    group.mem_warn,
                    p,
                ))
                // Ping is per core; the server row shows the core with the WORST level among the
                // server's READY cores (each judged against its own baseline, so a core whose high
                // ping is normal does not dominate) and paints that core's value. Two cells:
                // client↔core, then core→exchange.
                .child({
                    let (value, level) = worst_by_level(&group.cores, |c| {
                        c.sys
                            .round_trip_ms
                            .map(|v| (v, ping_level(Some(v), c.ping_base)))
                    });
                    metric_cell(
                        "icons/globe.svg",
                        ping(value),
                        PING_W,
                        level_color(level, p),
                        group.ping_warn,
                        p,
                    )
                })
                .child({
                    let (value, level) = worst_by_level(&group.cores, |c| {
                        c.sys
                            .order_api_latency_ms
                            .map(|v| (u32::from(v), order_level(Some(v), c.exch_base)))
                    });
                    metric_cell(
                        "icons/external-link.svg",
                        ping(value),
                        PING_W,
                        level_color(level, p),
                        group.exch_warn,
                        p,
                    )
                })
                .child(
                    div()
                        .flex_none()
                        .text_color(rgb(p.text_soft))
                        .child(format!("{}/{}", group.ready_count, group.cores.len())),
                )
                // A dropped core while others still run — the connectivity warning, next to the dot.
                .children(group.conn_warn.then(|| {
                    svg()
                        .path("icons/triangle-alert.svg")
                        .size(px(12.0))
                        .flex_none()
                        .text_color(rgb(p.amber))
                }))
                .child(crate::design::status_dot(dot_color, app)),
        )
}

/// Render one core row: its name (status-toned plain text), then its process CPU, memory, and pings.
///
/// Clicking the row charts this core in the detached window (in place of the server aggregate).
///
/// Args:
///     core: Per-process snapshot.
///     weak_view: Non-owning panel handle for the chart-this-core click.
///     p: Active Moon palette.
///     _app: Application context (reserved for symmetry with the server row).
///
/// Returns:
///     An indented core row whose metrics align under the server metrics.
fn core_row(
    core: &CoreStatusRow,
    weak_view: &WeakEntity<CoreStatusView>,
    p: MoonPalette,
    _app: &App,
) -> impl IntoElement {
    // The core name reads as plain text (not a badge pill); its colour still conveys the connection
    // state, so an offline or failed core is legible at a glance.
    let name_color = match core.status {
        ConnStatus::Ready => p.text,
        ConnStatus::Connecting | ConnStatus::Stage(_) => p.yellow,
        ConnStatus::Failed(_) => p.red,
        ConnStatus::Disconnected => p.text_muted,
    };
    // Only a Ready core has a live ping; a dropped core keeps a stale reading that must not show an
    // alarm colour, so its ping/exch cells stay neutral (the name colour already marks it offline).
    let ready = core.status == ConnStatus::Ready;
    let ping_lvl = if ready {
        ping_level(core.sys.round_trip_ms, core.ping_base)
    } else {
        LoadLevel::Normal
    };
    let exch_lvl = if ready {
        order_level(core.sys.order_api_latency_ms, core.exch_base)
    } else {
        LoadLevel::Normal
    };
    h_flex()
        .w_full()
        .min_w_0()
        .items_center()
        .gap_2()
        .overflow_hidden()
        .pl(px(20.0))
        .cursor_pointer()
        // Clicking a core row charts that core; the detached window reads `chart_core`.
        .on_mouse_down(MouseButton::Left, {
            let weak_view = weak_view.clone();
            let id = core.id;
            move |_, _, app| {
                if let Some(view) = weak_view.upgrade() {
                    view.update(app, |this, cx| this.select_chart_core(id, cx));
                }
            }
        })
        .child(
            div()
                .flex_none()
                .truncate()
                .text_color(rgb(name_color))
                .child(core.name.clone()),
        )
        .child(div().flex_1())
        .child(metric_cell(
            "icons/cpu.svg",
            percent(core.sys.process_cpu_percent),
            CPU_W,
            level_color(cpu_level(core.sys.process_cpu_percent), p),
            false,
            p,
        ))
        .child(metric_cell(
            "icons/memory-stick.svg",
            memory_u16(core.sys.used_memory_mb),
            MEM_W,
            p.text_soft,
            false,
            p,
        ))
        // This core's client↔core round-trip (globe), coloured relative to this core's own baseline;
        // the cell's own warn triangle marks a sustained above-baseline ping, like the CPU/memory cells.
        .child(metric_cell(
            "icons/globe.svg",
            ping(core.sys.round_trip_ms),
            PING_W,
            level_color(ping_lvl, p),
            core.ping_warn,
            p,
        ))
        // This core's core→exchange order latency (external-link), also relative to its own baseline.
        .child(metric_cell(
            "icons/external-link.svg",
            ping(core.sys.order_api_latency_ms.map(u32::from)),
            PING_W,
            level_color(exch_lvl, p),
            core.exch_warn,
            p,
        ))
}

/// The latency to show on a server row: among the READY cores, the one with the WORST colour level
/// (ties broken by the higher value), and that level. `(None, Normal)` when no ready core has a
/// reading. `pick` maps a core to its `(value ms, level)` for the axis, or `None` when it has none.
///
/// Each core is judged against its OWN baseline (inside `pick`), so a core whose high ping is its
/// normal does not paint the server row red just for being the numerically highest.
fn worst_by_level(
    cores: &[CoreStatusRow],
    pick: impl Fn(&CoreStatusRow) -> Option<(u32, LoadLevel)>,
) -> (Option<u32>, LoadLevel) {
    cores
        .iter()
        .filter(|c| c.status == ConnStatus::Ready)
        .filter_map(pick)
        .max_by(|a, b| a.1.cmp(&b.1).then(a.0.cmp(&b.0)))
        .map(|(value, level)| (Some(value), level))
        .unwrap_or((None, LoadLevel::Normal))
}

/// Render the server name as an inline rename field or a name plus a pencil edit action.
///
/// Args:
///     group: Server snapshot supplying the display name and identity key.
///     edit_input: Present only while this server's name is being renamed inline.
///     weak_view: Non-owning panel handle for the rename callback.
///     p: Active Moon palette.
///
/// Returns:
///     A fixed-width identity element; the name keeps priority over the masked IP.
fn server_identity(
    group: &ServerStatusGroup,
    edit_input: Option<Entity<MoonInputState>>,
    weak_view: &WeakEntity<CoreStatusView>,
    p: MoonPalette,
) -> AnyElement {
    let key = group.key;
    if let Some(state) = edit_input {
        return h_flex()
            .flex_none()
            .w(px(130.0))
            // The row's mouse-down toggles selection; keep it off the edit field.
            .on_mouse_down(MouseButton::Left, |_, _, app| app.stop_propagation())
            .child(
                MoonInput::new(SharedString::from(format!(
                    "core-status-name-{}",
                    key.tree_id()
                )))
                .state(&state)
                .small(),
            )
            .into_any_element();
    }
    let weak_view = weak_view.clone();
    let start_name = group.display_name.clone();
    h_flex()
        .flex_none()
        .items_center()
        .gap_1()
        .child(
            div()
                .max_w(px(150.0))
                .truncate()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(rgb(p.text))
                .child(group.display_name.clone()),
        )
        .child(
            div()
                .on_mouse_down(MouseButton::Left, |_, _, app| app.stop_propagation())
                .child(
                    MoonButton::new(SharedString::from(format!(
                        "core-status-edit-{}",
                        key.tree_id()
                    )))
                    .xsmall()
                    .ghost()
                    .label("\u{270E}")
                    .tooltip(t!("core_status.rename").to_string())
                    .on_click(move |_, window, app| {
                        let Some(view) = weak_view.upgrade() else {
                            return;
                        };
                        view.update(app, |this, cx| {
                            this.start_rename(key, start_name.clone(), window, cx)
                        });
                    }),
                ),
        )
        .into_any_element()
}

/// Render the eye control that momentarily reveals a masked server IP.
///
/// Args:
///     key: Server identity toggled by the control.
///     revealed: Whether the IP is currently shown.
///     weak_view: Non-owning panel handle for the reveal callback.
///
/// Returns:
///     A ghost icon button that stops the row's selection mouse-down.
fn eye_action(
    key: ServerKey,
    revealed: bool,
    weak_view: &WeakEntity<CoreStatusView>,
) -> impl IntoElement {
    let weak_view = weak_view.clone();
    div()
        .on_mouse_down(MouseButton::Left, |_, _, app| app.stop_propagation())
        .child(
            MoonButton::new(SharedString::from(format!(
                "core-status-eye-{}",
                key.tree_id()
            )))
            .xsmall()
            .ghost()
            .icon(if revealed {
                "icons/eye.svg"
            } else {
                "icons/eye-off.svg"
            })
            .tooltip(if revealed {
                t!("core_status.hide_ip").to_string()
            } else {
                t!("core_status.show_ip").to_string()
            })
            .on_click(move |_, window, app| {
                let Some(view) = weak_view.upgrade() else {
                    return;
                };
                view.update(app, |this, cx| this.toggle_reveal(key, window, cx));
            }),
        )
}

/// Render one inline metric as a themed icon next to a fixed-width value.
///
/// The value box is a fixed width so a changing number never reflows the row.
///
/// Args:
///     icon: Bundled MoonUI icon path.
///     value: Preformatted localized metric text.
///     value_w: Fixed width in pixels for the value box.
///     p: Active Moon palette.
///
/// Returns:
///     A compact icon-and-value pair with a stable footprint.
fn metric_cell(
    icon: &'static str,
    value: String,
    value_w: f32,
    color: u32,
    warn: bool,
    p: MoonPalette,
) -> impl IntoElement {
    h_flex()
        .flex_none()
        .items_center()
        .gap_1()
        .child(metric_icon(icon, p))
        .child(
            div()
                .w(px(value_w))
                .overflow_hidden()
                .whitespace_nowrap()
                .text_color(rgb(color))
                .child(value),
        )
        // A warning is a SUSTAINED/trend signal, distinct from the threshold-based number color.
        .children(warn.then(|| {
            svg()
                .path("icons/triangle-alert.svg")
                .size(px(12.0))
                .flex_none()
                .text_color(rgb(p.amber))
        }))
}

#[cfg(test)]
mod tests;
