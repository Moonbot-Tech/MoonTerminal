//! Identity and availability contracts for guarded toolbar-metric edits.

// NOT `use super::*`: the glob would pull in the `gpui::test` macro re-exported by the parent,
// and `#[test]` would expand into itself (recursion limit).
use super::{MetricTarget, TradeMetric};

/// First distinct core identity used by target-comparison tests.
const CORE_A: u64 = 7;
/// Second distinct core identity used by target-comparison tests.
const CORE_B: u64 = 9;

/// Build a target for a metric stored only at core scope.
fn per_core(core: u64) -> MetricTarget {
    MetricTarget { core, market: None }
}

/// Build a target for leverage, which is stored at core-and-market scope.
fn per_market(core: u64, market: &str) -> MetricTarget {
    MetricTarget {
        core,
        market: Some(market.to_string()),
    }
}

/// Regression: weakening target equality can keep editing a core after the UI moves elsewhere.
#[test]
fn a_seeded_address_does_not_match_a_different_core() {
    // Plausible future edit: `controls::metric::MetricTarget` loses its `core` field, or the
    // comparison behind `is_live` is weakened to the flags alone — someone reads the guard as an
    // availability check and decides the identity comparison beside it is redundant. The two
    // cores here are indistinguishable by flags, so availability cannot tell them apart: the
    // popup seeded for core A can remain interactive after the UI moves to core B, and one nudge
    // silently changes the no-longer-visible core A.
    assert_eq!(per_core(CORE_A), per_core(CORE_A));
    assert_ne!(
        per_core(CORE_A),
        per_core(CORE_B),
        "a popup seeded from one core must not match another"
    );
}

/// Regression: dropping market identity can keep editing leverage after the Main market moves.
#[test]
fn a_seeded_leverage_address_does_not_match_a_different_market() {
    // Leverage is the one metric stored and applied per (core, MARKET): the coin can change on
    // the Main chart with the core untouched, and Apply is an EXCHANGE write. Comparing only the
    // core would leave the old market's leverage editor active after the Main chart moves.
    assert_ne!(
        per_market(CORE_A, "BTCUSDT"),
        per_market(CORE_A, "ETHUSDT"),
        "leverage seeded for one market must not match another"
    );
    assert_ne!(
        per_market(CORE_A, "BTCUSDT"),
        per_core(CORE_A),
        "a per-market address must not match a per-core one"
    );
}

/// Regression: weakening `available_with` can expose an editor whose writes have no effect.
#[test]
fn availability_gates_each_metric_on_its_own_condition() {
    // Independent of the implementation: the three conditions come from the contract — no core
    // means nothing to edit, the manual strategy takes over TP and SL, and SL additionally
    // needs its toggle on.
    assert!(!TradeMetric::Lev.available_with(false, true, false));
    assert!(TradeMetric::Lev.available_with(true, false, true));
    assert!(TradeMetric::Tp.available_with(true, false, false));
    assert!(!TradeMetric::Tp.available_with(true, false, true));
    assert!(TradeMetric::Sl.available_with(true, true, false));
    assert!(!TradeMetric::Sl.available_with(true, false, false));
    assert!(!TradeMetric::Sl.available_with(true, true, true));
}
