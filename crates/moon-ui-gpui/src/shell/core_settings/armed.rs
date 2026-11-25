//! The two-click confirmation shared by the core-settings popup's destructive actions.
//!
//! Cancel All Orders and Cancel Entries On Exchange sit side by side above the tab strip and both
//! act on the exchange the moment they are confirmed. Each asks once: the first click arms it, and
//! only a second click on the SAME button sends. One armed slot for both, rather than a flag each,
//! so a second click can never confirm the action beside the one that was armed, and arming one
//! disarms the other.
//!
//! The confirmation stays inside the popup instead of a modal: a `MoonPopover` paints in a deferred
//! layer above Root's dialogs, so a dialog opened over it would sit underneath, and closing the
//! popup first would discard the user's unsaved edits on its tabs.

use moon_core::feed::CoreFeatures;
use moon_core::session::CoreId;

/// A destructive popup action that can wait for its confirming click.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ArmedAction {
    /// Moonbot's account-wide Cancel All Orders (`account().cancel_all_orders()`).
    CancelAll,
    /// The Markets Table's exchange-wide entry-order cancellation
    /// (`trade().cancel_all_exchange_buys()`, MoonBot 7.73 N1+).
    ExchangeEntries,
}

/// What the exchange-entries button offers for the popup's core.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ExchangeCancelOffer {
    /// Connected and proved 7.73 N1+: the button is live.
    Offered,
    /// Connected, but the core has not reported the N1 build — older, or its runtime report has
    /// not arrived yet. An older core ignores the request, so the button is disabled and says
    /// which version it needs instead of offering an action that would do nothing.
    NeedsNewerCore,
    /// Not connected, or the popup belongs to no core. Nothing can be sent.
    Offline,
}

impl ExchangeCancelOffer {
    /// Whether a click may arm or send the action.
    pub(crate) fn available(self) -> bool {
        self == Self::Offered
    }
}

/// Decide what the exchange-entries button offers.
///
/// Online is checked first: the store forgets the build on any departure from Ready, so an offline
/// core would otherwise read as "needs a newer core", which is not why it cannot be used.
///
/// Args:
///     online: Whether the popup's core is `Ready`, `CoreRunState::online`.
///     features: The core's reported optional commands, `CoreData::features`.
///
/// Returns:
///     The button's state.
pub(crate) fn exchange_cancel_offer(online: bool, features: CoreFeatures) -> ExchangeCancelOffer {
    if !online {
        ExchangeCancelOffer::Offline
    } else if features.exchange_buys_cancel {
        ExchangeCancelOffer::Offered
    } else {
        ExchangeCancelOffer::NeedsNewerCore
    }
}

/// What one click on an armable action does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Press {
    /// Arm this action; nothing is sent yet.
    Arm(ArmedAction),
    /// The confirming click: send the action, once, to this core.
    Send(ArmedAction, CoreId),
    /// Clear the armed slot and send nothing.
    Disarm,
}

/// Resolve one click on an armable action.
///
/// Args:
///     armed: The action currently waiting for its confirmation, if any.
///     pressed: The action whose button was clicked.
///     target: The core the popup may write to, `resolve_core_settings_write` at click time;
///         `None` once the active core moved away from the one the popup was opened over.
///     available: Whether `pressed` can act on that core right now.
///
/// Returns:
///     `Send` only for a second click on the same armed action with a target and the action still
///     available; `Arm` for a first click; `Disarm` for everything that must send nothing.
pub(crate) fn press(
    armed: Option<ArmedAction>,
    pressed: ArmedAction,
    target: Option<CoreId>,
    available: bool,
) -> Press {
    if !available {
        return Press::Disarm;
    }
    if armed != Some(pressed) {
        return Press::Arm(pressed);
    }
    match target {
        Some(core) => Press::Send(pressed, core),
        None => Press::Disarm,
    }
}

#[cfg(test)]
mod tests;
