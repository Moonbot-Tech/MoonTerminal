//! Warnings-mode table: recorded warning episodes, the ones still open first.
//!
//! A read-only log — one row per episode (CPU / memory / ping / connectivity / API key), with the
//! server and core resolved to their display names (never the raw IP). Still-open episodes lead,
//! then newest first; the columns are not sortable.

use std::collections::HashMap;
use std::net::IpAddr;
use std::rc::Rc;

use moon_core::session::CoreId;
use moon_ui::{MoonDataCell, MoonDataRow, MoonDataTable, MoonDataTableColumn};

use super::*;
use crate::backend::core_warn::{WarnAxis, WarnEpisode};

/// The fixed set of Warnings columns (no sorting: the backend already orders the rows).
fn columns() -> Vec<MoonDataTableColumn> {
    vec![
        MoonDataTableColumn::new("time", t!("core_status.col.time").to_string(), 150.0),
        MoonDataTableColumn::new("dur", t!("core_status.col.dur").to_string(), 72.0).right(),
        MoonDataTableColumn::new("server", t!("core_status.col.server").to_string(), 110.0),
        MoonDataTableColumn::new("core", t!("core_status.col.core").to_string(), 130.0),
        MoonDataTableColumn::new("type", t!("core_status.col.type").to_string(), 80.0),
        MoonDataTableColumn::new("peak", t!("core_status.col.peak").to_string(), 90.0).right(),
    ]
}

/// Render the warning-episode list table.
///
/// Args:
///     id: Stable table element identity.
///     episodes: Episodes in display order (open first, then newest).
///     server_names: Server display name per endpoint IP.
///     core_names: Core display name per core id.
///     state: Persisted table interaction state.
///     cx: Panel context.
///
/// Returns:
///     A full-size data-table host, or the localized empty state.
pub(super) fn warnings_table(
    id: &'static str,
    episodes: Rc<Vec<WarnEpisode>>,
    server_names: Rc<HashMap<IpAddr, String>>,
    core_names: Rc<HashMap<CoreId, String>>,
    state: &Entity<MoonDataTableState>,
    cx: &Context<CoreStatusView>,
) -> impl IntoElement {
    let empty = episodes.is_empty();
    let row_count = episodes.len();
    let p = MoonPalette::active(cx);

    crate::panels::common::data_table_host(
        SharedString::from(format!("{id}-host")),
        empty,
        t!("core_status.warn_empty").to_string(),
        p,
        cx,
        MoonDataTable::new(id, row_count, move |ix, _window, _app| {
            warn_row(&episodes[ix], &server_names, &core_names)
        })
        .columns(columns())
        .state(state)
        .header_height(design::TABLE_HEAD_H)
        .row_height(design::TABLE_ROW_H),
    )
}

/// Render one episode row in column order.
fn warn_row(
    episode: &WarnEpisode,
    server_names: &HashMap<IpAddr, String>,
    core_names: &HashMap<CoreId, String>,
) -> MoonDataRow {
    let server = episode
        .server_ip
        .and_then(|ip| server_names.get(&ip).cloned())
        .unwrap_or_else(|| "—".to_string());
    let core = episode
        .core_id
        .and_then(|id| core_names.get(&id).cloned())
        .unwrap_or_else(|| "—".to_string());
    MoonDataRow::new([
        MoonDataCell::text(moon_core::db::fmt_unix(episode.start_ms / 1000)),
        MoonDataCell::text(crate::panels::common::warn_duration_text(
            episode.start_ms,
            episode.end_ms,
        )),
        MoonDataCell::text(server),
        MoonDataCell::text(core),
        MoonDataCell::text(axis_label(episode.axis)),
        MoonDataCell::text(peak(episode)),
    ])
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
        WarnAxis::Unreachable => "—".to_string(),
    }
}
