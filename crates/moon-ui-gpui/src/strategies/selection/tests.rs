//! Regression tests for plain-folder ownership of reorder commands.

use super::retire_strategy_for_folder;
use std::collections::HashSet;

/// Retaining any strategy state after a plain folder click would leave arrows on the old row.
#[test]
fn plain_folder_click_retires_the_previous_strategy() {
    let mut selected = Some((7, 42));
    let mut anchor = selected;
    let mut sel = HashSet::from([(7, 42)]);
    assert!(retire_strategy_for_folder(
        "Folder",
        false,
        false,
        &mut selected,
        &mut anchor,
        &mut sel
    ));
    assert_eq!(selected, None);
    assert_eq!(anchor, None);
    assert!(sel.is_empty());
}

/// Clearing on modified gestures or core clicks would widen the change into existing multiselection.
#[test]
fn modified_and_core_clicks_preserve_strategy_selection() {
    for (path, shift, command) in [
        ("Folder", true, false),
        ("Folder", false, true),
        ("", false, false),
    ] {
        let mut selected = Some((7, 42));
        let mut anchor = selected;
        let mut sel = HashSet::from([(7, 42)]);
        assert!(!retire_strategy_for_folder(
            path,
            shift,
            command,
            &mut selected,
            &mut anchor,
            &mut sel
        ));
        assert_eq!(selected, Some((7, 42)));
        assert_eq!(anchor, selected);
        assert_eq!(sel, HashSet::from([(7, 42)]));
    }
}
