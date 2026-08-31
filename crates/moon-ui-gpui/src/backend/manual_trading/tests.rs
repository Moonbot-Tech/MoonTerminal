//! Manual-trading state, conversion, and group-generation regressions.

use std::time::{Duration, Instant};

use moon_core::config::{
    DEFAULT_ORDER_SIZES_USD, GroupExitSettings, GroupTradeSettings, TakeProfitMode,
};
use moon_core::feed::{ClientSettingsEdit, StrategyRow};

use super::{
    IGNORE_SELL_LOCAL_TTL, MANUAL_STRATEGY_KIND, PANIC_LOCAL_TTL, PANIC_TOGGLE_DEBOUNCE,
    apply_group_exit_edit, effective_ignore_sell, effective_manual_strat_state,
    effective_panic_armed, panic_local_settled, panic_press_absorbed, planned_sell_price,
    seed_on_enable, stop_price, update_group_trade_pair, usd_to_base_amount,
};

/// Build a strategy row carrying only the kind used by manual-state validation.
///
/// Args:
///     id: Stable strategy id.
///     kind_ordinal: Strategy kind used by the effective-state filter.
///
/// Returns:
///     Minimal retained strategy row.
fn strategy(id: u64, kind_ordinal: u8) -> StrategyRow {
    StrategyRow {
        id,
        name: "Test".to_string(),
        kind: "Test".to_string(),
        kind_ordinal,
        folder_path: String::new(),
        checked: false,
        is_short: false,
        fields: Vec::new(),
    }
}

/// Regression target: seeding a core's own generation on EVERY enable (rather than only when it has
/// none) throws away the set the trader configured the last time the switch was on, so toggling off
/// and back on silently replaces their per-core numbers with the group's current ones.
#[test]
fn enabling_own_trade_seeds_from_the_group_only_when_the_core_has_no_set() {
    let mut group = GroupTradeSettings::default();
    group.order_sizes_usd[0] = 111.0;
    let mut own = GroupTradeSettings::default();
    own.order_sizes_usd[0] = 999.0;

    assert_eq!(
        seed_on_enable(None, &group),
        Some(group.clone()),
        "a core with no generation must start from the group's"
    );
    assert_eq!(
        seed_on_enable(Some(&own), &group),
        None,
        "a core that already has one must keep it across an off/on cycle"
    );
}

/// Regression target: rendering the sell-price checkbox from the core's value alone leaves a click
/// with no effect on screen for a whole slow-channel round trip, and holding the override past the
/// core's agreement makes the NEXT click look like the no-op instead.
#[test]
fn a_queued_sell_price_request_outranks_the_core_until_it_agrees() {
    // Fresh disagreement: what the trader asked for is what the checkbox shows.
    assert!(effective_ignore_sell(
        Some((true, Duration::from_secs(1))),
        false
    ));
    // The core came back with the same value: the override has nothing left to assert.
    assert!(effective_ignore_sell(
        Some((true, Duration::from_secs(1))),
        true
    ));
    // Stale request the core never took: the core's own value comes back rather than a lie.
    assert!(!effective_ignore_sell(
        Some((true, IGNORE_SELL_LOCAL_TTL)),
        false
    ));
    // No request at all.
    assert!(effective_ignore_sell(None, true));
}

/// Regression target: the visible take profit reaches a manual order ONLY as a price stored with
/// it, so getting the direction wrong puts a short's target ABOVE its entry — an order that closes
/// at a loss the moment it fills.
#[test]
fn a_planned_sell_target_mirrors_for_a_short() {
    let long = planned_sell_price(100.0, 2.0, false).expect("a long target");
    let short = planned_sell_price(100.0, 2.0, true).expect("a short target");
    assert!((long - 102.0).abs() < 1e-9, "long target {long}");
    assert!((short - 98.0).abs() < 1e-9, "short target {short}");

    // No percentage set, or nonsense input: no target, which the wire spells as zero.
    assert_eq!(planned_sell_price(100.0, 0.0, false), None);
    assert_eq!(planned_sell_price(0.0, 2.0, false), None);
    assert_eq!(planned_sell_price(100.0, f64::NAN, false), None);
    // A short cannot be asked for 100% down: that is a target of zero.
    assert_eq!(planned_sell_price(100.0, 100.0, true), None);
}

/// Regression target: a manual order with a strategy takes its stop FROM THE STRATEGY, so the stop
/// the trader sees only reaches the order as an absolute price computed here. Getting the direction
/// wrong puts a long's stop above its entry, where it fires immediately.
#[test]
fn a_visible_stop_becomes_an_absolute_price_on_the_correct_side() {
    let long = stop_price(100.0, -3.0, false).expect("a long stop");
    let short = stop_price(100.0, -3.0, true).expect("a short stop");
    assert!((long - 97.0).abs() < 1e-9, "long stop {long}");
    assert!((short - 103.0).abs() < 1e-9, "short stop {short}");

    // Nothing usable: the order keeps whatever the core would have applied.
    assert_eq!(stop_price(100.0, 0.0, false), None);
    assert_eq!(stop_price(0.0, -3.0, false), None);
    assert_eq!(stop_price(100.0, -100.0, false), None);
}

/// Treating revision zero as confirmed empty would expose TP/SL before the first snapshot arrives.
#[test]
fn pending_strategy_snapshot_keeps_raw_manual_state() {
    assert_eq!(effective_manual_strat_state((true, 77), 0, &[]), (true, 77));
}

/// Preserving raw enabled state after a confirmed zero-Manual snapshot would strand TP/SL disabled.
#[test]
fn confirmed_snapshot_without_manual_strategies_disables_effective_mode() {
    assert_eq!(
        effective_manual_strat_state((true, 77), 1, &[]),
        (false, 77)
    );
    assert_eq!(
        effective_manual_strat_state((true, 77), 1, &[strategy(11, MANUAL_STRATEGY_KIND - 1)]),
        (false, 77)
    );
}

/// Requiring the selected id to resolve would erase the existing invalid-id warning and repair UI.
#[test]
fn any_manual_strategy_preserves_raw_state_and_selected_id() {
    assert_eq!(
        effective_manual_strat_state((true, 77), 1, &[strategy(11, MANUAL_STRATEGY_KIND)]),
        (true, 77)
    );
}

#[test]
/// Regression target: removing the preview branch in `backend::update_group_trade_pair` lets an
/// already-open Settings window save its stale TP and undo the value the user sees in the toolbar.
fn live_group_edits_are_mirrored_without_erasing_preview_imports() {
    let mut live = GroupTradeSettings::default();
    let mut preview = live.clone();
    preview.order_sizes_usd = [1.0, 2.0, 3.0, 4.0, 5.0, 6.0];

    update_group_trade_pair(&mut live, Some(&mut preview), |trade| {
        trade.order_size_sel = 4;
    });

    assert_eq!(live.order_size_sel, 4);
    assert_eq!(preview.order_size_sel, 4);
    assert_eq!(preview.order_sizes_usd, [1.0, 2.0, 3.0, 4.0, 5.0, 6.0]);
    assert_eq!(live.order_sizes_usd, DEFAULT_ORDER_SIZES_USD);
}

#[test]
/// Regression target: changing `backend::usd_to_base_amount` to default a missing/zero FX rate to
/// one places a base-coin order with the visible dollar number and can oversize it catastrophically.
fn usd_conversion_fails_closed_without_a_positive_rate() {
    assert_eq!(usd_to_base_amount(100.0, None), None);
    assert_eq!(usd_to_base_amount(100.0, Some(0.0)), None);
    assert_eq!(usd_to_base_amount(100.0, Some(f64::NAN)), None);
    assert_eq!(usd_to_base_amount(f64::MAX, Some(f64::MIN_POSITIVE)), None);
    assert_eq!(usd_to_base_amount(100.0, Some(50_000.0)), Some(0.002));
}

/// Regression target: removing the finite guard from `backend::apply_group_exit_edit` persists NaN;
/// because NaN never equals its echo, every later manual order remains behind the settings barrier.
#[test]
fn nonfinite_exit_edits_cannot_poison_the_group_generation() {
    let mut exit = GroupExitSettings {
        take_profit_pct: 10.0,
        take_profit_mode: TakeProfitMode::Normal,
        fixed_sell_pcts: [1.0; 6],
        fixed_sell_slot: None,
        stop_loss_pct: -2.0,
        stop_loss_enabled: true,
        use_stop_market: false,
    };
    let original = exit;

    assert!(!apply_group_exit_edit(
        &mut exit,
        ClientSettingsEdit::TakeProfit {
            pct: f64::NAN,
            extended: false,
        }
    ));
    assert!(!apply_group_exit_edit(
        &mut exit,
        ClientSettingsEdit::StopLossPct(f32::NAN)
    ));
    assert!(!apply_group_exit_edit(
        &mut exit,
        ClientSettingsEdit::SetFixedSellPct {
            slot: 1,
            pct: f64::NAN,
        }
    ));
    assert!(!apply_group_exit_edit(
        &mut exit,
        ClientSettingsEdit::SetFixedSellPct {
            slot: 1,
            pct: 1.0e300,
        }
    ));
    assert_eq!(exit, original);
}

/// Regression target: removing the absorbed-branch timestamp insert in
/// `backend::manual_trading::Backend::panic_sell_hotkey` reanchors a press burst to its first
/// command, so the 800 ms re-jab executes a disarm while the trader believes it was ignored.
#[test]
fn panic_hotkey_bursts_restart_the_debounce_window_after_every_press() {
    let start = Instant::now();
    let presses = [
        start,
        start + Duration::from_millis(400),
        start + Duration::from_millis(800),
    ];
    let mut last_press = None;
    let mut executed = 0;
    for press in presses {
        if !panic_press_absorbed(last_press, press) {
            executed += 1;
        }
        last_press = Some(press);
    }

    assert_eq!(
        executed, 1,
        "a 0/400/800 ms burst must not execute a second toggle"
    );
    assert!(
        !panic_press_absorbed(Some(presses[2]), presses[2] + PANIC_TOGGLE_DEBOUNCE),
        "the exact debounce boundary remains an intentional reversal"
    );

    let source = include_str!("../manual_trading.rs");
    let hotkey = source
        .split("fn panic_sell_hotkey(")
        .nth(1)
        .and_then(|tail| tail.split("fn tick_panic_local(").next())
        .expect("panic_sell_hotkey must remain a distinct backend method");
    let absorbed_branch = hotkey
        .split("if panic_press_absorbed(")
        .nth(1)
        .and_then(|tail| tail.split("let accepted =").next())
        .expect("panic_sell_hotkey must retain a separate absorbed-press branch");
    assert!(
        absorbed_branch.contains("self.last_panic_press.insert(key, now);"),
        "an absorbed press must become the next debounce anchor"
    );
}

/// Regression target: removing `panic_rev`'s reconciliation bump in
/// `backend::manual_trading::Backend::tick_panic_local` leaves a stale Stop Panic label on a quiet
/// market, where clicking that label recomputes from the core snapshot and arms panic sell.
#[test]
fn panic_reconciliation_settles_overrides_and_repaints_from_the_slow_tick() {
    assert!(
        panic_local_settled(true, PANIC_LOCAL_TTL, false),
        "an expired override must stop outranking a disagreeing snapshot"
    );
    assert!(
        panic_local_settled(false, Duration::ZERO, false),
        "a matching snapshot settles an override before its TTL"
    );
    assert!(
        !panic_local_settled(true, Duration::ZERO, false),
        "a fresh disagreement must retain the optimistic override"
    );

    let source = include_str!("../manual_trading.rs");
    let tick = source
        .split("fn tick_panic_local(")
        .nth(1)
        .and_then(|tail| tail.split("fn cancel_all_buys_for_core(").next())
        .expect("tick_panic_local must remain a distinct backend method");
    assert!(
        tick.contains("self.panic_rev = self.panic_rev.wrapping_add(1);"),
        "settling an override must advance the revision that repaints the control"
    );

    let boot = include_str!("../../startup/boot.rs");
    let coordination = boot
        .split("let coord_backend = backend.clone();")
        .nth(1)
        .and_then(|tail| tail.split("cx.spawn(async move |cx| {").nth(1))
        .expect("boot must retain its slow coordination task");
    assert!(
        coordination.contains("executor.timer(Duration::from_millis(100)).await;")
            && coordination.contains("if b.tick_panic_local() {")
            && coordination.contains("b.mark_backend_dirty(cx);"),
        "reconciliation must run on the unconditional 100 ms coordination loop and mark a repaint"
    );
}

/// Regression target: changing `backend::manual_trading::effective_panic_armed` to union a fresh
/// local disarm with an armed snapshot keeps Stop Panic visible and invites a re-press that re-arms
/// panic sell while the trader believes it is off.
#[test]
fn fresh_panic_disarm_override_precedes_an_armed_snapshot() {
    assert!(
        !effective_panic_armed(Some((false, Duration::ZERO)), || true),
        "a fresh local disarm must outrank an armed core snapshot"
    );
}

/// Regression target: changing `backend::manual_trading::effective_panic_armed` to reject a fresh
/// local arm when the core has not echoed it yet makes Panic Sell look inactive after an accepted
/// command and encourages an unsafe repeat press.
#[test]
fn fresh_panic_arm_override_precedes_a_disarmed_snapshot() {
    assert!(
        effective_panic_armed(Some((true, Duration::ZERO)), || false),
        "a fresh local arm must outrank a disarmed core snapshot"
    );
}
