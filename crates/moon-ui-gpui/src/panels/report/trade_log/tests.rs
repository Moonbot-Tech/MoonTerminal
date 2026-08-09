//! Delayed trade-log identity and publication regressions.

use super::TradeLogWorkspaceIdentity;

/// Removing either check from `TradeLogWorkspaceIdentity::is_current` would publish an old core's
/// scan after the user switched the Auto workspace while the menu or file read was pending.
#[test]
fn delayed_group_request_requires_same_generation_and_core_authority() {
    let captured = TradeLogWorkspaceIdentity::new("alpha".to_string(), 41, 7);

    assert!(captured.is_current(7, true));
    assert!(!captured.is_current(8, true));
    assert!(!captured.is_current(7, false));
}

/// Replacing the `None` branch in `trade_log_request_is_current` with group authority would make
/// Analytics-owned standalone Reports inherit whichever Auto workspace currently has focus.
#[test]
fn standalone_request_has_no_workspace_identity() {
    let request = super::scan::trade_log_request("core", Some("core"), "BTC", 9, 1, 2, 3);
    let actions = include_str!("../actions.rs");
    let binding = actions
        .split("let workspace = if self.standalone {")
        .nth(1)
        .and_then(|tail| tail.split("(config_name, workspace)").next())
        .expect("Report action must bind group authority explicitly");

    assert!(request.workspace.is_none());
    assert!(binding.contains("None\n            } else"));
}

/// Removing either call to `trade_log_request_is_current` from `trade_log.rs:open_trade_log`
/// would open a stale request or publish its old-core scan after authority changed in flight.
#[test]
fn scan_revalidates_before_launch_and_before_completion_publish() {
    let source = include_str!("../trade_log.rs");
    let open = source
        .split("pub(super) fn open_trade_log(")
        .nth(1)
        .and_then(|tail| tail.split("fn trade_log_request_is_current(").next())
        .expect("trade-log open path must exist");
    let first_check = open
        .find("trade_log_request_is_current(&request")
        .expect("request must be checked before launch");
    let entity = open
        .find("let entity = cx.new")
        .expect("dialog construction must remain visible to the guard test");
    let completion_check = open
        .find("trade_log_request_is_current(&completion_request")
        .expect("completion must be revalidated before publishing");
    let publish = open
        .find("this.state = TradeLogState::Ready")
        .expect("completion publication must remain visible to the guard test");

    assert!(first_check < entity);
    assert!(completion_check < publish);
}
