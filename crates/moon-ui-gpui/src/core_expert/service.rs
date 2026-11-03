//! What the window's service-settings blocks share — the hang watchdog (`watchdog`) and the Remote
//! block's outgoing bot (`remote`): waiting on the core's verdict for a request this window queued,
//! and deciding what the banner above a block says about it.
//!
//! Both blocks talk to the core through moonproto's service API, whose every action is answered by
//! a `ServiceSettingsApplied` acknowledgement the session store tracks per block
//! (`CoreData::service`). A block remembers the track's registered-request count when it queues one,
//! and reads the verdict once that count moved with nothing pending — see
//! `ServiceTrack::settled_since`.

use moon_core::feed::{ServiceAction, ServiceTrack, ServiceVerdict};

/// A request a block queued and is waiting on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Awaiting {
    pub(super) action: ServiceAction,
    /// The track's registered-request count when the request was queued; see
    /// `ServiceTrack::settled_since`.
    pub(super) sent_before: u64,
    /// The saved observer row a Remove asked the core to delete. It leaves the edit only once the
    /// core confirms: a refused or lost Remove leaves the row on the core, and the table must show
    /// it. `None` for every other action.
    pub(super) removed: Option<String>,
}

/// What the banner above a block says about this window's last request, if anything.
pub(super) enum Verdict<'a> {
    /// Queued and not answered yet.
    Waiting,
    /// The core's (or the link's) answer.
    Settled(ServiceAction, &'a ServiceVerdict),
}

/// The verdict to draw, or `None` when this window sent the block nothing worth reporting — an
/// outcome already on the track describes someone else's request, or one from an earlier open.
///
/// Args:
///     sent_here: Whether this window queued anything for the block since its anchor last moved.
///     waiting: Whether that request is still unanswered.
///     track: The block's track in the store.
pub(super) fn verdict(sent_here: bool, waiting: bool, track: &ServiceTrack) -> Option<Verdict<'_>> {
    if !sent_here {
        return None;
    }
    if waiting {
        return Some(Verdict::Waiting);
    }
    track
        .outcome
        .as_ref()
        .map(|(action, verdict)| Verdict::Settled(*action, verdict))
}

/// Whether a verdict says the core holds what was asked — "applied but not saved" included: the
/// change is live in memory, and only its persistence failed.
pub(super) fn verdict_applied(verdict: &ServiceVerdict) -> bool {
    matches!(
        verdict,
        ServiceVerdict::Applied | ServiceVerdict::AppliedNotSaved(_)
    )
}

#[cfg(test)]
mod tests;
