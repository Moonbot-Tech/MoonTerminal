use moonproto::shared_config::SharedConfig;

use super::{
    FieldMask, MAX_ATTEMPTS, SequenceAction, SharedConfigSequence, apply_core_config,
    core_config_from_proto, edit_satisfied,
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

/// Regression target: the "already reflected" check is the same comparison as the confirmation, so
/// it must be scoped the same way.
///
/// An OK that changed nothing used to send the whole snapshot anyway whenever ANY projected field
/// had drifted since the surface seeded — the drift is in the surface's frozen copy of an area this
/// edit never named, and says nothing about whether the edit has work to do.
#[test]
fn a_no_op_edit_is_dropped_even_when_an_untouched_area_drifted() {
    let base = SharedConfig::default();
    let mut sequence = SharedConfigSequence::new();
    // The surface's copy: nothing changed on its own page.
    sequence.enqueue(edit_from(&base, |_| {}), FieldMask::RENDERED_SECTIONS);

    // Meanwhile the core moved something no mask here names.
    let drifted = {
        let mut cfg = base.clone();
        cfg.trading.multi_orders.buy_move_click = 7;
        cfg
    };
    let mut events = Vec::new();
    assert!(
        matches!(
            sequence.next_action(&drifted, TEST_CORE, &mut events),
            SequenceAction::Idle
        ),
        "the edit asks for nothing this core does not already hold"
    );
    assert!(
        matches!(
            events.as_slice(),
            [CoreConfigEditEvent::Resolved(
                CoreConfigEditResult::Confirmed
            )]
        ),
        "dropping it without resolving leaves the toolbar cell pending forever, got {events:?}"
    );
}

/// Regression target: one core has ONE edit row, and `Confirmed` clears it outright, so a second
/// entry's success in the same pass must not erase the verdict the first one just earned.
///
/// Narrowing the echo comparison to the mask is what made this reachable: "the core already holds
/// this" went from near-impossible to common, and the drop that follows it used to announce itself
/// after a `GaveUp` had already been announced.
#[test]
fn a_satisfied_drop_does_not_erase_a_give_up_from_the_same_pass() {
    let base = SharedConfig::default();
    let mut sequence = SharedConfigSequence::new();
    let mask = FieldMask::RENDERED_SECTIONS;

    // Spend the whole budget on an edit the core never reflects, WITHOUT letting the drain run
    // afterwards: the last packet's echo is timed out instead, so the pass under test carries a
    // give-up and no rejection beside it.
    sequence.enqueue(edit_from(&base, |s| s.errors_level = 9), mask);
    for _ in 0..MAX_ATTEMPTS {
        let sent = next_config(&mut sequence, &base);
        sequence.observe_send_success(&sent, 1, mask);
        sequence.observe_update();
    }
    sequence.observe_echo_timeout();

    // A second edit asking for nothing lands behind it and is drained in the same pass.
    sequence.enqueue(edit_from(&base, |_| {}), mask);
    let mut events = Vec::new();
    let _ = sequence.next_action(&base, TEST_CORE, &mut events);

    assert!(
        events.iter().any(|event| matches!(
            event,
            CoreConfigEditEvent::Resolved(CoreConfigEditResult::GaveUp)
        )),
        "the first edit exhausted its budget, got {events:?}"
    );
    assert!(
        !events.iter().any(|event| matches!(
            event,
            CoreConfigEditEvent::Resolved(CoreConfigEditResult::Confirmed)
        )),
        "a Confirmed beside it clears the row carrying the give-up, got {events:?}"
    );
}

/// A core holding a non-finite number in the AutoStart or BTC-blink area must still compare equal
/// to itself.
///
/// Both areas are in the compact popup's own mask, and the echo comparison is now the single
/// answer to "confirmed", "rejected" and "already satisfied" alike — so an area that cannot equal
/// itself makes every OK naming it unresolvable, three sends and a `GaveUp` at a time.
#[test]
fn a_non_finite_autostart_or_blink_number_still_equals_itself() {
    let mut base = SharedConfig::default();
    base.trading.auto_start.auto_stop_loss = f64::NAN;
    base.trading.auto_start.panic_btc_delta = f64::NAN;
    base.visual.blink_config.blink_btc_delta = f64::NAN;
    let projected = core_config_from_proto(&base);

    assert_eq!(projected.auto_start, projected.auto_start.clone());
    assert_eq!(projected.btc_blink, projected.btc_blink.clone());
    assert!(edit_satisfied(
        &base,
        &projected,
        FieldMask::RENDERED_SECTIONS
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

/// Regression target: a core-side change to a field this write never named must not cost the write
/// its confirmation.
///
/// The echo comparison used to be `actual == expected` over the WHOLE projection, so a trader
/// moving anything in Moonbot's own dialogs while a packet was in flight made the echo differ,
/// resolved nothing, and sent the whole snapshot again. Enough of them in a row and an edit that
/// LANDED is dropped as `GaveUp`. `rejection_within_mask` already knew the difference; the
/// confirmation did not ask it.
#[test]
fn a_concurrent_change_outside_the_mask_still_confirms_the_edit() {
    let base = SharedConfig::default();
    let mut sequence = SharedConfigSequence::new();
    let mask = FieldMask::RENDERED_SECTIONS;
    sequence.enqueue(edit_from(&base, |s| s.errors_level = 9), mask);

    let sent = next_config(&mut sequence, &base);
    sequence.observe_send_success(&sent, 1, mask);
    sequence.observe_update();

    // The core applied the edit AND changed a field this write never named — someone moved a mouse
    // gesture in Moonbot's own Hotkeys dialog while the packet was in flight.
    let echoed = {
        let mut cfg = base.clone();
        cfg.trading.auto_start.errors_level = 9;
        cfg.trading.multi_orders.buy_move_click = 7;
        cfg
    };
    let mut events = Vec::new();
    assert!(
        matches!(
            sequence.next_action(&echoed, TEST_CORE, &mut events),
            SequenceAction::Idle
        ),
        "the edit landed; nothing should be re-sent"
    );
    assert!(
        matches!(
            events.as_slice(),
            [CoreConfigEditEvent::Resolved(
                CoreConfigEditResult::Confirmed
            )]
        ),
        "expected one Confirmed, got {events:?}"
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

/// A snapshot whose interface fields all DIFFER from the wire's own defaults, so a projection that
/// dropped one, or crossed two, shows up as a mismatch rather than as a passing coincidence.
///
/// Every flag is negated rather than set to `true`: several of these default to `true` on the wire,
/// and writing `true` over them would have made the round-trip test blind to exactly those.
fn interface_base() -> SharedConfig {
    let mut cfg = SharedConfig::default();
    cfg.trading.buy_on_enter = !cfg.trading.buy_on_enter;
    cfg.trading.dbl_click_panic_sell = !cfg.trading.dbl_click_panic_sell;
    cfg.trading.chart_split_zones = !cfg.trading.chart_split_zones;
    cfg.trading.draw_stop = !cfg.trading.draw_stop;
    cfg.trading.pending_orders_spread = 0.125;
    cfg.trading.pending_orders_spread_h_delta = 0.0625;
    cfg.visual.hide_forum_label = !cfg.visual.hide_forum_label;
    cfg.visual.scrolling_charts = !cfg.visual.scrolling_charts;
    cfg.visual.startup_load_charts = !cfg.visual.startup_load_charts;
    cfg.visual.hide_right_chart_panel = !cfg.visual.hide_right_chart_panel;
    cfg.visual.left_chart_info = !cfg.visual.left_chart_info;
    cfg.visual.show_iceberg = !cfg.visual.show_iceberg;
    cfg.visual.show_orders_captions = !cfg.visual.show_orders_captions;
    cfg.visual.orders_captions_lower = !cfg.visual.orders_captions_lower;
    cfg.visual.hide_pnl = !cfg.visual.hide_pnl;
    cfg.visual.hide_buy_button = !cfg.visual.hide_buy_button;
    cfg.visual.hide_cashback_button = !cfg.visual.hide_cashback_button;
    cfg.visual.remember_chart_buttons = !cfg.visual.remember_chart_buttons;
    cfg.visual.show_filters.scale_tool = !cfg.visual.show_filters.scale_tool;
    cfg.visual.icon_selection = 3;
    cfg.visual.colors.price_line_width = 4;
    cfg.visual.panic_sell_opacity = 55;
    cfg.visual.book_cumulative_opacity = 60;
    cfg.visual.book_orders_opacity = 65;
    cfg.visual.book_orders_width = 7;
    cfg.signals.play_signal_sound = !cfg.signals.play_signal_sound;
    cfg.ui.confirm_close = !cfg.ui.confirm_close;
    cfg.ui.hide_demo_button = !cfg.ui.hide_demo_button;
    cfg
}

/// Every interface field must survive read -> write unchanged. A projection that forgets a field
/// silently REVERTS it on the next OK, because the write rebuilds the whole snapshot.
#[test]
fn the_interface_page_round_trips_through_the_projection() {
    let base = interface_base();
    let projected = core_config_from_proto(&base);
    // Without this the test could pass on a projection that read nothing: every field would then be
    // the default on both sides.
    assert_ne!(
        projected.interface,
        core_config_from_proto(&SharedConfig::default()).interface,
        "the base must differ from the wire default, or this test proves nothing"
    );

    let mut written = SharedConfig::default();
    apply_core_config(&mut written, &projected, FieldMask::EMPTY.with_interface());

    assert_eq!(
        core_config_from_proto(&written).interface,
        projected.interface
    );
}

/// Changing one interface field must reach the wire, and must not disturb the section's neighbours.
#[test]
fn an_interface_edit_writes_its_own_field_and_leaves_the_section_alone() {
    let mut base = interface_base();
    base.trading.g_take_profit = 3.5;
    base.visual.glass_opacity = 42;
    base.ui.coins_sort_order = 2;
    base.visual.unknown_tail = vec![9, 9, 9];

    let mut wanted = core_config_from_proto(&base);
    wanted.interface.book_orders_width = 11;

    let mut sequence = SharedConfigSequence::new();
    sequence.enqueue(wanted, FieldMask::EMPTY.with_interface());
    let sent = next_config(&mut sequence, &base);

    assert_eq!(sent.visual.book_orders_width, 11);
    assert_eq!(sent.trading.g_take_profit, 3.5);
    assert_eq!(sent.visual.glass_opacity, 42);
    assert_eq!(sent.ui.coins_sort_order, 2);
    assert_eq!(sent.visual.unknown_tail, vec![9, 9, 9]);
}

/// The mask decides, not the projection: an edit that does not name the interface must not carry the
/// interface fields it happens to hold — the guard that stops the expert window's stale copy of a
/// page it never drew from being written back.
#[test]
fn an_unnamed_interface_is_not_written() {
    let base = interface_base();
    let mut wanted = core_config_from_proto(&base);
    wanted.interface.book_orders_width = 11;
    wanted.interface.hide_pnl = false;

    let mut written = interface_base();
    apply_core_config(&mut written, &wanted, FieldMask::RENDERED_SECTIONS);

    assert_eq!(written.visual.book_orders_width, 7);
    assert!(written.visual.hide_pnl);
}

/// A snapshot whose autobuy fields all DIFFER from the wire's own defaults, for the same reason
/// [`interface_base`] negates its flags: several of these default to `true`, and writing `true` over
/// them would leave the round-trip test blind to exactly those.
fn auto_buy_base() -> SharedConfig {
    let mut cfg = SharedConfig::default();
    let sig = &mut cfg.signals;
    sig.monitor_clipboard = !sig.monitor_clipboard;
    sig.clipboard_auto_buy = !sig.clipboard_auto_buy;
    sig.lower_case_token_cbd = !sig.lower_case_token_cbd;
    sig.look_full_link_cbd = !sig.look_full_link_cbd;
    sig.advanced_filter_clipboard = !sig.advanced_filter_clipboard;
    sig.telegram_auto_buy = !sig.telegram_auto_buy;
    sig.lower_case_token_tlg = !sig.lower_case_token_tlg;
    sig.look_full_link_tlg = !sig.look_full_link_tlg;
    sig.advanced_filter = !sig.advanced_filter;
    sig.dont_buy_reply = !sig.dont_buy_reply;
    sig.msg_keywords_long = "pump,long".to_string();
    sig.msg_keywords_short = "dump,short".to_string();
    sig.msg_black_words = "called,dont".to_string();
    sig.msg_token_tags = "@,%".to_string();
    sig.lower_price_words = "wait dip".to_string();
    let c = &mut sig.signal_config;
    c.use_keywords = !c.use_keywords;
    c.buy_key_dist = 7;
    c.use_black_words = !c.use_black_words;
    c.use_words_count = !c.use_words_count;
    c.words_count = 42;
    c.use_lower_price_words = !c.use_lower_price_words;
    c.x_lower_price = -3;
    c.x_found_price = 5;
    c.buy_if_price_found = !c.buy_if_price_found;
    c.use_price = !c.use_price;
    c.use_stops = !c.use_stops;
    c.only_1_token = !c.only_1_token;
    c.use_token_tags = !c.use_token_tags;
    c.tokens_no_tags = !c.tokens_no_tags;
    c.token_links = !c.token_links;
    c.special_formats = !c.special_formats;
    cfg.trading.auto_cancel_lower_buy = 15;
    cfg
}

/// Every autobuy field must survive read -> write unchanged: a projection that forgets one silently
/// REVERTS it on the next OK, because the write rebuilds the whole snapshot.
#[test]
fn the_auto_buy_page_round_trips_through_the_projection() {
    let base = auto_buy_base();
    let projected = core_config_from_proto(&base);
    assert_ne!(
        projected.auto_buy,
        core_config_from_proto(&SharedConfig::default()).auto_buy,
        "the base must differ from the wire default, or this test proves nothing"
    );

    let mut written = SharedConfig::default();
    apply_core_config(&mut written, &projected, FieldMask::EMPTY.with_auto_buy());

    assert_eq!(
        core_config_from_proto(&written).auto_buy,
        projected.auto_buy
    );
}

/// The autobuy page and the alert sounds share the `signals` wire section but NOT a mask bit: an
/// autobuy write must leave the two price-approach alerts exactly as the core sent them, or the two
/// surfaces would revert each other.
#[test]
fn an_auto_buy_edit_leaves_the_alert_sounds_and_its_neighbours_alone() {
    let mut base = auto_buy_base();
    base.signals.play_sell_alert = true;
    base.signals.sell_alert_level = 3;
    base.signals.signal_sound_2 = 9;
    base.signals.play_signal_sound = true;
    base.signals.unknown_tail = vec![7, 7];
    base.trading.g_take_profit = 2.5;

    let mut wanted = core_config_from_proto(&base);
    wanted.auto_buy.words_count = 11;
    // What another surface would have staged, and what this write must NOT carry.
    wanted.signals.play_sell_alert = false;
    wanted.signals.sell_alert_level = 0;
    wanted.interface.play_signal_sound = false;

    let mut sequence = SharedConfigSequence::new();
    sequence.enqueue(wanted, FieldMask::EMPTY.with_auto_buy());
    let sent = next_config(&mut sequence, &base);

    assert_eq!(sent.signals.signal_config.words_count, 11);
    assert!(sent.signals.play_sell_alert);
    assert_eq!(sent.signals.sell_alert_level, 3);
    assert_eq!(sent.signals.signal_sound_2, 9);
    assert!(sent.signals.play_signal_sound);
    assert_eq!(sent.signals.unknown_tail, vec![7, 7]);
    assert_eq!(sent.trading.g_take_profit, 2.5);
}

/// The mask decides, not the projection: the compact popup's mask must not carry the autobuy page.
#[test]
fn an_unnamed_auto_buy_is_not_written() {
    let base = auto_buy_base();
    let mut wanted = core_config_from_proto(&base);
    wanted.auto_buy.words_count = 11;
    wanted.auto_buy.msg_token_tags = "!".to_string();

    let mut written = auto_buy_base();
    apply_core_config(&mut written, &wanted, FieldMask::RENDERED_SECTIONS);

    assert_eq!(written.signals.signal_config.words_count, 42);
    assert_eq!(written.signals.msg_token_tags, "@,%");
}

/// A snapshot whose Telegram fields all differ from the wire's own defaults, the same discipline
/// [`interface_base`] and [`auto_buy_base`] follow.
fn telegram_base() -> SharedConfig {
    let mut cfg = SharedConfig::default();
    cfg.signals.pump_channel = "primary".to_string();
    cfg.signals.pump_channels = vec!["extra_one".to_string(), "extra_two".to_string()];
    cfg.signals.multi_channels = !cfg.signals.multi_channels;
    cfg.signals.more_then_1_channel = !cfg.signals.more_then_1_channel;
    cfg.signals.listen_moon_channel = !cfg.signals.listen_moon_channel;
    cfg.trading.use_moon_bl = !cfg.trading.use_moon_bl;
    cfg
}

/// Every Telegram field must survive read -> write unchanged, the channel list included: the write
/// rebuilds the whole snapshot, so a field the projection forgets is a field the next OK reverts.
#[test]
fn the_telegram_page_round_trips_through_the_projection() {
    let base = telegram_base();
    let projected = core_config_from_proto(&base);
    assert_ne!(
        projected.telegram,
        core_config_from_proto(&SharedConfig::default()).telegram,
        "the base must differ from the wire default, or this test proves nothing"
    );

    let mut written = SharedConfig::default();
    apply_core_config(&mut written, &projected, FieldMask::EMPTY.with_telegram());

    assert_eq!(
        core_config_from_proto(&written).telegram,
        projected.telegram
    );
}

/// The Telegram page and the autobuy page share the `signals` section but not a mask bit: editing
/// the channel list must leave the message filter exactly as the core sent it.
#[test]
fn a_telegram_edit_leaves_the_message_filter_alone() {
    let mut base = telegram_base();
    base.signals.signal_config.words_count = 33;
    base.signals.msg_keywords_long = "pump".to_string();
    base.signals.play_sell_alert = true;
    base.signals.unknown_tail = vec![5];
    base.trading.g_take_profit = 1.5;

    let mut wanted = core_config_from_proto(&base);
    wanted.telegram.pump_channels.push("added".to_string());
    // What another surface would have staged, and what this write must NOT carry.
    wanted.auto_buy.words_count = 0;
    wanted.auto_buy.msg_keywords_long = String::new();

    let mut sequence = SharedConfigSequence::new();
    sequence.enqueue(wanted, FieldMask::EMPTY.with_telegram());
    let sent = next_config(&mut sequence, &base);

    assert_eq!(
        sent.signals.pump_channels,
        vec![
            "extra_one".to_string(),
            "extra_two".to_string(),
            "added".to_string()
        ]
    );
    assert_eq!(sent.signals.signal_config.words_count, 33);
    assert_eq!(sent.signals.msg_keywords_long, "pump");
    assert!(sent.signals.play_sell_alert);
    assert_eq!(sent.signals.unknown_tail, vec![5]);
    assert_eq!(sent.trading.g_take_profit, 1.5);
}

/// The mask decides, not the projection: the compact popup's mask must not carry the channel list.
#[test]
fn an_unnamed_telegram_is_not_written() {
    let base = telegram_base();
    let mut wanted = core_config_from_proto(&base);
    wanted.telegram.pump_channel = "hijacked".to_string();
    wanted.telegram.pump_channels.clear();

    let mut written = telegram_base();
    apply_core_config(&mut written, &wanted, FieldMask::RENDERED_SECTIONS);

    assert_eq!(written.signals.pump_channel, "primary");
    assert_eq!(written.signals.pump_channels.len(), 2);
}

/// A snapshot whose Special fields all differ from the wire's own defaults, the discipline the other
/// page bases follow.
fn special_base() -> SharedConfig {
    let mut cfg = SharedConfig::default();
    let t = &mut cfg.trading;
    t.log_level = 4;
    t.auto_delete_logs = 21;
    t.chart_clean_up_time = 37;
    t.max_orders = 11;
    t.unlimited_orders = !t.unlimited_orders;
    t.random_price = !t.random_price;
    t.correct_order_price = !t.correct_order_price;
    t.use_book_ticker = !t.use_book_ticker;
    t.m_avg_use_vol_weight = !t.m_avg_use_vol_weight;
    t.auto_buy_bnb = !t.auto_buy_bnb;
    t.auto_buy_bnb_level = 0.125;
    t.auto_buy_bnb_volume = 0.75;
    t.auto_reduce_order = !t.auto_reduce_order;
    t.auto_close_zero_pos = !t.auto_close_zero_pos;
    t.auto_lower_lev = !t.auto_lower_lev;
    t.use_websocket_api = !t.use_websocket_api;
    t.iceberg_step = 0.0625;
    t.sell_x2_level = 73;
    t.no_trades_markets_text = "BTC\nETH".to_string();
    t.multi_commands = !t.multi_commands;
    let shots = &mut t.send_shots_config;
    shots.may_send = !shots.may_send;
    shots.profit_abs = 12;
    shots.profit_pers = 13;
    shots.profit_session = 14;
    shots.send_negative = !shots.send_negative;
    shots.send_public = !shots.send_public;
    shots.time_scale = 15;
    shots.price_scale = 16;
    let t = &mut cfg.trading;
    t.h_pos_black_list_text = "BNB, USDC".to_string();
    let oc = &mut t.orders_control;
    oc.liq_control = !oc.liq_control;
    oc.ignore_replacing_bug = !oc.ignore_replacing_bug;
    oc.ignore_protection = 1;
    oc.active = !oc.active;
    oc.h_pos_report = !oc.h_pos_report;
    oc.h_pos_auto_sell = !oc.h_pos_auto_sell;
    // Off the default too, so the assertion that this page never writes it can actually fail.
    oc.sign_orders = !oc.sign_orders;
    // The three more this page must never write, off their defaults for the same reason.
    oc.h_pos_control = !oc.h_pos_control;
    oc.min_price = 0.25;
    oc.max_time = 900;
    cfg
}

/// Every Special field must survive read -> write unchanged: the write rebuilds the whole snapshot,
/// so a field the projection forgets is a field the next OK reverts.
#[test]
fn the_special_page_round_trips_through_the_projection() {
    let base = special_base();
    let projected = core_config_from_proto(&base);
    assert_ne!(
        projected.special,
        core_config_from_proto(&SharedConfig::default()).special,
        "the base must differ from the wire default, or this test proves nothing"
    );

    let mut written = SharedConfig::default();
    apply_core_config(&mut written, &projected, FieldMask::EMPTY.with_special());

    assert_eq!(core_config_from_proto(&written).special, projected.special);
}

/// A core holding a non-finite amount must still compare equal to itself, or `edit_satisfied` is
/// false for it forever and every OK on that core burns its attempts. The hand-written `PartialEq`
/// on `SpecialSettings` exists for exactly this.
#[test]
fn a_non_finite_special_amount_still_equals_itself() {
    let mut base = special_base();
    base.trading.auto_buy_bnb_level = f64::NAN;
    let projected = core_config_from_proto(&base);

    assert_eq!(projected.special, projected.special.clone());
    assert!(edit_satisfied(
        &base,
        &projected,
        FieldMask::EMPTY.with_special()
    ));
}

/// The Special page shares `trading` with General, AutoStart and the Interface page but not a mask
/// bit: its write must leave their fields exactly as the core sent them.
#[test]
fn a_special_edit_leaves_the_other_trading_pages_alone() {
    let mut base = special_base();
    base.trading.g_take_profit = 4.25;
    base.trading.buy_iceberg = true;
    base.trading.auto_start.auto_stop_loss = 99.0;
    base.trading.buy_on_enter = true;
    base.trading.unknown_tail = vec![3, 3];

    let mut wanted = core_config_from_proto(&base);
    wanted.special.max_orders = 42;
    // What another surface would have staged, and what this write must NOT carry.
    wanted.general.take_profit_pct = 0.0;
    wanted.general.buy_iceberg = false;
    wanted.interface.buy_on_enter = false;

    let mut sequence = SharedConfigSequence::new();
    sequence.enqueue(wanted, FieldMask::EMPTY.with_special());
    let sent = next_config(&mut sequence, &base);

    assert_eq!(sent.trading.max_orders, 42);
    assert_eq!(sent.trading.g_take_profit, 4.25);
    assert!(sent.trading.buy_iceberg);
    assert_eq!(sent.trading.auto_start.auto_stop_loss, 99.0);
    assert!(sent.trading.buy_on_enter);
    assert_eq!(sent.trading.unknown_tail, vec![3, 3]);
    // The four fields of that sub-record this page must never write: `sign_orders` mirrors the
    // compact channel and two routes writing one field would fight; the other three have no row on
    // the page at all.
    let sent_oc = &sent.trading.orders_control;
    let base_oc = &base.trading.orders_control;
    assert_eq!(sent_oc.sign_orders, base_oc.sign_orders);
    assert_eq!(sent_oc.h_pos_control, base_oc.h_pos_control);
    assert_eq!(sent_oc.min_price, base_oc.min_price);
    assert_eq!(sent_oc.max_time, base_oc.max_time);
}

/// The mask decides, not the projection: the compact popup's mask must not carry this page.
#[test]
fn an_unnamed_special_is_not_written() {
    let base = special_base();
    let mut wanted = core_config_from_proto(&base);
    wanted.special.max_orders = 1;
    wanted.special.log_level = 0;

    let mut written = special_base();
    apply_core_config(&mut written, &wanted, FieldMask::RENDERED_SECTIONS);

    assert_eq!(written.trading.max_orders, 11);
    assert_eq!(written.trading.log_level, 4);
}

/// A snapshot whose gesture fields all differ from the wire's own defaults, the discipline the other
/// page bases follow.
///
/// The twelve gesture ordinals are distinct from each other and the four move kinds from each
/// other, so a projection that crossed two fields of the same kind — the long and short columns of
/// one row, or a row and its "additional command" twin — fails rather than passing on a
/// coincidence. The round trip below additionally pins each field to its NAMED wire slot, which is
/// what catches a cross that is symmetric between the read and the write.
fn gesture_base() -> SharedConfig {
    let mut cfg = SharedConfig::default();
    cfg.trading.pending_order_set_click = 3;
    let mo = &mut cfg.trading.multi_orders;
    mo.buy_set_click = 4;
    mo.short_set_click = 5;
    mo.pending_short_set_click = 6;
    mo.same_hotkeys_for_move = !mo.same_hotkeys_for_move;
    mo.buy_move_click = 7;
    mo.short_buy_move_click = 8;
    mo.replace_buy_kind = 2;
    mo.sell_move_click = 9;
    mo.short_sell_move_click = 10;
    mo.replace_sell_kind = 3;
    mo.buy_move_click_2 = 11;
    mo.short_buy_move_click_2 = 12;
    mo.replace_buy_kind_2 = 4;
    mo.sell_move_click_2 = 13;
    mo.short_sell_move_click_2 = 14;
    mo.replace_sell_kind_2 = 5;
    // The one field of the sub-record this page must never write, off its default so the assertion
    // that it survives can actually fail.
    mo.join_sell_kind = 2;
    // And the six the record holds for Moonbot's chart rather than for its gestures.
    mo.use_multi_orders = !mo.use_multi_orders;
    mo.split_sells = 4;
    mo.show_orders_num = !mo.show_orders_num;
    mo.kir_style = !mo.kir_style;
    mo.fix_pos = 1;
    mo.done_opacity = 0.25;
    cfg
}

/// Every gesture field must survive read -> write unchanged: the write rebuilds the whole snapshot,
/// so a field the projection forgets is a field the next OK reverts.
#[test]
fn the_gesture_block_round_trips_through_the_projection() {
    let base = gesture_base();
    let projected = core_config_from_proto(&base);
    assert_ne!(
        projected.gestures,
        core_config_from_proto(&SharedConfig::default()).gestures,
        "the base must differ from the wire default, or this test proves nothing"
    );

    let mut written = SharedConfig::default();
    apply_core_config(&mut written, &projected, FieldMask::EMPTY.with_gestures());

    assert_eq!(
        core_config_from_proto(&written).gestures,
        projected.gestures
    );

    // Against the NAMED wire slots too. The assertion above compares a projection with a
    // projection, so a read and a write that cross the same two fields cancel out and pass; these
    // do not.
    assert_eq!(written.trading.pending_order_set_click, 3);
    let mo = &written.trading.multi_orders;
    assert_eq!(mo.buy_set_click, 4);
    assert_eq!(mo.short_set_click, 5);
    assert_eq!(mo.pending_short_set_click, 6);
    assert_eq!(mo.buy_move_click, 7);
    assert_eq!(mo.short_buy_move_click, 8);
    assert_eq!(mo.replace_buy_kind, 2);
    assert_eq!(mo.sell_move_click, 9);
    assert_eq!(mo.short_sell_move_click, 10);
    assert_eq!(mo.replace_sell_kind, 3);
    assert_eq!(mo.buy_move_click_2, 11);
    assert_eq!(mo.short_buy_move_click_2, 12);
    assert_eq!(mo.replace_buy_kind_2, 4);
    assert_eq!(mo.sell_move_click_2, 13);
    assert_eq!(mo.short_sell_move_click_2, 14);
    assert_eq!(mo.replace_sell_kind_2, 5);
    assert_eq!(
        mo.same_hotkeys_for_move,
        gesture_base().trading.multi_orders.same_hotkeys_for_move
    );
}

/// The gesture block shares `trading` and its `multi_orders` record with rows no page of this
/// window draws, and with one field the compact channel also carries. A gesture write must leave
/// every one of them exactly as the core sent it.
#[test]
fn a_gesture_edit_leaves_the_rest_of_multi_orders_alone() {
    let base = gesture_base();
    let mut wanted = core_config_from_proto(&base);
    wanted.gestures.buy_set_click = 15;
    // What another surface would have staged, and what this write must NOT carry.
    wanted.general.take_profit_pct = 0.0;
    wanted.special.max_orders = 1;

    let mut sequence = SharedConfigSequence::new();
    sequence.enqueue(wanted, FieldMask::EMPTY.with_gestures());
    let sent = next_config(&mut sequence, &base);

    assert_eq!(sent.trading.multi_orders.buy_set_click, 15);
    assert_eq!(sent.trading.g_take_profit, base.trading.g_take_profit);
    assert_eq!(sent.trading.max_orders, base.trading.max_orders);
    // `join_sell_kind` mirrors `ClientSettingsCommand::join_sell_kind`, so two routes writing it
    // would fight; the six below are in the record but on Moonbot's chart, not on this page.
    let sent_mo = &sent.trading.multi_orders;
    let base_mo = &base.trading.multi_orders;
    assert_eq!(sent_mo.join_sell_kind, base_mo.join_sell_kind);
    assert_eq!(sent_mo.use_multi_orders, base_mo.use_multi_orders);
    assert_eq!(sent_mo.split_sells, base_mo.split_sells);
    assert_eq!(sent_mo.show_orders_num, base_mo.show_orders_num);
    assert_eq!(sent_mo.kir_style, base_mo.kir_style);
    assert_eq!(sent_mo.fix_pos, base_mo.fix_pos);
    assert_eq!(sent_mo.done_opacity, base_mo.done_opacity);
}

/// The mask decides, not the projection: the compact popup's mask must not carry this block, and
/// neither must a mask naming only the General page it shares `trading` with.
#[test]
fn an_unnamed_gesture_block_is_not_written() {
    let base = gesture_base();
    let mut wanted = core_config_from_proto(&base);
    wanted.gestures.buy_set_click = 0;
    wanted.gestures.replace_sell_kind_2 = 0;

    let mut written = gesture_base();
    apply_core_config(&mut written, &wanted, FieldMask::RENDERED_SECTIONS);
    assert_eq!(written.trading.multi_orders.buy_set_click, 4);
    assert_eq!(written.trading.multi_orders.replace_sell_kind_2, 5);

    let mut written = gesture_base();
    apply_core_config(&mut written, &wanted, FieldMask::EMPTY.with_general());
    assert_eq!(written.trading.multi_orders.buy_set_click, 4);
    assert_eq!(written.trading.multi_orders.replace_sell_kind_2, 5);
}

/// The order-rules area reaches one field of the `signals` section — the startup analysis Moonbot
/// draws above its two columns — while the alert sounds in the same section belong to another mask
/// bit. Each must leave the other exactly as the core sent it, or the two surfaces revert each
/// other.
#[test]
fn the_order_rules_and_alert_halves_of_signals_do_not_cross() {
    let mut base = SharedConfig::default();
    base.signals.load_deep_history = !base.signals.load_deep_history;
    base.signals.play_buy_alert = !base.signals.play_buy_alert;
    base.signals.buy_alert_level = 7;
    base.signals.unknown_tail = vec![9, 9];

    let mut wanted = core_config_from_proto(&base);
    wanted.order_rules.analyze_on_start = !wanted.order_rules.analyze_on_start;
    wanted.signals.buy_alert_level = 0;

    let mut written = base.clone();
    apply_core_config(&mut written, &wanted, FieldMask::EMPTY.with_order_rules());
    assert_eq!(
        written.signals.load_deep_history,
        wanted.order_rules.analyze_on_start
    );
    assert_eq!(written.signals.buy_alert_level, 7);
    assert_eq!(written.signals.play_buy_alert, base.signals.play_buy_alert);
    assert_eq!(written.signals.unknown_tail, vec![9, 9]);

    let mut written = base.clone();
    apply_core_config(&mut written, &wanted, FieldMask::EMPTY.with_signals());
    assert_eq!(written.signals.buy_alert_level, 0);
    assert_eq!(
        written.signals.load_deep_history,
        base.signals.load_deep_history
    );
}

/// The seven General rows below the compact popup's own must reach the wire and come back.
///
/// `deltas_by_trades` is deliberately among them: it is a TAIL field of the `trading` section, and
/// a projection that dropped it would look identical on a modern core until the first OK reverted
/// whatever the trader had set in Moonbot.
#[test]
fn the_order_rules_area_round_trips() {
    let mut base = SharedConfig::default();
    let t = &mut base.trading;
    t.trailing_float = 0.375;
    t.auto_sell_partial = 55;
    t.auto_cancel_buy_order = 44;
    t.cancel_buy_on_sell_fill = !t.cancel_buy_on_sell_fill;
    t.dont_buy_new_coins = 123;
    t.deltas_by_trades = !t.deltas_by_trades;
    base.signals.load_deep_history = !base.signals.load_deep_history;

    let projected = core_config_from_proto(&base);
    assert_ne!(
        projected.order_rules,
        core_config_from_proto(&SharedConfig::default()).order_rules,
        "the base must differ from the wire default, or this test proves nothing"
    );

    let mut written = SharedConfig::default();
    apply_core_config(
        &mut written,
        &projected,
        FieldMask::EMPTY.with_order_rules(),
    );

    assert_eq!(
        core_config_from_proto(&written).order_rules,
        projected.order_rules
    );
    // And against the named wire slots, for the reason the gesture round trip states.
    assert_eq!(written.trading.trailing_float, 0.375);
    assert_eq!(written.trading.auto_sell_partial, 55);
    assert_eq!(written.trading.auto_cancel_buy_order, 44);
    assert_eq!(written.trading.dont_buy_new_coins, 123);
    assert_eq!(
        written.trading.deltas_by_trades,
        base.trading.deltas_by_trades
    );
    assert_eq!(
        written.signals.load_deep_history,
        base.signals.load_deep_history
    );
}

/// The compact popup draws the exits and the blacklist and NOT the seven rows beside them, so its
/// own mask must not carry them: an OK pressed there would otherwise stamp all seven back from a
/// draft frozen when the popup opened, over whatever Moonbot changed meanwhile.
///
/// This is the whole reason the General page is two areas rather than one, and the assertion that
/// keeps it that way.
#[test]
fn the_compact_popups_mask_does_not_carry_the_order_rules() {
    let mut base = SharedConfig::default();
    base.trading.auto_cancel_buy_order = 44;
    base.trading.dont_buy_new_coins = 123;
    base.signals.load_deep_history = !base.signals.load_deep_history;

    let mut wanted = core_config_from_proto(&base);
    wanted.order_rules.auto_cancel_buy_order = 0;
    wanted.order_rules.dont_buy_new_coins = 0;
    wanted.order_rules.analyze_on_start = !wanted.order_rules.analyze_on_start;
    // Something the popup DOES draw, so the write is not a no-op and the assertions below are
    // about the mask rather than about nothing having happened.
    wanted.general.take_profit_pct = 4.5;

    let mut written = base.clone();
    apply_core_config(&mut written, &wanted, FieldMask::RENDERED_SECTIONS);

    assert_eq!(written.trading.g_take_profit, 4.5);
    assert_eq!(written.trading.auto_cancel_buy_order, 44);
    assert_eq!(written.trading.dont_buy_new_coins, 123);
    assert_eq!(
        written.signals.load_deep_history,
        base.signals.load_deep_history
    );
}

/// A core holding a non-finite number in either half of the General page must still compare equal
/// to itself, or `edit_satisfied` is false for it forever and every OK on that core burns its whole
/// retry budget. The hand-written `PartialEq` on both structs exists for exactly this.
#[test]
fn a_non_finite_general_number_still_equals_itself() {
    let mut base = SharedConfig::default();
    base.trading.g_take_profit = f64::NAN;
    base.trading.trailing_float = f64::NAN;
    let projected = core_config_from_proto(&base);

    assert_eq!(projected.general, projected.general.clone());
    assert_eq!(projected.order_rules, projected.order_rules.clone());
    assert!(edit_satisfied(
        &base,
        &projected,
        FieldMask::EMPTY.with_general().with_order_rules()
    ));
}
