//! Servers mode: one card per server, warned machines first.
//!
//! The card takes the data the panel already has. It does not read `CoreStatusView` during
//! render: a helper that does panics, because render is already that entity's update.

use std::collections::HashMap;
use std::net::IpAddr;

use gpui::prelude::FluentBuilder;
use gpui::*;
use moon_core::session::CoreId;
use moon_ui::{
    MoonInput, MoonInputState, MoonPalette, MoonProgress, MoonScrollableElement, MoonTag, h_flex,
    v_flex,
};
use rust_i18n::t;

use crate::controls::core_update;
use crate::controls::row_selection::RowSelection;
use crate::design;

use super::CoreStatusView;
use super::chart::paint_series;
use super::ip_cell::{ip_cell, ip_text};
use super::model::{ServerKey, ServerStatusGroup};
use super::ordering::natural_cmp;
use super::presentation::{
    LoadLevel, card_attention_color, cpu_level, cpu_load, free_mem_level, group_build_parts,
    memory_free, status_level,
};
use super::update_menu;

/// Minimum card width in design pixels. [`design::ui_px`] scales it; the card does not set a
/// raw width on a tiered control.
const CARD_MIN_W: f32 = 260.0;

/// Servers in card order: a warned machine first, then natural display name.
///
/// Args:
///     groups: Server snapshots. Address order from the cache is not the card order.
///
/// Returns:
///     The same groups, borrowed, in the order the grid paints.
pub(super) fn card_order(groups: &[ServerStatusGroup]) -> Vec<&ServerStatusGroup> {
    let mut ordered: Vec<&ServerStatusGroup> = groups.iter().collect();
    ordered.sort_by(|a, b| {
        b.has_warn()
            .cmp(&a.has_warn())
            .then_with(|| natural_cmp(&a.display_name, &b.display_name))
    });
    ordered
}

/// Flattened core list in card order, one `Some` per core the cards show.
///
/// Selection, Ctrl+A and the bulk scope read this. A card has no heading row, so every entry
/// is a core.
///
/// Args:
///     groups: Server snapshots, in any order. This function sorts them.
///
/// Returns:
///     Core ids in the order the cards paint their chips.
pub(super) fn card_core_order(groups: &[ServerStatusGroup]) -> Vec<Option<CoreId>> {
    card_order(groups)
        .into_iter()
        .flat_map(|group| group.cores.iter().map(|core| Some(core.id)))
        .collect()
}

/// Server cards the grid draws for this search.
///
/// Selection and both chip handlers read this same filter, so a hidden card cannot stay
/// selected or sit in the bulk range.
///
/// Args:
///     groups: Cached server snapshots.
///     query: Toolbar search, already stored by the panel. Empty keeps every card.
///     ip_masked: Whether addresses are hidden from search.
///
/// Returns:
///     The groups [`servers_view`] paints, in the cached order.
pub(super) fn shown_server_groups(
    groups: &[ServerStatusGroup],
    query: &str,
    ip_masked: bool,
) -> Vec<ServerStatusGroup> {
    groups
        .iter()
        .filter(|group| super::cache::card_matches_query(group, query, ip_masked))
        .cloned()
        .collect()
}

/// The Servers grid.
///
/// Args:
///     groups: Cached server snapshots.
///     ip_masked: Whether addresses are hidden for the whole panel.
///     editing: Server whose name is being renamed, if any.
///     edit_input: The rename field, present only while `editing` is set.
///     chart_server: Server the detached chart follows.
///     chart_core: Core the detached chart follows, when a chip is selected.
///     cpu_history: CPU series already read off `core_chart_hist`, keyed by address.
///     query: Toolbar search, already stored by the panel. Empty keeps every card.
///     cx: Panel context. Used to downgrade a callback handle and to read the palette.
///         Never to read the view.
///
/// Returns:
///     A wrapping grid of server cards.
#[allow(clippy::too_many_arguments)]
pub(super) fn servers_view(
    groups: &[ServerStatusGroup],
    ip_masked: bool,
    editing: Option<ServerKey>,
    edit_input: Option<Entity<MoonInputState>>,
    chart_server: Option<ServerKey>,
    chart_core: Option<CoreId>,
    cpu_history: &HashMap<IpAddr, Vec<(f32, f32)>>,
    query: &str,
    selection: RowSelection<CoreId>,
    available_w: f32,
    cx: &Context<CoreStatusView>,
) -> impl IntoElement + use<> {
    let weak_view = cx.entity().downgrade();
    let shown = shown_server_groups(groups, query, ip_masked);
    server_grid(
        &shown,
        ip_masked,
        editing,
        edit_input,
        chart_server,
        chart_core,
        cpu_history,
        &selection,
        available_w,
        weak_view,
        cx,
    )
}

/// Width probe for the server grid. `0` stays "not measured" until this fires.
///
/// Args:
///     grid: The card grid already built for the current minimum.
///     on_width: Called with the host width in pixels during prepaint.
///
/// Returns:
///     The measuring host. It fills its parent and hides overflow.
pub(super) fn servers_measure_host(
    grid: impl IntoElement,
    on_width: impl Fn(f32, &mut App) + 'static,
) -> Div {
    div()
        .size_full()
        .min_w(px(0.0))
        .overflow_hidden()
        .relative()
        .debug_selector(|| "cs-servers-measure".into())
        .child(
            canvas(
                move |bounds, _, app| on_width(f32::from(bounds.size.width), app),
                |_, _, _, _| {},
            )
            .absolute()
            .size_full(),
        )
        .child(
            div()
                .size_full()
                .min_w(px(0.0))
                .overflow_hidden()
                .child(grid),
        )
}

/// The card grid, given a callback handle the caller already downgraded.
///
/// Args:
///     groups: Cached server snapshots.
///     ip_masked: Whether addresses are hidden for the whole panel.
///     editing: Server whose name is being renamed, if any.
///     edit_input: The rename field, present only while `editing` is set.
///     chart_server: Server the detached chart follows.
///     chart_core: Core the detached chart follows, when a chip is selected.
///     cpu_history: CPU series already read off `core_chart_hist`, keyed by address.
///     weak_view: Panel handle, moved in so the grid does not borrow the caller's local.
///         Clicks no-op when it cannot upgrade.
///     cx: Application context used for the palette and the density scale. Never the view.
///
/// Returns:
///     A wrapping grid of server cards.
#[allow(clippy::too_many_arguments)]
pub(super) fn server_grid(
    groups: &[ServerStatusGroup],
    ip_masked: bool,
    editing: Option<ServerKey>,
    edit_input: Option<Entity<MoonInputState>>,
    chart_server: Option<ServerKey>,
    chart_core: Option<CoreId>,
    cpu_history: &HashMap<IpAddr, Vec<(f32, f32)>>,
    selection: &RowSelection<CoreId>,
    available_w: f32,
    weak_view: WeakEntity<CoreStatusView>,
    cx: &App,
) -> impl IntoElement + use<> {
    let p = MoonPalette::active(cx);
    let cards = card_order(groups);
    div()
        .id("core-status-servers")
        .debug_selector(|| "cs-servers".into())
        .size_full()
        .min_w(px(0.0))
        .flex()
        .flex_wrap()
        .items_start()
        .content_start()
        .gap(design::ui_px(cx, 10.0))
        .p(design::ui_px(cx, 12.0))
        .children(cards.into_iter().map(|group| {
            let series = group
                .address
                .and_then(|ip| cpu_history.get(&ip))
                .map(Vec::as_slice)
                .unwrap_or(&[]);
            server_card(
                group,
                ip_masked,
                editing,
                edit_input.clone(),
                chart_server,
                chart_core,
                series,
                selection,
                available_w,
                &weak_view,
                p,
                cx,
            )
        }))
        .overflow_y_scrollbar()
}

/// One server card.
fn server_card(
    group: &ServerStatusGroup,
    ip_masked: bool,
    editing: Option<ServerKey>,
    edit_input: Option<Entity<MoonInputState>>,
    chart_server: Option<ServerKey>,
    _chart_core: Option<CoreId>,
    series: &[(f32, f32)],
    selection: &RowSelection<CoreId>,
    available_w: f32,
    weak_view: &WeakEntity<CoreStatusView>,
    p: MoonPalette,
    cx: &App,
) -> impl IntoElement + use<> {
    let key = group.key;
    let selected = chart_server == Some(key);
    let border = if group.has_warn() {
        design::moon_alpha(p.amber, 0.35)
    } else if selected {
        design::moon(p.accent)
    } else {
        design::moon(p.border)
    };
    let cpu_text = cpu_load(group.system_cpu_percent, group.logical_cpu_count);
    let ram_text = memory_free(group.process_memory_mb, group.free_physical_memory_mb);
    let parts = group_build_parts(&group.version);
    let _ = parts;
    let cpu_color = bar_color(cpu_level(group.system_cpu_percent), p);
    let ram_color = bar_color(free_mem_level(group.free_physical_memory_mb), p);
    let rename_view = weak_view.clone();
    let select_view = weak_view.clone();
    let name = group.display_name.clone();
    let editing_this = editing == Some(key);
    let cell = ip_cell(group.address, ip_masked);
    let ip_unknown = matches!(cell, super::ip_cell::IpCell::Unknown);
    let ip_label = ip_text(cell);

    let designed = design::ui_px(cx, CARD_MIN_W);
    // Grid padding is 12 on each side. A measured dock narrower than the design minimum
    // shrinks the card to the inner width instead of clipping past the dock.
    let min_w = if available_w > 0.0 {
        let pad = f32::from(design::ui_px(cx, 24.0));
        let inner = px((available_w - pad).max(0.0));
        if f32::from(designed) <= f32::from(inner) {
            designed
        } else {
            inner
        }
    } else {
        designed
    };
    let card_selector = format!("cs-server-{}", key.tree_id());
    v_flex()
        .id(SharedString::from(format!("cs-server-{}", key.tree_id())))
        .debug_selector(move || card_selector)
        .min_w(min_w)
        .max_w_full()
        .overflow_hidden()
        .flex_grow(1.0)
        .flex_basis(min_w)
        .gap(design::ui_px(cx, 8.0))
        .px(design::ui_px(cx, 12.0))
        .py(design::ui_px(cx, 10.0))
        .border_1()
        .border_color(border)
        .rounded(design::ui_px(cx, 6.0))
        .bg(rgb(p.table_body))
        .on_click(move |_, _, app| {
            let Some(view) = select_view.upgrade() else {
                return;
            };
            view.update(app, |this, cx| this.select_chart_server(key, cx));
        })
        .child(
            h_flex()
                .w_full()
                .items_center()
                .gap(design::ui_px(cx, 8.0))
                .child(design::status_dot_sized(worst_dot(group, p), 8.0, cx))
                .child(server_name(
                    key,
                    &name,
                    editing_this,
                    edit_input,
                    &rename_view,
                    p,
                    cx,
                ))
                .child(
                    div()
                        .id(SharedString::from(format!(
                            "core-status-ip-{}",
                            key.tree_id()
                        )))
                        .ml_auto()
                        .flex_none()
                        .font_family(design::mono())
                        .text_size(design::t_caption(cx))
                        .text_color(rgb(p.text_muted))
                        .when(ip_unknown, |cell| {
                            cell.tooltip(crate::panels::common::text_tooltip(
                                t!("core_status.ip_unknown").to_string(),
                            ))
                        })
                        .child(ip_label),
                ),
        )
        .child(metric_row(
            SharedString::from(format!("cs-card-cpu-{}", key.tree_id())),
            t!("core_status.servers.cpu").to_string(),
            group.system_cpu_percent.map(f32::from).unwrap_or(0.0),
            cpu_color,
            cpu_text,
            p,
            cx,
        ))
        .child(metric_row(
            SharedString::from(format!("cs-card-ram-{}", key.tree_id())),
            t!("core_status.servers.ram_free").to_string(),
            free_bar(group.free_physical_memory_mb),
            ram_color,
            ram_text,
            p,
            cx,
        ))
        .child(sparkline(
            SharedString::from(format!("cs-spark-{}", key.tree_id())),
            series,
            p,
            cx,
        ))
        .child(core_chips(group, selection, weak_view, cx))
}

/// The name, or the inline rename field while this server is being edited.
fn server_name(
    key: ServerKey,
    name: &str,
    editing: bool,
    edit_input: Option<Entity<MoonInputState>>,
    weak_view: &WeakEntity<CoreStatusView>,
    p: MoonPalette,
    _cx: &App,
) -> AnyElement {
    if editing && let Some(state) = edit_input {
        return h_flex()
            .flex_1()
            .min_w_0()
            .on_mouse_down(MouseButton::Left, |_, _, app| app.stop_propagation())
            .child(
                MoonInput::new(SharedString::from(format!(
                    "core-status-name-{}",
                    key.tree_id()
                )))
                .state(&state)
                .size(design::INPUT_SIZE),
            )
            .into_any_element();
    }
    let weak_view = weak_view.clone();
    let start_name = name.to_string();
    div()
        .id(SharedString::from(format!("cs-name-{}", key.tree_id())))
        .flex_1()
        .min_w_0()
        .truncate()
        .font_family(design::mono())
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(rgb(p.text))
        .tooltip(crate::panels::common::text_tooltip(
            t!("core_status.rename").to_string(),
        ))
        .child(name.to_string())
        .on_click(move |event, window, app| {
            if event.click_count() < 2 {
                return;
            }
            app.stop_propagation();
            let Some(view) = weak_view.upgrade() else {
                return;
            };
            view.update(app, |this, cx| {
                this.start_rename(key, start_name.clone(), window, cx);
            });
        })
        .into_any_element()
}

/// CPU or free-RAM row: caption, bar, mono value.
fn metric_row(
    id: SharedString,
    caption: String,
    value: f32,
    color: u32,
    text: String,
    p: MoonPalette,
    cx: &App,
) -> impl IntoElement + use<> {
    let selector = id.to_string();
    let figure = format!("cs-fig-{id}");
    h_flex()
        .debug_selector(move || selector)
        .w_full()
        .min_w(px(0.0))
        .items_center()
        .gap(design::ui_px(cx, 8.0))
        .child(
            div()
                .w(design::ui_px(cx, 60.0))
                .flex_none()
                .text_size(design::t_caption(cx))
                .text_color(rgb(p.text_muted))
                .child(caption),
        )
        .child(
            div().flex_1().min_w_0().child(
                MoonProgress::new(id)
                    .value(value.clamp(0.0, 100.0))
                    .color(color)
                    .height(5.0),
            ),
        )
        .child(
            div()
                .debug_selector(move || figure)
                .flex_none()
                .font_family(design::mono())
                .text_size(design::t_caption(cx))
                .text_color(rgb(p.text))
                .child(text),
        )
}

/// A short CPU history line. Fewer than two samples paints nothing.
fn sparkline(
    selector: SharedString,
    series: &[(f32, f32)],
    p: MoonPalette,
    cx: &App,
) -> impl IntoElement + use<> {
    let series = series.to_vec();
    let line = design::moon(p.accent);
    div()
        .debug_selector(move || selector.to_string())
        .w_full()
        .h(design::ui_px(cx, 26.0))
        .child(canvas(
            |_, _, _| (),
            move |bounds, _, window, _| {
                let w = f32::from(bounds.size.width);
                let h = f32::from(bounds.size.height);
                if w < 4.0 || h < 4.0 {
                    return;
                }
                paint_series(window, bounds.origin, w, h, &series, None, line, 1.3);
            },
        ))
}

/// One chip per core on this server. An offline core is a danger chip. A selected core is
/// accent, which is not the chart's info tone.
fn core_chips(
    group: &ServerStatusGroup,
    selection: &RowSelection<CoreId>,
    weak_view: &WeakEntity<CoreStatusView>,
    cx: &App,
) -> impl IntoElement + use<> {
    let group_key = group.key;
    let chips = group.cores.iter().map(|core| {
        let id = core.id;
        let offline = matches!(status_level(&core.status), LoadLevel::Critical);
        let selected = selection.contains(Some(id));
        let weak_view = weak_view.clone();
        let menu_view = weak_view.clone();
        let menu_selection = selection.clone();
        let name = core.name.clone();
        let tag = if offline {
            MoonTag::danger()
        } else if selected {
            MoonTag::new().tone(moon_ui::MoonTone::Accent)
        } else {
            MoonTag::new()
        };
        let chip_selector = format!("cs-chip-{id}");
        div()
            .id(SharedString::from(format!("cs-chip-{id}")))
            .debug_selector(move || chip_selector)
            .max_w_full()
            .overflow_hidden()
            .tooltip(crate::panels::common::text_tooltip(name.clone()))
            .on_mouse_down(MouseButton::Left, |_, _, app| app.stop_propagation())
            .on_mouse_down(MouseButton::Right, move |_, window, app| {
                app.stop_propagation();
                let Some(view) = menu_view.upgrade() else {
                    return;
                };
                let (backend, rows, order_cores) = view.update(app, |this, _cx| {
                    let order_cores: Vec<CoreId> = card_core_order(&shown_server_groups(
                        &this.cached_groups,
                        &this.search_query,
                        this.ip_masked,
                    ))
                    .into_iter()
                    .flatten()
                    .collect();
                    (this.backend.clone(), this.cached_rows.clone(), order_cores)
                });
                let cores = core_update::resolve_menu_scope(id, &order_cores, &menu_selection);
                let scope = update_menu::UpdateScope::from_cores(&cores, &rows);
                if scope.is_empty() {
                    return;
                }
                update_menu::open_update_row_menu(
                    &backend,
                    scope,
                    window.mouse_position(),
                    window,
                    app,
                );
            })
            .on_click(move |_, window, app| {
                app.stop_propagation();
                let modifiers = window.modifiers();
                let Some(view) = weak_view.upgrade() else {
                    return;
                };
                view.update(app, |this, cx| {
                    let order = card_core_order(&shown_server_groups(
                        &this.cached_groups,
                        &this.search_query,
                        this.ip_masked,
                    ));
                    this.chart_server = Some(group_key);
                    this.chart_core = Some(id);
                    this.select_core_row(Some(id), &order, modifiers, cx);
                });
            })
            .child(
                tag.mono(true)
                    .child(div().max_w_full().overflow_hidden().truncate().child(name)),
            )
    });
    let selector = format!("cs-cores-{}", group.key.tree_id());
    div()
        .debug_selector(move || selector)
        .flex()
        .flex_wrap()
        .gap(design::ui_px(cx, 6.0))
        .children(chips)
}

fn worst_dot(group: &ServerStatusGroup, p: MoonPalette) -> u32 {
    let mut rank = 0u8;
    for core in &group.cores {
        let next = match status_level(&core.status) {
            LoadLevel::Critical => 3,
            LoadLevel::Warning => 2,
            LoadLevel::Notice => 1,
            LoadLevel::Normal => 0,
        };
        rank = rank.max(next);
    }
    match rank {
        3 => p.red,
        2 | 1 => p.amber,
        _ => p.green,
    }
}

fn bar_color(level: LoadLevel, p: MoonPalette) -> u32 {
    match level {
        LoadLevel::Critical => p.red,
        LoadLevel::Warning => p.amber,
        LoadLevel::Notice => card_attention_color(p),
        LoadLevel::Normal => p.green,
    }
}

/// Fill of the free-RAM bar, from the free-memory level rather than a share of a total.
///
/// MoonProto never reports machine-total RAM. `free / (process + free)` treated the process
/// footprint as the rest of the machine. The figure beside the bar stays [`memory_free`].
/// A missing reading is an empty bar. The three fills are a short, mid, and long mark for
/// critical, warning, and a normal reading.
fn free_bar(free_mb: Option<u16>) -> f32 {
    if free_mb.is_none() {
        return 0.0;
    }
    match free_mem_level(free_mb) {
        LoadLevel::Critical => 18.0,
        LoadLevel::Warning => 45.0,
        LoadLevel::Notice | LoadLevel::Normal => 82.0,
    }
}

#[cfg(test)]
mod tests;
