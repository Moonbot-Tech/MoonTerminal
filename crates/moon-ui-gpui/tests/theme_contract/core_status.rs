//! Source-level Core Status table contracts for the binary-only GPUI crate.

use super::support::{braced_body, code_only, read_src};

/// `table.rs:section_row` must create one cell per declared table column through `column_count`.
///
/// Mutation: replace `0..column_count` with a literal count. MoonUI would skip the heading-row
/// cell permutation after a column is added, leaving a misplaced, partially unpainted section band.
#[test]
fn core_status_section_row_derives_its_cell_count_from_declared_columns() {
    let table = read_src("panels/core_status/table.rs");
    let host = code_only(braced_body(&table, "pub(super) fn core_status_table("));
    let section = code_only(braced_body(&table, "fn section_row("));

    assert!(
        host.contains("let section_columns = columns();")
            && host.contains("section_columns.len(),"),
        "the section-row argument must derive from the table's declared columns"
    );
    assert!(
        section.contains("MoonDataRow::new((0..column_count).map(|_index| {"),
        "the heading row must emit exactly one cell for every declared column"
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
        flat.contains("backend: &Entity<Backend>") && !flat.contains("view.read(cx)"),
        "the Flat render helper must use its handed-in backend without synchronously reading its view"
    );
}
