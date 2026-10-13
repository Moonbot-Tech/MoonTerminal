//! Flat-mode Core Status table: one core per row with plain, sortable columns.
//!
//! Telemetry metrics render as a dash until their `Event::KernelHealth` fields have arrived at
//! least once; the build column does the same until the core reports its build. Header clicks sort
//! through the panel, like every other data table.

use moon_core::session::core_order;
use std::collections::{HashMap, HashSet};

use super::columns_fit;
use super::model::{ApiKeyState, ServerKey};
use super::ordering::{self, FlatLine, FlatSection};
use super::presentation::{
    LoadLevel, api_expiry_level, api_expiry_text, api_expiry_tooltip, api_quota_level,
    api_quota_text, build_parts, card_attention_color, connection_presentation, cpu_level,
    free_mem_level, lat_level, level_color, memory_u16, percent, ping, ping_cell_level,
    status_level, update_badge, version_behind_tooltip, version_color,
};
use super::startup::{
    startup_cell, startup_cell_text, startup_facts, startup_level, startup_tooltip,
};
use super::time_offset::{tz_offset_cell, tz_offset_cell_text, tz_offset_facts, tz_offset_tooltip};
use super::update_menu;
use super::version_mark::version_mark;
use super::*;
use crate::conn_diag::{fault_facts, fault_short, fault_tooltip};
use crate::controls::core_update::{self, OfferCounts};
use crate::controls::row_selection::RowSelection;
use gpui::prelude::FluentBuilder;
use moon_core::feed::{Diagnosis, FailureClass, diagnose};
use moon_core::session::core_update::{CoreUpdateOutcome, CoreUpdatePhase};
use moon_ui::{
    MoonBadge, MoonBadgeVariant, MoonDataCell, MoonDataRow, MoonDataTable, MoonDataTableColumn,
    MoonProgress, MoonTone,
};

/// Canonical Flat column order before data-only API columns are filtered.
const COLUMN_KEYS: [&str; 15] = [
    "status",
    "core",
    "server",
    "version",
    "ping",
    "ping_exch",
    "cpu_proc",
    "cpu_sys",
    "cpus",
    "mem_used",
    "free_phys",
    "api_key",
    "api_quota",
    "startup",
    "tz_off",
];

/// Every Flat column id, in source order, including columns hidden until a core reports them.
///
/// Persistence merges against this full set. Merging against only the columns on screen would drop
/// `api_key` while no row has one, then append it the next time a key appears.
///
/// Returns:
///     The canonical Flat column ids.
pub(super) fn order_keys() -> &'static [&'static str] {
    &COLUMN_KEYS
}

/// Decide whether one Flat column has information to display in the visible rows.
///
/// Args:
///     key: Canonical Flat column key.
///     rows: Current visible-core snapshots.
///
/// Returns:
///     Whether the column belongs in the rendered table.
fn column_visible(key: &str, rows: &[CoreStatusRow]) -> bool {
    match key {
        "api_key" => rows
            .iter()
            .any(|row| matches!(row.api_key, ApiKeyState::Days(_))),
        "api_quota" => rows.iter().any(|row| row.api_quota.is_some()),
        _ => true,
    }
}

/// Return the canonical Flat column keys that still belong in the table.
///
/// Args:
///     rows: Current visible-core snapshots. Data-empty API columns are decided from these.
///     hidden_by_user: Column ids unchecked in the Columns menu. Empty keeps the data rules only.
///     uniform: Column ids every reporting core agrees on. Those move to the same-for-all line.
///         Empty keeps every key the other two rules kept.
///
/// Returns:
///     Ordered keys shared by column descriptors and every rendered row.
pub(super) fn visible_column_keys(
    rows: &[CoreStatusRow],
    hidden_by_user: &HashSet<String>,
    uniform: &[&str],
) -> Vec<&'static str> {
    COLUMN_KEYS
        .into_iter()
        .filter(|key| column_visible(key, rows))
        .filter(|key| !hidden_by_user.contains(*key))
        .filter(|key| !uniform.contains(key))
        .collect()
}

/// Column keys the Flat table draws, after the width drop.
///
/// Data rules, the Columns menu and the same-for-all line run first. The measured dock width
/// then removes [`columns_fit::DROP_TIERS`] from that list. Header and rows both take this vec,
/// so they cannot disagree. A width of `0.5` or less means the canvas has not measured yet and
/// drops nothing: the first frame would otherwise flash a stripped table.
///
/// Args:
///     rows: Current visible-core snapshots.
///     hidden_by_user: Column ids unchecked in the Columns menu.
///     uniform: Column ids every reporting core agrees on.
///     dragged: User-dragged widths keyed by column id. Missing keys use the defaults.
///     available: Measured table width in pixels, or `0` before the first measure.
///     floors: Minimum content width per column, including the 12+8 px cell padding.
///         Missing keys contribute 0. The drop uses the wider of this and the current width.
///
/// Returns:
///     Ordered keys shared by column descriptors and every rendered row.
pub(super) fn projected_column_keys(
    rows: &[CoreStatusRow],
    hidden_by_user: &HashSet<String>,
    uniform: &[&str],
    dragged: &HashMap<String, f32>,
    available: f32,
    floors: &HashMap<String, f32>,
) -> Vec<&'static str> {
    let keys = visible_column_keys(rows, hidden_by_user, uniform);
    if available <= 0.5 {
        return keys;
    }
    let measured: Vec<(&str, f32)> = keys
        .iter()
        .map(|key| (*key, width_for_drop(key, dragged, floors)))
        .collect();
    let level = columns_fit::drop_level(&measured, available);
    columns_fit::apply_drop(&keys, level)
}

/// Host whose measured width is the dock slot, not the table's content width.
///
/// The wrapper and its child both take a zero minimum width and hide overflow, so a wide row
/// cannot expand the slot, get measured as wide, and then put the dropped columns back.
///
/// Args:
///     table: The flat table, already built for the current drop level.
///     on_width: Called with the host width in pixels during prepaint.
///
/// Returns:
///     The measuring host. It fills its parent.
pub(super) fn flat_measure_host(
    table: impl IntoElement,
    on_width: impl Fn(f32, &mut App) + 'static,
) -> Div {
    div()
        .size_full()
        .min_w(px(0.0))
        .overflow_hidden()
        .relative()
        .debug_selector(|| "cs-flat-measure".into())
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
                .child(table),
        )
}

/// Width one column contributes to the drop decision.
///
/// A dragged width wins. Otherwise the same default [`columns`] declares, so the fit test and
/// the table cannot use two different numbers for one key.
fn column_width(key: &str, dragged: &HashMap<String, f32>) -> f32 {
    dragged
        .get(key)
        .copied()
        .unwrap_or_else(|| default_column_width(key))
}

/// Width one column contributes to the drop decision.
///
/// The current width is the drag, or the declared default. The content floor is the widest
/// figure plus cell padding. The drop keeps whichever is wider, so a figure that does not fit
/// the declared slot can push a tier out instead of being clipped.
///
/// Args:
///     key: Canonical column id.
///     dragged: User-dragged widths. Missing keys use the defaults.
///     floors: Measured content floors. Missing keys contribute 0.
///
/// Returns:
///     The width, in the same pixels the table uses for column descriptors.
pub(super) fn width_for_drop(
    key: &str,
    dragged: &HashMap<String, f32>,
    floors: &HashMap<String, f32>,
) -> f32 {
    column_width(key, dragged).max(floors.get(key).copied().unwrap_or(0.0))
}

/// Unscaled font size `MoonTableCell` draws when the caller does not set one.
const CELL_FONT: f32 = 10.5;

/// Weight of that default cell face.
const CELL_WEIGHT: f32 = 400.0;

/// Widest rendered figure of each column, plus the table's 12+8 px cell padding.
///
/// Name, server and the status dot are omitted: those cells truncate or are not a figure.
/// The result is cached by the caller on the row generation and the locale, because the
/// strings move with the language and with the snapshot, not with the frame.
///
/// Args:
///     rows: Rows whose figures can appear. An empty slice yields an empty map.
///     cx: Application context used to measure the mono cell face.
///
/// Returns:
///     Column id to the minimum width that keeps its widest figure inside the padding.
pub(super) fn content_floors(rows: &[CoreStatusRow], cx: &App) -> HashMap<String, f32> {
    COLUMN_KEYS
        .into_iter()
        .filter_map(|key| {
            let mut widest = 0.0_f32;
            for row in rows {
                let text = figure_for_floor(row, key);
                if text.is_empty() {
                    continue;
                }
                let width = crate::design::ui_text_width(cx, &text, CELL_FONT, CELL_WEIGHT, true);
                widest = widest.max(width);
            }
            (widest > 0.0).then_some((key.to_string(), widest + CELL_PAD_LEFT + CELL_PAD_RIGHT))
        })
        .collect()
}

/// The string whose width is the column's content floor.
///
/// Identity columns that truncate, and the status dot, contribute nothing. The build column
/// measures the joined number and letter. Every other key measures the same text the cell draws.
fn figure_for_floor(row: &CoreStatusRow, key: &str) -> String {
    match key {
        "server" | "core" | "status" => String::new(),
        "version" => super::presentation::build_label(&build_parts(
            row.server_version,
            row.server_version_suffix.as_deref(),
        )),
        _ => cell_text_for(row, key),
    }
}

/// Raise each descriptor to its content floor, then spend only the slack above those floors
/// when the dock is narrower than the declared sum.
///
/// `MoonDataTable`'s Fit policy otherwise scales every column toward a shared 40 px minimum,
/// which cuts a retained ping below the figure the drop decision just reserved. After this
/// pass the widths sum to the dock whenever the floors themselves fit, so Fit has nothing
/// left to shrink.
///
/// Args:
///     columns: Descriptors at their declared widths, in display order.
///     floors: Content floors from [`content_floors`]. Missing keys stay at the declared width.
///     available: Measured dock width. `0` leaves the declared widths alone.
///
/// Returns:
///     The same descriptors with widths that are at least the floor and, when possible, fit.
pub(super) fn respect_content_floors(
    mut columns: Vec<MoonDataTableColumn>,
    floors: &HashMap<String, f32>,
    available: f32,
) -> Vec<MoonDataTableColumn> {
    for column in &mut columns {
        let floor = floors.get(column.key.as_ref()).copied().unwrap_or(0.0);
        column.width = column.width.max(floor);
    }
    if available <= 0.5 {
        return columns;
    }
    let sum: f32 = columns.iter().map(|column| column.width).sum();
    if sum <= available + 0.5 {
        return columns;
    }
    let floor_of =
        |column: &MoonDataTableColumn| floors.get(column.key.as_ref()).copied().unwrap_or(0.0);
    let floor_sum: f32 = columns.iter().map(floor_of).sum();
    if floor_sum >= available {
        for column in &mut columns {
            let floor = floor_of(column);
            if floor > 0.0 {
                column.width = floor;
            }
        }
        return columns;
    }
    let slack_total = (sum - floor_sum).max(0.0);
    if slack_total <= 0.0 {
        return columns;
    }
    let extra = available - floor_sum;
    let mut used = 0.0_f32;
    let last = columns.len().saturating_sub(1);
    for (index, column) in columns.iter_mut().enumerate() {
        let floor = floor_of(column);
        let slack = (column.width - floor).max(0.0);
        let width = if index == last {
            (available - used).max(floor)
        } else {
            floor + extra * (slack / slack_total)
        };
        column.width = width;
        used += width;
    }
    columns
}

/// Declared width of one Flat column, in logical pixels.
///
/// The status column is a dot, so it stays narrow and does not grow. The core column is wider
/// than the old name column because a problem reason sits beside the name. The CPU columns are
/// wide enough for a mini bar plus the percent.
fn default_column_width(key: &str) -> f32 {
    match key {
        "status" => 36.0,
        "core" => 180.0,
        "server" => 110.0,
        "version" => 128.0,
        "cpu_proc" | "cpu_sys" => 120.0,
        "mem_used" => 100.0,
        "free_phys" => 110.0,
        "ping" => 84.0,
        "ping_exch" => 96.0,
        "cpus" => 80.0,
        "api_key" => 96.0,
        "api_quota" => 110.0,
        "startup" | "tz_off" => 110.0,
        _ => 80.0,
    }
}

/// Build the selected sortable server, core, connection, and telemetry columns.
///
/// Args:
///     keys: Canonically ordered visible-column keys.
///
/// Returns:
///     Left-aligned identity, build, and CPU columns. Pings, both memories, the API key,
///     and startup are right-aligned. The CPU count and the API quota stay centred.
fn columns(keys: &[&str]) -> Vec<MoonDataTableColumn> {
    let column = |key: &'static str, title: String, align: MetricAlign| {
        let column = MoonDataTableColumn::new(key, title, default_column_width(key)).sortable(true);
        match align {
            MetricAlign::Start => column,
            MetricAlign::Center => column.center(),
            MetricAlign::End => column.right(),
        }
    };
    keys.iter()
        .map(|key| match *key {
            "server" => MoonDataTableColumn::new(
                "server",
                t!("core_status.col.server").to_string(),
                default_column_width("server"),
            )
            .sortable(true),
            "core" => MoonDataTableColumn::new(
                "core",
                t!("core_status.col.core").to_string(),
                default_column_width("core"),
            )
            .sortable(true),
            // A dot, not a sentence. The full word stays on the header tooltip because 36 px
            // cannot show it. `no_grow` keeps the extra dock width for the name and the metrics.
            "status" => MoonDataTableColumn::new(
                "status",
                t!("core_status.col.status").to_string(),
                default_column_width("status"),
            )
            .center()
            .no_grow()
            .sortable(true)
            .tooltip(t!("core_status.col.status").to_string()),
            // Left-aligned. The letter tag keeps a reserved slot, so a named build does not
            // shift the digits. Wide enough for that slot plus the update glyph and the hover
            // button. It follows `status` because it completes the identity block — what this
            // core IS — rather than reporting how it is doing. Mid-list insertion costs nothing:
            // persisted widths are keyed by column key, never by index.
            "version" => column(
                "version",
                t!("core_status.col.version").to_string(),
                MetricAlign::Start,
            ),
            "cpu_proc" => column(
                "cpu_proc",
                t!("core_status.col.cpu_proc").to_string(),
                MetricAlign::Start,
            ),
            "cpu_sys" => column(
                "cpu_sys",
                t!("core_status.col.cpu_sys").to_string(),
                MetricAlign::Start,
            ),
            "mem_used" => column(
                "mem_used",
                t!("core_status.col.mem_used").to_string(),
                MetricAlign::End,
            ),
            "free_phys" => column(
                "free_phys",
                t!("core_status.col.free_phys").to_string(),
                MetricAlign::End,
            ),
            "ping" => column(
                "ping",
                t!("core_status.col.ping").to_string(),
                MetricAlign::End,
            ),
            "ping_exch" => column(
                "ping_exch",
                t!("core_status.col.ping_exch").to_string(),
                MetricAlign::End,
            ),
            "cpus" => column(
                "cpus",
                t!("core_status.col.cpus").to_string(),
                MetricAlign::Center,
            )
            .tooltip(t!("core_status.col_tip.cpus").to_string()),
            // Right-aligned: the cells are bare day counts, and a column of numbers aligns on
            // the digit. The three word forms ("-", "∞", "expired") are short enough to sit
            // right without reading oddly.
            "api_key" => column(
                "api_key",
                t!("core_status.col.api_key").to_string(),
                MetricAlign::End,
            )
            .tooltip(t!("core_status.col_tip.api_key").to_string()),
            // Beside the key it belongs with: both answer "can this core still trade", one by date and
            // one by budget. Centred, and wide enough for the seven digits a HyperLiquid address reports.
            "api_quota" => column(
                "api_quota",
                t!("core_status.col.api_quota").to_string(),
                MetricAlign::Center,
            ),
            // Right-aligned with the other figures. The cell is a short phrase ("8.4 s", "3/8 · 12.4 s").
            "startup" => column(
                "startup",
                t!("core_status.col.startup").to_string(),
                MetricAlign::End,
            ),
            // Left-aligned, right after startup: the cell is a short phrase (`UTC+02:00`) or
            // the localized never-measured marker, not a figure to align on the digit.
            "tz_off" => MoonDataTableColumn::new(
                "tz_off",
                t!("core_status.col.tz_off").to_string(),
                default_column_width("tz_off"),
            )
            .sortable(true)
            .tooltip(t!("core_status.col_tip.tz_off").to_string()),
            _ => unreachable!("canonical Flat column key"),
        })
        .collect()
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
///     column_keys: Exact ordered keys used for both descriptors and row cells.
///     server_names: Server display name per server key, for the "server" column.
///     logos_ready: Whether the off-thread logo prewarm has landed.
///     sorted: Whether a column sort is active, which the headings explain.
///     state: Persisted table interaction state.
///     backend: Shared terminal backend, handed to row builders that must not read this view.
///     selection: Snapshot of the panel's controlled row selection, drawn per row and resolved
///         into a scope by the right-click handler.
///     marker: This panel's scope marker, which swaps the empty-state sentence when the active
///         preset hid every configured core.
///     floors: Content floors for the visible columns. The descriptors are raised to these
///         and then slack-compressed into `available`.
///     available: Measured table width. `0` keeps the declared widths.
///     cx: Panel context used for palette, empty-state localization, and the sort callback.
///
/// Returns:
///     Full-size data-table host with real, sortable columns.
#[allow(clippy::too_many_arguments)]
pub(super) fn core_status_table(
    id: &'static str,
    rows: Rc<Vec<CoreStatusRow>>,
    lines: Rc<Vec<FlatLine>>,
    column_keys: Rc<Vec<&'static str>>,
    server_names: Rc<HashMap<ServerKey, String>>,
    logos_ready: bool,
    sorted: bool,
    state: &Entity<MoonDataTableState>,
    backend: &Entity<Backend>,
    marker: &ScopeMarker,
    selection: RowSelection<CoreId>,
    floors: &HashMap<String, f32>,
    available: f32,
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
    // HANDED IN by the caller, never read off `view` -- `view` IS `cx.entity()`, and this runs
    // inside `CoreStatusView`'s own update, so reading it here panics `cannot read ... while it
    // is already being updated` in a non-unwinding frame: the process dies as the panel renders.
    // `view` itself stays: cloning a handle and calling `update` from a deferred closure is fine,
    // it is only the SYNCHRONOUS read that is illegal here.
    let backend = backend.clone();
    // Taken from `columns()` rather than written as a literal: a heading row must emit EXACTLY as
    // many cells as there are columns, or `MoonDataTable` skips the whole cell permutation for it.
    // Deriving the count keeps a column added elsewhere in this table a no-op for the headings.
    let table_columns = columns(&column_keys);
    let table_columns = respect_content_floors(table_columns, floors, available);
    let section_column_count = column_keys.len();
    let row_column_keys = column_keys.clone();
    // Cloned BEFORE `lines`/`rows`/`view` are moved into the row-builder and sort closures below:
    // `MoonDataTable` addresses every line it draws by INDEX, headings included, so the right-click
    // handler resolves that index back to a core itself instead of assuming it already is one.
    let menu_lines = lines.clone();
    let menu_rows = rows.clone();
    let menu_view = view.clone();
    // Computed ONCE and shared by every closure below: the line-to-core projection is what
    // makes a `MoonDataTable` row index addressable as a selection, and all four handlers
    // must agree about it or a click, a range and a menu would each resolve a different set.
    let order: Rc<[Option<CoreId>]> = Rc::from(ordering::flat_order(&lines, &rows));
    // The same order with the headings dropped -- what `resolve_menu_scope` filters against.
    let order_cores: Rc<[CoreId]> = order.iter().flatten().copied().collect();
    let row_order = order.clone();
    let click_order = order.clone();
    let all_order = order.clone();
    let click_view = view.clone();
    let all_view = view.clone();
    let row_selection = selection.clone();

    crate::panels::common::data_table_host(
        SharedString::from(format!("{id}-host")),
        empty,
        scope_marker::scope_empty_text(Some(marker), t!("core_status.empty").to_string()),
        p,
        cx,
        MoonDataTable::new(id, row_count, move |ix, _window, app| match &lines[ix] {
            FlatLine::Section(section) => {
                // The column COUNT is all a heading needs now. It used to need the column
                // ORDER too, to pick which cell hosted the caption; the banner spans the row,
                // so which column sits leftmost stopped mattering.
                section_row(section, logos_ready, sorted, section_column_count, p, app)
            }
            FlatLine::Core(row) => core_status_row(
                &table_rows[*row],
                &row_column_keys,
                &server_names,
                Some(&backend),
                row_selection.contains(row_order.get(ix).copied().flatten()),
                p,
                app,
            ),
        })
        .columns(table_columns)
        .state(state)
        .style(design::table_style(p))
        // Controlled: the highlight is whatever the rendered rows say it is, so the panel's
        // own `CoreId`-keyed selection is the single source of truth and the widget's internal
        // `selected_row` (a LINE index, which a re-sort invalidates) never competes with it.
        .controlled_row_selection(true)
        .on_select_row(move |ix, window, app| {
            let clicked = click_order.get(ix).copied().flatten();
            let order = click_order.clone();
            let modifiers = window.modifiers();
            click_view.update(app, |this, cx| {
                this.select_core_row(clicked, &order, modifiers, cx);
            });
        })
        .on_select_all_rows(move |_window, app| {
            let order = all_order.clone();
            all_view.update(app, |this, cx| this.select_all_visible_cores(&order, cx));
        })
        // A right-click resolves a SCOPE and never moves the selection: a core row inside the
        // selection stands for the whole selection, a core row outside it stands for itself, and
        // an exchange heading stands for its own section. `resolve_menu_scope` owns that rule.
        .on_right_click_row(move |ix, window, app| {
            let scope = match menu_lines.get(ix) {
                Some(FlatLine::Core(row)) => {
                    let Some(core) = menu_rows.get(*row) else {
                        return;
                    };
                    let cores = core_update::resolve_menu_scope(core.id, &order_cores, &selection);
                    update_menu::UpdateScope::from_cores(&cores, &menu_rows)
                }
                Some(FlatLine::Section(_)) => {
                    let cores = ordering::section_cores(&menu_lines, ix, &menu_rows);
                    update_menu::UpdateScope::from_cores(&cores, &menu_rows)
                }
                None => return,
            };
            if scope.is_empty() {
                return;
            }
            let backend = menu_view.read(app).backend.clone();
            update_menu::open_update_row_menu(
                &backend,
                scope,
                window.mouse_position(),
                window,
                app,
            );
        })
        .on_sort(move |key, ascending, _window, app| {
            let key = key.to_string();
            view.update(app, |this, cx| this.set_flat_sort(&key, ascending, cx));
        }),
    )
}

/// Render one core and its latest telemetry sample in the table's column order.
///
/// Each status and metric cell now carries an explicit colour. Cells representing shared severity
/// use the same [`level_color`]; identity cells remain plain, and `version`
/// keeps its dedicated colour logic. Before this, the numeric cells rendered at full `p.text`, so
/// a dense table made every value compete equally for attention. The resting `text_soft` tone is
/// what gives the two or three deviating cells something to stand out from.
///
/// Which classifier each column uses, and why:
/// - `cpu_proc` / `cpu_sys` / `free_phys` — the panel's absolute display thresholds
///   ([`cpu_level`], [`free_mem_level`]).
/// - `ping` / `ping_exch` — the ENGINE's severity, never a millisecond constant. Core ping also
///   lifts to warning while `ping_warn` is set ([`ping_cell_level`]).
/// - `startup` — warning while the core is still coming up ([`startup_level`]). A finished start
///   stays at the resting tone: there is no elapsed threshold in the model.
/// - `mem_used` / `cpus` / `tz_off` — `Normal` unconditionally. A core's own resident memory is
///   only interpretable against that core's history, which this table does not carry; a machine's
///   core count is a fact about the hardware, never a problem.
/// - `server` / `core` are identifiers. A core that is not healthy draws its short reason beside
///   the name. `version` keeps its own colour logic inside [`version_hover_cell`].
/// - An offline or failed core (critical connection) draws an em dash in every numeric cell, so
///   a retained sample cannot read as a live number.
///
/// The row constructor below carries `#[rustfmt::skip]`, and it is LOAD-BEARING, not tidiness.
/// `tests/theme_contract/core_status.rs` pins the constructor's opening line as source TEXT, and
/// once the arms grew a `.text_color(...)` each, rustfmt began rewriting that closure into the
/// block form `|key| { match *key { ... } }` — which breaks the contract test with NO compile
/// error and no fmt complaint, since `cargo fmt --check` is perfectly happy with the form it
/// itself produced. The formatter and the contract genuinely disagree here; pinning the format is
/// what resolves it. The `let row = ...; row` binding exists only to give the attribute a
/// statement to attach to.
///
/// If you edit the arms, re-run `cargo test -p moon-ui-gpui --test theme_contract` and do NOT
/// trust a `grep` for the pinned line — this doc comment contains it too, so a naive count
/// matches even when the code no longer does.
///
/// Args:
///     r: Cached core snapshot.
///     column_keys: Exact ordered keys used to build the table descriptors.
///     server_names: Server display name per server key.
///     backend: Shared terminal state the hover-revealed update button commands. `None` draws
///         the build without that button, which is how a layout test renders the row.
///     selected: Whether this core belongs to the panel's controlled selection. Applied to the
///         built row rather than inside the pinned constructor below.
///     p: Active Moon palette, for the API and MoonBot cells' colour.
///     app: Application context used to scale the update button's geometry.
///
/// Returns:
///     One row in column order, with server, core, connection, build, and telemetry cells.
fn core_status_row(
    r: &CoreStatusRow,
    column_keys: &[&str],
    server_names: &HashMap<ServerKey, String>,
    backend: Option<&Entity<Backend>>,
    selected: bool,
    p: MoonPalette,
    app: &App,
) -> MoonDataRow {
    let sys = &r.sys;
    let server = server_names
        .get(&ServerKey::for_row(r))
        .cloned()
        .unwrap_or_default();
    // One verdict per row, derived once and shared by the name cell and its hover, so the two can
    // never state different things about the same core.
    let diag = diagnose(&r.status, r.fault.as_ref(), &r.startup);
    let offline = is_offline(r);
    #[rustfmt::skip]
    let row = MoonDataRow::new(column_keys.iter().map(|key| match *key {
        "server" => server_cell(server.clone(), offline, p, SharedString::from(format!("cs-srv-{}", r.id)), app),
        "core" => name_cell(r, diag.as_ref(), column_keys, offline, p, app),
        "status" => status_dot_cell(r, level_color(status_level(&r.status), p), offline, p),
        "version" => MoonDataCell::element(version_hover_cell(r, backend, offline, p, app)),
        "cpu_proc" => cpu_bar_cell(r, "proc", sys.process_cpu_percent, cell_text_for(r, "cpu_proc"), level_color(cpu_level(sys.process_cpu_percent), p), offline, p, app),
        "cpu_sys" => cpu_bar_cell(r, "sys", sys.system_cpu_percent, cell_text_for(r, "cpu_sys"), level_color(cpu_level(sys.system_cpu_percent), p), offline, p, app),
        "mem_used" => metric_cell(cell_text_for(r, "mem_used"), level_color(LoadLevel::Normal, p), offline, p, SharedString::from(format!("cs-mem-{}", r.id)), MetricAlign::End),
        "free_phys" => metric_cell(cell_text_for(r, "free_phys"), level_color(free_mem_level(sys.free_physical_memory_mb), p), offline, p, SharedString::from(format!("cs-free-{}", r.id)), MetricAlign::End),
        "ping" => metric_cell(cell_text_for(r, "ping"), level_color(ping_cell_level(r.ping_warn, r.ping_sev), p), offline, p, SharedString::from(format!("cs-ping-{}", r.id)), MetricAlign::End),
        "ping_exch" => metric_cell(cell_text_for(r, "ping_exch"), level_color(lat_level(r.exch_sev), p), offline, p, SharedString::from(format!("cs-exch-{}", r.id)), MetricAlign::End),
        "cpus" => metric_cell(cell_text_for(r, "cpus"), level_color(LoadLevel::Normal, p), offline, p, SharedString::from(format!("cs-cpus-{}", r.id)), MetricAlign::Center),
        "api_key" => api_key_cell(r, offline, p),
        "api_quota" => metric_cell(cell_text_for(r, "api_quota"), level_color(api_quota_level(r.api_quota, r.api_quota_warn), p), offline, p, SharedString::from(format!("cs-quota-{}", r.id)), MetricAlign::Center),
        "startup" => MoonDataCell::element(startup_hover_cell(r, offline, p)).text_color(level_color(startup_level(startup_cell(&r.status, &r.startup)), p)),
        "tz_off" => MoonDataCell::element(tz_offset_hover_cell(r, offline, p)).text_color(level_color(LoadLevel::Normal, p)),
        _ => unreachable!("canonical Flat column key"),
    }));
    // Applied HERE, not inside the match above: `tests/theme_contract/core_status.rs` pins that
    // constructor's opening line as source TEXT, and rustfmt rewrites the closure into block form
    // the moment its shape changes -- which breaks the contract with no compile error and no fmt
    // complaint. The `#[rustfmt::skip]` binding stays exactly as it was.
    row.selected(selected)
}

/// The API-key cell: a bare day count, or the infinity glyph with the phrase behind it.
///
/// Two shapes rather than one because `MoonDataCell::text` carries no tooltip — only an element
/// cell can host one — and paying for a stateful element on every row to explain a glyph that
/// appears on some of them would be waste. The colour is identical either way, so the two shapes
/// are indistinguishable until the reader hovers.
///
/// Args:
///     r: The row being rendered.
///     offline: Whether the row paints the alarm tint. The key text itself stays: expiry is not
///         a live sample that goes stale when the core drops.
///     p: Active Moon palette.
///
/// Returns:
///     The cell, with a hover only where there is something to explain.
fn api_key_cell(r: &CoreStatusRow, offline: bool, p: MoonPalette) -> MoonDataCell {
    let text = api_expiry_text(r.api_key);
    let api_level = api_expiry_level(r.api_key, r.api_warn, r.api_notice);
    let color = if matches!(api_level, LoadLevel::Warning | LoadLevel::Notice) {
        card_attention_color(p)
    } else {
        level_color(api_level, p)
    };
    let mut cell = div()
        .id(SharedString::from(format!("cs-api-key-{}", r.id)))
        .relative()
        .size_full()
        .flex()
        .items_center()
        .justify_end()
        .when(offline, |cell| cell.child(offline_band(p)))
        .child(text);
    if let Some(tip) = api_expiry_tooltip(r.api_key) {
        cell = cell.tooltip(crate::panels::common::text_tooltip(tip));
    }
    MoonDataCell::element(cell).text_color(color)
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
        core_order::ExchangeSection::Venue(id) => {
            format!("{}-{}", id.code, id.dex)
        }
        core_order::ExchangeSection::Unidentified => "unknown".to_string(),
    };
    let logo = logos_ready
        .then_some(section.brand)
        .flatten()
        .and_then(crate::media::exchange_logos::exchange_logo);
    // `aggregate.cores` is the partition size, written once in `ordering::flat_lines`.
    let summary = ordering::section_aggregate_text(&section.aggregate);
    // The sort arrow MoonUI draws says nothing about being section-scoped, so the hover says it.
    // The caption itself no longer needs the hover to be readable — the banner gives it the row.
    let mut hover = format!("{} - {}", section.label, summary);
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
                    .bg(rgb(p.panel_head))
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
    // follows the caption on the same flex line rather than needing a cell of its own.
    .banner(
        h_flex()
            .id(SharedString::from(format!("cs-exchange-{key}")))
            .size_full()
            .items_center()
            .pl(px(CELL_PAD_LEFT))
            .pr(px(CELL_PAD_RIGHT))
            .gap(design::ui_px(cx, 6.0))
            .text_size(design::t_caption(cx))
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
                    .child(
                        h_flex()
                            .min_w_0()
                            .items_center()
                            .child(
                                div()
                                    .min_w_0()
                                    .truncate()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(rgb(p.text))
                                    .child(section.label.clone()),
                            )
                            .child(section_aggregate_line(
                                &summary,
                                section.aggregate.not_connected,
                                p,
                                cx,
                            )),
                    ),
            )
            .tooltip(crate::panels::common::text_tooltip(hover)),
    )
}

/// The section caption beside the exchange name.
///
/// The whole sentence stays muted when every core is connected. When some are not, the clause
/// after the last " · " — the not-connected count — is painted in the alarm colour. The
/// dictionaries already put that clause last.
fn section_aggregate_line(summary: &str, not_connected: usize, p: MoonPalette, cx: &App) -> Div {
    let line = div()
        .flex_none()
        .ml(design::ui_px(cx, 10.0))
        .text_size(design::t_caption(cx))
        .font_weight(FontWeight::NORMAL);
    if not_connected > 0
        && let Some(at) = summary.rfind(" · ")
    {
        let split = at + " · ".len();
        let (head, clause) = summary.split_at(split);
        return line
            .flex()
            .child(div().text_color(rgb(p.text_muted)).child(head.to_string()))
            .child(div().text_color(rgb(p.red)).child(clause.to_string()));
    }
    line.text_color(rgb(p.text_muted))
        .child(summary.to_string())
}

/// Text one Flat cell shows.
///
/// An offline or failed core renders an em dash for every numeric key, so a sample retained
/// from before the drop cannot read as a live number. A core that is up renders the same figure
/// the cell draws. The em dash is distinct from the ASCII `"-"` the formatters use for a value
/// that has not arrived yet.
///
/// Numeric keys are the telemetry columns: both pings, both CPUs, both memories, the CPU count,
/// the API quota, startup and the clock offset. Name, server, status, build and the API key are
/// not numeric.
///
/// Args:
///     row: The core snapshot.
///     key: Canonical Flat column key.
///
/// Returns:
///     The cell text. An unknown key is empty.
pub(super) fn cell_text_for(row: &CoreStatusRow, key: &str) -> String {
    if is_offline(row) && is_numeric_key(key) {
        return "—".to_string();
    }
    let sys = &row.sys;
    match key {
        "cpu_proc" => percent(sys.process_cpu_percent),
        "cpu_sys" => percent(sys.system_cpu_percent),
        "mem_used" => memory_u16(sys.used_memory_mb),
        "free_phys" => memory_u16(sys.free_physical_memory_mb),
        "ping" => ping(sys.round_trip_ms),
        "ping_exch" => ping(sys.order_api_latency_ms.map(u32::from)),
        "cpus" => count(sys.logical_cpu_count),
        "api_quota" => api_quota_text(row.api_quota),
        "startup" => startup_cell_text(startup_cell(&row.status, &row.startup)),
        "tz_off" => tz_offset_cell_text(tz_offset_cell(&row.time_offset)),
        "core" => row.name.clone(),
        "api_key" => api_expiry_text(row.api_key),
        _ => String::new(),
    }
}

/// Whether the row is offline or failed.
///
/// Critical is the alarm level. Connecting and a ready core that still holds a fault are not
/// offline: the first is in progress, the second is healthy.
fn is_offline(row: &CoreStatusRow) -> bool {
    matches!(status_level(&row.status), LoadLevel::Critical)
}

/// Columns whose text is a number or a metric phrase that goes stale when the core drops.
fn is_numeric_key(key: &str) -> bool {
    matches!(
        key,
        "ping"
            | "ping_exch"
            | "cpu_proc"
            | "cpu_sys"
            | "mem_used"
            | "free_phys"
            | "cpus"
            | "api_quota"
            | "startup"
            | "tz_off"
    )
}

/// Alarm tint painted inside one cell.
///
/// `MoonDataRow` has no row background, so an offline row cannot be tinted from the table. The
/// section heading already covers the cell padding with an absolutely positioned band, and this
/// uses the same inset. The alpha is the one the distribution chips use for an alarm row.
fn offline_band(p: MoonPalette) -> Div {
    div()
        .absolute()
        .top_0()
        .bottom_0()
        .left(px(-CELL_PAD_LEFT))
        .right(px(-CELL_PAD_RIGHT))
        .bg(design::moon_alpha(p.red, 0.08))
}

/// Server name: mono caption in the muted text colour, with the offline band when needed.
///
/// Always an element. `MoonDataCell::text` cannot carry the font, so an online server would
/// otherwise paint in the table's default face.
fn server_cell(
    text: String,
    offline: bool,
    p: MoonPalette,
    id: SharedString,
    app: &App,
) -> MoonDataCell {
    MoonDataCell::element(
        div()
            .id(id)
            .relative()
            .size_full()
            .flex()
            .items_center()
            .min_w_0()
            .when(offline, |cell| cell.child(offline_band(p)))
            .child(
                div()
                    .min_w_0()
                    .truncate()
                    .font_family(design::mono())
                    .text_size(design::t_caption(app))
                    .text_color(rgb(p.text_muted))
                    .child(text),
            ),
    )
}

/// Horizontal alignment shared by a Flat telemetry column and the cell that fills it.
enum MetricAlign {
    /// Identity, the MoonBot build, and the two CPU columns.
    Start,
    /// CPU count and API quota, which this round does not move.
    Center,
    /// Pings, both memories, the API key, and startup.
    End,
}

/// Apply [`MetricAlign`] to a flex cell. Start is the flex default.
fn align_cell(cell: Stateful<Div>, align: MetricAlign) -> Stateful<Div> {
    match align {
        MetricAlign::Start => cell,
        MetricAlign::Center => cell.justify_center(),
        MetricAlign::End => cell.justify_end(),
    }
}

/// A numeric cell. Offline rows draw the em dash on the alarm band. Live rows are an element
/// too, so a layout test can find the figure by the same id the offline cell already carried.
fn metric_cell(
    text: String,
    color: u32,
    offline: bool,
    p: MoonPalette,
    id: SharedString,
    align: MetricAlign,
) -> MoonDataCell {
    let selector = id.to_string();
    if !offline {
        let selector = selector.clone();
        return MoonDataCell::element(
            align_cell(
                div()
                    .id(id)
                    .debug_selector(move || selector)
                    .size_full()
                    .flex()
                    .items_center()
                    .min_w_0()
                    .font_family(design::mono()),
                align,
            )
            .child(text),
        )
        .text_color(color);
    }
    MoonDataCell::element(
        align_cell(
            div()
                .id(id)
                .debug_selector(move || selector)
                .relative()
                .size_full()
                .flex()
                .items_center()
                .child(offline_band(p)),
            align,
        )
        .child(
            div()
                .font_family(design::mono())
                .text_color(rgb(color))
                .child(text),
        ),
    )
    .text_color(color)
}

/// CPU cell: a mini bar plus the mono percent.
///
/// The bar is a fixed 38 design px wide and 4 px high. `MoonProgress` has no width setter and
/// fills that slot. An offline core draws the em dash instead of a bar at the last sample. An
/// unknown percent draws the marker with no bar.
fn cpu_bar_cell(
    r: &CoreStatusRow,
    which: &str,
    value: Option<u8>,
    text: String,
    color: u32,
    offline: bool,
    p: MoonPalette,
    app: &App,
) -> MoonDataCell {
    let id = SharedString::from(format!("cs-cpu-{which}-{}", r.id));
    if offline {
        return metric_cell(text, color, true, p, id, MetricAlign::Start);
    }
    let selector = id.to_string();
    let bar = value.map(|pct| {
        div().w(design::ui_px(app, 38.0)).flex_none().child(
            MoonProgress::new(SharedString::from(format!("cs-cpu-bar-{which}-{}", r.id)))
                .value(f32::from(pct))
                .color(color)
                .height(4.0),
        )
    });
    MoonDataCell::element(
        h_flex()
            .id(id)
            .debug_selector(move || selector)
            .w_full()
            .items_center()
            .gap(design::ui_px(app, 6.0))
            .when_some(bar, |row, bar| row.child(bar))
            .child(
                div()
                    .flex_none()
                    .font_family(design::mono())
                    .text_color(rgb(color))
                    .child(text),
            ),
    )
    .text_color(color)
}

/// Status column: a dot coloured by the connection level.
///
/// A ready core is green. [`level_color`] of a normal level is the resting text grey, which
/// would leave a healthy dot looking uncolored, so that one case uses the palette green. Warning
/// and critical use the colour the row already computed from [`level_color`].
fn status_dot_cell(
    r: &CoreStatusRow,
    level_paint: u32,
    offline: bool,
    p: MoonPalette,
) -> MoonDataCell {
    let level = status_level(&r.status);
    let color = if matches!(level, LoadLevel::Normal) {
        p.green
    } else {
        level_paint
    };
    let tone = match level {
        LoadLevel::Normal => MoonTone::Positive,
        LoadLevel::Notice => MoonTone::Notice,
        LoadLevel::Warning => MoonTone::Warning,
        LoadLevel::Critical => MoonTone::Danger,
    };
    MoonDataCell::element(
        div()
            .id(SharedString::from(format!("cs-status-{}", r.id)))
            .relative()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .when(offline, |cell| cell.child(offline_band(p)))
            .child(
                MoonBadge::new("")
                    .dot()
                    .variant(MoonBadgeVariant::Solid)
                    .tone(tone)
                    .bg_color(color)
                    .bg_alpha(1.0),
            ),
    )
}

/// Name cell: the core name, and for a core that is not healthy the short reason beside it.
///
/// The hover is the fault tooltip the status column used to carry. When startup or the clock
/// offset is not on screen, those hovers are appended here so the facts stay reachable.
fn name_cell(
    r: &CoreStatusRow,
    diag: Option<&Diagnosis>,
    column_keys: &[&str],
    offline: bool,
    p: MoonPalette,
    app: &App,
) -> MoonDataCell {
    let reason = problem_reason(r, diag);
    let tip = name_hover(r, diag, column_keys);
    let mut cell = div()
        .id(SharedString::from(format!("cs-core-{}", r.id)))
        .relative()
        .size_full()
        .min_w_0()
        .flex()
        .items_center()
        .gap(design::ui_px(app, 6.0))
        .when(offline, |cell| cell.child(offline_band(p)))
        .child(
            div()
                .min_w_0()
                .truncate()
                .font_family(design::mono())
                .child(r.name.clone()),
        );
    if let Some((text, level)) = reason {
        cell = cell.child(
            div()
                .min_w_0()
                .truncate()
                .font_family(design::ui_font())
                .text_size(design::t_caption(app))
                .text_color(rgb(level_color(level, p)))
                .child(text),
        );
    }
    if let Some(tip) = tip {
        cell = cell.tooltip(crate::panels::common::text_tooltip(tip));
    }
    MoonDataCell::element(cell)
}

/// Short reason drawn beside the name, or `None` when the core is healthy.
///
/// Ready is healthy even when a fault is still retained. A verdict supplies `fault.short` plus
/// the elapsed time when that class has one. Without a verdict the coarse connection label is
/// used, so a connecting core is not given a fabricated cause.
fn problem_reason(r: &CoreStatusRow, diag: Option<&Diagnosis>) -> Option<(String, LoadLevel)> {
    let level = status_level(&r.status);
    if matches!(level, LoadLevel::Normal) {
        return None;
    }
    let Some(d) = diag else {
        return Some((connection_presentation(&r.status, None).label, level));
    };
    let short = fault_short(&d.class);
    let text = match fault_duration(&d.class) {
        Some(dur) => t!("core_status.row.reason_for", reason = short, dur = dur).to_string(),
        None => short,
    };
    Some((text, level))
}

/// Elapsed figure for a verdict that recorded one, in the startup clock's wording.
fn fault_duration(class: &FailureClass) -> Option<String> {
    let ms = match class {
        FailureClass::NoResponse { elapsed_ms, .. } => *elapsed_ms,
        FailureClass::Syncing { elapsed_ms, .. } => *elapsed_ms,
        _ => return None,
    };
    Some(t!("core_status.startup.secs", t = crate::conn_diag::secs(ms)).to_string())
}

/// Hover for the name cell: the fault, plus startup and clock offset when those columns are hidden.
fn name_hover(r: &CoreStatusRow, diag: Option<&Diagnosis>, column_keys: &[&str]) -> Option<String> {
    let mut parts = Vec::new();
    if let Some(d) = diag {
        let tip = fault_tooltip(&fault_facts(d, r.mode_suggestion));
        if !tip.is_empty() {
            parts.push(tip);
        }
    }
    if !column_keys.contains(&"startup") {
        parts.push(startup_tooltip(&startup_facts(&r.startup)));
    }
    if !column_keys.contains(&"tz_off") {
        parts.push(tz_offset_tooltip(&tz_offset_facts(&r.time_offset)));
    }
    if parts.is_empty() {
        None
    } else {
        Some(parts.join("\n"))
    }
}

/// The MoonBot build cell, carrying [`super::presentation::notice_color`] when this core is
/// behind the fleet's newest reported build, with the hover naming both builds. No warning
/// treatment beyond the colour: no triangle, no icon column, same as the by-IP tree's
/// `version_slot`.
///
/// Args:
///     r: The row being rendered.
///     backend: Shared terminal state the hover-revealed update button commands. `None`
///         leaves the button off, so a layout fixture can draw the cell without a backend.
///     offline: Whether the row paints the alarm tint behind the build.
///     p: Active Moon palette.
///     app: Application context used to scale the update button's geometry.
///
/// Returns:
///     The cell element.
fn version_hover_cell(
    r: &CoreStatusRow,
    backend: Option<&Entity<Backend>>,
    offline: bool,
    p: MoonPalette,
    app: &App,
) -> Stateful<Div> {
    let badge = update_badge(r.update.as_ref());
    let hover_group = SharedString::from(format!("core-update-hover:{}", r.id));
    let mut update_counts = OfferCounts::default();
    update_counts.add(core_update::offer_state(
        &r.status,
        r.server_version,
        r.endpoint.is_some(),
        r.update.as_ref(),
    ));
    let failed_retry = matches!(
        r.update.as_ref(),
        Some(CoreUpdatePhase::Done(CoreUpdateOutcome::Failed(_)))
    )
    .then_some(r.id);
    let update_btn = backend.and_then(|backend| {
        core_update::update_button(
            format!("cs-update-{}", r.id),
            hover_group.clone(),
            Rc::from([r.id]),
            update_counts,
            failed_retry,
            backend,
            p,
            app,
        )
    });
    let parts = build_parts(r.server_version, r.server_version_suffix.as_deref());
    div()
        .id(SharedString::from(format!("cs-version-{}", r.id)))
        .relative()
        .size_full()
        .group(hover_group)
        .flex()
        .items_center()
        .gap_1()
        .when(offline, |cell| cell.child(offline_band(p)))
        .text_color(rgb(if r.version_behind.is_some() {
            card_attention_color(p)
        } else {
            version_color(false, r.server_version.is_some(), p)
        }))
        // The column is left-aligned. The mark keeps its letter slot reserved on every row, so
        // it has one width everywhere and every row's digits land on the same line.
        .child(version_mark(parts, p, app, true))
        .when_some(badge, |c, badge| {
            // The badge is its OWN stateful child, carrying its own tooltip, exactly as
            // `server_view.rs::version_slot` already does for its badge -- `tooltip` panics in
            // debug builds if called twice on one element, which is why the phase hover cannot
            // also live on the outer cell below. Without this the Flat table's badge had NO
            // tooltip of its own whenever `version_behind.is_some()` claimed the cell's one slot
            // -- exactly the cores a bulk "Update the cores behind" campaign is watching.
            c.child(
                div()
                    .id(SharedString::from(format!("cs-version-{}-badge", r.id)))
                    .flex_none()
                    .text_color(rgb(level_color(badge.level, p)))
                    .child(badge.glyph)
                    .tooltip(crate::panels::common::text_tooltip(
                        t!(badge.locale_key).to_string(),
                    )),
            )
        })
        .when_some(update_btn, |c, btn| c.child(btn))
        .when_some(r.version_behind, |c, newest| {
            c.tooltip(crate::panels::common::text_tooltip(version_behind_tooltip(
                r.server_version,
                r.server_version_suffix.as_deref(),
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
///     offline: Whether the visible figure is the offline em dash on the alarm band.
///     p: Active Moon palette.
///
/// Returns:
///     The cell element.
fn startup_hover_cell(r: &CoreStatusRow, offline: bool, p: MoonPalette) -> Stateful<Div> {
    div()
        .id(SharedString::from(format!("cs-startup-{}", r.id)))
        .relative()
        .size_full()
        .flex()
        .items_center()
        .justify_end()
        .when(offline, |cell| cell.child(offline_band(p)))
        .child(cell_text_for(r, "startup"))
        .tooltip(crate::panels::common::text_tooltip(startup_tooltip(
            &startup_facts(&r.startup),
        )))
}

/// The tz-offset cell, with the same structured hover the by-IP tree shows for a core row.
///
/// Args:
///     r: The row being rendered.
///     offline: Whether the visible figure is the offline em dash on the alarm band.
///     p: Active Moon palette.
///
/// Returns:
///     The cell element.
fn tz_offset_hover_cell(r: &CoreStatusRow, offline: bool, p: MoonPalette) -> Stateful<Div> {
    div()
        .id(SharedString::from(format!("cs-tz-off-{}", r.id)))
        .relative()
        .size_full()
        .flex()
        .items_center()
        .when(offline, |cell| cell.child(offline_band(p)))
        .child(cell_text_for(r, "tz_off"))
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

#[cfg(test)]
mod tests;
