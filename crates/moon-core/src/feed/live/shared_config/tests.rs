use moonproto::shared_config::SharedConfig;

use super::{
    apply_core_config, core_config_from_proto, SequenceAction, SharedConfigSequence, MAX_ATTEMPTS,
};
use crate::feed::{AutoStartSettings, CoreConfig};

/// Build a write out of a base config's own projection, changed by `mutate`.
fn edit_from(cfg: &SharedConfig, mutate: impl FnOnce(&mut AutoStartSettings)) -> CoreConfig {
    let mut projected = core_config_from_proto(cfg);
    mutate(&mut projected.auto_start);
    projected
}

/// Extract the next full config or fail with the unexpected action.
fn next_config(sequence: &mut SharedConfigSequence, base: &SharedConfig) -> SharedConfig {
    match sequence.next_action(base) {
        SequenceAction::Send { config, .. } => *config,
        SequenceAction::Idle => panic!("expected a shared config send"),
    }
}

/// Regression target: writing `work_time_from` back from the projected minute value on every OK
/// press walks the core's own boundary (0.9999 -> 0.99930...) even when the user never opened the
/// time control.
#[test]
fn unchanged_work_time_window_is_not_rewritten() {
    let mut base = SharedConfig::default();
    base.trading.auto_start.work_time_from = 0.0;
    base.trading.auto_start.work_time_to = 0.9999;

    let mut sequence = SharedConfigSequence::new();
    sequence.enqueue(edit_from(&base, |s| s.auto_stop_loss = 250.0));
    let sent = next_config(&mut sequence, &base);

    assert_eq!(sent.trading.auto_start.work_time_to, 0.9999);
    assert_eq!(sent.trading.auto_start.auto_stop_loss, 250.0);
}

#[test]
fn changed_work_time_window_is_written() {
    let mut base = SharedConfig::default();
    base.trading.auto_start.work_time_to = 0.9999;

    let mut sequence = SharedConfigSequence::new();
    sequence.enqueue(edit_from(&base, |s| s.work_time_to_min = 720));
    let sent = next_config(&mut sequence, &base);

    assert!((sent.trading.auto_start.work_time_to - 0.5).abs() < f64::EPSILON);
}

/// Regression target: dropping the satisfied-edit check makes an OK press that changed nothing send
/// the whole config and wait for an echo that confirms nothing.
#[test]
fn edit_already_reflected_is_dropped_without_a_send() {
    let base = SharedConfig::default();
    let mut sequence = SharedConfigSequence::new();
    sequence.enqueue(edit_from(&base, |_| {}));

    assert!(matches!(sequence.next_action(&base), SequenceAction::Idle));
}

/// Regression target: removing the echo barrier lets a second OK press build on the pre-edit
/// snapshot, so the first press is silently reverted.
#[test]
fn send_waits_for_the_core_echo() {
    let base = SharedConfig::default();
    let mut sequence = SharedConfigSequence::new();
    sequence.enqueue(edit_from(&base, |s| s.errors_level = 9));
    let sent = next_config(&mut sequence, &base);
    sequence.observe_send_success(&sent, 1);

    // The core has not echoed yet, so the still-stale base must produce no second send.
    assert!(matches!(sequence.next_action(&base), SequenceAction::Idle));

    sequence.observe_update();
    let echoed = {
        let mut cfg = base.clone();
        cfg.trading.auto_start.errors_level = 9;
        cfg
    };
    assert!(matches!(
        sequence.next_action(&echoed),
        SequenceAction::Idle
    ));
}

/// Regression target: a core that clamps or refuses a value never echoes what was queued, and an
/// unbounded retry then sends the full config on every drive for the rest of the session.
#[test]
fn unconfirmed_edit_is_dropped_after_three_attempts() {
    let base = SharedConfig::default();
    let mut sequence = SharedConfigSequence::new();
    sequence.enqueue(edit_from(&base, |s| s.errors_level = 9));

    for _ in 0..MAX_ATTEMPTS {
        let sent = next_config(&mut sequence, &base);
        // Attempts are charged to a SENT packet, so a test that only plans one would loop forever.
        sequence.observe_send_success(&sent, 1);
        sequence.observe_update();
    }
    assert!(matches!(sequence.next_action(&base), SequenceAction::Idle));
}

/// Regression target: a field added to [`CoreConfig`] but forgotten in `apply_core_config` reads
/// back with the core's old value, so the echo never matches what was queued and every OK press
/// burns the retry budget before being dropped. Round-tripping the whole projection catches that
/// the moment the field is added rather than on a live core.
#[test]
fn every_projected_field_survives_a_write_and_read_back() {
    let base = SharedConfig::default();
    let mut wanted = core_config_from_proto(&base);

    wanted.auto_start.auto_start = !wanted.auto_start.auto_start;
    wanted.auto_start.work_time_from_min = 615;
    wanted.auto_start.auto_stop_loss = 1234.5;
    wanted.auto_start.errors_level = 9;
    wanted.btc_blink.blink_btc = !wanted.btc_blink.blink_btc;
    wanted.btc_blink.blink_btc_delta = -2.25;
    wanted.btc_blink.alarm_type = 3;
    wanted.general.take_profit_on = !wanted.general.take_profit_on;
    wanted.general.take_profit_pct = 3.8;
    wanted.general.trailing_on = !wanted.general.trailing_on;
    wanted.general.trailing_pct = -0.7;
    wanted.general.vstop_on = !wanted.general.vstop_on;
    wanted.general.vol_drop_level = -12;
    wanted.general.buy_iceberg = !wanted.general.buy_iceberg;
    wanted.general.sell_iceberg = !wanted.general.sell_iceberg;
    wanted.general.blacklist_on = !wanted.general.blacklist_on;
    wanted.general.blacklist_text = "ALPACA,hmstr".to_string();
    wanted.general.exclude_blacklisted_from_deltas =
        !wanted.general.exclude_blacklisted_from_deltas;
    wanted.leverage.auto_max_order = !wanted.leverage.auto_max_order;
    wanted.leverage.auto_isolated = !wanted.leverage.auto_isolated;
    wanted.leverage.fix_lev = 25;
    wanted.leverage.auto_fix_lev = true;
    wanted.leverage.lev_control = "x2".to_string();

    let mut written = base.clone();
    apply_core_config(&mut written, &wanted);
    assert_eq!(core_config_from_proto(&written), wanted);
}
