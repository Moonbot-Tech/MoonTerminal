//! By-IP rendering for Core Status: a one-line server summary that expands to one row per core.
//!
//! Servers start collapsed. `MoonTree` owns expansion, keyboard navigation, and virtual row
//! layout; the renderer distinguishes a server header (root) from a core row (child) by item id.

use std::collections::HashMap;
use std::rc::Rc;

use gpui::prelude::FluentBuilder;
use gpui::*;
use moon_core::feed::diagnose;
use moon_ui::{
    MoonButton, MoonDisclosure, MoonInput, MoonInputState, MoonListItem, MoonPalette, MoonTree,
    MoonTreeItem, MoonTreeState, h_flex, v_flex,
};
use rust_i18n::t;

use crate::conn_diag::fault_short;

use crate::design;
use moon_core::feed::ConnStatus;
use moon_core::session::CoreId;

use super::CoreStatusView;
use super::by_ip_widths::{ByIpWidths, CELL_GAP_W, CHEVRON_W, ROW_GAP_W, TREE_SCROLLBAR_W};
use super::ip_cell::{IpCell, ip_cell};
use super::model::{
    CoreStatusRow, GroupVersion, ServerConnectivity, ServerKey, ServerStatusGroup, TzOffsetGroup,
};
use super::ordering::GroupSortField;
use super::presentation::{
    LoadLevel, api_expiry_level, api_expiry_text, cpu_level, cpu_load, free_mem_level, lat_level,
    level_color, memory_free, memory_u16, percent, ping_plain, tz_offset_group_text,
    version_behind_group_tooltip, version_behind_tooltip, version_color, version_group_text,
    version_text,
};
use super::startup::{
    StartupCell, problem_diagnostic_text, startup_cell, startup_cell_text, startup_facts,
    startup_tooltip,
};
use super::time_offset::{
    TzOffsetCell, tz_offset_cell, tz_offset_cell_text, tz_offset_facts, tz_offset_tooltip,
};

/// IP mask shown while the column is hidden; a fixed run avoids leaking the address length.
const IP_MASK: &str = "************";

/// Stand-in for a server whose address the terminal does not have.
///
/// The house glyph for an absent value (the calendar and summary cells use the same one), so a
/// server the terminal cannot name reads like every other missing number instead of like a cell
/// that failed to draw.
const IP_ABSENT: &str = "\u{2014}";

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
///     ip_masked: Whether the whole IP column is hidden, one panel-wide state set from the
///         column header.
///     editing: The server whose name is being renamed inline, if any.
///     edit_input: Shared input state backing the inline rename field.
///     chart_selected: The server highlighted by a body click (the chart target), if any.
///     chart_core: The core highlighted by a core-row click (charts that core), if any.
///     sort: The active By-IP column sort, for the header arrows.
///     measured_width: Width the width probe recorded on the previous frame; `0` before the first.
///     rem_size: The window's rem size, which sets the row insets.
///     overrides: The user's dragged column widths, keyed by `ByIpCol::key`; empty until one drag.
///         Borrowed, not owned: both reads below are synchronous and yield `Copy` geometries, so
///         nothing in the render tree outlives this call and a per-frame clone would be pure waste.
///     state: MoonTree state that owns scrolling and selection.
///     window: Host window, for the header's divider drag listener.
///     cx: Panel context used to create a weak action callback.
///
/// Returns:
///     A full-size list element.
#[allow(clippy::too_many_arguments)]
pub(super) fn grouped_server_view(
    groups: Rc<Vec<ServerStatusGroup>>,
    ip_masked: bool,
    editing: Option<ServerKey>,
    edit_input: Option<Entity<MoonInputState>>,
    chart_selected: Option<ServerKey>,
    chart_core: Option<CoreId>,
    sort: (GroupSortField, bool),
    measured_width: f32,
    rem_size: f32,
    overrides: &HashMap<String, f32>,
    state: &Entity<MoonTreeState>,
    window: &Window,
    cx: &Context<CoreStatusView>,
) -> AnyElement {
    let groups_empty = groups.is_empty();
    let weak_view = cx.entity().downgrade();
    // A second handle for the (sortable) header, since the tree's render closure moves `weak_view`.
    let header_weak = weak_view.clone();
    // A third for the width probe below, which outlives this call inside the canvas closure.
    let weak_measure = weak_view.clone();
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
    let header_palette = MoonPalette::active(cx);
    // One shrink factor for the whole view, from the width the previous frame measured. Header and
    // rows read the SAME widths, so the captions cannot drift off their values.
    //
    // The right edge must stay clear of the status dot AND of the tree's scrollbar: MoonUI draws
    // that scrollbar as an OVERLAY, taking no layout width, so a row sized to the full measurement
    // would slide its last column underneath it.
    //
    // Two geometries, and the difference matters: `logical` is what the user's dragged widths say
    // the columns are, and `widths` is what actually paints this frame after the shrink. The header
    // needs both — it draws with `widths` but anchors each divider drag in `logical`, so a drag on a
    // shrunk panel does not write back an already-scaled value for the resolver to scale again.
    let logical = ByIpWidths::resolved(overrides);
    let widths = ByIpWidths::for_width(
        measured_width,
        crate::design::status_dot_w(cx) + TREE_SCROLLBAR_W,
        rem_size,
        overrides,
    );
    // Headless tree: it installs no row click/expand handlers and does not override `.selected`, so
    // the panel drives selection (body click) and expansion (chevron click) itself.
    let tree = MoonTree::custom(state, move |entry, meta, _window, app| {
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
                            ip_masked,
                            entry.is_expanded(),
                            editing_input,
                            widths,
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
                    .child(core_row(core, widths, &weak_view, p, app));
            }
        }
        MoonListItem::new(meta.index)
            .selected(false)
            .child(entry.item().label().clone())
    })
    .w_full();

    // A fixed caption row above the scrolling tree: names each metric column so the stripped-unit
    // values (no "яд."/"своб"/"мс") still read as a table. It mirrors the row layout — same insets,
    // chevron gutter, `flex_1` spacer and fixed column widths — so its captions land over the values.
    v_flex()
        .size_full()
        .relative()
        // Width probe: an absolutely positioned zero-cost canvas that measures the view and stores
        // the result on the panel, so the NEXT frame can shrink the columns to fit. It notifies only
        // when the width actually moved (half a pixel of hysteresis), which is what keeps a repaint
        // from feeding itself.
        //
        // It sits OUTSIDE the empty-state branch below on purpose: a resize performed while no
        // server has reported yet would otherwise leave a stale width for the first real frame.
        .child(
            canvas(
                {
                    let weak = weak_measure;
                    move |bounds, _, app| {
                        let w = f32::from(bounds.size.width);
                        let Some(view) = weak.upgrade() else { return };
                        if (view.read(app).by_ip_width - w).abs() <= 0.5 {
                            return;
                        }
                        // DEFERRED, not applied here: this runs during the frame's prepaint, and
                        // `WindowInvalidator::invalidate_view` refuses to dirty the window while a
                        // draw phase is active — a `notify` raised here is dropped and no follow-up
                        // frame is scheduled. The deferred callback runs as a normal effect after
                        // the frame, where the notify lands and the next frame uses the new width.
                        // (MoonDataTable notifies straight from its own probe, which is why its
                        // width only ever settles on a later, unrelated repaint.)
                        app.defer(move |app| {
                            view.update(app, |this, cx| {
                                if (this.by_ip_width - w).abs() > 0.5 {
                                    this.by_ip_width = w;
                                    cx.notify();
                                }
                            });
                        });
                    }
                },
                |_, _, _, _| {},
            )
            .absolute()
            .size_full(),
        )
        .map(|root| {
            // Nothing to label and nothing to lay out: show the empty state, keeping the probe.
            if groups_empty {
                return root.child(
                    div()
                        .flex_1()
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_color(rgb(header_palette.text_muted))
                        .child(t!("core_status.empty").to_string()),
                );
            }
            root.child(super::by_ip_header::server_header(
                header_palette,
                sort,
                ip_masked,
                widths,
                logical,
                &header_weak,
                window,
                cx,
            ))
            .child(tree.flex_1().min_h_0())
        })
        .into_any_element()
}

/// Render one server summary header (name, masked IP, machine CPU/memory, status).
///
/// Args:
///     group: Aggregated server snapshot.
///     masked: Whether the IP column is currently hidden behind its mask.
///     edit_input: Present only while this server's name is being renamed inline.
///     w: Shared column widths for this frame, already shrunk to the measured row width.
///     weak_view: Non-owning panel handle for the row actions.
///     p: Active Moon palette.
///     app: Application context used to scale the status dot.
///
/// Returns:
///     A single header row; the name stays fixed while a spacer pushes metrics right.
// One more argument than clippy's default: the shared column widths, which every row must read
// from the same source as the header.
#[allow(clippy::too_many_arguments)]
fn server_row(
    group: &ServerStatusGroup,
    masked: bool,
    expanded: bool,
    edit_input: Option<Entity<MoonInputState>>,
    w: ByIpWidths,
    weak_view: &WeakEntity<CoreStatusView>,
    p: MoonPalette,
    app: &App,
) -> impl IntoElement {
    let dot_color = match group.connectivity {
        ServerConnectivity::Online => p.green,
        ServerConnectivity::Degraded => p.amber,
        ServerConnectivity::Offline => p.red,
    };

    h_flex()
        .w_full()
        .min_w_0()
        .items_center()
        .gap(px(ROW_GAP_W))
        .overflow_hidden()
        // The chevron is the ONLY expand trigger and toggles expansion directly (headless tree).
        .child({
            let weak_view = weak_view.clone();
            let key = group.key;
            div()
                .w(px(CHEVRON_W))
                .flex_none()
                .cursor_pointer()
                .on_mouse_down(MouseButton::Left, move |_, _, app| {
                    if let Some(view) = weak_view.upgrade() {
                        view.update(app, |this, cx| this.toggle_server_expand(key, cx));
                    }
                })
                // Passive: this div owns the click, so the caret must not take a hitbox of its own
                // and swallow it. Leave `box_size` unset because this raw-width gutter, matched by
                // the header spacer and each core row's empty slot, is the alignment cell. The
                // caret's default box scales from its smaller marker size; forcing the standard
                // 12-unit box would scale that child from the gutter's full unscaled width while
                // the three matching gutters stay fixed.
                .child(MoonDisclosure::glyph(expanded).size(design::DISCLOSURE_GLYPH_MARKER))
        })
        // Clicking the body selects this server for the chart AND blocks the row's expand toggle
        // (only the chevron expands). The pencil stops propagation first, preserving the rename action.
        .child(
            h_flex()
                .flex_1()
                .min_w_0()
                .items_center()
                .gap(px(ROW_GAP_W))
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
                .child(server_identity(group, edit_input, w, weak_view, p))
                .child(ip_column(group, masked, w, p))
                .child(div().flex_1())
                .child(metric_cell(
                    cpu_load(group.system_cpu_percent, group.logical_cpu_count),
                    w.cpu,
                    w.icon,
                    level_color(cpu_level(group.system_cpu_percent), p),
                    group.cpu_warn,
                    p,
                ))
                .child(metric_cell(
                    memory_free(group.process_memory_mb, group.free_physical_memory_mb),
                    w.mem,
                    w.icon,
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
                        c.sys.round_trip_ms.map(|v| (v, lat_level(c.ping_sev)))
                    });
                    metric_cell(
                        ping_plain(value),
                        w.ping,
                        w.icon,
                        level_color(level, p),
                        group.ping_warn,
                        p,
                    )
                })
                .child({
                    let (value, level) = worst_by_level(&group.cores, |c| {
                        c.sys
                            .order_api_latency_ms
                            .map(|v| (u32::from(v), lat_level(c.exch_sev)))
                    });
                    metric_cell(
                        ping_plain(value),
                        w.exch,
                        w.icon,
                        level_color(level, p),
                        group.exch_warn,
                        p,
                    )
                })
                // The API key belongs to the core, so the server row shows its MOST URGENT one —
                // the same key the warning is about, picked once during aggregation.
                .child(metric_cell(
                    api_expiry_text(group.api_key),
                    w.api,
                    w.icon,
                    level_color(
                        api_expiry_level(group.api_key, group.api_warn, group.api_notice),
                        p,
                    ),
                    group.api_warn,
                    p,
                ))
                // Rolled up from this server's cores so a COLLAPSED group still tells the truth: a
                // build is shown only when EVERY core reported and all agree, because a silent
                // sibling is a process this row cannot vouch for. Disagreement reads as an
                // ellipsis, whose whole message is "expand me".
                .child(
                    version_slot(
                        version_group_text(group.version),
                        matches!(group.version, GroupVersion::Uniform(_)),
                        group.version_behind.is_some(),
                        w.version,
                        p,
                    )
                    .when_some(group.version_behind, |c, (have, newest)| {
                        c.tooltip(crate::panels::common::text_tooltip(
                            version_behind_group_tooltip(have, newest),
                        ))
                    }),
                )
                // Rolled up from this server's cores so a COLLAPSED group still tells the
                // truth: an unfinished core wins, otherwise the longest any of them took.
                .child(startup_text_cell(
                    group.startup.unwrap_or(StartupCell::Absent),
                    w.startup,
                    p,
                ))
                // Rolled up from this server's cores so a COLLAPSED group still tells the truth: a
                // silent sibling beside a measured core reads as `Mixed`, never as the sibling's own
                // offset. The hover names the representative core the text is drawn from.
                .child(tz_offset_group_cell(group.tz_offset, w.tz_off, p).tooltip(
                    crate::panels::common::text_tooltip(tz_offset_group_tooltip(
                        &group.cores,
                        group.tz_offset,
                    )),
                ))
                .child(
                    div()
                        .w(px(w.cores))
                        .flex_none()
                        .text_color(rgb(p.text_soft))
                        .child(format!("{}/{}", group.ready_count, group.cores.len())),
                )
                // A dropped core while others still run — the connectivity warning, next to the
                // dot. The SLOT is always present, warning or not: rendering it only on a degraded
                // row would push that row's whole metric block left of the captions, which is
                // exactly the row whose numbers are being read.
                .child(div().w(px(w.icon)).flex_none().overflow_hidden().children(
                    group.conn_warn.then(|| {
                        svg()
                            .path("icons/triangle-alert.svg")
                            // Follows its slot: a fixed 12 would overhang once the row shrinks.
                            .size(px(w.icon))
                            .flex_none()
                            .text_color(rgb(p.amber))
                    }),
                ))
                .child(crate::design::status_dot(dot_color, app))
                // Keep the row clear of the tree's overlay scrollbar, which takes no layout width
                // and would otherwise cover the dot.
                .child(div().w(px(TREE_SCROLLBAR_W)).flex_none()),
        )
}

/// Render one core row: an indented, greyed name, then its process CPU, memory, and pings — laid out
/// as the SAME columns as the server row (empty chevron/IP/ratio/dot slots) so every value lines up
/// under its server heading instead of drifting.
///
/// Clicking the row charts this core in the detached window (in place of the server aggregate).
///
/// Args:
///     core: Per-process snapshot.
///     w: Shared column widths for this frame, matching the server row above it.
///     weak_view: Non-owning panel handle for the chart-this-core click.
///     p: Active Moon palette.
///     app: Application context, for the font-scaled dot-column width.
///
/// Returns:
///     A core row whose metrics align under the server metrics.
fn core_row(
    core: &CoreStatusRow,
    w: ByIpWidths,
    weak_view: &WeakEntity<CoreStatusView>,
    p: MoonPalette,
    app: &App,
) -> impl IntoElement {
    // The core name reads as plain text (not a badge pill); a READY core is greyed (`text_soft`) so it
    // reads as subordinate to its bold server name, while a connecting/failed/offline core keeps its
    // state colour so it stays legible at a glance.
    let name_color = match core.status {
        ConnStatus::Ready => p.text_soft,
        ConnStatus::Connecting | ConnStatus::Stage(_) => p.yellow,
        ConnStatus::Failed(_) => p.red,
        ConnStatus::Disconnected => p.text_muted,
    };
    // The engine computes each ping's severity against the core's own baseline and leaves a dropped
    // core at `Normal` (only a Ready core has a live reading), so a stale offline ping shows no alarm
    // colour without a separate gate here.
    let ping_lvl = lat_level(core.ping_sev);
    let exch_lvl = lat_level(core.exch_sev);
    h_flex()
        .w_full()
        .min_w_0()
        .items_center()
        .gap(px(ROW_GAP_W))
        .overflow_hidden()
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
        // Empty chevron gutter, matching the server row's 12 px expand column so the body aligns.
        .child(div().w(px(CHEVRON_W)).flex_none())
        .child(
            h_flex()
                .flex_1()
                .min_w_0()
                .items_center()
                .gap(px(ROW_GAP_W))
                .overflow_hidden()
                // Name in the server's name column, indented as a tree child and greyed.
                .child(
                    div()
                        .w(px(w.name))
                        .flex_none()
                        .pl(px(w.indent))
                        .overflow_hidden()
                        .truncate()
                        .text_color(rgb(name_color))
                        .child(core.name.clone()),
                )
                // Empty IP column so the metrics start under the server's columns.
                .child(div().w(px(w.ip)).flex_none())
                .child(div().flex_1())
                .child(metric_cell(
                    percent(core.sys.process_cpu_percent),
                    w.cpu,
                    w.icon,
                    level_color(cpu_level(core.sys.process_cpu_percent), p),
                    false,
                    p,
                ))
                .child(metric_cell(
                    memory_u16(core.sys.used_memory_mb),
                    w.mem,
                    w.icon,
                    p.text_soft,
                    false,
                    p,
                ))
                // This core's client↔core round-trip, coloured relative to this core's own baseline; the
                // cell's own warn triangle marks a sustained above-baseline ping, like the CPU cell.
                .child(metric_cell(
                    ping_plain(core.sys.round_trip_ms),
                    w.ping,
                    w.icon,
                    level_color(ping_lvl, p),
                    core.ping_warn,
                    p,
                ))
                // This core's core→exchange order latency, also relative to its own baseline.
                .child(metric_cell(
                    ping_plain(core.sys.order_api_latency_ms.map(u32::from)),
                    w.exch,
                    w.icon,
                    level_color(exch_lvl, p),
                    core.exch_warn,
                    p,
                ))
                // This core's own API key: the value is per core, and so is the warning triangle.
                .child(metric_cell(
                    api_expiry_text(core.api_key),
                    w.api,
                    w.icon,
                    level_color(
                        api_expiry_level(core.api_key, core.api_warn, core.api_notice),
                        p,
                    ),
                    core.api_warn,
                    p,
                ))
                // This core's own reported build. No warning treatment beyond the "behind the
                // fleet" mark: a ramp needs a threshold, and this workspace defines no minimum
                // version by explicit decision. The one age claim the terminal makes — `legacy_core`,
                // about a missing PROTOCOL version — stays in the fault hover one cell to the right,
                // where it can co-occur with this number instead of being duplicated by it.
                .child(
                    version_slot(
                        version_text(core.server_version),
                        core.server_version.is_some(),
                        core.version_behind.is_some(),
                        w.version,
                        p,
                    )
                    .when_some(core.version_behind, |c, newest| {
                        c.tooltip(crate::panels::common::text_tooltip(version_behind_tooltip(
                            core.server_version,
                            newest,
                        )))
                    }),
                )
                // The per-core cell carries the full detail behind it; the server row above only
                // summarises, so the hover lives here where there is one snapshot to describe.
                //
                // A core with a VERDICT gives up its progress figure for it. That figure explains
                // nothing a stopped core needs explained, while the reason and its next step are
                // the entire point of this column for such a row; the ordinary startup hover stays
                // one row-hover away on every core that is actually starting.
                .child(
                    match diagnose(&core.status, core.fault.as_ref(), &core.startup) {
                        Some(d) => plain_slot(
                            "core-status-startup",
                            fault_short(&d.class),
                            w.startup,
                            p.red,
                        )
                        .tooltip(crate::panels::common::text_tooltip(
                            problem_diagnostic_text(&d, core.fault.as_ref(), &core.startup),
                        )),
                        None => startup_text_cell(
                            startup_cell(&core.status, &core.startup),
                            w.startup,
                            p,
                        )
                        .tooltip(crate::panels::common::text_tooltip(startup_tooltip(
                            &startup_facts(&core.startup),
                        ))),
                    },
                )
                // This core's own measured offset: the full facts live here, one snapshot away
                // from the summarised server row above.
                .child(
                    tz_offset_text_cell(tz_offset_cell(&core.time_offset), w.tz_off, p).tooltip(
                        crate::panels::common::text_tooltip(tz_offset_tooltip(&tz_offset_facts(
                            &core.time_offset,
                        ))),
                    ),
                )
                // Empty ratio, warning, dot and scrollbar slots, matching the server row's trailing
                // width so `key` aligns.
                .child(div().w(px(w.cores)).flex_none())
                .child(div().w(px(w.icon)).flex_none())
                .child(div().w(px(crate::design::status_dot_w(app))).flex_none())
                .child(div().w(px(TREE_SCROLLBAR_W)).flex_none()),
        )
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
///     w: Shared column widths, supplying the name column.
///     weak_view: Non-owning panel handle for the rename callback.
///     p: Active Moon palette.
///
/// Returns:
///     A fixed-width identity element; the name keeps priority over the masked IP.
fn server_identity(
    group: &ServerStatusGroup,
    edit_input: Option<Entity<MoonInputState>>,
    w: ByIpWidths,
    weak_view: &WeakEntity<CoreStatusView>,
    p: MoonPalette,
) -> AnyElement {
    let key = group.key;
    if let Some(state) = edit_input {
        return h_flex()
            .flex_none()
            .w(px(w.name))
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
    // A fixed-width column: the name takes the room and truncates, the pencil stays pinned at its
    // right edge, so the IP column beside it starts at the same x on every row.
    h_flex()
        .w(px(w.name))
        .flex_none()
        .items_center()
        .gap_1()
        .overflow_hidden()
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(rgb(p.text))
                .child(group.display_name.clone()),
        )
        .child(
            div()
                .flex_none()
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

/// Render the IP column: the address, its fixed-length mask, or a visible "no address" glyph, in a
/// fixed-width slot so every server's cell lines up under the "IP" header.
///
/// All THREE states draw something, which is the point. A server with no known endpoint used to get
/// an EMPTY slot with no glyph and no control, so "the terminal does not have this address" was
/// indistinguishable from a panel that had failed to render. The mask control itself now lives ONCE
/// in the column header rather than once per row, so the row carries only the value: one click masks
/// a fleet of servers instead of one click each, and the freed width goes back to the address.
///
/// Args:
///     group: Server snapshot supplying the address.
///     masked: Whether the user has hidden the whole IP column.
///     w: Shared column widths, supplying the IP column.
///     p: Active Moon palette.
///
/// Returns:
///     A fixed-width IP cell.
fn ip_column(
    group: &ServerStatusGroup,
    masked: bool,
    w: ByIpWidths,
    p: MoonPalette,
) -> impl IntoElement {
    let cell = ip_cell(group.address, masked);
    // Only the unknown state carries a tooltip: the other two are self-explanatory, and a tooltip on
    // every row of a 56-server fleet is noise.
    let unknown = matches!(cell, IpCell::Unknown);
    let (text, color) = match cell {
        IpCell::Shown(address) => (address.to_string(), p.text_soft),
        IpCell::Masked => (IP_MASK.to_string(), p.text_muted),
        IpCell::Unknown => (IP_ABSENT.to_string(), p.text_muted),
    };
    // A constant id shared by every row, like `startup_slot`: the id exists to make the element
    // stateful enough to carry a tooltip, not to identify the row.
    div()
        .id("core-status-ip")
        .w(px(w.ip))
        .flex_none()
        .truncate()
        .text_color(rgb(color))
        .when(unknown, |cell| {
            cell.tooltip(crate::panels::common::text_tooltip(
                t!("core_status.ip_unknown").to_string(),
            ))
        })
        .child(text)
}

/// One startup cell: fixed width, clipped, never wrapping.
///
/// Deliberately NOT a [`metric_cell`]: that helper reserves a warning-icon lead driven by a
/// `WarnAxis`, and startup has none — the lead would be space that can never light. This follows
/// the core-ratio cell instead, which is sized the same way for the same reason.
///
/// Args:
///     cell: The decision from `startup::startup_cell`, or a group's rolled-up equivalent.
///     value_w: Current column width from [`ByIpWidths`].
///     p: Active Moon palette.
///
/// Returns:
///     A compact startup cell with a stable footprint.
fn startup_text_cell(cell: StartupCell, value_w: f32, p: MoonPalette) -> Stateful<Div> {
    // A finished figure is subordinate to the live metrics beside it: it is frozen history, and
    // the past-tense wording in the text carries the meaning. Colour is only the second cue.
    let color = match cell {
        StartupCell::Progress { .. } => p.amber,
        StartupCell::Done { .. } => p.text_soft,
        StartupCell::Absent => p.text_muted,
    };
    plain_slot(
        "core-status-startup",
        startup_cell_text(cell),
        value_w,
        color,
    )
}

/// One rolled-up tz-offset cell for a collapsed server row: fixed width, clipped, never wrapping.
///
/// Deliberately NOT a [`metric_cell`], for the same reason [`startup_text_cell`] is not one: a
/// clock offset has no `WarnAxis` behind it, so a warning-icon lead here would be space that can
/// never light.
///
/// Args:
///     group: The rollup from `model::group_tz_offset`.
///     value_w: Current column width from [`ByIpWidths`].
///     p: Active Moon palette.
///
/// Returns:
///     A compact tz-offset cell with a stable footprint.
fn tz_offset_group_cell(group: TzOffsetGroup, value_w: f32, p: MoonPalette) -> Stateful<Div> {
    let color = match group {
        TzOffsetGroup::Uniform(_) => p.text_soft,
        TzOffsetGroup::Mixed | TzOffsetGroup::Absent => p.text_muted,
    };
    plain_slot(
        "core-status-tz-off",
        tz_offset_group_text(group),
        value_w,
        color,
    )
}

/// Hover for the collapsed server row's tz-offset cell.
///
/// A rollup carries no facts of its own (only the agreement state), so `Uniform` borrows the full
/// facts of the FIRST measured core — every measured core already agrees on the offset, so any one
/// of them is a true account of it. `Mixed` and `Absent` fall back to the same never-measured hover
/// a lone `Unknown` core shows, because a collapsed group in either state has nothing it can vouch
/// for.
///
/// Args:
///     cores: The group's core rows, already collected.
///     group: The rollup from `model::group_tz_offset`, over the SAME cores.
///
/// Returns:
///     The hover body for the collapsed row.
fn tz_offset_group_tooltip(cores: &[CoreStatusRow], group: TzOffsetGroup) -> String {
    let representative = match group {
        TzOffsetGroup::Uniform(_) => cores
            .iter()
            .find(|core| core.time_offset.offset_secs.is_some())
            .map(|core| core.time_offset),
        TzOffsetGroup::Mixed | TzOffsetGroup::Absent => None,
    }
    .unwrap_or_default();
    tz_offset_tooltip(&tz_offset_facts(&representative))
}

/// One per-core tz-offset cell: fixed width, clipped, never wrapping.
///
/// Deliberately NOT a [`metric_cell`], for the same reason [`tz_offset_group_cell`] is not one.
///
/// Args:
///     cell: The decision from `time_offset::tz_offset_cell`.
///     value_w: Current column width from [`ByIpWidths`].
///     p: Active Moon palette.
///
/// Returns:
///     A compact tz-offset cell with a stable footprint.
fn tz_offset_text_cell(cell: TzOffsetCell, value_w: f32, p: MoonPalette) -> Stateful<Div> {
    let color = match cell {
        TzOffsetCell::Measured { .. } => p.text_soft,
        TzOffsetCell::Unknown => p.text_muted,
    };
    plain_slot(
        "core-status-tz-off",
        tz_offset_cell_text(cell),
        value_w,
        color,
    )
}

/// One reported-build cell, in the same slot chrome the startup column uses.
///
/// A reported build is a frozen identity fact rather than a live measurement, so it is subordinate
/// to the metrics beside it — `text_soft`, the same treatment [`startup_text_cell`] gives a
/// FINISHED startup. Absence and disagreement are `text_muted`: they mean "no answer", and painting
/// them red or a warning colour would turn a fact this terminal cannot establish into an
/// accusation. A build behind the fleet's newest takes [`super::presentation::notice_color`] --
/// a STATE mark against a fleet-derived reference, not a threshold ramp and not an episode: no
/// triangle, no sound, no warning source of its own. That function owns which colour it is, and
/// why it is not the amber used elsewhere in this file.
///
/// Args:
///     text: The already-composed cell text.
///     reported: Whether there is a build to show, as opposed to absence or disagreement.
///     behind: Whether this build is behind the fleet's newest reported build.
///     value_w: Current column width from [`ByIpWidths`].
///     p: Active Moon palette.
///
/// Returns:
///     A compact build cell with a stable footprint.
fn version_slot(
    text: String,
    reported: bool,
    behind: bool,
    value_w: f32,
    p: MoonPalette,
) -> Stateful<Div> {
    plain_slot(
        "core-status-version",
        text,
        value_w,
        version_color(behind, reported, p),
    )
}

/// A fixed-width text cell with NO warning-icon lead, for a column that has no `WarnAxis` behind
/// it.
///
/// Deliberately NOT [`metric_cell`]: that helper reserves a lead driven by a `*_warn` bool, and a
/// column with no warning source would reserve space that can never light. The startup column, the
/// per-core connection VERDICT that replaces it, and the reported build all render through this one
/// owner, so their width, clipping and no-wrap behaviour cannot drift apart.
///
/// The id is a PARAMETER because a row now draws more than one of these; two children sharing a
/// stateful element id is a real GPUI hazard, not a style point.
///
/// Args:
///     id: Stable element identity, unique among the cells of one row.
///     text: Already-composed cell text.
///     value_w: Current column width from [`ByIpWidths`].
///     color: Packed theme colour for the text.
///
/// Returns:
///     A compact cell with a stable footprint.
fn plain_slot(id: &'static str, text: String, value_w: f32, color: u32) -> Stateful<Div> {
    div()
        .id(id)
        .w(px(value_w))
        .flex_none()
        .overflow_hidden()
        .whitespace_nowrap()
        .text_color(rgb(color))
        .child(text)
}

/// Render one inline metric value in a fixed-width box, with a leading slot for its warning mark.
///
/// No decorative icon: the icon lead holds the SUSTAINED-warning triangle for THIS metric when set,
/// so the mark sits directly left of its own value instead of trailing the fixed box into the next
/// column (which read as the neighbour's warning). Empty otherwise, so every column stays aligned and
/// matches the header caption's lead.
///
/// Args:
///     value: Preformatted localized metric text.
///     value_w: Width in pixels for the value box, from the frame's shared widths.
///     icon_w: Width of the warning-icon lead, from the same widths.
///     color: Threshold color of the number.
///     warn: Whether this metric has an open sustained-warning episode.
///     p: Active Moon palette.
///
/// Returns:
///     A compact metric cell with a stable footprint.
fn metric_cell(
    value: String,
    value_w: f32,
    icon_w: f32,
    color: u32,
    warn: bool,
    p: MoonPalette,
) -> impl IntoElement {
    h_flex()
        .flex_none()
        .items_center()
        .gap(px(CELL_GAP_W))
        .child(
            div()
                .w(px(icon_w))
                .overflow_hidden()
                .flex_none()
                // A warning is a SUSTAINED/trend signal, distinct from the threshold-based number color.
                .children(warn.then(|| {
                    svg()
                        .path("icons/triangle-alert.svg")
                        // Follows its slot: a fixed 12 would overhang into the value box once the
                        // row shrinks.
                        .size(px(icon_w))
                        .flex_none()
                        .text_color(rgb(p.amber))
                })),
        )
        .child(
            div()
                .w(px(value_w))
                .overflow_hidden()
                .whitespace_nowrap()
                .text_color(rgb(color))
                .child(value),
        )
}

#[cfg(test)]
mod tests;
