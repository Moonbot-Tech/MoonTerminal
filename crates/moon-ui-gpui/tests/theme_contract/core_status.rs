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
