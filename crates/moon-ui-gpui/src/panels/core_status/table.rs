//! Flat-mode Core Status table: one core per row with plain, sortable columns.
//!
//! Telemetry metrics render as a dash until their `Event::KernelHealth` fields have arrived at
//! least once; the build column does the same until the core reports its build. Header clicks sort
//! through the panel, like every other data table.

use std::collections::HashMap;

use super::model::ServerKey;
use super::ordering::{FlatLine, FlatSection};
use super::presentation::{
    api_expiry_level, api_expiry_text, api_quota_level, api_quota_text, connection_presentation,
    level_color, memory_u16, percent, ping, version_behind_tooltip, version_color, version_text,
};
use super::startup::{startup_cell, startup_cell_text, startup_facts, startup_tooltip};
use super::time_offset::{tz_offset_cell, tz_offset_cell_text, tz_offset_facts, tz_offset_tooltip};
use super::*;
use crate::conn_diag::{fault_facts, fault_tooltip};
use gpui::prelude::FluentBuilder;
use moon_core::feed::{Diagnosis, diagnose};
use moon_ui::{MoonDataCell, MoonDataRow, MoonDataTable, MoonDataTableColumn};

/// Build the fixed set of sortable server, core, connection, and telemetry columns.
///
/// Returns:
///     Left-aligned identity columns followed by the right-aligned build and numeric telemetry
///     columns.
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
        // Right-aligned like the metrics: a column of 3-5 digit build numbers has to align on the
        // digit, and the one word form ("-") is short enough to sit right. It follows `status`
        // because it completes the identity block — what this core IS — rather than reporting how
        // it is doing. Mid-list insertion costs nothing: persisted widths are keyed by column key,
        // never by index.
        numeric("version", t!("core_status.col.version").to_string(), 96.0),
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
        // Right-aligned like every other metric: the cells are now bare day counts, and a column of
        // numbers has to align on the digit. The three word forms ("-", "∞", "истёк") are short
        // enough to sit right without reading oddly.
        numeric("api_key", t!("core_status.col.api_key").to_string(), 96.0),
        // Beside the key it belongs with: both answer "can this core still trade", one by date and
        // one by budget. Right-aligned like every other count, and wide enough for the seven digits
        // a HyperLiquid address reports.
        numeric(
            "api_quota",
            t!("core_status.col.api_quota").to_string(),
            110.0,
        ),
        // Left-aligned, unlike the metrics around it: the cell is a phrase ("за 8.4 с", "3/8 · 12.4 с"),
        // not a figure to align on the digit.
        MoonDataTableColumn::new("startup", t!("core_status.col.startup").to_string(), 110.0)
            .sortable(true),
        // Left-aligned like `startup`, right after it: the cell is a short phrase (`UTC+02:00`) or
        // the localized never-measured marker, not a figure to align on the digit.
        MoonDataTableColumn::new("tz_off", t!("core_status.col.tz_off").to_string(), 110.0)
            .sortable(true),
    ]
}

/// Render the telemetry table: one row per core, under a heading per exchange.
///
/// `MoonDataTable` has no group-row concept — every line it draws is a row of one uniform height —
/// so an exchange heading IS a row, and `lines` rather than `rows` is what indexes the table. That
/// makes the row index address a LINE, not a core: `state.selected_row` and every
/// `MoonDataTableEvent` index are line indices here. Nothing in this panel reads them today, but a
/// row action added later must map back through `lines` instead of indexing `rows`.
///
/// Args:
///     id: Stable table element identity.
///     rows: Immutable, already-sorted visible-core snapshot.
///     lines: Headings and member rows in render order, addressing `rows` by index.
///     server_names: Server display name per server key, for the "server" column.
///     logos_ready: Whether the off-thread logo prewarm has landed.
///     sorted: Whether a column sort is active, which the headings explain.
///     state: Persisted table interaction state.
///     cx: Panel context used for palette, empty-state localization, and the sort callback.
///
/// Returns:
///     Full-size data-table host with real, sortable columns.
#[allow(clippy::too_many_arguments)]
pub(super) fn core_status_table(
    id: &'static str,
    rows: Rc<Vec<CoreStatusRow>>,
    lines: Rc<Vec<FlatLine>>,
    server_names: Rc<HashMap<ServerKey, String>>,
    logos_ready: bool,
    sorted: bool,
    state: &Entity<MoonDataTableState>,
    cx: &Context<CoreStatusView>,
) -> impl IntoElement {
    // Keyed on the CORES, not the lines: a table holding nothing but headings is not representable
    // (a section is emitted only when it has members), and the empty message must not appear
    // beneath a heading.
    let empty = rows.is_empty();
    let row_count = lines.len();
    let table_rows = rows.clone();
    let p = MoonPalette::active(cx);
    let view = cx.entity();
    // Taken from `columns()` rather than written as a literal: a heading row must emit EXACTLY as
    // many cells as there are columns, or `MoonDataTable` skips the whole cell permutation for it.
    // Deriving the count keeps a column added elsewhere in this table a no-op for the headings.
    let section_columns = columns();

    crate::panels::common::data_table_host(
        SharedString::from(format!("{id}-host")),
        empty,
        t!("core_status.empty").to_string(),
        p,
        cx,
        MoonDataTable::new(id, row_count, move |ix, _window, app| match &lines[ix] {
            FlatLine::Section(section) => {
                // The column COUNT is all a heading needs now. It used to need the column
                // ORDER too, to pick which cell hosted the caption; the banner spans the row,
                // so which column sits leftmost stopped mattering.
                section_row(section, logos_ready, sorted, section_columns.len(), p, app)
            }
            FlatLine::Core(row) => core_status_row(&table_rows[*row], &server_names, p),
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
///     p: Active Moon palette, for the API and MoonBot cells' colour.
///
/// Returns:
///     One row in column order, with server, core, connection, build, and telemetry cells.
fn core_status_row(
    r: &CoreStatusRow,
    server_names: &HashMap<ServerKey, String>,
    p: MoonPalette,
) -> MoonDataRow {
    let sys = &r.sys;
    let server = server_names
        .get(&ServerKey::for_row(r))
        .cloned()
        .unwrap_or_default();
    // One verdict per row, derived once and shared by the status cell and its hover, so the two can
    // never state different things about the same core.
    let diag = diagnose(&r.status, r.fault.as_ref(), &r.startup);
    MoonDataRow::new([
        MoonDataCell::text(server),
        MoonDataCell::text(r.name.clone()),
        MoonDataCell::element(status_cell(r, diag.as_ref())),
        MoonDataCell::element(version_hover_cell(r, p)),
        MoonDataCell::text(percent(sys.process_cpu_percent)),
        MoonDataCell::text(percent(sys.system_cpu_percent)),
        MoonDataCell::text(memory_u16(sys.used_memory_mb)),
        MoonDataCell::text(memory_u16(sys.free_physical_memory_mb)),
        MoonDataCell::text(ping(sys.round_trip_ms)),
        MoonDataCell::text(ping(sys.order_api_latency_ms.map(u32::from))),
        MoonDataCell::text(count(sys.logical_cpu_count)),
        MoonDataCell::text(api_expiry_text(r.api_key)).text_color(level_color(
            api_expiry_level(r.api_key, r.api_warn, r.api_notice),
            p,
        )),
        MoonDataCell::text(api_quota_text(r.api_quota)).text_color(level_color(
            api_quota_level(r.api_quota, r.api_quota_warn),
            p,
        )),
        MoonDataCell::element(startup_hover_cell(r)),
        MoonDataCell::element(tz_offset_hover_cell(r)),
    ])
}

/// Left padding `MoonDataTable` puts inside every cell, mirrored from MoonUI's own
/// `MoonTableColumn::cell_pad_left` default. Only the section band needs it: it has to paint the
/// area the cell reserves for padding, which is the space between two columns.
const CELL_PAD_LEFT: f32 = 12.0;

/// Right padding `MoonDataTable` puts inside every cell, mirroring `cell_pad_right`.
const CELL_PAD_RIGHT: f32 = 8.0;

/// Render one exchange heading as a full-width band across the table.
///
/// Same tokens as the Assets panel's exchange heading, so the two read as one component — with one
/// deliberate difference: the height is the TABLE ROW height rather than Assets' 23 px, because
/// `MoonDataTable` draws a uniform-height virtual list and a heading here is one of its rows. Do
/// not "fix" that to match Assets.
///
/// The band is an ABSOLUTELY POSITIONED child of each cell rather than a plain full-size one: a
/// cell carries 12 px of left and 8 px of right padding and clips its overflow, so an in-flow child
/// would paint the content box only and leave a 20 px gap at every column boundary — the band would
/// read as dashes rather than as a stripe.
///
/// Args:
///     section: The heading's identity, caption, brand and member count.
///     logos_ready: Whether the off-thread logo prewarm has landed.
///     sorted: Whether a column sort is active, which the hover explains.
///     column_count: How many cells the row must emit; anything else disables cell ordering.
///     p: Active palette.
///     cx: Application context, for font-scaled geometry.
///
/// Returns:
///     One row whose every cell paints the band, with the caption on a row-wide banner above them.
fn section_row(
    section: &FlatSection,
    logos_ready: bool,
    sorted: bool,
    column_count: usize,
    p: MoonPalette,
    cx: &App,
) -> MoonDataRow {
    // Keyed on the venue IDENTITY, never on the caption: an element id built from rendered text
    // changes with the interface language and with a core build's spelling, which makes GPUI treat
    // one heading as a different element and drop its hover state.
    let key = match section.section {
        crate::core_order::ExchangeSection::Venue(id) => format!("{}-{}", id.code, id.dex),
        crate::core_order::ExchangeSection::Unidentified => "unknown".to_string(),
    };
    let logo = logos_ready
        .then_some(section.brand)
        .flatten()
        .and_then(crate::media::exchange_logos::exchange_logo);
    let count = t!("core_status.cores_n", n = section.members).to_string();
    // The sort arrow MoonUI draws says nothing about being section-scoped, so the hover says it.
    // The caption itself no longer needs the hover to be readable — the banner gives it the row.
    let mut hover = format!("{} - {}", section.label, count);
    if sorted {
        hover.push('\n');
        hover.push_str(&t!("core_status.section_sort_hint"));
    }

    MoonDataRow::new((0..column_count).map(|_index| {
        // Inset NEGATIVELY by the cell's own padding, which is what makes the band read as one
        // stripe: a cell pads its content by 12 px left and 8 px right and clips its overflow, so a
        // band bounded by that content box would leave 20 px unpainted at every column boundary and
        // the heading would look like separated segments. Pulling the edges back out covers the
        // padding, and the cell's `overflow_hidden` trims whatever overshoots -- so this lands
        // correctly whether the absolute box resolves against the content box or the padding box.
        //
        // The band stays PER CELL for the SAME reason it always did, and for no reason involving
        // scroll: a cell clips its own overflow, so painting the stripe inside each one is what
        // covers the 20 px of padding at every column boundary. (An earlier version of this comment
        // claimed the band had to stay per cell because a banner-wide stripe would sit still during
        // horizontal scroll. That was WRONG and three independent reviews said so: the banner and
        // the cells are children of the same natively-scrolled content subtree in
        // `MoonDataTable`, so they move together. Collapsing the band into the banner is therefore
        // an option, not a hazard — it is simply not this change.)
        MoonDataCell::element(
            div().relative().size_full().child(
                div()
                    .absolute()
                    .top_0()
                    .bottom_0()
                    .left(px(-CELL_PAD_LEFT))
                    .right(px(-CELL_PAD_RIGHT))
                    .bg(design::moon_alpha(p.panel_high, 0.72))
                    .border_b_1()
                    .border_color(rgb(p.border_soft)),
            ),
        )
    }))
    // The heading's own content spans the WHOLE row instead of living in one cell. It used to sit
    // in whichever column the user had dragged leftmost, where "BitGet Futures" was cut to
    // "BitGet Futu..." by a ~110 px column while the rest of the row sat empty -- a caption is the
    // one thing on this line that has to be readable. `MoonDataRow::banner` is MoonUI's escape from
    // the per-cell clipping that caused it, so the caption is bounded by the ROW now, and the count
    // rides the same flex line at the far end rather than needing a cell of its own.
    .banner(
        h_flex()
            .id(SharedString::from(format!("cs-exchange-{key}")))
            .size_full()
            .items_center()
            .justify_between()
            .pl(px(CELL_PAD_LEFT))
            .pr(px(CELL_PAD_RIGHT))
            .gap(design::ui_px(cx, 6.0))
            .text_size(design::t_caption(cx))
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(rgb(p.text_muted))
            .child(
                h_flex()
                    .min_w_0()
                    .items_center()
                    .gap(design::ui_px(cx, 6.0))
                    .when_some(logo, |row, logo| {
                        row.child(
                            img(logo)
                                .flex_none()
                                .w(design::ui_px(cx, 13.0))
                                .h(design::ui_px(cx, 13.0))
                                .rounded(design::ui_px(cx, 2.0)),
                        )
                    })
                    .child(div().min_w_0().truncate().child(section.label.clone())),
            )
            .child(div().flex_none().child(count))
            .tooltip(crate::panels::common::text_tooltip(hover)),
    )
}

/// The status cell: the short verdict, with reason and next step behind a hover.
///
/// The cell text alone is two words, because the column is 110 px wide; the action a user needs is
/// in the hover, which is where the rest of this panel already puts its detail. Without the hover
/// the flat table would name a cause and withhold the fix.
///
/// Args:
///     r: The row being rendered.
///     diag: That row's verdict, when one was derived.
///
/// Returns:
///     The cell element.
fn status_cell(r: &CoreStatusRow, diag: Option<&Diagnosis>) -> Stateful<Div> {
    let cell = div()
        .id(SharedString::from(format!("cs-status-{}", r.id)))
        .child(connection_presentation(&r.status, diag).label);
    match diag {
        Some(d) => cell.tooltip(crate::panels::common::text_tooltip(fault_tooltip(
            &fault_facts(d),
        ))),
        None => cell,
    }
}

/// The MoonBot build cell, carrying [`super::presentation::notice_color`] when this core is
/// behind the fleet's newest reported build, with the hover naming both builds. No warning
/// treatment beyond the colour: no triangle, no icon column, same as the by-IP tree's
/// `version_slot`.
///
/// Args:
///     r: The row being rendered.
///     p: Active Moon palette.
///
/// Returns:
///     The cell element.
fn version_hover_cell(r: &CoreStatusRow, p: MoonPalette) -> Stateful<Div> {
    div()
        .id(SharedString::from(format!("cs-version-{}", r.id)))
        .text_color(rgb(version_color(
            r.version_behind.is_some(),
            r.server_version.is_some(),
            p,
        )))
        .child(version_text(r.server_version))
        .when_some(r.version_behind, |c, newest| {
            c.tooltip(crate::panels::common::text_tooltip(version_behind_tooltip(
                r.server_version,
                newest,
            )))
        })
}

/// The startup cell, with the same structured hover the by-IP tree already shows.
///
/// The flat table used to render this figure with no hover at all, so the channel measurements that
/// explain a slow start were reachable from one presentation and not the other. Same facts, same
/// helper, one fewer place the two views disagree.
///
/// Args:
///     r: The row being rendered.
///
/// Returns:
///     The cell element.
fn startup_hover_cell(r: &CoreStatusRow) -> Stateful<Div> {
    div()
        .id(SharedString::from(format!("cs-startup-{}", r.id)))
        .child(startup_cell_text(startup_cell(&r.status, &r.startup)))
        .tooltip(crate::panels::common::text_tooltip(startup_tooltip(
            &startup_facts(&r.startup),
        )))
}

/// The tz-offset cell, with the same structured hover the by-IP tree shows for a core row.
///
/// Args:
///     r: The row being rendered.
///
/// Returns:
///     The cell element.
fn tz_offset_hover_cell(r: &CoreStatusRow) -> Stateful<Div> {
    div()
        .id(SharedString::from(format!("cs-tz-off-{}", r.id)))
        .child(tz_offset_cell_text(tz_offset_cell(&r.time_offset)))
        .tooltip(crate::panels::common::text_tooltip(tz_offset_tooltip(
            &tz_offset_facts(&r.time_offset),
        )))
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
