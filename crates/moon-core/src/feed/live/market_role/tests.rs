use super::*;

/// `market_role.rs:MarketRoleState::default` pre-filling an account-only desired plan would make
/// the first coordinator assignment a no-op and leave unsolicited TradesStream packets flowing.
#[test]
fn first_account_only_assignment_is_actionable_and_repeats_are_idempotent() {
    let mut role = MarketRoleState::default();

    assert!(role.update(false, Vec::new(), Vec::new()));
    assert!(!role.update(false, Vec::new(), Vec::new()));
}

/// `market_role.rs:begin_connection` clearing `desired` would lose the coordinator's only
/// account-only assignment after a failed Init and application-level retry.
#[test]
fn a_new_connection_reapplies_the_retained_complete_plan() {
    let mut role = MarketRoleState {
        desired: Some(MarketPlan::new(false, Vec::new(), Vec::new())),
        applied: Some(MarketPlan::new(false, Vec::new(), Vec::new())),
        operational: true,
    };
    role.begin_connection();
    assert!(role.applied.is_none());
    role.operational = true;

    assert!(role.needs_apply());
}

/// `market_role.rs:set_non_operational` retaining `applied` would suppress the account-only
/// unsubscribe after MoonProto reconnects and can resume a stale remote stream.
#[test]
fn reconnect_invalidates_the_connection_local_application() {
    let plan = MarketPlan::new(false, Vec::new(), Vec::new());
    let mut role = MarketRoleState {
        desired: Some(plan.clone()),
        applied: Some(plan),
        operational: true,
    };

    role.set_non_operational();
    role.operational = true;

    assert!(role.needs_apply());
}
