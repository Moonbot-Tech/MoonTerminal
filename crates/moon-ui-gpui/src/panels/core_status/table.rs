//! Core Status table columns and cells.
//!
//! A numeric metric renders as a dash until its `Event::KernelHealth` field has
//! arrived at least once.

use super::*;
use moon_ui::{MoonDataCell, MoonDataRow, MoonDataTable, MoonDataTableColumn};

/// Build columns for core name, connection state, process and system CPU,
/// process memory, free physical memory, logical CPU count, and sample age.
fn columns() -> Vec<MoonDataTableColumn> {
    let numeric =
        |key: &'static str, title: String, w: f32| MoonDataTableColumn::new(key, title, w).right();
    vec![
        MoonDataTableColumn::new("core", t!("core_status.col.core").to_string(), 130.0),
        MoonDataTableColumn::new("status", t!("core_status.col.status").to_string(), 120.0),
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
        numeric("cpus", t!("core_status.col.cpus").to_string(), 90.0),
        numeric("updated", t!("core_status.col.updated").to_string(), 100.0),
    ]
}

pub(super) fn core_status_table(
    id: &'static str,
    rows: Rc<Vec<CoreStatusRow>>,
    now_ms: i64,
    state: &Entity<MoonDataTableState>,
    cx: &Context<CoreStatusView>,
) -> impl IntoElement {
    let empty = rows.is_empty();
    let row_count = rows.len();
    let table_rows = rows.clone();
    let p = MoonPalette::active(cx);

    crate::panels::common::data_table_host(
        SharedString::from(format!("{id}-host")),
        empty,
        t!("core_status.empty").to_string(),
        p,
        cx,
        MoonDataTable::new(id, row_count, move |ix, _window, _app| {
            core_status_row(&table_rows[ix], now_ms, p)
        })
        .columns(columns())
        .state(state)
        .header_height(design::TABLE_HEAD_H)
        .row_height(design::TABLE_ROW_H),
    )
}

/// Render one core and its latest telemetry sample in the table's column order.
fn core_status_row(r: &CoreStatusRow, now_ms: i64, p: MoonPalette) -> MoonDataRow {
    let sys = &r.sys;
    MoonDataRow::new([
        MoonDataCell::text(r.name.clone()),
        MoonDataCell::element(status_cell(&r.status, p)),
        MoonDataCell::text(pct(sys.process_cpu_percent)),
        MoonDataCell::text(pct(sys.system_cpu_percent)),
        MoonDataCell::text(mb(sys.used_memory_mb)),
        MoonDataCell::text(mb(sys.free_physical_memory_mb)),
        MoonDataCell::text(count(sys.logical_cpu_count)),
        MoonDataCell::text(ago(sys.updated_ms, now_ms)),
    ])
}

/// Ячейка статуса: цветной кружок (зелёный=Ready, янтарь=подключение, красный=ошибка,
/// серый=нет связи) + локализованная подпись. Порт `status_dot` из настроек подключений.
fn status_cell(status: &ConnStatus, p: MoonPalette) -> impl IntoElement + 'static {
    let (color, label) = match status {
        ConnStatus::Ready => (p.green, t!("conn.status.ready").to_string()),
        ConnStatus::Connecting => (p.amber, t!("conn.status.connecting").to_string()),
        ConnStatus::Stage(s) => (
            p.amber,
            t!("conn.status.stage", stage = s.clone()).to_string(),
        ),
        ConnStatus::Failed(e) => (p.red, t!("conn.status.failed", err = e.clone()).to_string()),
        ConnStatus::Disconnected => (p.text_soft, t!("conn.status.disconnected").to_string()),
    };
    h_flex()
        .w_full()
        .h_full()
        .items_center()
        .gap_2()
        .child(
            div()
                .w(px(8.0))
                .h(px(8.0))
                .flex_none()
                .rounded_full()
                .bg(rgb(color)),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .text_color(rgb(p.text_soft))
                .child(label),
        )
}

/// Format an integer `KernelHealth` CPU percentage, or a dash when unavailable.
fn pct(v: Option<u8>) -> String {
    v.map(|v| format!("{v}%"))
        .unwrap_or_else(|| "—".to_string())
}

/// Format an integer megabyte value with its localized unit, or a dash.
fn mb(v: Option<u16>) -> String {
    v.map(|v| format!("{v} {}", t!("core_status.mb")))
        .unwrap_or_else(|| "—".to_string())
}

/// Format a logical CPU count without a unit, or a dash when unavailable.
fn count(v: Option<u8>) -> String {
    v.map(|v| v.to_string()).unwrap_or_else(|| "—".to_string())
}

/// Format the age of the latest telemetry sample for the narrow right column.
/// Values under a minute use seconds, values under an hour use minutes, and
/// older values use hours. A zero or future timestamp renders as a dash. Bind
/// the number locally because rust-i18n path arguments handle identifiers more
/// reliably than expressions.
fn ago(last_ms: i64, now_ms: i64) -> String {
    if last_ms <= 0 {
        return "—".to_string();
    }
    let secs = ((now_ms - last_ms) / 1000).max(0);
    if secs < 60 {
        let n = secs;
        t!("core_status.ago_s", n = n).to_string()
    } else if secs < 3600 {
        let n = secs / 60;
        t!("core_status.ago_m", n = n).to_string()
    } else {
        let n = secs / 3600;
        t!("core_status.ago_h", n = n).to_string()
    }
}
