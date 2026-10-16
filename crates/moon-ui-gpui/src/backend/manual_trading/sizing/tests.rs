//! Synthetic collateral sizing regressions shared by manual orders and FireTest quote overrides.

use crate::Backend;
use moon_core::market::{MarketQuantityUnit, OrderSizeRules};

/// Dividing a linear quote amount by the token price oversizes the core's money amount.
#[test]
fn linear_quote_size_stays_in_account_collateral() {
    let rules = OrderSizeRules {
        unit: MarketQuantityUnit::Coins,
        min_order_usd: Some(4.0),
    };
    assert_eq!(
        Backend::manual_order_collateral_size(1, "XALPHA_UNIT", 12.0, 0.08, rules, || Some(1.0)),
        Some(12.0)
    );
    // A non-dollar account still buys the requested USD equivalent, as manual presets did.
    assert_eq!(
        Backend::manual_order_collateral_size(1, "XALPHA_UNIT", 12.0, 0.08, rules, || Some(2.0)),
        Some(6.0)
    );
}

/// Treating inverse collateral as quote money or contracts spends the wrong coin amount.
#[test]
fn inverse_quote_size_uses_coin_collateral_without_contract_multiplier() {
    let rules = OrderSizeRules {
        unit: MarketQuantityUnit::Contracts(25.0),
        min_order_usd: Some(4.0),
    };
    assert_eq!(
        Backend::manual_order_collateral_size(1, "XALPHA_UNIT", 12.0, 80.0, rules, || {
            panic!("inverse collateral must not query the account USD rate")
        }),
        Some(0.15)
    );
}

/// Bypassing the shared floor lets quote overrides queue orders the venue cannot accept.
#[test]
fn quote_size_refuses_below_minimum_in_both_collateral_units() {
    for unit in [
        MarketQuantityUnit::Coins,
        MarketQuantityUnit::Contracts(25.0),
    ] {
        let rules = OrderSizeRules {
            unit,
            min_order_usd: Some(4.0),
        };
        assert_eq!(
            Backend::manual_order_collateral_size(1, "XALPHA_UNIT", 3.0, 80.0, rules, || Some(1.0)),
            None
        );
        let expected = match unit {
            MarketQuantityUnit::Coins => 4.0,
            MarketQuantityUnit::Contracts(_) => 0.05,
        };
        assert_eq!(
            Backend::manual_order_collateral_size(1, "XALPHA_UNIT", 4.0, 80.0, rules, || Some(1.0)),
            Some(expected)
        );
    }
}

/// Removing validation sends non-finite/zero amounts, or guesses a missing collateral rate.
#[test]
fn quote_size_requires_valid_amount_and_collateral_rate() {
    for unit in [
        MarketQuantityUnit::Coins,
        MarketQuantityUnit::Contracts(25.0),
    ] {
        let rules = OrderSizeRules {
            unit,
            min_order_usd: None,
        };
        for usd in [0.0, -1.0, f64::NAN, f64::INFINITY] {
            assert_eq!(
                Backend::manual_order_collateral_size(1, "XALPHA_UNIT", usd, 80.0, rules, || Some(
                    1.0
                )),
                None
            );
        }
        let (price, rate) = match unit {
            MarketQuantityUnit::Coins => (80.0, None),
            MarketQuantityUnit::Contracts(_) => (0.0, Some(1.0)),
        };
        assert_eq!(
            Backend::manual_order_collateral_size(1, "XALPHA_UNIT", 12.0, price, rules, || rate),
            None
        );
    }
}
