//! Regression tests for detection presentation scoping.

use super::detection_core_visible;

/// `detects/mod.rs:ingest` filtering by the effective Auto core would advance every cursor while
/// dropping hidden cards, so returning to Overview could neither reveal nor replay those detects.
#[test]
fn presentation_scope_keeps_hidden_detection_cards_retained() {
    let retained = vec![11, 22, 11];

    let selected: Vec<u64> = retained
        .iter()
        .copied()
        .filter(|core| detection_core_visible(*core, &[22]))
        .collect();
    assert_eq!(selected, vec![22]);
    assert_eq!(retained, vec![11, 22, 11]);
    assert!(
        retained
            .iter()
            .all(|core| detection_core_visible(*core, &[11, 22]))
    );

    let src = include_str!("mod.rs");
    let ingest = src
        .split("fn ingest(")
        .nth(1)
        .and_then(|tail| tail.split("\n    }").next())
        .expect("Detects ingest must exist");
    assert!(ingest.contains(".filter(|s| s.group == self.group)"));
    assert!(!ingest.contains("effective_workspace_scope"));

    let render = src
        .split("impl Render for DetectsPanel")
        .nth(1)
        .expect("Detects render must exist");
    assert!(render.contains("effective_workspace_scope"));
    assert!(render.contains("detection_core_visible(item.core, &visible_cores)"));
}

/// Detect cards must validate Main/Compare authority before removing their retained card.
///
/// Mutation: move either `retain` call before its authorized request. A stale card click would
/// disappear and navigate to a core hidden by the current rail selection.
#[test]
fn stale_detect_navigation_is_rejected_before_card_removal() {
    let source = include_str!("mod.rs");
    for (method, authority) in [
        ("fn open(&mut self", "open_on_main_if_authorized"),
        ("fn open_compare(&mut self", "open_compare_if_authorized"),
    ] {
        let body = source
            .split(method)
            .nth(1)
            .expect("Detect navigation method must exist");
        let guard = body.find(authority).expect("workspace guard must exist");
        let removal = body.find("self.items").expect("card removal must remain");
        assert!(
            guard < removal,
            "{method} removes a stale card before authority"
        );
    }
}
