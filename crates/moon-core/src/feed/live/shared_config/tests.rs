use moonproto::shared_config::SharedConfig;

use super::{
    FieldMask, MAX_ATTEMPTS, SequenceAction, SharedConfigSequence, apply_core_config,
    core_config_from_proto,
};
use crate::feed::{AutoStartSettings, CoreConfig, CoreConfigEditEvent, CoreConfigEditResult};

/// Core id these tests plan against. Only ever reaches the log line naming the core, so its value
/// is arbitrary; the planner's decisions do not read it.
const TEST_CORE: u64 = 1;

/// Build a write out of a base config's own projection, changed by `mutate`.
fn edit_from(cfg: &SharedConfig, mutate: impl FnOnce(&mut AutoStartSettings)) -> CoreConfig {
    let mut projected = core_config_from_proto(cfg);
    mutate(&mut projected.auto_start);
    projected
}

/// Extract the next full config or fail with the unexpected action.
fn next_config(sequence: &mut SharedConfigSequence, base: &SharedConfig) -> SharedConfig {
    let mut events = Vec::new();
    match sequence.next_action(base, TEST_CORE, &mut events) {
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
    sequence.enqueue(
        edit_from(&base, |s| s.auto_stop_loss = 250.0),
        FieldMask::RENDERED_SECTIONS,
    );
    let sent = next_config(&mut sequence, &base);

    assert_eq!(sent.trading.auto_start.work_time_to, 0.9999);
    assert_eq!(sent.trading.auto_start.auto_stop_loss, 250.0);
}

#[test]
fn changed_work_time_window_is_written() {
    let mut base = SharedConfig::default();
    base.trading.auto_start.work_time_to = 0.9999;

    let mut sequence = SharedConfigSequence::new();
    sequence.enqueue(
        edit_from(&base, |s| s.work_time_to_min = 720),
        FieldMask::RENDERED_SECTIONS,
    );
    let sent = next_config(&mut sequence, &base);

    assert!((sent.trading.auto_start.work_time_to - 0.5).abs() < f64::EPSILON);
}

/// Regression target: dropping the satisfied-edit check makes an OK press that changed nothing send
/// the whole config and wait for an echo that confirms nothing.
#[test]
fn edit_already_reflected_is_dropped_without_a_send() {
    let base = SharedConfig::default();
    let mut sequence = SharedConfigSequence::new();
    sequence.enqueue(edit_from(&base, |_| {}), FieldMask::RENDERED_SECTIONS);

    let mut events = Vec::new();
    assert!(matches!(
        sequence.next_action(&base, TEST_CORE, &mut events),
        SequenceAction::Idle
    ));
}

/// Regression target: removing the echo barrier lets a second OK press build on the pre-edit
/// snapshot, so the first press is silently reverted.
#[test]
fn send_waits_for_the_core_echo() {
    let base = SharedConfig::default();
    let mut sequence = SharedConfigSequence::new();
    sequence.enqueue(
        edit_from(&base, |s| s.errors_level = 9),
        FieldMask::RENDERED_SECTIONS,
    );
    let sent = next_config(&mut sequence, &base);
    sequence.observe_send_success(&sent, 1, FieldMask::RENDERED_SECTIONS);

    // The core has not echoed yet, so the still-stale base must produce no second send.
    let mut events = Vec::new();
    assert!(matches!(
        sequence.next_action(&base, TEST_CORE, &mut events),
        SequenceAction::Idle
    ));

    sequence.observe_update();
    let echoed = {
        let mut cfg = base.clone();
        cfg.trading.auto_start.errors_level = 9;
        cfg
    };
    assert!(matches!(
        sequence.next_action(&echoed, TEST_CORE, &mut events),
        SequenceAction::Idle
    ));
}

/// Regression target: a packet whose echo never arrives must not be reported as a REFUSED write.
///
/// Before `observe_echo_timeout` dropped the confirmation with the barrier, the next plan compared
/// the sent packet against the pre-write base — a mismatch inside the mask by construction — and
/// resolved it as `NotApplied`, so a silent core produced a rejection naming a field it had never
/// answered about at all.
#[test]
fn an_echo_that_never_arrives_is_not_a_rejection() {
    let base = SharedConfig::default();
    let mut sequence = SharedConfigSequence::new();
    let mask = FieldMask::RENDERED_SECTIONS;
    sequence.enqueue(edit_from(&base, |s| s.errors_level = 9), mask);

    let sent = next_config(&mut sequence, &base);
    sequence.observe_send_success(&sent, 1, mask);
    sequence.observe_echo_timeout();

    // The base is unchanged — the core answered nothing — so this plan must only re-send.
    let mut events = Vec::new();
    assert!(matches!(
        sequence.next_action(&base, TEST_CORE, &mut events),
        SequenceAction::Send { .. }
    ));
    assert!(
        events.is_empty(),
        "a timed-out echo must resolve nothing, got {events:?}"
    );
}

/// Regression target: an echo that lands AFTER its timeout still resolves the edit.
///
/// `observe_echo_timeout` drops the confirmation, so a late echo reaches the queue through
/// `edit_satisfied` instead of the comparison. That path must still emit `Confirmed`:
/// `CoreData::core_config_edit` clears on nothing else, so a silent drop leaves the toolbar cell
/// on its pending marker for the rest of the session while the write actually succeeded.
#[test]
fn an_echo_after_the_timeout_still_confirms_the_edit() {
    let base = SharedConfig::default();
    let mut sequence = SharedConfigSequence::new();
    let mask = FieldMask::RENDERED_SECTIONS;
    sequence.enqueue(edit_from(&base, |s| s.errors_level = 9), mask);

    let sent = next_config(&mut sequence, &base);
    sequence.observe_send_success(&sent, 1, mask);
    sequence.observe_echo_timeout();

    // The core did apply it, just later than the timeout allowed: its echo is the packet itself.
    let echoed = sent.clone();
    let mut events = Vec::new();
    assert!(matches!(
        sequence.next_action(&echoed, TEST_CORE, &mut events),
        SequenceAction::Idle
    ));
    assert!(
        matches!(
            events.as_slice(),
            [CoreConfigEditEvent::Resolved(
                CoreConfigEditResult::Confirmed
            )]
        ),
        "a late echo must resolve as confirmed, got {events:?}"
    );
}

/// Regression target: a core that clamps or refuses a value never echoes what was queued, and an
/// unbounded retry then sends the full config on every drive for the rest of the session.
#[test]
fn unconfirmed_edit_is_dropped_after_three_attempts() {
    let base = SharedConfig::default();
    let mut sequence = SharedConfigSequence::new();
    sequence.enqueue(
        edit_from(&base, |s| s.errors_level = 9),
        FieldMask::RENDERED_SECTIONS,
    );

    for _ in 0..MAX_ATTEMPTS {
        let sent = next_config(&mut sequence, &base);
        // Attempts are charged to a SENT packet, so a test that only plans one would loop forever.
        sequence.observe_send_success(&sent, 1, FieldMask::RENDERED_SECTIONS);
        sequence.observe_update();
    }
    let mut events = Vec::new();
    assert!(matches!(
        sequence.next_action(&base, TEST_CORE, &mut events),
        SequenceAction::Idle
    ));
}

/// Regression target: a field added to one of the four popup-rendered `CoreConfig` sections but
/// forgotten in `apply_core_config` reads back with the core's old value, so the echo never
/// matches what was queued and every OK press burns the retry budget before being dropped.
#[test]
fn every_rendered_field_survives_a_write_and_read_back() {
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
    // Every one of the six differs from its partner, so a sell/buy swap or a level/sound mix-up in
    // `apply_signals` shows up as a mismatch instead of round-tripping through itself.
    wanted.signals.play_sell_alert = true;
    wanted.signals.sell_alert_level = 3;
    wanted.signals.signal_sound_2 = 7;
    wanted.signals.play_buy_alert = true;
    wanted.signals.buy_alert_level = 5;
    wanted.signals.buy_signal_sound = 6;

    let mut written = base.clone();
    apply_core_config(&mut written, &wanted, FieldMask::RENDERED_SECTIONS);
    let round_tripped = core_config_from_proto(&written);
    assert_eq!(round_tripped.auto_start, wanted.auto_start);
    assert_eq!(round_tripped.btc_blink, wanted.btc_blink);
    assert_eq!(round_tripped.general, wanted.general);
    assert_eq!(round_tripped.leverage, wanted.leverage);
    assert_eq!(round_tripped.signals, wanted.signals);
}

/// Regression target: `apply_core_config` must not write the manual block AT ALL — the terminal
/// owns those values locally now and delivers them with the order, so a popup OK carrying a stale
/// projection of them must leave the core's own copy exactly as it found it.
#[test]
fn popup_commit_mask_preserves_manual_order_size_changed_after_seed() {
    let base = SharedConfig::default();
    let mut popup_draft = core_config_from_proto(&base);
    popup_draft.general.take_profit_on = true;

    let mut latest = base.clone();
    latest.ui.hotkeys_config.o_size[4] = 640.0;
    apply_core_config(&mut latest, &popup_draft, FieldMask::RENDERED_SECTIONS);

    assert!(latest.trading.use_g_take_profit);
    assert_eq!(latest.ui.hotkeys_config.o_size[4], 640.0);
}
