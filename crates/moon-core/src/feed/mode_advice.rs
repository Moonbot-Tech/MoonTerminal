//! Whether a fleet-wide sibling comparison makes a "try another transport mode" suggestion honest.
//!
//! Per-core, `conn_verdict::diagnose` genuinely cannot tell a blocked route from a mode mismatch —
//! see that module's honesty rule. Fleet-wide, comparing a failing core's transport mode against
//! every OTHER core this terminal manages is a real signal: when every core that DID connect is on
//! a mode different from the one that did not, the mode is the most likely difference, and that is
//! exactly the reasoning a support engineer performs by hand when told "10 of 42 connected, all on
//! V1". Absence of that clean signal must say nothing more than the plain per-core reason already
//! says: [`suggest_alternate_mode`] returns `None` rather than guess between candidates.

use crate::config::TransportVersion;

/// One sibling core's resolved transport mode and whether its latest attempt is connected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SiblingOutcome {
    /// The sibling's own effective transport mode (see `config::seeded_transport`).
    pub mode: TransportVersion,
    /// Whether the sibling is currently `Ready`.
    pub connected: bool,
}

/// Suggest an alternate transport mode for a failing core, from what its siblings show.
///
/// The failing core itself must not appear in `siblings`. A candidate is only ever named when
/// every connected sibling agrees on it: a single connected sibling sharing the failing core's own
/// mode already disproves the mode as the cause, and connected siblings split across more than one
/// OTHER mode leave no single honest suggestion — both cases return `None` rather than guess,
/// matching `conn_verdict`'s "say less, not more" discipline.
///
/// Args:
///     failing_mode: The failing core's own effective transport mode.
///     siblings: Every OTHER core this terminal manages, with its resolved mode and whether it is
///         currently connected.
///
/// Returns:
///     The one alternate mode to suggest, or `None` when the fleet gives no clean signal.
pub fn suggest_alternate_mode(
    failing_mode: TransportVersion,
    siblings: impl IntoIterator<Item = SiblingOutcome>,
) -> Option<TransportVersion> {
    let mut candidate: Option<TransportVersion> = None;
    for sibling in siblings {
        if !sibling.connected {
            continue;
        }
        if sibling.mode == failing_mode {
            // A sibling on the SAME mode connected fine: mode alone does not explain this failure.
            return None;
        }
        match candidate {
            None => candidate = Some(sibling.mode),
            Some(mode) if mode != sibling.mode => {
                // Connected siblings disagree about which OTHER mode works.
                return None;
            }
            _ => {}
        }
    }
    candidate
}

#[cfg(test)]
mod tests;
