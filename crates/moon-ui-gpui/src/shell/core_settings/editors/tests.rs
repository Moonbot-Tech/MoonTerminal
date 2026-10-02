//! Regression coverage for ranges outside MoonUI's default slider bounds.

use super::slider_with_bounds;

/// Installing the real maximum first panics for the trailing-stop range against the default zero.
#[test]
fn negative_range_initializes_without_a_thumb_clamp_panic() {
    assert!(
        std::panic::catch_unwind(|| slider_with_bounds(-10.0, -0.1).default_value(-5.0)).is_ok()
    );
}

/// Installing the minimum first panics for the book-width range against the default maximum 100.
#[test]
fn range_above_default_max_initializes_without_a_thumb_clamp_panic() {
    assert!(
        std::panic::catch_unwind(|| slider_with_bounds(120.0, 600.0).default_value(220.0)).is_ok()
    );
}
