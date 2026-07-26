use super::*;

/// `market_role.rs:MarketRoleState::default` pre-filling an account-only desired plan would make
/// the first coordinator assignment a no-op and leave unsolicited TradesStream packets flowing.
#[test]
fn first_account_only_assignment_is_actionable_and_repeats_are_idempotent() {
    let mut role = MarketRoleState::default();

    assert!(role.update(false, Vec::new(), Vec::new()));
    assert!(!role.update(false, Vec::new(), Vec::new()));
}

/// `market_role.rs:begin_client` clearing `desired` would lose the coordinator's only
/// account-only assignment after a failed Init and application-level retry.
#[test]
fn a_new_client_reapplies_the_retained_complete_plan() {
    let mut role = MarketRoleState {
        desired: Some(MarketPlan::new(false, Vec::new(), Vec::new())),
        applied: Some(MarketPlan::new(false, Vec::new(), Vec::new())),
    };
    role.begin_client();

    assert!(role.applied.is_none());
    assert!(role.needs_apply());
}
