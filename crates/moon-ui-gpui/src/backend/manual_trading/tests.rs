//! Manual-trading state, conversion, and group-generation regressions.

use std::time::{Duration, Instant};

use moon_core::config::{
    DEFAULT_ORDER_SIZES_USD, GroupExitSettings, GroupTradeSettings, TakeProfitMode,
};
use moon_core::feed::{ClientSettingsEdit, StrategyRow};

use super::{
    HOOK_STRATEGY_KIND, IGNORE_SELL_LOCAL_TTL, MANUAL_STRATEGY_KIND, PANIC_LOCAL_TTL,
    PANIC_TOGGLE_DEBOUNCE, apply_group_exit_edit, effective_ignore_sell,
    effective_manual_strat_state, effective_panic_armed, exit_source, manual_selection_is_broken,
    manual_strat_seed, manual_strategy_id, panic_local_settled, panic_press_absorbed,
    planned_sell_price, resolve_manual_selection, seed_on_enable, stop_price,
    stop_write_is_redundant, update_group_trade_pair, usd_to_base_amount,
};

/// Regression target, three times over in one session: the rule deciding whether the per-order stop
/// write can be skipped. Skipping it wrongly is silent — the order simply keeps a stop the trader
/// cannot see — which is how a visible -3% shipped as a hook's -4.51%.
#[test]
fn the_per_order_stop_write_is_skipped_only_when_it_would_change_nothing() {
    assert!(
        stop_write_is_redundant((true, 3.0), (true, -3.0)),
        "the strategy's positive distance is the visible signed percent: nothing to send"
    );
    assert!(
        stop_write_is_redundant((false, 3.0), (false, -3.0)),
        "both sides disabled: the percentage is not on screen and cannot differ"
    );
    assert!(
        !stop_write_is_redundant((true, 3.0), (true, -5.0)),
        "the trader moved the stop, so the order must carry the new one"
    );
    assert!(
        !stop_write_is_redundant((true, 3.0), (false, -3.0)),
        "the trader turned the stop off, which the order must be told"
    );
    assert!(
        !stop_write_is_redundant((false, 0.0), (true, -3.0)),
        "the strategy has no stop but the screen shows one: it has to be written"
    );
    // The asymmetry that matters: the overlay was clamped to the protocol range on the way in, the
    // strategy's own value was not. Calling these equal would let the order ride -30% while the
    // toolbar reads -20%.
    assert!(
        !stop_write_is_redundant((true, 30.0), (true, -20.0)),
        "an out-of-range strategy stop is exactly what the per-order write must override"
    );
}
use moon_core::config::ManualStratState;

/// Build a stored selection as the config holds it.
///
/// Args:
///     name: Strategy name the trader selected.
///     id: Pinned strategy id, or `0` for a config written before ids were kept.
///
/// Returns:
///     The stored per-core manual-strategy state.
fn stored(name: &str, id: u64) -> ManualStratState {
    ManualStratState {
        on: true,
        strategy: name.to_string(),
        id,
        ..ManualStratState::default()
    }
}

/// Regression target (BB1, 2026-09-01): re-deriving the id from the stored NAME before every order.
///
/// Moonbot substitutes manual hook strategies while they run — its own log says "Manual strategy
/// HookTest12 turned into Hook HookTest1" — so the same name can resolve to a different strategy
/// between two clicks. Two live orders went out on HookTest1 with its -4.51% stop while the trader
/// had HookTest12 with -3% selected and had touched nothing. The pinned id is what makes the
/// selection hold still.
#[test]
fn a_pinned_id_outranks_a_name_that_now_resolves_elsewhere() {
    let rows = [
        manual_strategy(7394, "HookTest12"),
        manual_strategy(2981, "HookTest12"),
    ];
    assert_eq!(
        resolve_manual_selection(&rows, &stored("HookTest12", 2981)),
        Some(2981),
        "the strategy the trader actually picked, not whichever the name reaches first"
    );
}

/// The name is the anchor for exactly one case: the pinned strategy is gone, because a re-created
/// strategy keeps its name and loses its number. Without this the selection would die with the id.
#[test]
fn the_name_takes_over_once_the_pinned_id_is_gone() {
    let rows = [manual_strategy(555, "HookTest12")];
    assert_eq!(
        resolve_manual_selection(&rows, &stored("HookTest12", 2981)),
        Some(555),
        "a rebuilt strategy is found again by name"
    );
    assert_eq!(
        resolve_manual_selection(
            &[manual_strategy(555, "Other")],
            &stored("HookTest12", 2981)
        ),
        None,
        "neither the pinned id nor the name resolves, so there is nothing to place on"
    );
}

/// A pinned id that names a NON-Manual row must not be honoured: ids are per-core numbers and
/// another kind of strategy can inherit one, which would place a manual order on it.
#[test]
fn a_pinned_id_must_still_name_a_manual_strategy() {
    let rows = [
        strategy(2981, MANUAL_STRATEGY_KIND - 1),
        manual_strategy(555, "HookTest12"),
    ];
    assert_eq!(
        resolve_manual_selection(&rows, &stored("HookTest12", 2981)),
        Some(555),
        "the pin is ignored when it no longer names a Manual strategy"
    );
}

/// Build a NAMED Manual-kind strategy row, for the seed's id-to-name resolution.
///
/// Args:
///     id: Stable strategy id.
///     name: Strategy name as the core reports it.
///
/// Returns:
///     Minimal retained Manual-kind row carrying that name.
fn manual_strategy(id: u64, name: &str) -> StrategyRow {
    StrategyRow {
        name: name.to_string(),
        ..strategy(id, MANUAL_STRATEGY_KIND)
    }
}

/// Regression target (BB1, 2026-09-01): reading the SELECTED strategy's own stop while a MoonHook
/// supplied the real one. Both the seed and the per-order comparison resolve the source here, so a
/// wrong answer either shows a stop no order uses or suppresses the write that would have fixed it.
#[test]
fn the_exit_source_is_the_hook_whenever_one_is_named() {
    let manual = manual_strategy(7, "manual2");
    let hook = StrategyRow {
        name: "HookTest1".to_string(),
        ..strategy(31, HOOK_STRATEGY_KIND)
    };
    let snapshot = [manual.clone(), hook.clone()];
    assert_eq!(
        exit_source(&snapshot, "", 7),
        Some(7),
        "no hook: the strategy carries its own exits"
    );
    assert_eq!(
        exit_source(&snapshot, "HookTest1", 7),
        Some(31),
        "a named hook owns both the sell price and the stop, so it is the source"
    );
    assert_eq!(
        exit_source(&snapshot, "HookTest9", 7),
        None,
        "a hook this core does not have leaves the exits unknowable, which is not the same as none"
    );
    // The kind is part of the match, not decoration: `UseHookStrategy` is a picklist over MoonHook
    // strategies, and a Manual strategy sharing the name has entirely different exits.
    let namesake = [
        manual,
        StrategyRow {
            name: "HookTest1".to_string(),
            ..strategy(44, MANUAL_STRATEGY_KIND)
        },
    ];
    assert_eq!(
        exit_source(&namesake, "HookTest1", 7),
        None,
        "only a MoonHook-kind row can be a hook, whatever else carries the name"
    );
}

/// Regression target: gating the seed on a non-zero revision instead of on a non-empty list. The
/// feed publishes `InitialStrategies::new(0, Vec::new())` during init, so a revision check alone
/// lets the seed resolve every id against nothing and store that answer permanently.
#[test]
fn the_seed_waits_while_no_strategy_has_arrived() {
    // The mode is OFF here on purpose: with it on, the unresolved-selection guard would answer
    // `None` too and this would pass with the empty-list rule reverted.
    assert_eq!(
        manual_strat_seed(false, 0, &[]),
        None,
        "an empty list is 'ask again', not 'this core has no manual strategy'"
    );
}

/// Regression target: answering at all for an enabled core whose selection does not resolve. The
/// strategy list arrives in partial payloads, so the first non-empty one can simply not carry the
/// selected row yet — and either possible answer is destructive, because a stored value is what
/// stops the seed from running again: `on` with no name latches a mode that names no strategy, and
/// `off` discards the selection this seed exists to carry across the upgrade.
#[test]
fn the_seed_waits_for_an_enabled_core_whose_selection_has_not_arrived() {
    let rows = [manual_strategy(11, "Alpha")];
    assert_eq!(
        manual_strat_seed(true, 77, &rows),
        None,
        "an incompletely read core must be asked again, not answered"
    );
}

/// A core that has selected NOTHING is answerable whatever its list holds: there is no selection
/// to lose, so the seed must settle it rather than re-examine it on every later tick.
#[test]
fn the_seed_settles_a_core_that_selected_nothing() {
    let rows = [strategy(11, MANUAL_STRATEGY_KIND - 1)];
    assert_eq!(
        manual_strat_seed(false, 0, &rows),
        Some(super::ManualStratState::default()),
        "a resolved 'nothing to select' is an answer, not a reason to wait"
    );
}

/// Regression target: applying the unresolved-selection guard only while the mode is ON. A core
/// with the mode off still HAS a selection, and it is what the trader gets back the moment they
/// switch the mode on — settling it as "nothing selected" throws that away for good, because a
/// stored answer is what stops the seed from asking again.
#[test]
fn the_seed_waits_for_an_unresolved_selection_even_with_the_mode_off() {
    let rows = [manual_strategy(11, "Alpha")];
    assert_eq!(
        manual_strat_seed(false, 77, &rows),
        None,
        "the selection has not arrived yet, whatever the mode says"
    );
}

/// The upgrade path this whole change hinges on: a trader who left Moonbot on a manual strategy
/// must find the terminal on that same strategy, stored by NAME.
#[test]
fn the_seed_adopts_the_core_selection_by_name() {
    let rows = [
        manual_strategy(11, "Alpha"),
        manual_strategy(77, "  Beta  "),
    ];
    let seeded = manual_strat_seed(true, 77, &rows).expect("a resolved selection must answer");
    assert!(seeded.on, "the core had the mode on and it resolved");
    assert_eq!(
        seeded.strategy, "Beta",
        "the seed trims on the way in, so the stored name is the canonical one"
    );
    assert_eq!(
        seeded.id, 77,
        "the id is pinned at the same moment, so no later order re-derives it"
    );
}

/// Regression target, twice over: an EMPTY strategy list read as "this core has no manual
/// strategies" (which switches the refusal off in exactly the window where the selection cannot be
/// resolved, so a bare order goes out under the group's TP/SL), and a CONFIRMED list without any
/// Manual strategy read as a broken selection (which refuses every order on a core whose whole MS
/// cluster is hidden, leaving no control able to clear it).
#[test]
fn a_broken_selection_is_told_apart_from_a_list_that_has_not_arrived() {
    let selection = stored("HookTest12", 2981);
    assert!(
        manual_selection_is_broken(&[], &selection),
        "an empty list cannot resolve the selection, so the order must be refused"
    );
    assert!(
        !manual_selection_is_broken(&[strategy(11, MANUAL_STRATEGY_KIND - 1)], &selection),
        "a confirmed list with no Manual strategy is a core this mode does not apply to"
    );
    assert!(
        manual_selection_is_broken(&[manual_strategy(11, "Other")], &selection),
        "Manual strategies exist but not this one: renamed, deleted, or not published yet"
    );
    assert!(
        !manual_selection_is_broken(&[manual_strategy(2981, "HookTest12")], &selection),
        "the selection resolves, so there is nothing to refuse"
    );
    assert!(
        !manual_selection_is_broken(&[], &ManualStratState::default()),
        "no selection at all is never broken, whatever the list holds"
    );
    // The two halves of that first guard, each on its own: a selection the trader turned OFF, and
    // the mode left ON with nothing chosen. Neither can be refused — there is no strategy an order
    // is missing.
    assert!(
        !manual_selection_is_broken(
            &[],
            &ManualStratState {
                on: false,
                ..selection.clone()
            }
        ),
        "the mode is off, so no order is waiting on this selection"
    );
    assert!(
        !manual_selection_is_broken(
            &[],
            &ManualStratState {
                on: true,
                strategy: "   ".to_string(),
                id: 0,
                ..ManualStratState::default()
            }
        ),
        "the mode is on but nothing is chosen, which is an ordinary manual order"
    );
}

/// Regression target: trimming only one side of the name comparison. The seed stores a trimmed
/// name while the core reports it untrimmed, so a one-sided test would fail to resolve the very
/// strategy that was just stored — and every order on that core would be refused, permanently.
#[test]
fn the_name_lookup_trims_both_sides() {
    let rows = [manual_strategy(77, "  Beta  ")];
    assert_eq!(
        manual_strategy_id(&rows, "Beta"),
        Some(77),
        "the stored, trimmed name must resolve against the core's untrimmed one"
    );
    assert_eq!(
        manual_strategy_id(&rows, "   "),
        None,
        "a blank name is no selection at all"
    );
    assert_eq!(
        manual_strategy_id(&[strategy(77, MANUAL_STRATEGY_KIND - 1)], "Test"),
        None,
        "only Manual-kind rows may be selected"
    );
}

/// The mode being off on the core must still record the SELECTION, or turning the switch on in the
/// terminal would find nothing chosen and force the trader to pick their strategy again.
#[test]
fn the_seed_keeps_the_selection_of_a_core_with_the_mode_off() {
    let rows = [manual_strategy(77, "Beta")];
    let seeded = manual_strat_seed(false, 77, &rows).expect("a populated list must answer");
    assert!(!seeded.on, "the core had the mode off");
    assert_eq!(
        seeded.strategy, "Beta",
        "the selection is still worth keeping"
    );
    assert_eq!(seeded.id, 77, "and so is the id it resolved to");
}

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

/// Regression target: treating an EMPTY list as a confirmed answer. The feed publishes an empty
/// list at revision 1 during init, so a revision-based gate reads every fresh connection as "no
/// manual strategy here" — which silently turns the mode off for a while after each reconnect and,
/// since the order now names its own strategy, places a bare order instead of refusing.
#[test]
fn pending_strategy_snapshot_keeps_raw_manual_state() {
    assert_eq!(effective_manual_strat_state((true, 77), &[]), (true, 77));
}

/// Preserving raw enabled state after a confirmed zero-Manual snapshot would strand TP/SL disabled.
#[test]
fn confirmed_snapshot_without_manual_strategies_disables_effective_mode() {
    assert_eq!(
        effective_manual_strat_state((true, 77), &[strategy(11, MANUAL_STRATEGY_KIND - 1)]),
        (false, 77)
    );
}

/// Requiring the selected id to resolve would erase the existing invalid-id warning and repair UI.
#[test]
fn any_manual_strategy_preserves_raw_state_and_selected_id() {
    assert_eq!(
        effective_manual_strat_state((true, 77), &[strategy(11, MANUAL_STRATEGY_KIND)]),
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
