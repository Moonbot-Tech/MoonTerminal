//! Which optional MoonBot commands the connected core build acts on.
//!
//! MoonProto connects to older cores with the same protocol and lets them silently ignore a request
//! they do not know, so a control for a newer command has to be gated by the terminal itself. The
//! version number alone cannot do it: 7.73 N1 and 7.73 N2 both report `server_version = 773`, and
//! the build letter is a string whose ordering MoonBot never defined. What does tell the builds
//! apart is what each one REPORTS: every build appends fields to `TRuntimeStateCommand`, and
//! MoonProto documents which build first sends which tail (`moonproto::core_compatibility`). That
//! report is therefore the signal used here, not a parsed version string.

/// Optional core commands this terminal offers only where the core acts on them.
///
/// Unknown is unsupported: [`Default`] — every flag `false` — is what a core that has not yet sent
/// its runtime state, or has just disconnected, is treated as. A control gated on a flag stays
/// hidden or disabled until the core proves the build, rather than sending a request an older core
/// would drop without a word.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CoreFeatures {
    /// MoonBot 7.73 N1+: `cancel_all_exchange_buys`, the exchange-wide entry-order cancellation.
    /// Gates that command twice (#1290): the core-settings popup offers its button only on such a
    /// core, and `SessionManager::cancel_exchange_entries` refuses to queue it anywhere else.
    ///
    /// Driven by `RuntimeStateCommand::moon_stream_enabled`, the Moon Streamer switch N1 added to
    /// the same report; MoonProto documents it as `None` on every core before N1. An N2 tail
    /// implies it too, since N2 extends N1's report.
    pub exchange_buys_cancel: bool,
    /// MoonBot 7.73 N2+: `orders().force_cancel`, the forced cancellation of a stuck order.
    ///
    /// Driven by `RuntimeStateCommand::moon_streamer`, the extended Moon Streamer state N2 appends
    /// to the runtime report; MoonProto documents it as omitted by cores before N2.
    pub force_cancel: bool,
}

/// Whether the order menu offers a forced cancellation for one order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ForceCancelGate {
    /// The order has no active BUY or SELL leg on the exchange — a pending entry, a finished order
    /// or a fill in between. MoonProto sends nothing for these, so the menu shows nothing; a
    /// pending entry is cancelled the ordinary way.
    NotEligible,
    /// The order is eligible, but the core has not proved it is 7.73 N2+. An older core ignores
    /// the request, so the row is shown disabled with that reason instead of an action that
    /// would do nothing.
    NeedsNewerCore,
    /// Eligible on a core that acts on it.
    Offered,
}

impl CoreFeatures {
    /// Derive the flags from the two runtime-report tails that identify the build.
    ///
    /// Takes the two fields themselves, not two booleans, so the call site cannot swap them: the
    /// switch is an `Option<bool>` and the streamer state any other type, and passing them the
    /// wrong way round does not compile.
    ///
    /// Args:
    ///     moon_stream_enabled: `RuntimeStateCommand::moon_stream_enabled`, `Some` on 7.73 N1+.
    ///     moon_streamer: `RuntimeStateCommand::moon_streamer`, `Some` on 7.73 N2+.
    ///
    /// Returns:
    ///     The commands the build acts on. N2's tail alone also proves N1.
    pub fn from_runtime_report<Streamer>(
        moon_stream_enabled: Option<bool>,
        moon_streamer: Option<Streamer>,
    ) -> Self {
        let n2 = moon_streamer.is_some();
        Self {
            exchange_buys_cancel: moon_stream_enabled.is_some() || n2,
            force_cancel: n2,
        }
    }

    /// Decide whether a forced cancellation is offered for one order of this core.
    ///
    /// Only an order whose worker sits in `BuySet` or `SellSet` has a leg MoonProto can force:
    /// the runtime picks that leg itself and sends nothing for any other status. An order the core
    /// already reports finished (`job_is_done`) is not offered either, whatever its status says.
    ///
    /// Args:
    ///     status: The order's worker status name, `OrderRow::status`.
    ///     job_is_done: The core's terminal flag, `OrderRow::job_is_done`.
    ///
    /// Returns:
    ///     Whether the menu hides the row, shows it disabled, or offers it.
    pub fn force_cancel_gate(self, status: &str, job_is_done: bool) -> ForceCancelGate {
        let active_leg = status == moonproto::OrderWorkerStatus::BuySet.name()
            || status == moonproto::OrderWorkerStatus::SellSet.name();
        if !active_leg || job_is_done {
            ForceCancelGate::NotEligible
        } else if self.force_cancel {
            ForceCancelGate::Offered
        } else {
            ForceCancelGate::NeedsNewerCore
        }
    }
}

#[cfg(test)]
mod tests;
