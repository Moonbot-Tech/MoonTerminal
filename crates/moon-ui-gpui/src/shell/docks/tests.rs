// NOT `use super::*`: the glob would pull in the `gpui::test` macro, and `#[test]` would
// expand into itself (recursion limit).
use super::DOCK_TAB_ORDER;
use crate::panel_meta::tab_label;

#[test]
fn every_home_tab_has_a_localized_label() {
    // Pins the link between TWO files rather than the map's own behaviour: adding a panel
    // touches nine places (see CLAUDE.md), and "added the name to DOCK_TAB_ORDER, forgot the
    // label" reddens here. It does NOT claim every panel calls this helper, and it does not
    // check translation completeness in locales/.
    for name in DOCK_TAB_ORDER {
        assert!(
            tab_label(name).is_some(),
            "{name} is a home dock tab but has no entry in panel_meta::tab_label"
        );
    }
}
