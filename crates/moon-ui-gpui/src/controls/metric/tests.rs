//! Identity and availability contracts for guarded toolbar-metric edits.

// NOT `use super::*`: the glob would pull in the `gpui::test` macro re-exported by the parent,
// and `#[test]` would expand into itself (recursion limit).
use super::{MetricTarget, TradeMetric, position_cap_text};
use moon_core::market::MarketLimits;

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
    assert!(!TradeMetric::Lev.available_with(false, true, false, false));
    assert!(TradeMetric::Lev.available_with(true, false, true, false));
    assert!(TradeMetric::Tp.available_with(false, false, false, false));
    assert!(!TradeMetric::Tp.available_with(true, false, true, false));
    assert!(TradeMetric::Sl.available_with(false, true, false, false));
    assert!(!TradeMetric::Sl.available_with(true, false, false, false));
    // SL stays editable in manual-strategy mode, unlike TP: there the button and its toggle carry
    // the STRATEGY's stop (level plus `UseStopLoss`), which is exactly what a trader has to reach
    // while MS is on. Disabling it here left the strategy's stop unreachable from the toolbar.
    assert!(TradeMetric::Sl.available_with(true, true, true, false));
    // The one thing that does close it: the core following Moonbot's own rule, where the strategy
    // owns the stop outright and no per-order override is sent. The lock is about the STOP alone —
    // leverage is a different account setting and TP is governed by the sell-price flag.
    assert!(!TradeMetric::Sl.available_with(true, true, true, true));
    assert!(TradeMetric::Lev.available_with(true, true, true, true));
}

/// Limits carrying only a position cap, for the position-cap readout tests.
fn with_cap(cap: Option<f64>) -> Option<MarketLimits> {
    Some(MarketLimits {
        position_cap: cap,
        ..MarketLimits::default()
    })
}

/// Regression: an unknown position cap must hide the row, never print `0`.
#[test]
fn an_unknown_position_cap_hides_its_row() {
    assert_eq!(position_cap_text(None, "USDT"), None);
    assert_eq!(position_cap_text(with_cap(None), "USDT"), None);
}

/// Regression: a known position cap is grouped like the per-order MAX and carries its quote.
#[test]
fn a_known_position_cap_is_grouped_with_its_quote() {
    let cap = 1_250_000.0;
    let grouped = moon_core::util::fmt::usd_grouped(cap);
    assert_eq!(
        position_cap_text(with_cap(Some(cap)), "USDT"),
        Some(format!("{grouped} USDT"))
    );
    assert_eq!(position_cap_text(with_cap(Some(cap)), ""), Some(grouped));
}
