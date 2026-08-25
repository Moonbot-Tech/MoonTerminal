//! Unit proofs for chart-label popup label fitting.

use super::fit_row_name;

/// `labels_popup::fit_row_name` must reserve the suffix before fitting the head: fitting the whole
/// name first makes the rendered label wider than its button and hides the used-parts count.
#[test]
fn fits_name_head_before_pinned_parts_count() {
    let suffix = "  ·5";
    let budget = 11.0;
    let measure = |text: &str| text.chars().count() as f32;

    let fitted = fit_row_name("orders exposure liquidity", suffix, budget, measure);

    assert!(
        fitted.ends_with(suffix),
        "the used-parts count must remain verbatim at the end of the fitted label"
    );
    assert!(
        measure(&fitted) <= budget,
        "the fitted label must stay inside the name button's text budget"
    );
}
