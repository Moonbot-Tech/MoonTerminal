//! Manual-trading state, conversion, and group-generation regressions.

use std::time::{Duration, Instant};

use moon_core::config::{
    DEFAULT_ORDER_SIZES_USD, GroupExitSettings, GroupTradeSettings, TakeProfitMode,
};
use moon_core::feed::{ClientSettingsEdit, StrategyRow};

use super::{
    MANUAL_STRATEGY_KIND, PANIC_LOCAL_TTL, PANIC_TOGGLE_DEBOUNCE, apply_group_exit_edit,
    effective_manual_strat_state, effective_panic_armed, panic_local_settled, panic_press_absorbed,
    update_group_trade_pair, usd_to_base_amount,
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
