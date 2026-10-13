//! Source-level Core Status table contracts for the binary-only GPUI crate.

use super::support::{braced_body, code_only, read_module, read_src};

/// `panels/core_status/**` must route update requests through `controls::core_update` and
/// `update_menu.rs` must not call `select_core_row`. Mutation: call an enqueue method directly
/// or select the right-clicked row; either bypasses the per-IP queue or changes the update scope
/// the menu is meant to preserve. `code_only` strips every line and doc comment before the bans,
/// so the module documentation may explain these names without satisfying or tripping this test.
#[test]
fn core_status_bulk_updates_stay_queued_and_right_click_preserves_selection() {
    let panel = code_only(&read_module("panels/core_status"));
    for forbidden in [
        "enqueue_core_update",
        "enqueue_core_updates",
        "update_core_version",
    ] {
        assert!(
            !panel.contains(forbidden),
            "Core Status must route updates through controls::core_update, not `{forbidden}`"
        );
    }

    let menu = code_only(&read_src("panels/core_status/update_menu.rs"));
    assert!(
        !menu.contains("select_core_row("),
        "opening a Core Status update menu must not move the existing selection"
    );
}

/// `table.rs:core_status_row` must route every explicitly classified Flat cell through
/// `level_color`. Mutation: delete one arm's `.text_color(level_color(...))`; that column would
/// silently return to full-strength text and the Flat table would lose the visual contrast that
/// surfaces troubled cores.
#[test]
fn core_status_flat_cells_apply_the_shared_level_colour() {
    let table = read_src("panels/core_status/table.rs");
    let row = code_only(braced_body(&table, "fn core_status_row("));
    let classified_arms = [
        "\"status\" => status_dot_cell(r, level_color(status_level(&r.status), p), offline, p),",
        "\"cpu_proc\" => cpu_bar_cell(r, \"proc\", sys.process_cpu_percent, cell_text_for(r, \"cpu_proc\"), level_color(cpu_level(sys.process_cpu_percent), p), offline, p, app),",
        "\"cpu_sys\" => cpu_bar_cell(r, \"sys\", sys.system_cpu_percent, cell_text_for(r, \"cpu_sys\"), level_color(cpu_level(sys.system_cpu_percent), p), offline, p, app),",
        "\"free_phys\" => metric_cell(cell_text_for(r, \"free_phys\"), level_color(free_mem_level(sys.free_physical_memory_mb), p), offline, p, SharedString::from(format!(\"cs-free-{}\", r.id)), MetricAlign::End),",
        "\"ping\" => metric_cell(cell_text_for(r, \"ping\"), level_color(ping_cell_level(r.ping_warn, r.ping_sev), p), offline, p, SharedString::from(format!(\"cs-ping-{}\", r.id)), MetricAlign::End),",
        "\"ping_exch\" => metric_cell(cell_text_for(r, \"ping_exch\"), level_color(lat_level(r.exch_sev), p), offline, p, SharedString::from(format!(\"cs-exch-{}\", r.id)), MetricAlign::End),",
        "\"api_quota\" => metric_cell(cell_text_for(r, \"api_quota\"), level_color(api_quota_level(r.api_quota, r.api_quota_warn), p), offline, p, SharedString::from(format!(\"cs-quota-{}\", r.id)), MetricAlign::Center),",
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

/// A saved sort whose column is hidden falls back inside the one projection, and render does
/// not write that preference. Applying the absent sort makes rows appear to move for no visible
/// header; deleting it discards the user's choice when the column comes back.
///
/// Mutation: sort by the hidden key inside `project_rows`, or assign `self.flat_sort` from
/// `render`. The rows would follow a column that is not on screen, or the saved choice would
/// be gone when the column returns.
#[test]
fn core_status_hidden_api_sort_contract_falls_back_without_erasing_preference() {
    let cache = read_src("panels/core_status/cache.rs");
    let project = code_only(braced_body(&cache, "pub(super) fn project_rows("));
    let sort_rows = code_only(braced_body(&cache, "fn sort_flat_rows("));
    let flat = code_only(braced_body(&cache, "pub(super) fn flat_view("));
    assert!(
        project.contains(".filter(|(key, _)| visible_columns.contains(&key.as_str()))")
            && sort_rows.contains("model::ordered_flat_rows(rows)")
            && flat.contains("project_rows("),
        "a hidden sort column falls back to attention order inside the projection"
    );

    let panel = read_src("panels/core_status/render.rs");
    let render = code_only(braced_body(&panel, "fn render("));
    assert!(
        !render.contains("self.flat_sort =")
            && !render.contains("table_persist::set_sort(")
            && !render.contains("self.set_flat_sort(")
            && !render.contains("if saved_sort_visible"),
        "rendering an invisible sort must not rewrite the persisted preference"
    );
}

/// Servers, Flat, the health strip, the mode bar, and the Updates pane must not synchronously
/// read `CoreStatusView` while its render is updating. Restoring that entity read panics when
/// Core Status opens and terminates the process. The Flat table still takes the caller-owned
/// backend. The card grid does not: its caller hands the groups in.
#[test]
fn core_status_render_helpers_never_read_their_own_view_entity() {
    let servers = read_src("panels/core_status/servers_view.rs");
    let servers_view = code_only(braced_body(&servers, "pub(super) fn servers_view("));
    let table = read_src("panels/core_status/table.rs");
    let flat = code_only(braced_body(&table, "pub(super) fn core_status_table("));
    let strip_src = read_src("panels/core_status/health_strip.rs");
    let strip = code_only(braced_body(&strip_src, "pub(super) fn health_strip("));
    let bar_src = read_src("panels/core_status/core_bar.rs");
    let bar = code_only(braced_body(&bar_src, "pub(super) fn core_bar("));
    let updates_src = read_src("panels/core_status/updates_pane.rs");
    let updates = code_only(braced_body(&updates_src, "pub(super) fn updates_pane("));

    assert!(
        !servers_view.contains("cx.entity().read(cx)"),
        "the Servers render helper must not read CoreStatusView"
    );
    assert!(
        flat.contains("backend: &Entity<Backend>")
            && !flat.contains("view.read(cx)")
            && !flat.contains("cx.entity().read(cx)"),
        "the Flat render helper must use its handed-in backend without synchronously reading its view"
    );
    for (name, body) in [
        ("health_strip", strip.as_str()),
        ("core_bar", bar.as_str()),
        ("updates_pane", updates.as_str()),
    ] {
        assert!(
            !body.contains("cx.entity().read(cx)"),
            "{name} must not read CoreStatusView while it renders"
        );
    }
}

/// `updates_pane` must choose its blocks through `updates_blocks` and must not compute a
/// caught-up boolean beside that call. A local `caught_up` flag drifts from the block enum and
/// paints the empty state on a pane that still has history.
#[test]
fn updates_pane_delegates_caught_up_to_updates_blocks() {
    let updates_src = read_src("panels/core_status/updates_pane.rs");
    let updates = code_only(braced_body(&updates_src, "pub(super) fn updates_pane("));
    assert!(
        updates.contains("updates_blocks("),
        "updates_pane must call updates_blocks"
    );
    assert!(
        !updates.contains("let caught_up") && !updates.contains("caught_up:"),
        "updates_pane must not compute a caught_up boolean outside updates_blocks"
    );
}
