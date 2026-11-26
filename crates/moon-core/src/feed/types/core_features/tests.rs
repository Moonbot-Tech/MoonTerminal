//! Build detection from the runtime report and the force-cancel gate over order status.

use super::*;

/// A core that sent neither tail is older than N1 and gets neither command.
///
/// Mutation: default either flag to `true`. The menu would then offer a forced cancellation that
/// a 7.72 core drops without a word.
#[test]
fn a_core_without_either_tail_supports_nothing() {
    assert_eq!(
        CoreFeatures::from_runtime_report(None, None::<()>),
        CoreFeatures::default()
    );
    assert!(!CoreFeatures::default().force_cancel);
    assert!(!CoreFeatures::default().exchange_buys_cancel);
}

/// N1 reports the switch but not the extended state: exchange-wide buys cancel yes, force no.
///
/// Mutation: drive `force_cancel` from the switch. Every N1 core would then show an action it
/// ignores.
#[test]
fn n1_tail_enables_only_the_exchange_wide_cancel() {
    // A switch that is OFF is still reported: presence, not value, proves the build.
    let n1 = CoreFeatures::from_runtime_report(Some(false), None::<()>);
    assert!(n1.exchange_buys_cancel);
    assert!(!n1.force_cancel);
}

/// N2's tail proves both builds, even if the switch byte were somehow absent.
///
/// Mutation: read `exchange_buys_cancel` from the switch alone. A core whose report carried the
/// N2 tail would then lose the N1 command.
#[test]
fn n2_tail_enables_both() {
    for switch in [None, Some(true)] {
        let n2 = CoreFeatures::from_runtime_report(switch, Some(()));
        assert!(n2.force_cancel && n2.exchange_buys_cancel);
    }
}

/// Only BuySet and SellSet are active legs; everything else is hidden, whatever the core supports.
///
/// Mutation: accept `None` (a pending entry) or `BuyDone`. MoonProto sends nothing for those, so
/// the confirmation would promise an action that never leaves the terminal.
#[test]
fn only_active_buy_or_sell_legs_are_eligible() {
    let n2 = CoreFeatures::from_runtime_report(Some(true), Some(()));
    for status in ["BuySet", "SellSet"] {
        assert_eq!(
            n2.force_cancel_gate(status, false),
            ForceCancelGate::Offered
        );
    }
    for status in [
        "None",
        "BuyFail",
        "BuyCancel",
        "BuyDone",
        "SellFail",
        "SellCancel",
        "SellDone",
        "SellAlmostDone",
        "",
    ] {
        assert_eq!(
            n2.force_cancel_gate(status, false),
            ForceCancelGate::NotEligible,
            "{status:?}"
        );
    }
}

/// A finished order is never offered, even while its last status still reads as an active leg.
///
/// Mutation: drop the `job_is_done` check. A filled sell awaiting removal would offer a forced
/// cancellation of an order that no longer exists.
#[test]
fn a_finished_order_is_not_eligible() {
    let n2 = CoreFeatures::from_runtime_report(Some(true), Some(()));
    assert_eq!(
        n2.force_cancel_gate("SellSet", true),
        ForceCancelGate::NotEligible
    );
}

/// An eligible order on an older or not-yet-reported core is disabled, not offered.
///
/// Mutation: return `Offered` whenever the leg is active. An N1 core would get a live action.
#[test]
fn an_eligible_order_on_an_older_core_needs_a_newer_core() {
    for features in [
        CoreFeatures::default(),
        CoreFeatures::from_runtime_report(Some(true), None::<()>),
    ] {
        assert_eq!(
            features.force_cancel_gate("BuySet", false),
            ForceCancelGate::NeedsNewerCore
        );
    }
}
