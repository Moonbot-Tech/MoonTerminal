//! Regression coverage for Report natural-width purpose bounds.

use super::{natural_widths_environment, width_bounds};
use crate::design::MonoBodyFontSignature;
use gpui::FontId;

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

/// Removing locale from `report/widths.rs:NaturalWidthsEnvironment` must make this assertion fail:
/// a live language change would otherwise reuse widths measured for the previous translated headers.
#[test]
fn natural_width_environment_changes_with_locale_or_resolved_font() {
    let environment = |locale: &str, normal: usize| {
        natural_widths_environment(
            locale,
            MonoBodyFontSignature {
                normal: FontId(normal),
                semibold: FontId(2),
                size_bits: 12.0_f32.to_bits(),
            },
        )
    };

    assert_ne!(environment("ru", 1), environment("en", 1));
    assert_ne!(environment("ru", 1), environment("ru", 3));
}
