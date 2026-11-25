//! Which ceiling the toolbar's `MAX` states (#1067).

// NOT `use super::*`: the parent's glob re-exports reach `gpui::test`, and `#[test]` would expand
// into itself (recursion limit).
use super::{MaxCeiling, MaxOrderReadout};
use moon_core::market::{MarketLimits, MaxOrder, MaxOrderSource};

/// Limits carrying one per-order cap and, optionally, one position cap.
fn limits(value: f64, source: MaxOrderSource, cap: Option<f64>) -> Option<MarketLimits> {
    Some(MarketLimits {
        max_order: MaxOrder { value, source },
        max_leverage: 75,
        position_cap: cap,
    })
}

/// Regression: the row must state the position cap when it is the lower ceiling.
///
/// The edit this catches: `MaxCeiling::of` reverting to the per-order figure alone — Binance PENDLE
/// at ×75, a 1.23M$ quantity cap against a 5 000 USDT position cap, where the row then names a
/// ceiling two hundred times too high.
#[test]
fn a_lower_position_cap_is_what_the_row_states() {
    let ceiling = MaxCeiling::of(limits(1_230_000.0, MaxOrderSource::Derived, Some(5_000.0)));
    assert_eq!(
        ceiling,
        MaxCeiling::CapBinds {
            cap: 5_000.0,
            order: Some(1_230_000.0)
        }
    );
    assert_eq!(ceiling.value(), Some(5_000.0));
}

/// Regression: a position cap ABOVE the per-order cap must not displace it.
///
/// The edit this catches: taking the position cap whenever one is stated (a `max` or a plain
/// preference) — the row would then overstate the ceiling on an OKX account-wide cap of 350 000
/// beside a wide tier.
#[test]
fn a_higher_position_cap_leaves_the_per_order_cap_binding() {
    let ceiling = MaxCeiling::of(limits(350_000.0, MaxOrderSource::Stated, Some(2_000_000.0)));
    assert_eq!(ceiling.value(), Some(350_000.0));
    assert!(matches!(ceiling, MaxCeiling::OrderBinds { cap, .. } if cap == 2_000_000.0));
}

/// Regression: with no position cap stated the readout is exactly today's per-order one, its
/// unknowns included.
///
/// The edit this catches: folding "no position cap" into a dash or a zero, which would blank the
/// row on every spot market and every venue that states no such cap.
#[test]
fn without_a_position_cap_the_readout_is_unchanged() {
    for read in [
        limits(496_000.0, MaxOrderSource::Stated, None),
        limits(0.0, MaxOrderSource::Absent, None),
        limits(0.0, MaxOrderSource::Pending, None),
        None,
    ] {
        assert_eq!(
            MaxCeiling::of(read),
            MaxCeiling::Order(MaxOrderReadout::of(read))
        );
    }
}

/// Regression: a market with no per-order cap but a position cap states the position cap, not
/// "no maximum".
///
/// The edit this catches: requiring BOTH caps before the position cap may show, which tells the
/// trader the market has no ceiling while the exchange will stop the order.
#[test]
fn a_position_cap_alone_is_stated() {
    let ceiling = MaxCeiling::of(limits(0.0, MaxOrderSource::Absent, Some(25_000.0)));
    assert_eq!(
        ceiling,
        MaxCeiling::CapBinds {
            cap: 25_000.0,
            order: None
        }
    );
}

/// Regression: a per-order cap that is still converting must not be overruled by the position
/// cap — which binds cannot be told yet.
///
/// The edit this catches: treating `Pending` like `Absent`, which names the position cap the
/// ceiling during the first second of every linear market, even where the per-order cap is lower.
#[test]
fn a_pending_per_order_cap_keeps_the_not_loaded_readout() {
    let ceiling = MaxCeiling::of(limits(0.0, MaxOrderSource::Pending, Some(25_000.0)));
    assert_eq!(ceiling, MaxCeiling::Order(MaxOrderReadout::NoData));
    assert_eq!(ceiling.value(), None);
}

/// The position cap goes through the per-order compact unit rule: `$` for a USD stable only.
#[test]
fn a_binding_position_cap_uses_the_compact_unit_rule() {
    let ceiling = MaxCeiling::CapBinds {
        cap: 5_000.0,
        order: None,
    };
    assert_eq!(ceiling.format_compact(|v| format!("{v}"), "USDT"), "5000$");
    assert_eq!(
        ceiling.format_compact(|v| format!("{v}"), "BTC"),
        "5000 BTC"
    );
}
