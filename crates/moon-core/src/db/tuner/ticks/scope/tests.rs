use super::*;

/// The service rows as the live replica spells them: no strategy behind the row, or a reason
/// that names something other than a trade.
#[test]
fn service_rows_are_the_no_strategy_and_the_named_reasons() {
    assert!(is_service_row(0, "Manual Sell"));
    assert!(is_service_row(0, "Funding"));
    assert!(is_service_row(42, "Funding"));
    assert!(is_service_row(42, "LIQUIDATION"));
    assert!(is_service_row(42, "JoinedSell"));
}

/// A spot sale topped up from the wallet balance moved coins the entry never bought, so its
/// price is an average of something else.
#[test]
fn a_sale_bigger_than_its_entry_is_not_a_deal() {
    assert!(sold_more_than_bought(101.0, 100.0));
    assert!(!sold_more_than_bought(100.0, 100.0), "the ordinary case");
    // The fee is taken in coin on spot, so selling slightly LESS is normal.
    assert!(!sold_more_than_bought(99.88, 100.0));
    // Nothing to compare against is not a finding.
    assert!(!sold_more_than_bought(101.0, 0.0));
    assert!(!sold_more_than_bought(f64::NAN, 100.0));
    assert!(!sold_more_than_bought(101.0, f64::INFINITY));
    // Float noise on a lot must not read as a top-up.
    assert!(!sold_more_than_bought(100.0 + 1e-9, 100.0));
    assert!(!is_service_row(42, "Auto Price Down"));
    assert!(!is_service_row(42, "Sell Price"));
    assert!(!is_service_row(42, "StopLoss Market Sell"));
    // A strategy NAMED after liquidations trades like any other; only the exact reason is
    // the exchange's own closing.
    assert!(!is_service_row(
        42,
        "Liquidation short <Liquidations_Short_250620>"
    ));
}

/// Every trading kind is tunable; the containers and an unresolved kind are not.
#[test]
fn tunable_kinds_are_the_trading_ones_resolved() {
    for kind in [
        "MoonShot",
        "MoonHook",
        "Spread",
        "TopMarket",
        "EMA",
        "Liquidations",
    ] {
        assert!(tunable_kind(kind), "{kind}");
    }
    for kind in ["", "Manual", "Alerts", "Watcher"] {
        assert!(!tunable_kind(kind), "{kind:?}");
    }
}

/// A manual exit is the operator's whatever the strategy; the strategy's own reasons are not.
#[test]
fn manual_exits_leave_the_tunable_set_and_rule_exits_stay() {
    assert!(is_manual_exit("Manual Sell"));
    assert!(is_manual_exit("Manual PanicSell"));
    assert!(is_manual_exit("SellFromAssets"));
    assert!(!is_manual_exit("Auto Price Down"));
    assert!(is_tunable("MoonShot", "Auto Price Down"));
    assert!(!is_tunable("MoonShot", "Manual Sell"));
    assert!(!is_tunable("Alerts", "Sell Price"));
    assert!(!is_tunable("", "Sell Price"));
}
