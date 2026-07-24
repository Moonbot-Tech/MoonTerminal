// NOT `use super::*`: the glob would pull in the `gpui::test` macro, and `#[test]` would
// expand into itself (recursion limit).
use crate::panels::registry::home_ordered_names;
use crate::persistence::panel_meta::tab_label;

#[test]
fn every_home_tab_has_a_localized_label() {
    // Pins the link between the panel registry and the label map: a home tab whose name has no
    // `tab_label` entry would render an English caption. The broader "every registered panel has a
    // label" check lives in `panels::registry::tests`; this one guards the home strip specifically.
    for name in home_ordered_names() {
        assert!(
            tab_label(name).is_some(),
            "{name} is a home dock tab but has no entry in panel_meta::tab_label"
        );
    }
}
