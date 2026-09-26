//! The range cells' reading of what was typed.

use super::{RANGE_INPUT_PREFIX, Slot, parse_cell, range_input_id};

/// An empty cell is automatic; a comma is the decimal point; text that is no number is refused,
/// not read as zero.
#[test]
fn a_cell_reads_empty_as_automatic_and_refuses_what_is_no_number() {
    assert_eq!(parse_cell(""), Ok(None));
    assert_eq!(parse_cell("  "), Ok(None));
    assert_eq!(parse_cell("1,5"), Ok(Some(1.5)));
    assert_eq!(parse_cell("-0.2"), Ok(Some(-0.2)));
    assert_eq!(parse_cell("1.5%"), Err(()));
    assert_eq!(parse_cell("abc"), Err(()));
    assert_eq!(parse_cell("inf"), Err(()));
}

/// Each cell of each field keeps a box of its own, apart from the variant cells'.
#[test]
fn every_cell_has_its_own_box_id() {
    let ids: Vec<String> = Slot::ALL
        .iter()
        .map(|slot| range_input_id("SellPrice", *slot))
        .collect();
    assert_eq!(ids.len(), 3);
    assert!(ids.iter().all(|id| id.starts_with(RANGE_INPUT_PREFIX)));
    assert_ne!(ids[0], ids[1]);
    assert_ne!(
        range_input_id("SellPrice", Slot::From),
        range_input_id("StopLoss", Slot::From)
    );
    assert!(
        !ids.iter()
            .any(|id| id.starts_with(super::super::grid::VARIANT_INPUT_PREFIX))
    );
}
