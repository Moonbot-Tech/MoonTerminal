//! Persisted group-trade repair regressions.

use super::*;

/// Build a complete visible exit state with one caller-selected fixed-sell percentage.
fn exit_settings(take_profit_pct: f64, first_fixed_sell_pct: f64) -> GroupExitSettings {
    GroupExitSettings {
        take_profit_pct,
        take_profit_mode: TakeProfitMode::Normal,
        fixed_sell_pcts: [first_fixed_sell_pct, 2.0, 3.0, 4.0, 5.0, 6.0],
        fixed_sell_slot: None,
        stop_loss_pct: -5.0,
        stop_loss_enabled: true,
        use_stop_market: false,
    }
}

/// Regression target: changing `GroupExitSettings::default` back to a non-neutral TP makes a new
/// or migrated group submit an exit level that the user never selected.
#[test]
fn missing_group_exit_loads_a_complete_neutral_standard() {
    let trade: GroupTradeSettings =
        toml::from_str("").expect("an older group trade block without exit settings must parse");

    assert_eq!(
        trade.exit,
        GroupExitSettings {
            take_profit_pct: 0.0,
            take_profit_mode: TakeProfitMode::Scalp,
            fixed_sell_pcts: [0.0; 6],
            fixed_sell_slot: None,
            stop_loss_pct: 0.0,
            stop_loss_enabled: false,
            use_stop_market: false,
        }
    );
}

/// Regression target: removing the strict persisted-range preflight from
/// `GroupExitSettings::canonicalize` clamps a corrupt finite or infinite TP while preserving the
/// rest of its stale exit generation.
#[test]
fn invalid_take_profit_resets_the_complete_exit_generation() {
    for (mode, pct) in [
        (TakeProfitMode::Scalp, -0.01),
        (TakeProfitMode::Scalp, 2.01),
        (TakeProfitMode::Normal, 0.99),
        (TakeProfitMode::Normal, 100.01),
        (TakeProfitMode::Extended, 99.0),
        (TakeProfitMode::Extended, 901.0),
        (TakeProfitMode::Normal, f64::INFINITY),
    ] {
        let mut exit = exit_settings(pct, 1.0);
        exit.take_profit_mode = mode;
        let mut trade = GroupTradeSettings {
            exit,
            ..GroupTradeSettings::default()
        };

        assert!(trade.repair());
        assert_eq!(trade.exit, GroupExitSettings::default());
    }
}

/// Regression target: repairing a corrupt S/SL/slot field in place preserves the remaining stale
/// generation, so the toolbar no longer starts from the one documented neutral standard.
#[test]
fn any_corrupt_non_tp_component_resets_the_complete_exit_generation() {
    let fixed_sell_overflow = exit_settings(10.0, 1.0e300);
    let mut nonfinite_stop_loss = exit_settings(10.0, 1.0);
    nonfinite_stop_loss.stop_loss_pct = f32::NAN;
    let mut stop_loss_below_range = exit_settings(10.0, 1.0);
    stop_loss_below_range.stop_loss_pct = -20.01;
    let mut stop_loss_above_range = exit_settings(10.0, 1.0);
    stop_loss_above_range.stop_loss_pct = 1.01;
    let mut invalid_fixed_sell_slot = exit_settings(10.0, 1.0);
    invalid_fixed_sell_slot.fixed_sell_slot = Some(7);

    for corrupt in [
        fixed_sell_overflow,
        nonfinite_stop_loss,
        stop_loss_below_range,
        stop_loss_above_range,
        invalid_fixed_sell_slot,
    ] {
        let mut trade = GroupTradeSettings {
            exit: corrupt,
            ..GroupTradeSettings::default()
        };

        assert!(trade.repair());
        assert_eq!(trade.exit, GroupExitSettings::default());
    }
}

/// Regression target: changing the public interactive canonicalizers from clamp to reject makes a
/// field or slider edit outside its bounds disappear instead of landing on the nearest endpoint.
#[test]
fn interactive_canonicalizers_still_clamp_finite_edits() {
    assert_eq!(
        TakeProfitMode::Scalp.canonical_take_profit_pct(-1.0),
        Some(0.0)
    );
    assert_eq!(
        TakeProfitMode::Scalp.canonical_take_profit_pct(3.0),
        Some(2.0)
    );
    assert_eq!(
        TakeProfitMode::Normal.canonical_take_profit_pct(0.0),
        Some(1.0)
    );
    assert_eq!(
        TakeProfitMode::Normal.canonical_take_profit_pct(101.0),
        Some(100.0)
    );
    assert_eq!(
        TakeProfitMode::Extended.canonical_take_profit_pct(99.0),
        Some(100.0)
    );
    assert_eq!(
        TakeProfitMode::Extended.canonical_take_profit_pct(901.0),
        Some(900.0)
    );
    assert_eq!(
        GroupExitSettings::canonical_stop_loss_pct(-21.0),
        Some(-20.0)
    );
    assert_eq!(GroupExitSettings::canonical_stop_loss_pct(2.0), Some(1.0));
}

/// Regression target: making a persisted range endpoint exclusive in
/// `GroupExitSettings::canonicalize` resets a valid boundary value and all sibling exit fields.
#[test]
fn persisted_exit_range_endpoints_remain_valid() {
    for (mode, pct) in [
        (TakeProfitMode::Scalp, 0.0),
        (TakeProfitMode::Scalp, 2.0),
        (TakeProfitMode::Normal, 1.0),
        (TakeProfitMode::Normal, 100.0),
        (TakeProfitMode::Extended, 100.0),
        (TakeProfitMode::Extended, 900.0),
    ] {
        for stop_loss_pct in [-20.0, 1.0] {
            let mut exit = exit_settings(pct, 1.0);
            exit.take_profit_mode = mode;
            exit.stop_loss_pct = stop_loss_pct;

            assert!(
                exit.canonicalize(),
                "{mode:?} {pct}% with SL {stop_loss_pct}% must stay valid"
            );
        }
    }
}
