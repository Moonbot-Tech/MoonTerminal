use super::*;

const W: Window = Window {
    margin_ms: 60_000,
    long_ms: 600_000,
    tail_ms: 70_000,
};

fn id(rec_id: i64) -> TradeId {
    TradeId { core: 1, rec_id }
}

#[test]
fn an_open_trade_needs_the_run_up_and_as_long_as_it_may_still_be_short() {
    let mut plan = KeyPlan::default();
    plan.opened(id(1), 1_000_000);
    assert_eq!(plan.need(W).spans(), &[(940_000, 1_600_000)]);
    assert!(!plan.done(W, i64::MAX));
}

#[test]
fn a_short_trade_needs_one_stretch_through_its_tail() {
    let mut plan = KeyPlan::default();
    plan.opened(id(1), 1_000_000);
    plan.closed(id(1), 1_000_000, 1_300_000);
    assert_eq!(plan.need(W).spans(), &[(940_000, 1_370_000)]);
    assert!(!plan.done(W, 1_369_999));
    assert!(plan.done(W, 1_370_000));
}

#[test]
fn a_long_trade_needs_its_two_ends() {
    let mut plan = KeyPlan::default();
    plan.closed(id(1), 1_000_000, 5_000_000);
    assert_eq!(
        plan.need(W).spans(),
        &[(940_000, 1_600_000), (4_940_000, 5_070_000)]
    );
}

#[test]
fn trades_of_one_key_share_it_until_the_last_is_done() {
    let mut plan = KeyPlan::default();
    plan.closed(id(1), 1_000_000, 1_100_000);
    plan.opened(id(2), 1_150_000);
    assert_eq!(plan.need(W).spans(), &[(940_000, 1_750_000)]);
    assert!(!plan.done(W, 2_000_000));
    plan.closed(id(2), 1_150_000, 1_200_000);
    assert!(plan.done(W, 1_270_000));
}

#[test]
fn an_open_after_its_close_keeps_the_close() {
    let mut plan = KeyPlan::default();
    plan.closed(id(1), 1_000_000, 1_100_000);
    plan.opened(id(1), 1_000_000);
    assert!(plan.done(W, 1_170_000));
}

#[test]
fn an_open_whose_exit_never_came_is_forgotten() {
    let mut plan = KeyPlan::default();
    plan.opened(id(1), 0);
    plan.expire(OPEN_HORIZON_MS);
    assert!(!plan.done(W, i64::MAX));
    plan.expire(OPEN_HORIZON_MS + 1);
    assert!(plan.done(W, 0));
}
