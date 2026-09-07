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

/// A failed subscribe must not be recorded as applied, or the next drain never retries the book.
#[test]
fn reconcile_orderbook_subs_omits_a_failed_subscribe_so_the_next_pass_retries() {
    let desired = vec!["BTCUSDT".to_string(), "ETHUSDT".to_string()];
    let applied: Vec<String> = Vec::new();
    let live =
        super::reconcile_orderbook_subs(&desired, &applied, |market| market == "BTCUSDT", |_| true);
    assert_eq!(live, vec!["BTCUSDT".to_string()]);
}

/// A failed unsubscribe must stay applied, otherwise the book is left subscribed with nothing
/// that will try to drop it again.
#[test]
fn reconcile_orderbook_subs_keeps_a_failed_unsubscribe() {
    let desired = vec!["BTCUSDT".to_string()];
    let applied = vec!["BTCUSDT".to_string(), "ETHUSDT".to_string()];
    let live =
        super::reconcile_orderbook_subs(&desired, &applied, |_| true, |market| market != "ETHUSDT");
    assert_eq!(live, vec!["BTCUSDT".to_string(), "ETHUSDT".to_string()]);
}

/// Both sides succeeding must produce the desired set, sorted, so `needs_apply` can go idle.
#[test]
fn reconcile_orderbook_subs_matches_desired_when_every_call_succeeds() {
    let desired = vec!["AA".to_string(), "BB".to_string()];
    let applied = vec!["BB".to_string(), "CC".to_string()];
    let live = super::reconcile_orderbook_subs(&desired, &applied, |_| true, |_| true);
    assert_eq!(live, desired);
}
