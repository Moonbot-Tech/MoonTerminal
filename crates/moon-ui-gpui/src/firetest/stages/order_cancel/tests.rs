//! Executable FireTest override regressions using the real shared collateral sizing calculation.

use super::order_cancel_size_override;
use crate::Backend;
use moon_core::market::{MarketQuantityUnit, OrderSizeRules};

/// A quote/price override oversizes linear orders; skipping the shared floor accepts tiny orders.
#[test]
fn quote_override_keeps_linear_money_and_checks_both_market_minimums() {
    for (unit, price, expected) in [
        (MarketQuantityUnit::Coins, 0.08, 12.0),
        (MarketQuantityUnit::Contracts(25.0), 80.0, 0.15),
    ] {
        let resolve = |usd| {
            Backend::manual_order_collateral_size(
                1,
                "XALPHA_UNIT",
                usd,
                price,
                OrderSizeRules {
                    unit,
                    min_order_usd: Some(4.0),
                },
                || Some(1.0),
            )
            .map(|size| (size, usd))
        };
        assert_eq!(
            order_cancel_size_override(None, Some(12.0), resolve),
            Ok(Some(expected))
        );
        assert!(order_cancel_size_override(None, Some(3.0), resolve).is_err());
    }
}

/// Evaluating quote sizing before raw/default selection changes the existing override precedence.
#[test]
fn quote_override_preserves_raw_precedence_and_default_preset() {
    assert_eq!(
        order_cancel_size_override(Some(2.0), Some(12.0), |_| panic!("raw override wins")),
        Ok(Some(2.0))
    );
    assert_eq!(
        order_cancel_size_override(None, None, |_| panic!("normal preset needs no override")),
        Ok(None)
    );
    assert!(order_cancel_size_override(None, Some(12.0), |_| None).is_err());
}
