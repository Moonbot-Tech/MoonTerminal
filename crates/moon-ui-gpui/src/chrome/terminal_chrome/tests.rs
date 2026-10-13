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

/// `terminal_chrome.rs` must keep stale header callbacks passive in Auto and revalidate ticker
/// navigation against the current rail-owned core.
///
/// Mutation: restore `select_auto_workspace_core` in the selector callback or replace the ticker's
/// authorized request with `open_on_main`. Either change lets header chrome bypass the Shell rail.
#[test]
fn header_callbacks_cannot_bypass_auto_rail_authority() {
    let source = include_str!("../terminal_chrome.rs");
    let selector = source
        .split("fn core_selector(")
        .nth(1)
        .expect("core selector must exist");

    assert!(!selector.contains("select_auto_workspace_core"));
    assert!(selector.contains("if b.workspace_mode(&group) == WorkspaceMode::AutoTrading"));
    assert!(source.contains("b.open_on_main_if_authorized(Some(&group), (core, market), false)"));
}
