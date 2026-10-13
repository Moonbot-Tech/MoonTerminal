//! Warnings-mode table: recorded warning episodes, the ones still open first.
//!
//! A read-only log — one row per episode (CPU / memory / ping / connectivity / API key), with the
//! server and core resolved to their display names (never the raw IP). Still-open episodes lead,
//! then newest first; the columns are not sortable.

use std::collections::HashMap;
use std::net::IpAddr;
use std::rc::Rc;

use moon_core::session::CoreId;
use moon_ui::{
    MoonDataCell, MoonDataRow, MoonDataTable, MoonDataTableColumn, MoonScrollAxis,
    MoonScrollbarVisibility, MoonVirtualListScrollHandle, moon_scrollbar_overlay_with_palette,
};

use super::model::ServerStatusGroup;
use super::*;
use crate::backend::core_warn::{WarnAxis, WarnEpisode};

/// Warnings column ids in source order. A dragged order is stored under these ids.
pub(super) const ORDER_KEYS: &[&str] = &["time", "dur", "type", "peak", "where"];

/// Build the unsortable Warnings columns. `where` absorbs spare width.
///
/// Returns:
///     Ordered column descriptors.
fn columns() -> Vec<MoonDataTableColumn> {
    let mut place =
        MoonDataTableColumn::new("where", t!("core_status.warn.where").to_string(), 180.0).fill();
    place.resizable = false;

    vec![
        MoonDataTableColumn::new("time", t!("core_status.col.time").to_string(), 150.0).no_grow(),
        MoonDataTableColumn::new("dur", t!("core_status.col.dur").to_string(), 120.0)
            .right()
            .no_grow(),
        MoonDataTableColumn::new("type", t!("core_status.col.type").to_string(), 140.0).no_grow(),
        MoonDataTableColumn::new("peak", t!("core_status.col.peak").to_string(), 90.0)
            .right()
            .no_grow(),
        place,
    ]
}

/// Alarms list inputs, resolved once per panel cache rebuild rather than on every repaint.
///
/// The panel repaints on every row hover transition and every scroll tick (GPUI notifies the
/// view that built a `.hover()` row), so nothing here may be derived in `render`: the episodes
/// come from a SQLite query, and the name maps scan every server.
#[derive(Default)]
pub(super) struct AlarmsList {
    /// Episodes in display order (open first, then newest), capped at `WARN_LIST_LIMIT`.
    pub(super) episodes: Rc<Vec<WarnEpisode>>,
    /// Server display name per endpoint IP seen in [`Self::episodes`].
    pub(super) server_names: Rc<HashMap<IpAddr, String>>,
    /// Core display name per configured core id.
    pub(super) core_names: Rc<HashMap<CoreId, String>>,
}

impl AlarmsList {
    /// Resolve display names for a freshly queried episode list.
    ///
    /// Args:
    ///     episodes: Episodes in display order.
    ///     groups: Current server groups; a connected group's name wins.
    ///     saved_names: Saved custom server names keyed by IP text, the fallback.
    ///     core_names: Display name per configured core id.
    ///
    /// Returns:
    ///     The list with every server IP resolved to a display name, never the raw IP.
    pub(super) fn new(
        episodes: Vec<WarnEpisode>,
        groups: &[ServerStatusGroup],
        saved_names: &HashMap<String, String>,
        core_names: HashMap<CoreId, String>,
    ) -> Self {
        let server_names: HashMap<IpAddr, String> = episodes
            .iter()
            .filter_map(|episode| episode.server_ip)
            .map(|ip| {
                let name = groups
                    .iter()
                    .find(|group| group.address == Some(ip))
                    .map(|group| group.display_name.clone())
                    .or_else(|| saved_names.get(&ip.to_string()).cloned())
                    .unwrap_or_else(|| "—".to_string());
                (ip, name)
            })
            .collect();
        Self {
            episodes: Rc::new(episodes),
            server_names: Rc::new(server_names),
            core_names: Rc::new(core_names),
        }
    }
}

/// Render the warning-episode list table.
///
/// Args:
///     id: Stable table element identity.
///     list: Cached episodes and their resolved names.
///     state: Persisted table interaction state.
///     scroll: Row scroll handle shared with the always-visible vertical scrollbar.
///     zone: User-selected display time zone.
///     now_ms: Injected clock for an episode that is still open.
///     window: Host window; the scrollbar keeps its drag state there.
///     cx: Application context.
///
/// Returns:
///     The alarms table, or the title-and-body empty state when there are no episodes.
#[allow(clippy::too_many_arguments)]
pub(super) fn warnings_table(
    id: &'static str,
    list: &AlarmsList,
    state: &Entity<MoonDataTableState>,
    scroll: &MoonVirtualListScrollHandle,
    zone: chrono_tz::Tz,
    now_ms: i64,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    if list.episodes.is_empty() {
        return alarms_empty(cx).into_any_element();
    }
    let episodes = list.episodes.clone();
    let server_names = list.server_names.clone();
    let core_names = list.core_names.clone();
    alarms_table(
        id,
        episodes.len(),
        move |ix, _window, app| {
            warn_row(
                &episodes[ix],
                &server_names,
                &core_names,
                zone,
                now_ms,
                MoonPalette::active(app),
                app,
            )
        },
        state,
        scroll,
        window,
        cx,
    )
}

/// Host the alarms rows in a virtualized table with an always-visible vertical scrollbar.
///
/// `MoonDataTable`'s own vertical bar is opaque only while the pointer is on its track, which on
/// a 500-row log reads as no scrollbar at all. The overlay drawn here is the `Always` bar other
/// long lists in the app use, tracking the same row scroll handle.
///
/// Args:
///     id: Stable table element identity.
///     row_count: Number of rows; only the visible ones are built.
///     render_row: Builds one row by index.
///     state: Persisted table interaction state.
///     scroll: Row scroll handle the table and the bar share.
///     window: Host window.
///     cx: Application context.
///
/// Returns:
///     The hosted table.
fn alarms_table(
    id: &'static str,
    row_count: usize,
    render_row: impl Fn(usize, &mut Window, &mut App) -> MoonDataRow + 'static,
    state: &Entity<MoonDataTableState>,
    scroll: &MoonVirtualListScrollHandle,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let p = MoonPalette::active(cx);
    let bar = vertical_bar(id, scroll, window, cx);
    div()
        .relative()
        .flex_1()
        .w_full()
        .min_h(px(0.0))
        .flex()
        .flex_col()
        .child(crate::panels::common::data_table_host(
            SharedString::from(format!("{id}-host")),
            false,
            String::new(),
            p,
            cx,
            MoonDataTable::new(id, row_count, render_row)
                .columns(columns())
                .state(state)
                .track_scroll(scroll)
                .style(design::table_style(p)),
        ))
        .children(bar)
        .into_any_element()
}

/// Always-visible vertical scrollbar over the table body, below its header.
///
/// The overlay reads geometry the list writes during its own prepaint, so the first frame has
/// none; one extra frame is requested ONCE so the bar does not wait for the next repaint, and
/// a list that legitimately has no height cannot pin the window at 60 Hz.
///
/// Args:
///     id: Table id the bar's own ids derive from.
///     scroll: Row scroll handle the table tracks.
///     window: Host window holding the bar's drag state.
///     cx: Application context.
///
/// Returns:
///     The positioned bar layer; it paints a track and thumb only while the rows overflow.
fn vertical_bar(
    id: &'static str,
    scroll: &MoonVirtualListScrollHandle,
    window: &mut Window,
    cx: &mut App,
) -> Option<AnyElement> {
    let p = MoonPalette::active(cx);
    let base = scroll.0.borrow().base_handle.clone();
    let primed = window.use_keyed_state(
        ElementId::from(SharedString::from(format!("{id}:vbar-primed"))),
        cx,
        |_, _| false,
    );
    if base.bounds().size.height == px(0.0) && !*primed.read(cx) {
        primed.update(cx, |primed, _| *primed = true);
        window.request_animation_frame();
    }
    let head_h = design::table_head_h(cx);
    moon_scrollbar_overlay_with_palette(
        format!("{id}:vbar"),
        &base,
        MoonScrollAxis::Vertical,
        MoonScrollbarVisibility::Always,
        p,
        window,
        cx,
    )
    .map(|bar| {
        div()
            .id(SharedString::from(format!("{id}:vbar-box")))
            .absolute()
            .left_0()
            .right_0()
            .bottom_0()
            .top(px(head_h))
            .child(bar)
            .into_any_element()
    })
}

/// Title and body shown when the last 24 hours recorded nothing.
fn alarms_empty(cx: &App) -> impl IntoElement {
    let p = MoonPalette::active(cx);
    v_flex()
        .size_full()
        .items_center()
        .justify_center()
        .gap(design::ui_px(cx, 6.0))
        .child(
            div()
                .flex_none()
                .w(design::ui_px(cx, 40.0))
                .h(design::ui_px(cx, 40.0))
                .rounded_full()
                .flex()
                .items_center()
                .justify_center()
                .bg(design::moon_alpha(p.green, 0.18))
                .text_color(rgb(p.green))
                .font_weight(FontWeight::BOLD)
                .text_size(design::t_body_lg(cx))
                .child("✓"),
        )
        .child(
            div()
                .font_weight(FontWeight::SEMIBOLD)
                .text_size(design::t_body_lg(cx))
                .text_color(rgb(p.text))
                .child(t!("core_status.empty.no_alarms_title").to_string()),
        )
        .child(
            div()
                .text_size(design::t_body(cx))
                .text_color(rgb(p.text_muted))
                .child(t!("core_status.empty.no_alarms_body").to_string()),
        )
}

/// Duration cell text. An open episode is only the word "ongoing", in the active locale.
///
/// Args:
///     start_ms: Episode start. Used when the episode has ended.
///     end_ms: Episode end, or `None` while it is still open.
///     now_ms: Injected clock. The open cell does not print a clock.
///
/// Returns:
///     The localized ongoing word, or the closed duration.
pub(super) fn dur_text(start_ms: i64, end_ms: Option<i64>, now_ms: i64) -> String {
    if end_ms.is_none() {
        let _ = (start_ms, now_ms);
        return t!("core_status.warn_ongoing").to_string();
    }
    crate::panels::common::warn_duration_text(start_ms, end_ms)
}

/// Render one warning episode row in column order.
///
/// Args:
///     episode: Warning episode to present.
///     server_names: Display names keyed by server IP.
///     core_names: Display names keyed by core id.
///     zone: Selected IANA display zone used by the start timestamp.
///     now_ms: Injected clock passed to [`dur_text`].
///     p: Active palette. An open duration is red; an unreachable type dot is red.
///     cx: Application context used to scale the type dot.
///
/// Returns:
///     Complete warning table row.
fn warn_row(
    episode: &WarnEpisode,
    server_names: &HashMap<IpAddr, String>,
    core_names: &HashMap<CoreId, String>,
    zone: chrono_tz::Tz,
    now_ms: i64,
    p: MoonPalette,
    cx: &App,
) -> MoonDataRow {
    let server = episode
        .server_ip
        .and_then(|ip| server_names.get(&ip).cloned())
        .unwrap_or_else(|| "—".to_string());
    let core = episode
        .core_id
        .and_then(|id| core_names.get(&id).cloned())
        .unwrap_or_else(|| "—".to_string());
    let ongoing = episode.end_ms.is_none();
    let duration = dur_text(episode.start_ms, episode.end_ms, now_ms);
    let mut dur_cell = MoonDataCell::text(duration);
    if ongoing {
        dur_cell = dur_cell.text_color(p.red);
    }
    let dot = if episode.axis == WarnAxis::Unreachable {
        p.red
    } else {
        p.amber
    };
    MoonDataRow::new([
        MoonDataCell::text(moon_core::util::display_time::format_minute(
            episode.start_ms / 1000,
            zone,
        )),
        dur_cell,
        MoonDataCell::element(
            h_flex()
                .items_center()
                .gap(design::ui_px(cx, 6.0))
                .child(design::status_dot_sized(dot, 8.0, cx))
                .child(axis_label(episode.axis)),
        ),
        MoonDataCell::text(peak(episode)),
        MoonDataCell::text(where_text(&server, &core)),
    ])
}

/// Server and core display names in one cell. A missing side is omitted, never joined as a dash.
fn where_text(server: &str, core: &str) -> String {
    let server_real = !server.is_empty() && server != "—";
    let core_real = !core.is_empty() && core != "—";
    match (server_real, core_real) {
        (true, true) => format!("{server} · {core}"),
        (true, false) => server.to_string(),
        (false, true) => core.to_string(),
        (false, false) => "—".to_string(),
    }
}

/// Localized axis label (CPU / RAM / ping / exch ping / Link / key).
fn axis_label(axis: WarnAxis) -> String {
    match axis {
        WarnAxis::SysCpu => t!("core_status.chart_cpu"),
        WarnAxis::MemGrowth => t!("core_status.chart_mem"),
        WarnAxis::Ping => t!("core_status.chart_ping"),
        WarnAxis::ExchPing => t!("core_status.chart_exch"),
        WarnAxis::Unreachable => t!("core_status.warn_conn"),
        // The axis NAME, not the column heading: this list names a kind, and the heading carries a
        // unit ("АПИ (дн)") that would read as nonsense in a "Type" cell.
        WarnAxis::ApiExpiry => t!("core_status.axis_api"),
        WarnAxis::ApiQuota => t!("core_status.axis_api_quota"),
    }
    .to_string()
}

/// Peak reading with its axis unit; connectivity has none.
///
/// For the API-key axis the "peak" is the FEWEST days seen — the worst moment of an episode runs
/// downward there, not upward.
fn peak(episode: &WarnEpisode) -> String {
    match episode.axis {
        WarnAxis::SysCpu => format!("{}%", episode.peak),
        WarnAxis::MemGrowth => format!("{} {}", episode.peak, t!("core_status.mb")),
        WarnAxis::Ping | WarnAxis::ExchPing => format!("{} {}", episode.peak, t!("core_status.ms")),
        // A day count WITH its unit, unlike the panel's API column, which moved the unit into its
        // heading — this column is shared by axes measured in %, MB and ms, so each cell has to
        // carry its own. Never the "expired" word: `peak` is unsigned and clamps a negative count to
        // zero, so a key on its LAST DAY and one already dead are indistinguishable here, and
        // printing "expired" for both would label a still-valid key dead.
        WarnAxis::ApiExpiry => t!("core_status.api_days", n = episode.peak).to_string(),
        // The FEWEST requests seen, with its own unit like the rows above. `peak` is a `u16`, which
        // is why `API_QUOTA_WARN_MAX` stops at the same ceiling: a warning quota always fits, so
        // this cell never shows a truncated count.
        WarnAxis::ApiQuota => format!("{} {}", episode.peak, t!("core_status.requests")),
        WarnAxis::Unreachable => "—".to_string(),
    }
}

#[cfg(test)]
mod tests;
