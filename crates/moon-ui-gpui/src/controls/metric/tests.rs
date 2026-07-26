//! Identity and availability contracts for guarded toolbar-metric edits.

// NOT `use super::*`: the glob would pull in the `gpui::test` macro re-exported by the parent,
// and `#[test]` would expand into itself (recursion limit).
use super::{MetricTarget, TradeMetric};

/// First distinct core identity used by target-comparison tests.
const CORE_A: u64 = 7;
/// Second distinct core identity used by target-comparison tests.
const CORE_B: u64 = 9;

/// Build a target for a group-local TP or SL metric.
fn group_local() -> MetricTarget {
    MetricTarget {
        core: None,
        market: None,
    }
}

/// Build a target for leverage, which is stored at core-and-market scope.
fn per_market(core: u64, market: &str) -> MetricTarget {
    MetricTarget {
        core: Some(core),
        market: Some(market.to_string()),
    }
}

/// Regression: dropping core or market identity can redirect a seeded leverage popup.
#[test]
fn a_seeded_leverage_address_does_not_match_a_different_market() {
    // Plausible future edit: `controls::metric::MetricTarget` stops recording either the core or
    // market. A core selector or Main-chart switch would then leave Apply targeting stale leverage.
    assert_ne!(
        per_market(CORE_A, "BTCUSDT"),
        per_market(CORE_B, "BTCUSDT"),
        "leverage seeded for one core must not match another"
    );
    assert_ne!(
        per_market(CORE_A, "BTCUSDT"),
        per_market(CORE_A, "ETHUSDT"),
        "leverage seeded for one market must not match another"
    );
    assert_ne!(
        per_market(CORE_A, "BTCUSDT"),
        group_local(),
        "a leverage address must not match a group-local exit"
    );
}

/// Regression: reintroducing a blanket core gate disables valid group-local exit editing.
#[test]
fn availability_gates_each_metric_on_its_own_condition() {
    // Plausible future edit: `controls::metric::TradeMetric::available_with` puts `has_core &&`
    // around the match. TP and SL would become uneditable when a persisted group has no live core.
    assert!(!TradeMetric::Lev.available_with(false, true, false));
    assert!(TradeMetric::Lev.available_with(true, false, true));
    assert!(TradeMetric::Tp.available_with(false, false, false));
    assert!(!TradeMetric::Tp.available_with(true, false, true));
    assert!(TradeMetric::Sl.available_with(false, true, false));
    assert!(!TradeMetric::Sl.available_with(true, false, false));
    assert!(!TradeMetric::Sl.available_with(true, true, true));
}
