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

#[test]
fn home_strip_membership() {
    // CoreStatus is intentionally not a default bottom tab (home_order None).
    assert!(!home_ordered_names().contains(&"CoreStatus"));
    // The default trading tabs must remain in the strip; dropping one to None here would silently
    // remove it from a fresh layout.
    for name in ["Orders", "Assets", "Report", "Alerts", "Log"] {
        assert!(
            home_ordered_names().contains(&name),
            "{name} missing from the default home strip"
        );
    }
}
