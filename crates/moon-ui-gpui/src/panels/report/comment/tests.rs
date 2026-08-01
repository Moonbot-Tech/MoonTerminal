//! Regression tests for the Report comment pane's row resolution.

use moon_core::db;
use rusqlite::types::Value;

use super::current_comment;
use crate::panels::report::query::ReportData;
use crate::panels::report::selection::{ReportSelection, row_keys};

/// Build two replicated rows whose comments differ, plus one with an empty comment.
///
/// Returns:
///     Runtime columns and the parallel report snapshot with stable identities.
fn fixture() -> (Vec<String>, ReportData) {
    let cols = vec!["id".to_string(), "coin".to_string(), "comment".to_string()];
    let rows = vec![
        vec![
            Value::Integer(101),
            Value::Text("AAA".into()),
            Value::Text("first comment".into()),
        ],
        vec![
            Value::Integer(102),
            Value::Text("BBB".into()),
            Value::Text("second comment".into()),
        ],
        vec![
            Value::Integer(103),
            Value::Text("CCC".into()),
            Value::Text("   ".into()),
        ],
    ];
    let core_uids = vec![1, 1, 1];
    let rec_ids = vec![10, 11, 12];
    let row_keys = row_keys(&cols, &rows, &core_uids, &rec_ids);
    (
        cols,
        ReportData {
            rows,
            core_uids,
            row_keys,
            totals: db::QuoteBreakdown {
                orders: 3,
                ..Default::default()
            },
        },
    )
}

/// The pane must follow the LAST clicked row, not the first selected one. Resolving the comment
/// from `data.rows[0]`, or from any member of the selected set, passes a two-row Shift range but
/// breaks here: the anchor is the second row while the first is also selected.
#[test]
fn the_pane_shows_the_last_clicked_row() {
    let (cols, data) = fixture();
    let mut selection = ReportSelection::default();
    selection.click(data.row_keys[0], &data.row_keys, false, false);
    selection.click(data.row_keys[1], &data.row_keys, false, true);
    assert_eq!(selection.len(), 2, "Ctrl-click must extend the selection");
    assert_eq!(
        current_comment(&data, &cols, &selection).as_deref(),
        Some("second comment")
    );
}

/// A Shift range keeps its anchor at the range BASE so the next Shift click re-measures from there.
/// Resolving the pane from `anchor` would therefore describe the first row of the range while the
/// user just clicked its far end — this asserts the far end wins.
#[test]
fn a_shift_range_shows_the_row_that_ended_it() {
    let (cols, data) = fixture();
    let mut selection = ReportSelection::default();
    selection.click(data.row_keys[0], &data.row_keys, false, false);
    selection.click(data.row_keys[1], &data.row_keys, true, false);
    assert_eq!(selection.len(), 2, "Shift must select the whole range");
    assert_eq!(
        current_comment(&data, &cols, &selection).as_deref(),
        Some("second comment")
    );
}

/// Ctrl-clicking the current row removes it from the selection while `last_clicked` still points
/// at it. Reading that field without the membership check would keep describing a row highlighted
/// nowhere on screen.
#[test]
fn deselecting_the_current_row_empties_the_pane() {
    let (cols, data) = fixture();
    let mut selection = ReportSelection::default();
    selection.click(data.row_keys[0], &data.row_keys, false, false);
    selection.click(data.row_keys[0], &data.row_keys, false, true);
    assert_eq!(selection.len(), 0, "Ctrl-click must remove the row");
    assert_eq!(current_comment(&data, &cols, &selection), None);
}

/// GPUI starts a new visual line on `\n` only. A comment stored with CRLF or lone-CR breaks must
/// come out with plain `\n`, or the pane paints a stray glyph per line — or, for lone CR, folds
/// every line into one.
#[test]
fn carriage_returns_are_normalized_to_line_feeds() {
    let (cols, mut data) = fixture();
    data.rows[0][2] = Value::Text("first\r\nsecond\rthird".into());
    let mut selection = ReportSelection::default();
    selection.click(data.row_keys[0], &data.row_keys, false, false);
    assert_eq!(
        current_comment(&data, &cols, &selection).as_deref(),
        Some("first\nsecond\nthird")
    );
}

/// A whitespace-only comment must read as absent, so the pane shows its "no comment" marker rather
/// than a blank block that looks like a rendering failure.
#[test]
fn a_blank_comment_counts_as_none() {
    let (cols, data) = fixture();
    let mut selection = ReportSelection::default();
    selection.click(data.row_keys[2], &data.row_keys, false, false);
    assert_eq!(current_comment(&data, &cols, &selection), None);
}

/// A core whose report schema carries no comment column must resolve to nothing rather than
/// indexing another column by position.
#[test]
fn a_schema_without_the_column_resolves_to_none() {
    let (_, data) = fixture();
    let cols = vec!["id".to_string(), "coin".to_string(), "note".to_string()];
    let mut selection = ReportSelection::default();
    selection.click(data.row_keys[0], &data.row_keys, false, false);
    assert_eq!(current_comment(&data, &cols, &selection), None);
}
