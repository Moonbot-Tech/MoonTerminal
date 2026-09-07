use super::{
    ClientSettingsSequence, GroupExitSettings, ManualOrder, SequenceAction, TakeProfitMode,
    client_settings_from_proto,
};
use crate::feed::ClientSettingsEdit;

/// Core id the log lines in this module are stamped with; no assertion depends on it.
const TEST_CORE: u64 = 1;

/// Build a valid visible settings generation for serializer tests.
fn exit_settings(tp: f64, stop_market: bool) -> GroupExitSettings {
    GroupExitSettings {
        take_profit_pct: tp,
        take_profit_mode: TakeProfitMode::Normal,
        fixed_sell_pcts: [2.0, 3.0, 4.0, 5.0, 6.0, 7.0],
        fixed_sell_slot: Some(2),
        stop_loss_pct: -4.0,
        stop_loss_enabled: true,
        use_stop_market: stop_market,
    }
}

/// Extract the next full settings packet or fail with the unexpected action.
fn next_settings(
    sequence: &mut ClientSettingsSequence,
    snapshot: &moonproto::ClientSettingsCommand,
) -> moonproto::ClientSettingsCommand {
    match sequence.next_action(snapshot, TEST_CORE) {
        SequenceAction::Send { settings, .. } => settings,
        _ => panic!("expected a full ClientSettings send"),
    }
}

#[test]
/// Regression target: making `ClientSettingsSequence::next_action` apply only the newest mutation
/// drops an earlier targeted or blacklist edit, so the user's next order uses stale settings.
fn full_settings_mutations_compose_before_the_echo() {
    let base = moonproto::ClientSettingsCommand::default();
    let exit = exit_settings(12.0, true);
    let mut sequence = ClientSettingsSequence::new();
    sequence.enqueue_group_exit(exit);
    sequence.enqueue_edit(ClientSettingsEdit::SignOrders(true));
    sequence.enqueue_blacklist(true, "SCAM,TEST".to_string());

    let sent = next_settings(&mut sequence, &base);
    let projected = client_settings_from_proto(&sent);
    assert_eq!(projected.group_exit_settings(), exit);
    assert!(projected.sign_orders);
    assert!(projected.use_blacklist);
    assert_eq!(projected.blacklist_text, "SCAM,TEST");

    sequence.observe_update();
    assert!(matches!(
        sequence.next_action(&sent, TEST_CORE),
        SequenceAction::Idle
    ));
}

/// Regression target: removing the `x_tmode = false` write from
/// `feed::live::convert::apply_client_settings_edit` makes a Scalp generation inherit Extended
/// fixed-sell encoding, so its projected echo no longer equals the local S1-S6 values.
#[test]
fn scalp_generation_clears_extended_fixed_sell_encoding() {
    let mut base = moonproto::ClientSettingsCommand::default();
    base.x_tmode = true;
    let exit = GroupExitSettings {
        fixed_sell_pcts: [1.0, 2.0, 3.0, 4.0, 5.0, 6.0],
        ..GroupExitSettings::default()
    };
    let mut sequence = ClientSettingsSequence::new();
    sequence.enqueue_group_exit(exit);

    let sent = next_settings(&mut sequence, &base);

    assert!(!sent.x_tmode);
    assert_eq!(
        client_settings_from_proto(&sent).group_exit_settings(),
        exit
    );
}

/// Regression target: clearing `waiting_for_echo` in `enqueue_edit` lets a later full snapshot
/// overtake an unconfirmed group exit, so the later packet can erase the user's visible settings.
#[test]
fn commands_arriving_during_an_inflight_generation_wait_for_its_echo() {
    let base = moonproto::ClientSettingsCommand::default();
    let exit = exit_settings(12.0, true);
    let mut sequence = ClientSettingsSequence::new();
    sequence.enqueue_group_exit(exit);

    let first_sent = next_settings(&mut sequence, &base);
    sequence.observe_send_success(&first_sent, 1, Vec::new());
    sequence.enqueue_edit(ClientSettingsEdit::SignOrders(true));
    assert!(matches!(
        sequence.next_action(&base, TEST_CORE),
        SequenceAction::Idle
    ));

    sequence.observe_update();
    let second_sent = next_settings(&mut sequence, &first_sent);
    let projected = client_settings_from_proto(&second_sent);
    assert_eq!(projected.group_exit_settings(), exit);
    assert!(projected.sign_orders);
}

/// Regression target: confirming a composed packet without retiring its entire mutation prefix
/// leaves an older conflicting TP at the queue front and wedges every later order behind it.
#[test]
fn a_confirmed_packet_retires_all_mutations_it_composed() {
    let base = moonproto::ClientSettingsCommand::default();
    let first = exit_settings(10.0, false);
    let second = exit_settings(20.0, true);
    let mut sequence = ClientSettingsSequence::new();
    sequence.enqueue_group_exit(first);
    sequence.enqueue_group_exit(second);

    let sent = next_settings(&mut sequence, &base);
    sequence.observe_send_success(&sent, 2, Vec::new());
    sequence.observe_update();
    assert!(matches!(
        sequence.next_action(&sent, TEST_CORE),
        SequenceAction::Idle
    ));
}

/// Regression target: retaining the connection-local echo wait across `live::run` reconnects leaves
/// queued edits permanently idle because the dropped client can no longer emit their confirmation.
#[test]
fn reconnect_retries_unconfirmed_mutations_without_losing_them() {
    let base = moonproto::ClientSettingsCommand::default();
    let exit = exit_settings(12.0, true);
    let mut sequence = ClientSettingsSequence::new();
    sequence.enqueue_group_exit(exit);

    let sent = next_settings(&mut sequence, &base);
    sequence.observe_send_success(&sent, 1, Vec::new());
    sequence.prepare_reconnect();

    let retried = next_settings(&mut sequence, &base);
    assert_eq!(
        client_settings_from_proto(&retried).group_exit_settings(),
        exit
    );
}

#[test]
/// Regression target: removing the order barrier in `ClientSettingsSequence::next_action` lets the
/// second chart generation overtake the first, so a rapid double-click receives another TP/SL.
fn orders_release_after_their_own_confirmed_generation() {
    let base = moonproto::ClientSettingsCommand::default();
    let first = exit_settings(10.0, false);
    let second = exit_settings(20.0, true);
    let mut sequence = ClientSettingsSequence::new();
    sequence.enqueue_order(ManualOrder {
        market: "ETHBTC".to_string(),
        short: false,
        price: 0.04,
        size: 0.25,
        strategy_id: None,
        exit: first,
        planned_sell: 0.0,
        sync_exit: true,
    });
    sequence.enqueue_order(ManualOrder {
        market: "BTCUSDT".to_string(),
        short: true,
        price: 100_000.0,
        size: 0.001,
        strategy_id: None,
        exit: second,
        planned_sell: 0.0,
        sync_exit: true,
    });

    let first_echo = next_settings(&mut sequence, &base);
    sequence.observe_update();
    match sequence.next_action(&first_echo, TEST_CORE) {
        SequenceAction::Place(order) => assert_eq!(order.exit, first),
        _ => panic!("first order did not release after its settings echo"),
    }
    let second_echo = next_settings(&mut sequence, &first_echo);
    sequence.observe_update();
    match sequence.next_action(&second_echo, TEST_CORE) {
        SequenceAction::Place(order) => assert_eq!(order.exit, second),
        _ => panic!("second order did not release after its settings echo"),
    }
}

/// Regression target: a core that never echoes the exit generation back — which is what one does
/// while a manual strategy owns the sell price and it keeps rewriting the field under us — held the
/// order behind it FOREVER. The terminal then re-sent the same settings several times a second and
/// the trader's chart click did nothing at all, with no error anywhere.
#[test]
fn an_order_is_released_after_its_generation_is_refused_three_times() {
    // A core that always answers with its own values, never with what was sent.
    let core_settings = moonproto::ClientSettingsCommand::default();
    let mut sequence = ClientSettingsSequence::new();
    sequence.enqueue_order(ManualOrder {
        market: "BTCUSDT".to_string(),
        short: false,
        price: 100_000.0,
        size: 0.001,
        strategy_id: None,
        exit: exit_settings(30.0, false),
        planned_sell: 0.0,
        sync_exit: true,
    });

    for attempt in 0..3 {
        let action = sequence.next_action(&core_settings, TEST_CORE);
        let SequenceAction::Send { settings, .. } = action else {
            panic!("attempt {attempt} must send the generation");
        };
        sequence.observe_send_success(&settings, 1, Vec::new());
        sequence.observe_update();
    }

    // Fourth plan: the generation is abandoned and the ORDER goes out, rather than waiting on an
    // echo that never comes.
    match sequence.next_action(&core_settings, TEST_CORE) {
        SequenceAction::Place(order) => assert_eq!(order.market, "BTCUSDT"),
        _ => panic!("the order must be released after the retry budget"),
    }
}

/// Regression target: a manual-strategy order reads neither the visible TP nor the visible SL — the
/// core takes them from the strategy — so making it wait for that generation's echo cost a full
/// retry budget of round trips between the click and the order, which is plainly visible next to an
/// ordinary order that goes out at once.
#[test]
fn an_order_that_ignores_the_exit_generation_is_sent_immediately() {
    let core_settings = moonproto::ClientSettingsCommand::default();
    let mut sequence = ClientSettingsSequence::new();
    sequence.enqueue_order(ManualOrder {
        market: "BTCUSDT".to_string(),
        short: false,
        price: 100_000.0,
        size: 0.001,
        strategy_id: Some(77),
        // Deliberately a generation the core does NOT hold: it must not matter.
        exit: exit_settings(30.0, false),
        planned_sell: 101_000.0,
        sync_exit: false,
    });

    match sequence.next_action(&core_settings, TEST_CORE) {
        SequenceAction::Place(order) => {
            assert_eq!(order.market, "BTCUSDT");
            assert_eq!(order.planned_sell, 101_000.0);
        }
        _ => panic!("an order that does not sync its exits must go out on the first plan"),
    }
}

// --- Temporary blacklist (TempBL) -----------------------------------------------------------

use std::time::Duration;

/// A settings snapshot holding exactly these TempBL rows.
fn snapshot_with_temp(rows: &[(&str, Duration)]) -> moonproto::ClientSettingsCommand {
    let mut cmd = moonproto::ClientSettingsCommand::default();
    cmd.set_temp_blacklist_entries(rows.iter().map(|(symbol, left)| (*symbol, *left)));
    cmd
}

/// The next packet together with what it asked of the temporary blacklist.
fn next_send(
    sequence: &mut ClientSettingsSequence,
    snapshot: &moonproto::ClientSettingsCommand,
) -> (moonproto::ClientSettingsCommand, super::TempExpectation) {
    match sequence.next_action(snapshot, TEST_CORE) {
        SequenceAction::Send {
            settings,
            temp_expectation,
            ..
        } => (settings, temp_expectation),
        _ => panic!("expected a full ClientSettings send"),
    }
}

/// The rows a snapshot holds, as `(symbol, remaining)` pairs in wire order.
fn temp_rows(cmd: &moonproto::ClientSettingsCommand) -> Vec<(String, Duration)> {
    cmd.temp_blacklist_entries()
        .map(|row| (row.symbol.to_string(), row.remaining_duration()))
        .collect()
}

/// Regression target: assembling the TempBL list in the UI and sending it whole. The core writes
/// this list too — a cloud signal or an exchange rate limit adds rows — so a list captured before
/// the click erases whatever arrived while it travelled.
#[test]
fn a_temp_ban_merges_into_what_the_core_already_holds() {
    let mut sequence = ClientSettingsSequence::new();
    sequence.enqueue_temp_blacklist(
        vec![("ADAUSDT".to_string(), Duration::from_secs(7200))],
        Vec::new(),
    );

    // The core added this row after the click was queued.
    let base = snapshot_with_temp(&[("PEPEUSDT", Duration::from_secs(600))]);
    let sent = next_settings(&mut sequence, &base);

    let rows = temp_rows(&sent);
    assert_eq!(rows.len(), 2, "the core's own row must survive: {rows:?}");
    assert!(rows.iter().any(|(s, _)| s == "PEPEUSDT"));
    assert!(
        rows.iter()
            .any(|(s, left)| s == "ADAUSDT" && *left == Duration::from_secs(7200))
    );
}

/// The mutation lives outside the projection the queue confirms by, so it has to be judged by the
/// rows themselves: an echo carrying none of them must not retire the ban.
#[test]
fn a_temp_ban_is_not_retired_until_the_core_shows_it() {
    let mut sequence = ClientSettingsSequence::new();
    sequence.enqueue_temp_blacklist(
        vec![("ADAUSDT".to_string(), Duration::from_secs(7200))],
        Vec::new(),
    );

    let base = moonproto::ClientSettingsCommand::default();
    let (sent, expectation) = next_send(&mut sequence, &base);
    sequence.observe_send_success(&sent, 1, expectation);
    sequence.observe_update();

    // The core echoes the settings back WITHOUT the row.
    match sequence.next_action(&base, TEST_CORE) {
        SequenceAction::Send { settings, .. } => {
            assert_eq!(temp_rows(&settings).len(), 1, "the ban must be re-sent");
        }
        other => panic!(
            "an unapplied ban must be re-sent, got {}",
            action_name(&other)
        ),
    }
}

/// And it IS retired once the core shows it — with the remainder already a little smaller, which is
/// what a row does between the send and the echo.
#[test]
fn a_temp_ban_is_retired_when_the_core_shows_it() {
    let mut sequence = ClientSettingsSequence::new();
    sequence.enqueue_temp_blacklist(
        vec![("ADAUSDT".to_string(), Duration::from_secs(7200))],
        Vec::new(),
    );

    let base = moonproto::ClientSettingsCommand::default();
    let (sent, expectation) = next_send(&mut sequence, &base);
    sequence.observe_send_success(&sent, 1, expectation);
    sequence.observe_update();

    let echo = snapshot_with_temp(&[("ADAUSDT", Duration::from_secs(7200 - 12))]);
    assert!(
        matches!(sequence.next_action(&echo, TEST_CORE), SequenceAction::Idle),
        "a row within tolerance is the ban we asked for"
    );
}

/// A ban the core already holds for the same duration is not sent at all: re-sending the whole
/// settings snapshot to change nothing is a write other producers then have to queue behind.
#[test]
fn a_ban_the_core_already_holds_sends_nothing() {
    let mut sequence = ClientSettingsSequence::new();
    sequence.enqueue_temp_blacklist(
        vec![("ADAUSDT".to_string(), Duration::from_secs(7200))],
        Vec::new(),
    );

    let held = snapshot_with_temp(&[("ADAUSDT", Duration::from_secs(7200 - 5))]);
    assert!(matches!(
        sequence.next_action(&held, TEST_CORE),
        SequenceAction::Idle
    ));
}

/// The terminal's spelling of a symbol must not fork the core's own row into a second one, and a
/// removal has to match the same way.
#[test]
fn symbols_match_case_insensitively_and_keep_the_cores_spelling() {
    let mut sequence = ClientSettingsSequence::new();
    sequence.enqueue_temp_blacklist(
        vec![("adausdt".to_string(), Duration::from_secs(3600))],
        vec!["pepeusdt".to_string()],
    );

    let base = snapshot_with_temp(&[
        ("ADAUSDT", Duration::from_secs(600)),
        ("PEPEUSDT", Duration::from_secs(600)),
    ]);
    let sent = next_settings(&mut sequence, &base);

    let rows = temp_rows(&sent);
    assert_eq!(rows.len(), 1, "the removed row must be gone: {rows:?}");
    assert_eq!(rows[0].0, "ADAUSDT", "the core's spelling is kept");
    assert_eq!(rows[0].1, Duration::from_secs(3600), "the ban is re-timed");
}

/// Name an action for a failure message without teaching the enum to print itself.
fn action_name(action: &SequenceAction) -> &'static str {
    match action {
        SequenceAction::Idle => "Idle",
        SequenceAction::Send { .. } => "Send",
        SequenceAction::Place(_) => "Place",
    }
}

/// Regression target: a tolerance that reaches upward, or one scaled to the request. Clicking a
/// preset on a coin that already holds a LONGER ban is the user re-timing it, and the old
/// `max(60s, requested/20)` window swallowed that click for 72 minutes on a 24-hour ban.
#[test]
fn re_timing_a_longer_ban_to_a_shorter_one_is_sent() {
    let mut sequence = ClientSettingsSequence::new();
    sequence.enqueue_temp_blacklist(
        vec![("ADAUSDT".to_string(), Duration::from_secs(2 * 60 * 60))],
        Vec::new(),
    );

    let held = snapshot_with_temp(&[("ADAUSDT", Duration::from_secs(20 * 60 * 60))]);
    let (sent, _) = next_send(&mut sequence, &held);

    assert_eq!(
        temp_rows(&sent),
        vec![("ADAUSDT".to_string(), Duration::from_secs(2 * 60 * 60))]
    );
}

/// A row the core keeps at an expired value is not a row it still holds: demanding it disappear
/// entirely would leave a lift that can never be confirmed and is dropped after three sends.
#[test]
fn lifting_a_ban_the_core_zeroed_counts_as_done() {
    let mut sequence = ClientSettingsSequence::new();
    sequence.enqueue_temp_blacklist(Vec::new(), vec!["ADAUSDT".to_string()]);

    let expired = snapshot_with_temp(&[("ADAUSDT", Duration::ZERO)]);
    assert!(matches!(
        sequence.next_action(&expired, TEST_CORE),
        SequenceAction::Idle
    ));
}

/// Regression target: the order barrier waited for ANY queued mutation, so a coin banned while an
/// order was waiting held that order for good once the core refused its exit generation — neither
/// blacklist can move the exits an order synchronizes on.
#[test]
fn a_queued_ban_does_not_strand_an_order_whose_exits_were_refused() {
    // A core that will not hold the asked-for generation: it echoes its own settings unchanged,
    // which is what one does while a manual strategy owns the sell price.
    let core_holds = moonproto::ClientSettingsCommand::default();
    let mut sequence = ClientSettingsSequence::new();
    sequence.enqueue_order(ManualOrder {
        market: "ADAUSDT".to_string(),
        short: false,
        price: 1.0,
        size: 10.0,
        strategy_id: None,
        exit: exit_settings(12.0, true),
        planned_sell: 0.0,
        sync_exit: true,
    });
    // The trader bans a different coin while the order is still waiting.
    sequence.enqueue_temp_blacklist(
        vec![("PEPEUSDT".to_string(), Duration::from_secs(3600))],
        Vec::new(),
    );

    // The exit generation is offered its full budget of attempts and refused every time.
    for _ in 0..3 {
        let (sent, expectation) = next_send(&mut sequence, &core_holds);
        sequence.observe_send_success(&sent, 1, expectation);
        sequence.observe_update();
    }

    match sequence.next_action(&core_holds, TEST_CORE) {
        SequenceAction::Place(order) => assert_eq!(order.market, "ADAUSDT"),
        other => panic!(
            "the order must be released once its own generation is abandoned: got {}",
            action_name(&other)
        ),
    }
}
