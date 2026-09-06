//! Source-level Core Status table contracts for the binary-only GPUI crate.

use super::support::{braced_body, code_only, read_src};

/// `table.rs:core_status_row` must route every explicitly classified Flat cell through
/// `level_color`. Mutation: delete one arm's `.text_color(level_color(...))`; that column would
/// silently return to full-strength text and the Flat table would lose the visual contrast that
/// surfaces troubled cores.
#[test]
fn core_status_flat_cells_apply_the_shared_level_colour() {
    let table = read_src("panels/core_status/table.rs");
    let row = code_only(braced_body(&table, "fn core_status_row("));
    let classified_arms = [
        "\"status\" => MoonDataCell::element(status_cell(r, diag.as_ref()))\n            .text_color(level_color(status_level(&r.status), p)),",
        "\"cpu_proc\" => MoonDataCell::text(percent(sys.process_cpu_percent))\n            .text_color(level_color(cpu_level(sys.process_cpu_percent), p)),",
        "\"cpu_sys\" => MoonDataCell::text(percent(sys.system_cpu_percent))\n            .text_color(level_color(cpu_level(sys.system_cpu_percent), p)),",
        "\"free_phys\" => MoonDataCell::text(memory_u16(sys.free_physical_memory_mb))\n            .text_color(level_color(free_mem_level(sys.free_physical_memory_mb), p)),",
        "\"ping\" => MoonDataCell::text(ping(sys.round_trip_ms))\n            .text_color(level_color(lat_level(r.ping_sev), p)),",
        "\"ping_exch\" => MoonDataCell::text(ping(sys.order_api_latency_ms.map(u32::from)))\n            .text_color(level_color(lat_level(r.exch_sev), p)),",
        "\"api_quota\" => MoonDataCell::text(api_quota_text(r.api_quota)).text_color(level_color(\n            api_quota_level(r.api_quota, r.api_quota_warn),\n            p,\n        )),",
    ];

    for arm in classified_arms {
        assert!(
            row.contains(arm),
            "every classified Flat cell must use level_color; missing arm:\n{arm}"
        );
    }
}

/// `table.rs:core_status_table` must derive `section_column_count` from the shared `column_keys`
/// projection, and `table.rs:section_row` must create one cell per declared table column.
///
/// Mutation: replace `0..column_count` with a literal count. MoonUI would skip the heading-row
/// cell permutation after a column is added, leaving a misplaced, partially unpainted section band.
#[test]
fn core_status_section_row_derives_its_cell_count_from_declared_columns() {
    let table = read_src("panels/core_status/table.rs");
    let host = code_only(braced_body(&table, "pub(super) fn core_status_table("));
    let section = code_only(braced_body(&table, "fn section_row("));

    assert!(
        host.contains("let table_columns = columns(&column_keys);")
            && host.contains("let section_column_count = column_keys.len();")
            && host.contains("section_column_count,")
            && host.contains(".columns(table_columns)"),
        "the section-row argument and table descriptors must derive from the shared visible keys"
    );
    assert!(
        section.contains("MoonDataRow::new((0..column_count).map(|_index| {"),
        "the heading row must emit exactly one cell for every declared column"
    );
}

/// `table.rs:column_visible` must omit API placeholders, and the direct row constructor must
/// project its cells through the same filtered canonical keys. Treating `Unknown`, `Perpetual`,
/// or an absent quota as real wastes wide-panel space; a different direct projection renders
/// values under the wrong headers.
///
/// Mutation: make `Unknown`/`Perpetual` or `None` satisfy the visibility predicate, or make the
/// direct row constructor use a different key projection. A table with no real API values would
/// regain two useless wide columns, or direct cells would appear under the wrong headers.
#[test]
fn core_status_flat_visibility_contract_has_one_shared_visible_key_projection() {
    let table = read_src("panels/core_status/table.rs");
    let predicate = code_only(braced_body(&table, "fn column_visible("));
    let visible_keys = code_only(braced_body(&table, "pub(super) fn visible_column_keys("));
    let row = code_only(braced_body(&table, "fn core_status_row("));

    assert!(
        predicate.contains("matches!(row.api_key, ApiKeyState::Days(_))")
            && !predicate.contains("ApiKeyState::Unknown")
            && !predicate.contains("ApiKeyState::Perpetual")
            && predicate.contains("row.api_quota.is_some()"),
        "only dated API keys and present quotas may make their columns visible"
    );
    assert!(
        visible_keys.contains("COLUMN_KEYS")
            && visible_keys.contains(".filter(|key| column_visible(key, rows))")
            && row.contains("MoonDataRow::new(column_keys.iter().map(|key| match *key {"),
        "the direct row constructor must use the same filtered canonical key projection as headers"
    );
}

/// `mod.rs:CoreStatusView::render` must use attention order when a saved API sort targets a hidden
/// column while leaving that saved preference untouched. Applying the absent sort makes rows appear
/// to move for no visible header; deleting it discards the user's choice when data returns.
///
/// Mutation: force the saved-sort branch after an API column is hidden. The render helper would
/// apply invisible ordering instead of the independent default attention order without writing a
/// replacement preference.
#[test]
fn core_status_hidden_api_sort_contract_falls_back_without_erasing_preference() {
    let panel = read_src("panels/core_status/mod.rs");
    let render = code_only(braced_body(&panel, "fn render("));

    assert!(
        render.contains(".is_none_or(|(key, _)| column_keys.contains(&key.as_str()))")
            && render.contains("let (flat_rows, flat_lines) = if saved_sort_visible {")
            && render.contains("let flat_rows = model::ordered_flat_rows(&rows);"),
        "an absent saved API sort must select the default attention order"
    );
    assert!(
        !render.contains("self.flat_sort =")
            && !render.contains("table_persist::set_sort(")
            && !render.contains("self.set_flat_sort("),
        "rendering an invisible sort must not rewrite the persisted preference"
    );
}

/// `server_view.rs:grouped_server_view` and `table.rs:core_status_table` must accept the
/// caller-owned backend rather than synchronously reading `CoreStatusView` while its render is
/// updating. Restoring either entity read panics when Core Status opens and terminates the process.
#[test]
fn core_status_render_helpers_never_read_their_own_view_entity() {
    let server_view = read_src("panels/core_status/server_view.rs");
    let grouped = code_only(braced_body(
        &server_view,
        "pub(super) fn grouped_server_view(",
    ));
    let table = read_src("panels/core_status/table.rs");
    let flat = code_only(braced_body(&table, "pub(super) fn core_status_table("));

    assert!(
        grouped.contains("backend: &Entity<Backend>") && !grouped.contains("cx.entity().read(cx)"),
        "the By-IP render helper must use its handed-in backend without reading CoreStatusView"
    );
    assert!(
        flat.contains("backend: &Entity<Backend>")
            && !flat.contains("view.read(cx)")
            && !flat.contains("cx.entity().read(cx)"),
        "the Flat render helper must use its handed-in backend without synchronously reading its view"
    );
}
