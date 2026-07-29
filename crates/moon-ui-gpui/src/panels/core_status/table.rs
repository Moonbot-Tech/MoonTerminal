//! Flat-mode Core Status table: one core per row with plain, sortable columns.
//!
//! A numeric metric renders as a dash until its `Event::KernelHealth` field has arrived at least
//! once. Header clicks sort through the panel, like every other data table.

use std::collections::HashMap;

use super::model::ServerKey;
use super::presentation::{connection_presentation, memory_u16, percent, ping};
use super::*;
use moon_ui::{MoonDataCell, MoonDataRow, MoonDataTable, MoonDataTableColumn};

/// Build the fixed set of sortable server, core, connection, and telemetry columns.
///
/// Returns:
///     Left-aligned identity columns followed by right-aligned numeric metric columns.
fn columns() -> Vec<MoonDataTableColumn> {
    let numeric = |key: &'static str, title: String, w: f32| {
        MoonDataTableColumn::new(key, title, w)
            .right()
            .sortable(true)
    };
    vec![
        MoonDataTableColumn::new("server", t!("core_status.col.server").to_string(), 110.0)
            .sortable(true),
        MoonDataTableColumn::new("core", t!("core_status.col.core").to_string(), 130.0)
            .sortable(true),
        MoonDataTableColumn::new("status", t!("core_status.col.status").to_string(), 110.0)
            .sortable(true),
        numeric("cpu_proc", t!("core_status.col.cpu_proc").to_string(), 90.0),
        numeric("cpu_sys", t!("core_status.col.cpu_sys").to_string(), 90.0),
        numeric(
            "mem_used",
            t!("core_status.col.mem_used").to_string(),
            100.0,
        ),
        numeric(
            "free_phys",
            t!("core_status.col.free_phys").to_string(),
            110.0,
        ),
        numeric("ping", t!("core_status.col.ping").to_string(), 84.0),
        numeric(
            "ping_exch",
            t!("core_status.col.ping_exch").to_string(),
            96.0,
        ),
        numeric("cpus", t!("core_status.col.cpus").to_string(), 80.0),
    ]
}

/// Render the one-row-per-core telemetry table.
///
/// Args:
///     id: Stable table element identity.
///     rows: Immutable, already-sorted visible-core snapshot.
///     server_names: Server display name per server key, for the "server" column.
///     state: Persisted table interaction state.
///     cx: Panel context used for palette, empty-state localization, and the sort callback.
///
/// Returns:
///     Full-size data-table host with real, sortable columns.
pub(super) fn core_status_table(
    id: &'static str,
    rows: Rc<Vec<CoreStatusRow>>,
    server_names: Rc<HashMap<ServerKey, String>>,
    state: &Entity<MoonDataTableState>,
    cx: &Context<CoreStatusView>,
) -> impl IntoElement {
    let empty = rows.is_empty();
    let row_count = rows.len();
    let table_rows = rows.clone();
    let p = MoonPalette::active(cx);
    let view = cx.entity();

    crate::panels::common::data_table_host(
        SharedString::from(format!("{id}-host")),
        empty,
        t!("core_status.empty").to_string(),
        p,
        cx,
        MoonDataTable::new(id, row_count, move |ix, _window, _app| {
            core_status_row(&table_rows[ix], &server_names)
        })
        .columns(columns())
        .state(state)
        .header_height(design::TABLE_HEAD_H)
        .row_height(design::TABLE_ROW_H)
        .on_sort(move |key, ascending, _window, app| {
            let key = key.to_string();
            view.update(app, |this, cx| this.set_flat_sort(&key, ascending, cx));
        }),
    )
}

/// Render one core and its latest telemetry sample in the table's column order.
///
/// Args:
///     r: Cached core snapshot.
///     server_names: Server display name per server key.
///
/// Returns:
///     One row with server, core, connection, and the numeric metric cells (CPU, memory, both
///     pings, logical CPUs).
fn core_status_row(r: &CoreStatusRow, server_names: &HashMap<ServerKey, String>) -> MoonDataRow {
    let sys = &r.sys;
    let server = server_names
        .get(&ServerKey::for_row(r))
        .cloned()
        .unwrap_or_default();
    MoonDataRow::new([
        MoonDataCell::text(server),
        MoonDataCell::text(r.name.clone()),
        MoonDataCell::text(connection_presentation(&r.status).label),
        MoonDataCell::text(percent(sys.process_cpu_percent)),
        MoonDataCell::text(percent(sys.system_cpu_percent)),
        MoonDataCell::text(memory_u16(sys.used_memory_mb)),
        MoonDataCell::text(memory_u16(sys.free_physical_memory_mb)),
        MoonDataCell::text(ping(sys.round_trip_ms)),
        MoonDataCell::text(ping(sys.order_api_latency_ms.map(u32::from))),
        MoonDataCell::text(count(sys.logical_cpu_count)),
    ])
}

/// Format an optional logical-CPU count.
///
/// Args:
///     value: Logical CPU count from `Event::KernelHealth`.
///
/// Returns:
///     Decimal text or an ASCII unavailable marker.
fn count(value: Option<u8>) -> String {
    value
        .map(|value| value.to_string())
        .unwrap_or_else(|| "-".to_string())
}
