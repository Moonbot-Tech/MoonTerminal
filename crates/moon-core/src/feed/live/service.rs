//! What every service-settings action shares, whichever block it belongs to — the observers
//! (`feed::live::watchdog`) or the outgoing Telegram bot (`feed::live::telegram`): the
//! acknowledgement the core answers each one with, and the report that one was dropped unsent.

use moonproto::state::{ServiceSettingsResult, ServiceSettingsState};

use crate::feed::{CoreCmd, CoreServiceResult, FeedMsg, ServiceDomain};

/// The `ServiceRequest` reporting that a tracked service action was dropped unsent — the stale
/// gate's word for a command that waited out a lost link.
///
/// Args:
///     cmd: The dropped command.
///     reason: Why it was dropped, as the log states it.
///
/// Returns:
///     The message, or `None` for any command nobody waits on.
pub(super) fn dropped(cmd: &CoreCmd, reason: &str) -> Option<FeedMsg> {
    let action = match cmd {
        CoreCmd::Watchdog(cmd) => cmd.tracked_action(),
        CoreCmd::Telegram(cmd) => cmd.tracked_action(),
        _ => None,
    }?;
    Some(FeedMsg::ServiceRequest {
        action,
        sent: Err(format!("dropped: {reason}")),
    })
}

/// Project one acknowledgement, for any service block.
///
/// The request id travels as a decimal string (moonproto compares it with the id's string form);
/// one this build cannot read answers nothing this terminal waits on.
pub(super) fn result_from_proto(result: &ServiceSettingsResult) -> CoreServiceResult {
    CoreServiceResult {
        domain: match result.state {
            ServiceSettingsState::Watchdog(_) => ServiceDomain::Watchdog,
            ServiceSettingsState::Telegram(_) => ServiceDomain::Telegram,
        },
        request_id: result.request_id.trim().parse().ok(),
        applied: result.applied,
        error: result.error.clone(),
    }
}

#[cfg(test)]
mod tests;
