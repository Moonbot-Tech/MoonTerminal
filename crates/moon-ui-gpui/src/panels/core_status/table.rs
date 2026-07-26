//! Responsive Flat-mode Core Status rows.
//!
//! A numeric metric renders as a dash until its `Event::KernelHealth` field has
//! arrived at least once.

use super::presentation::{
    connection_presentation, memory_u16, normal_section_rule, optional_u8, percent,
};
use super::*;
use moon_ui::{MoonDataCell, MoonDataRow, MoonDataTable, MoonDataTableColumn};

/// Build one fill column so Flat mode never creates a horizontal scrollbar.
///
/// Returns:
///     A single responsive column whose cell owns both text lines.
fn columns() -> Vec<MoonDataTableColumn> {
    vec![MoonDataTableColumn::new("core", t!("core_status.col.core").to_string(), 80.0).fill()]
}

/// Render the responsive one-row-per-core presentation.
///
/// Args:
///     id: Stable table element identity.
///     rows: Immutable visible-core snapshot.
///     now_ms: Current Unix milliseconds used for sample age.
///     state: Persisted table interaction state.
///     cx: Panel context used for palette and empty-state localization.
///
/// Returns:
///     Full-size data-table host with no horizontal overflow requirement.
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
    let normal_section_start = super::model::normal_core_boundary(&rows);

    crate::panels::common::data_table_host(
        SharedString::from(format!("{id}-host")),
        empty,
        t!("core_status.empty").to_string(),
        p,
        cx,
        MoonDataTable::new(id, row_count, move |ix, _window, app| {
            core_status_row(
                &table_rows[ix],
                now_ms,
                normal_section_start == Some(ix),
                p,
                app,
            )
        })
        .columns(columns())
        .state(state)
        .header_height(design::TABLE_HEAD_H)
        .row_height(48.0),
    )
}

/// Render one core and its latest telemetry sample as two responsive lines.
///
/// Args:
///     r: Cached core snapshot.
///     now_ms: Current Unix milliseconds used for sample age.
///     normal_section_start: Whether this row starts the Ready section.
///     p: Active Moon palette.
///     cx: Application context used to scale the shared status dot.
///
/// Returns:
///     One single-cell table row whose secondary metric line clips at narrow widths.
fn core_status_row(
    r: &CoreStatusRow,
    now_ms: i64,
    normal_section_start: bool,
    p: MoonPalette,
    cx: &App,
) -> MoonDataRow {
    let sys = &r.sys;
    MoonDataRow::new([MoonDataCell::element(
        v_flex()
            .relative()
            .w_full()
            .min_w_0()
            .overflow_hidden()
            .gap(px(2.0))
            .children(normal_section_start.then(|| normal_section_rule(p)))
            .child(
                h_flex()
                    .w_full()
                    .min_w_0()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_color(rgb(p.text))
                            .child(r.name.clone()),
                    )
                    .child(status_cell(&r.status, p, cx)),
            )
            .child(
                div()
                    .w_full()
                    .min_w_0()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_color(rgb(p.text_muted))
                    .child(format!(
                        "{} {}  {} {}  {} {}  {} {}  {} {}  {} {}",
                        t!("core_status.col.cpu_proc"),
                        percent(sys.process_cpu_percent),
                        t!("core_status.col.cpu_sys"),
                        percent(sys.system_cpu_percent),
                        t!("core_status.col.mem_used"),
                        memory_u16(sys.used_memory_mb),
                        t!("core_status.col.free_phys"),
                        memory_u16(sys.free_physical_memory_mb),
                        t!("core_status.col.cpus"),
                        optional_u8(sys.logical_cpu_count),
                        t!("core_status.col.updated"),
                        ago(sys.updated_ms, now_ms)
                    )),
            ),
    )])
}

/// Render a localized connection state as a colored dot and label.
///
/// Ready is green, Connecting and Stage are amber, Failed is red, and Disconnected
/// is gray. This is the table-cell counterpart of connection settings' `status_dot`.
///
/// Args:
///     status: Current core connection state.
///     p: Active Moon palette.
///     cx: Application context used to scale the shared status dot.
///
/// Returns:
///     Compact colored dot and localized label.
fn status_cell(
    status: &ConnStatus,
    p: MoonPalette,
    cx: &App,
) -> impl IntoElement + use<> + 'static {
    let status = connection_presentation(status, p);
    h_flex()
        .min_w_0()
        .items_center()
        .gap_2()
        .child(crate::design::status_dot(status.color, cx))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .text_color(rgb(p.text_soft))
                .child(status.label),
        )
}

/// Format the age of the latest telemetry sample for the responsive metric line.
/// Values under a minute use seconds, values under an hour use minutes, and
/// older values use hours. A non-positive timestamp renders as a dash; a future
/// timestamp is clamped to zero seconds. Bind the number locally because
/// rust-i18n path arguments handle identifiers more reliably than expressions.
///
/// Args:
///     last_ms: Unix milliseconds of the latest telemetry sample.
///     now_ms: Current Unix milliseconds.
///
/// Returns:
///     Localized compact age text or an ASCII unavailable marker.
fn ago(last_ms: i64, now_ms: i64) -> String {
    if last_ms <= 0 {
        return "-".to_string();
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
