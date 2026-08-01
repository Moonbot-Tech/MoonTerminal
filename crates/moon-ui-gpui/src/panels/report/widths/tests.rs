//! Regression coverage for Report natural-width purpose bounds.

use super::width_bounds;

/// Removing the free-text ceilings in `report::widths::width_bounds` must fail this assertion;
/// otherwise one long comment can consume the standalone Report viewport and hide useful columns.
#[test]
fn free_text_columns_have_larger_but_bounded_widths() {
    let comment = width_bounds("comment");
    let profit = width_bounds("profitpct");

    assert!(
        comment.1 > profit.1,
        "comments may use more room than numbers"
    );
    assert_eq!(comment.1, 360.0, "comments remain capped");
    assert!(
        profit.0 >= 78.0,
        "signed percentages retain a readable floor"
    );
}
