//! Origin classification: what the Auto / Manual filter files each order under.

use moon_core::feed::strat_kind_name;

use super::is_manual_order;

/// The issue's decided case: an order driven by a Manual-kind strategy carries that strategy's
/// id, and must still be Manual. A predicate on `strat_id == 0` alone files it under Auto.
#[test]
fn an_order_driven_by_a_manual_strategy_is_manual() {
    // The row's kind exactly as moon-core's live converter writes it for a kind-12 strategy.
    let manual_kind = strat_kind_name(crate::backend::MANUAL_STRATEGY_KIND);
    let other_kind = strat_kind_name(crate::backend::MANUAL_STRATEGY_KIND - 1);

    assert!(
        is_manual_order(41, manual_kind),
        "Manual-kind strategy order"
    );
    assert!(is_manual_order(0, ""), "order with no strategy at all");
    assert!(
        !is_manual_order(42, other_kind),
        "order of another strategy kind"
    );
}

/// An unknown snapshot leaves the numeric id in the kind field; that is no evidence of a Manual
/// strategy.
#[test]
fn an_unresolved_strategy_kind_is_auto() {
    assert!(!is_manual_order(77, "77"));
}
