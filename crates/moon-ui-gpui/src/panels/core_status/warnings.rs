//! Warnings-mode table: recorded warning episodes from the database, newest first.
//!
//! A read-only log — one row per episode (CPU / memory / ping / connectivity), with the server and core
//! resolved to their display names (never the raw IP). Ordering comes from the query (newest first),
//! so the columns are not sortable.

use std::collections::HashMap;
use std::net::IpAddr;
use std::rc::Rc;

use moon_core::session::CoreId;
use moon_ui::{MoonDataCell, MoonDataRow, MoonDataTable, MoonDataTableColumn};

use super::*;
use crate::backend::core_warn::{WarnAxis, WarnEpisode};

/// The fixed set of Warnings columns (no sorting: the query already orders newest first).
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
///     episodes: Episodes newest first.
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
        MoonDataCell::text(duration(episode)),
        MoonDataCell::text(server),
        MoonDataCell::text(core),
        MoonDataCell::text(axis_label(episode.axis)),
        MoonDataCell::text(peak(episode)),
    ])
}

/// Localized axis label (CPU / RAM / ping / exch ping / Link).
fn axis_label(axis: WarnAxis) -> String {
    match axis {
        WarnAxis::SysCpu => t!("core_status.chart_cpu"),
        WarnAxis::MemGrowth => t!("core_status.chart_mem"),
        WarnAxis::Ping => t!("core_status.chart_ping"),
        WarnAxis::ExchPing => t!("core_status.chart_exch"),
        WarnAxis::Unreachable => t!("core_status.warn_conn"),
    }
    .to_string()
}

/// Peak reading with its axis unit; connectivity has none.
fn peak(episode: &WarnEpisode) -> String {
    match episode.axis {
        WarnAxis::SysCpu => format!("{}%", episode.peak),
        WarnAxis::MemGrowth => format!("{} {}", episode.peak, t!("core_status.mb")),
        WarnAxis::Ping | WarnAxis::ExchPing => format!("{} {}", episode.peak, t!("core_status.ms")),
        WarnAxis::Unreachable => "—".to_string(),
    }
}

/// Human duration, or "ongoing" while the episode has not closed.
fn duration(episode: &WarnEpisode) -> String {
    match episode.end_ms {
        None => t!("core_status.warn_ongoing").to_string(),
        Some(end) => {
            let secs = ((end - episode.start_ms).max(0)) / 1000;
            if secs < 60 {
                t!("core_status.ago_s", n = secs).to_string()
            } else {
                t!("core_status.ago_m", n = secs / 60).to_string()
            }
        }
    }
}
