//! The range cells' reading of what was typed.

use super::{RANGE_INPUT_PREFIX, Slot, parse_cell, range_input_id};

/// A generic population span would search unrelated formula scales instead of local thresholds.
#[test]
fn custom_ema_ui_range_matches_slot_grid() {
    use moon_core::db::tuner::ticks::params::range::{TickRange, custom_ema_slot_grid, field_span};
    let span = field_span(&[-100.0, 100.0], None, &[0.2, 0.3]).unwrap();
    let resolved = super::resolve_field(
        "SellByCustomEMA#1",
        Some(&span),
        &TickRange::default(),
        false,
        20,
    );
    assert_eq!(resolved.points, custom_ema_slot_grid(&[0.2, 0.3]));
    assert_eq!(
        resolved.points.as_ref(),
        &[
            0.1, 0.15, 0.18, 0.2, 0.22, 0.225, 0.25, 0.27, 0.3, 0.33, 0.375, 0.4, 0.45
        ]
    );
    let typed = TickRange {
        from: Some(0.2),
        to: Some(0.4),
        step: Some(0.1),
    };
    assert_eq!(
        super::resolve_field("SellByCustomEMA#1", Some(&span), &typed, false, 20)
            .points
            .as_ref(),
        &[0.2, 0.3, 0.4]
    );
}

/// Dropping the selected value or omitting a default-only field would silently narrow a batch search.
#[test]
fn automatic_batch_grids_cover_own_values_and_default_only_fields() {
    use moon_core::db::tuner::ticks::TICK_PARAMS;
    use std::collections::HashMap;
    let own = HashMap::from([("SellPrice".into(), "9.7".into())]);
    let defaults = HashMap::from([("sellprice".into(), 1.0), ("stoploss".into(), -3.0)]);
    let grids = super::auto_grids_for("Spread", &own, &defaults);
    let sell = grids.values(TICK_PARAMS.iter().find(|f| f.key == "SellPrice").unwrap());
    assert!(!sell.is_empty());
    assert!(sell.first().unwrap() <= &9.7 && sell.last().unwrap() >= &9.7);
    let stop = grids.values(TICK_PARAMS.iter().find(|f| f.key == "StopLoss").unwrap());
    assert!(!stop.is_empty());
}

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
