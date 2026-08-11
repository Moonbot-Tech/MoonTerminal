//! Static regressions for delayed Orders-table action authority.

// Named imports, never a glob: `table.rs` pulls in the GPUI prelude, whose own `test` attribute
// macro would shadow the standard one and blow the expansion recursion limit.
use super::{stop_look, MoonTone, OrderStopKind};

/// Every stop cell reads ON or OFF — a row must never show a stop's state as unknown.
///
/// Whether the stop is the order's own or the one its strategy supplies before the fill is settled
/// in the feed (`stop_inherited_from_strategy`); by the time a cell draws it, the question "will it
/// act?" already has an answer, and a dash in the column would only hide it.
///
/// Mutation: add a third label for some sub-state. Working orders would show a placeholder instead
/// of the protection their strategy is about to apply.
///
/// Returns:
///     Nothing; both flag values map to a definite label.
#[test]
fn every_stop_cell_reads_on_or_off() {
    for kind in [
        OrderStopKind::StopLoss,
        OrderStopKind::Trailing,
        OrderStopKind::VStop,
    ] {
        assert_eq!(stop_look(kind, true).0, "ON");
        assert_eq!(stop_look(kind, false).0, "OFF");
    }
}

/// Only a stop-loss is drawn as a danger: it is the one whose absence leaves a position exposed.
///
/// Mutation: tone TS or VStop as Danger, or soften SL. The row would either cry wolf on every
/// order without a trailing stop or stop flagging a genuinely unprotected position.
///
/// Returns:
///     Nothing; tones follow the stop kind.
#[test]
fn only_stop_loss_off_is_toned_as_danger() {
    assert!(matches!(
        stop_look(OrderStopKind::StopLoss, false).1,
        MoonTone::Danger
    ));
    assert!(matches!(
        stop_look(OrderStopKind::Trailing, false).1,
        MoonTone::Muted
    ));
    assert!(matches!(
        stop_look(OrderStopKind::VStop, false).1,
        MoonTone::Muted
    ));
    assert!(matches!(
        stop_look(OrderStopKind::StopLoss, true).1,
        MoonTone::Positive
    ));
}

/// `table.rs:flag_toggle_cell` must authorize its captured core inside the Backend update before
/// sending the stop command, and must not publish an optimistic overlay when authority is stale.
///
/// Mutation: insert the overlay before validation or send `set_order_stop` before the guard. Auto
/// could display or submit a stop change for core 7 after the group switched to core 9.
///
/// Returns:
///     Nothing; authority must precede both the command and optimistic UI state.
#[test]
fn stop_toggle_authorizes_before_command_and_optimistic_overlay() {
    let source = include_str!("../table.rs");
    let callback = source
        .split_once("fn flag_toggle_cell(")
        .expect("Orders stop-toggle cell must exist")
        .1;
    let backend_update = callback
        .find("this.backend.update(cx, |b, _|")
        .expect("stop dispatch must use one Backend update");
    let authority = callback
        .find("b.workspace_action_allows_core(Some(&group), core)")
        .expect("stop dispatch must validate current group authority");
    let command = callback
        .find("b.session.set_order_stop(")
        .expect("stop command must remain reachable");
    let overlay = callback
        .find("this.stop_overlay.insert(")
        .expect("authorized stop dispatch must retain its optimistic overlay");

    assert!(backend_update < authority && authority < command && command < overlay);
}

/// Orders must leave Auto selection to the Shell rail and revalidate chart/strategy navigation.
///
/// Mutation: restore the Core-cell Auto writer, call `open_on_main`, or omit the group argument to
/// `open_goto`. A retained row could then select or reveal its old core after a rail switch.
#[test]
fn row_shortcuts_preserve_rail_authority_at_dispatch() {
    let table = include_str!("../table.rs");
    let panel = include_str!("../mod.rs");

    assert!(!panel.contains("select_auto_workspace_core"));
    assert!(table.contains("b.open_on_main_if_authorized("));
    assert!(table.contains("Some(&group)"));
    assert!(table.contains("Some(workspace_group),"));
}
