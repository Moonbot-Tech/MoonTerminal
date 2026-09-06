//! Contract tests for Core Status context-menu update scope resolution.

use crate::controls::row_selection::RowSelection;
use moon_core::session::CoreId;

use super::resolve_menu_scope;

/// Build a selection through the public click rules rather than by reaching into its private set.
fn selected(ids: &[CoreId], selected_ids: &[CoreId]) -> RowSelection<CoreId> {
    let mut selection = RowSelection::default();
    for id in selected_ids {
        selection.click(
            Some(*id),
            &ids.iter().copied().map(Some).collect::<Vec<_>>(),
            false,
            true,
        );
    }
    selection
}

/// `controls/core_update/scope.rs:resolve_menu_scope` must use an in-selection clicked core's
/// whole selection in rendered order and filter stale keys. Iterating the set directly could send
/// updates in a nondeterministic order or target a core that is no longer shown.
#[test]
fn selected_click_uses_the_visible_selection_in_panel_order() {
    let order = [40, 10, 30, 20];
    let mut selection = selected(&order, &[10, 20]);
    let visible = order.map(Some);
    selection.click(Some(99), &visible, false, true);

    let scope = resolve_menu_scope(20, &order, &selection);

    assert_eq!(scope.as_ref(), &[10, 20]);
}

/// `controls/core_update/scope.rs:resolve_menu_scope` must treat an unselected right-click as
/// one core, even when another selection exists. Returning a non-empty selection unconditionally
/// would enqueue real updates on every previously highlighted trading core.
#[test]
fn unselected_click_never_expands_to_an_existing_selection() {
    let order = [40, 10, 30, 20];
    let selection = selected(&order, &[10, 20]);

    let scope = resolve_menu_scope(30, &order, &selection);

    assert_eq!(scope.as_ref(), &[30]);
}

/// `controls/core_update/scope.rs:resolve_menu_scope` must never include a selected id absent
/// from `order` or return an empty scope. Keeping a vanished selection would tell the menu one
/// thing while enqueueing an invisible core that the panel no longer owns.
#[test]
fn stale_selected_identity_is_filtered_from_a_nonempty_visible_scope() {
    let order = [40, 10, 30, 20];
    let selection = selected(&order, &[10, 99]);

    let scope = resolve_menu_scope(99, &order, &selection);

    assert_eq!(scope.as_ref(), &[10]);
    assert!(
        !scope.is_empty(),
        "a menu scope must always name at least one core"
    );
    assert!(scope.iter().all(|id| order.contains(id)));
}
