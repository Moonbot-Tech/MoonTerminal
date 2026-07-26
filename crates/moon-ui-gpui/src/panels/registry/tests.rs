// Explicit imports (not `use super::*`): the crate's panel modules re-export `gpui::*`, whose
// `test` macro would shadow the built-in `#[test]` and recurse. The rule is kept uniform.
use crate::panels::registry::{DOCK_PANELS, home_ordered_names};
use crate::persistence::panel_meta::tab_label;

#[test]
fn every_registry_panel_has_a_localized_label() {
    // Adding a panel to the registry but forgetting its label reddens here rather than shipping an
    // English tab caption. The oracle (panel_meta::tab_label) is an independent file.
    for kind in DOCK_PANELS {
        assert!(
            tab_label(kind.name).is_some(),
            "{} is a registered dock panel but has no entry in panel_meta::tab_label",
            kind.name
        );
    }
}

#[test]
fn panel_names_are_unique() {
    // A copy-paste that duplicates a name would make `find` shadow one entry silently.
    let mut seen = std::collections::HashSet::new();
    for kind in DOCK_PANELS {
        assert!(seen.insert(kind.name), "duplicate panel name {}", kind.name);
    }
}

/// `registry.rs:DOCK_PANELS` changing Log from `Some(6)` to `Some(3)` must fail here; otherwise
/// the diagnostic tab appears before News and Core Status instead of at the end of the home strip.
#[test]
fn home_strip_membership() {
    assert_eq!(
        home_ordered_names(),
        [
            "Orders",
            "Assets",
            "Report",
            "Alerts",
            "News",
            "CoreStatus",
            "Log",
        ]
    );
}
