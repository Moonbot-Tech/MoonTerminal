//! The core-settings popup's two-click confirmation and the exchange-entries gate.

use moon_core::feed::CoreFeatures;

use super::{ArmedAction, ExchangeCancelOffer, Press, exchange_cancel_offer, press};

const CORE: u64 = 77;

fn n1() -> CoreFeatures {
    CoreFeatures {
        exchange_buys_cancel: true,
        ..Default::default()
    }
}

/// The first click asks; only the second click on the same button sends, to the popup's core.
///
/// Mutation: send on the first click, or send the armed action to anything but `target`. The
/// exchange cancellation would then go out without the confirmation the issue requires.
#[test]
fn confirm_sends_once_to_the_popups_core() {
    let pressed = ArmedAction::ExchangeEntries;
    assert_eq!(
        press(None, pressed, Some(CORE), true),
        Press::Arm(pressed),
        "the first click must only arm"
    );
    assert_eq!(
        press(Some(pressed), pressed, Some(CORE), true),
        Press::Send(pressed, CORE)
    );
}

/// A second click on the OTHER destructive button re-arms that one and sends nothing.
///
/// Mutation: treat any armed slot as confirmation. Arming Cancel All and then clicking the
/// exchange button would send the exchange cancellation with no question asked about it.
#[test]
fn the_neighbour_button_never_confirms() {
    assert_eq!(
        press(
            Some(ArmedAction::CancelAll),
            ArmedAction::ExchangeEntries,
            Some(CORE),
            true
        ),
        Press::Arm(ArmedAction::ExchangeEntries)
    );
    assert_eq!(
        press(
            Some(ArmedAction::ExchangeEntries),
            ArmedAction::CancelAll,
            Some(CORE),
            true
        ),
        Press::Arm(ArmedAction::CancelAll)
    );
}

/// A confirmation answered after the popup lost its core, or after the action became unavailable,
/// sends nothing.
///
/// Mutation: fall back to the active core when the target is gone, or ignore `available` once
/// armed. A core that reconnected as an older build, or a different core now active, would take a
/// cancellation the user confirmed for something else.
#[test]
fn stale_or_unavailable_confirmations_send_nothing() {
    let pressed = ArmedAction::ExchangeEntries;
    assert_eq!(press(Some(pressed), pressed, None, true), Press::Disarm);
    assert_eq!(
        press(Some(pressed), pressed, Some(CORE), false),
        Press::Disarm
    );
    assert_eq!(press(None, pressed, Some(CORE), false), Press::Disarm);
}

/// The button is live only on a connected core that proved 7.73 N1.
///
/// Mutation: offer it on `CoreFeatures::default()` (an older core, or one whose runtime report has
/// not arrived), or test the flag before the connection. The first offers a request older cores
/// ignore; the second tells an offline N1 core it needs a newer version.
#[test]
fn offered_only_on_a_connected_n1_core() {
    assert_eq!(
        exchange_cancel_offer(true, n1()),
        ExchangeCancelOffer::Offered
    );
    assert_eq!(
        exchange_cancel_offer(true, CoreFeatures::default()),
        ExchangeCancelOffer::NeedsNewerCore
    );
    assert_eq!(
        exchange_cancel_offer(false, n1()),
        ExchangeCancelOffer::Offline
    );
    assert!(!ExchangeCancelOffer::NeedsNewerCore.available());
    assert!(!ExchangeCancelOffer::Offline.available());
}
