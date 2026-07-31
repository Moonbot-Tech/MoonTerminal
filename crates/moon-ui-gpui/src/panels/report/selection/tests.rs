//! Regression tests for Report row selection, protocol gating, and clipboard projection.

use std::collections::HashSet;

use gpui::SharedString;
use moon_ui::MoonDataTableState;
use rusqlite::types::Value;

use super::{ReportRowKey, ReportSelection, ordered_visible_indices, row_keys, selected_tsv};
use crate::panels::report::query::ReportData;

/// Build a compact current-order fixture with three replicated rows and one read-only legacy row.
///
/// Returns:
///     Runtime columns plus four rows with stable identities in their rendered order.
fn fixture() -> (Vec<String>, ReportData) {
    let cols = vec!["id".to_string(), "coin".to_string(), "comment".to_string()];
    let rows = vec![
        vec![
            Value::Integer(101),
            Value::Text("AAA".into()),
            Value::Text("a".into()),
        ],
        vec![
            Value::Integer(102),
            Value::Text("BBB".into()),
            Value::Text("b".into()),
        ],
        vec![
            Value::Integer(103),
            Value::Text("CCC".into()),
            Value::Text("c".into()),
        ],
        vec![
            Value::Integer(77),
            Value::Text("OLD".into()),
            Value::Text("legacy".into()),
        ],
    ];
    let core_uids = vec![1, 1, 2, 3];
    let rec_ids = vec![10, 11, 20, 0];
    let keys = row_keys(&cols, &rows, &core_uids, &rec_ids);
    (
        cols,
        ReportData {
            rows,
            core_uids,
            row_keys: keys,
            totals: (0.0, 4),
        },
    )
}

/// Changing `selection.rs:ReportSelection::click` so a plain click keeps old rows, Ctrl cannot
/// remove a row, Ctrl clears the set, or Shift toggles only its endpoint must fail the exact
/// selected-key assertions. Those regressions would make the action bar target different trades
/// from the highlighted block.
#[test]
fn plain_ctrl_and_shift_follow_the_visible_row_order() {
    let (_, data) = fixture();
    let mut selection = ReportSelection::default();

    selection.click(data.row_keys[1], &data.row_keys, false, false);
    selection.click(data.row_keys[3], &data.row_keys, false, true);
    assert_eq!(selection.len(), 2, "Ctrl adds the clicked legacy row");
    selection.click(data.row_keys[1], &data.row_keys, false, true);
    assert_eq!(selection.len(), 1, "Ctrl removes an already selected row");
    assert!(selection.contains(data.row_keys[3]));

    selection.click(data.row_keys[2], &data.row_keys, false, false);
    assert_eq!(selection.len(), 1, "plain click replaces the old selection");
    assert!(selection.contains(data.row_keys[2]));
    selection.click(data.row_keys[3], &data.row_keys, false, true);

    selection.click(data.row_keys[0], &data.row_keys, true, true);
    assert_eq!(
        selection.len(),
        4,
        "Shift wins over Ctrl and spans from the last clicked row"
    );
    assert!(selection.contains(data.row_keys[0]));
    assert!(selection.contains(data.row_keys[1]));
    assert!(selection.contains(data.row_keys[2]));
    assert!(selection.contains(data.row_keys[3]));
}

/// Changing `selection.rs:ReportSelection::select_all` to skip legacy rows, retain only the
/// viewport, or replace the last real-click anchor must fail these assertions. Those regressions
/// would make Ctrl+A disagree with the highlighted table or make the next Shift click start from
/// a synthetic row.
#[test]
fn select_all_uses_every_stable_table_row_and_preserves_the_click_anchor() {
    let (_, data) = fixture();
    let mut selection = ReportSelection::default();
    selection.click(data.row_keys[1], &data.row_keys, false, false);
    let mut order = data.row_keys.clone();
    order.push(None);
    order.push(data.row_keys[0]);

    selection.select_all(&order);

    assert_eq!(selection.len(), 4);
    assert!(
        selection.contains(data.row_keys[3]),
        "legacy rows remain copyable"
    );
    selection.click(data.row_keys[3], &data.row_keys, true, false);
    assert_eq!(selection.len(), 3, "Shift keeps the last real-click anchor");
    assert!(!selection.contains(data.row_keys[0]));

    let mut without_anchor = ReportSelection::default();
    without_anchor.select_all(&data.row_keys);
    without_anchor.click(data.row_keys[2], &data.row_keys, true, false);
    assert_eq!(
        without_anchor.len(),
        1,
        "Ctrl+A must not synthesize a Shift anchor"
    );
    assert!(without_anchor.contains(data.row_keys[2]));
}

/// Changing `selection.rs:ReportSelection::mutation_targets` to address a legacy row by its local
/// database id must add core 3 and fail. Command 48 accepts only protocol `newRecID` values, so
/// sending legacy id 77 could mutate an unrelated replicated trade on that core.
#[test]
fn mutation_targets_exclude_legacy_rows_without_new_rec_id() {
    let (_, data) = fixture();
    let mut selection = ReportSelection::default();
    for key in &data.row_keys {
        selection.click(*key, &data.row_keys, false, true);
    }

    let targets = selection.mutation_targets(&data);

    assert_eq!(targets.len(), 2);
    assert_eq!(targets.get(&1), Some(&vec![10, 11]));
    assert_eq!(targets.get(&2), Some(&vec![20]));
    assert!(
        !targets.contains_key(&3),
        "legacy row must never be mutated"
    );
}

/// Changing `selection.rs:ordered_visible_indices` to ignore MoonDataTable's retained
/// `column_order` must put `id` before `coin` and fail the whole TSV assertion. Leaving tab or
/// newline characters inside a cell must split the pasted selection into extra cells or rows.
#[test]
fn clipboard_tsv_uses_visual_column_and_row_order_without_embedded_delimiters() {
    let (cols, mut data) = fixture();
    data.rows[0][2] = Value::Text("left\tright\r\nnext".into());
    let mut selection = ReportSelection::default();
    selection.click(data.row_keys[2], &data.row_keys, false, false);
    selection.click(data.row_keys[0], &data.row_keys, false, true);
    let visible = HashSet::from(["id".to_string(), "coin".to_string(), "comment".to_string()]);
    let order = vec![
        SharedString::from("coin"),
        SharedString::from("comment"),
        SharedString::from("id"),
    ];
    let mut state = MoonDataTableState::new();
    state.column_order = order;
    let indices = ordered_visible_indices(&cols, &visible, &state);

    let copied = selected_tsv(&data, &cols, &indices, &selection);

    assert_eq!(
        copied,
        "coin\tcomment\tid\r\nAAA\tleft right next\t101\r\nCCC\tc\t103"
    );
}

/// Changing `selection.rs:row_keys` to key a legacy row by its visible index must fail this direct
/// identity assertion. A background sort would otherwise make an unchanged selection point at a
/// different historical trade.
#[test]
fn legacy_identity_uses_core_and_database_id() {
    let (_, data) = fixture();

    assert_eq!(
        data.row_keys[3],
        Some(ReportRowKey::Legacy {
            core_uid: 3,
            db_id: 77,
        })
    );
}
