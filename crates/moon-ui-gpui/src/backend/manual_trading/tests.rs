//! Manual-trading state, conversion, and group-generation regressions.

use moon_core::config::{
    DEFAULT_ORDER_SIZES_USD, GroupExitSettings, GroupTradeSettings, TakeProfitMode,
};
use moon_core::feed::{ClientSettingsEdit, StrategyRow};

use super::{
    MANUAL_STRATEGY_KIND, apply_group_exit_edit, effective_manual_strat_state,
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
