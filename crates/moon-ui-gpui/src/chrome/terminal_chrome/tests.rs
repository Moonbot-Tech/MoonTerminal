//! Regression coverage for terminal-header geometry helpers.

use super::fit_header_core_trigger;

/// `terminal_chrome.rs:fit_header_core_trigger` must add chrome to the fitted label width, not the
/// maximum label budget. Replacing `label_w` with `max_label_w` recreates the large empty tail for
/// ordinary server names while still passing the overflow cap.
#[test]
fn header_core_trigger_follows_content_until_the_label_cap() {
    let measure = |text: &str| text.chars().count() as f32;

    let short = fit_header_core_trigger("AB", 10.0, 4.0, measure);
    let capped = fit_header_core_trigger("ABCDEFGHIJK", 10.0, 4.0, measure);

    assert_eq!(short, ("AB".to_string(), 6.0));
    assert_eq!(capped, ("ABCDEFGHI…".to_string(), 14.0));
}
