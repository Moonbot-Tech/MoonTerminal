//! Debounced respawns for cores whose published exchange identity went stale (#734).
//!
//! A feed reports `FeedMsg::IdentityStale` when its client can no longer vouch for the venue it
//! published (see `feed::live::identity_refresh`). The only way to read the new one is a fresh
//! MoonProto client, i.e. the same respawn the Settings Reconnect button issues through
//! [`SessionManager::reconnect`], which also clears the core's venue and provider role. That
//! call needs the `AppConfig` this manager does not own, so requests leave as data, exactly like
//! the core-update queue's `Verifying` respawns.
//!
//! The gate is what keeps a flapping core from respawn-looping: at most one respawn per core per
//! [`MIN_RESPAWN_GAP`]. A request inside that window is deferred, never dropped, so the last
//! restart of a burst is still followed by one.

use std::collections::{BTreeSet, HashMap};
use std::time::{Duration, Instant};

use super::{CoreId, SessionManager};

/// Shortest interval between two identity respawns of one core.
pub(crate) const MIN_RESPAWN_GAP: Duration = Duration::from_secs(60);

/// Pending identity-respawn requests and when each core was last respawned for one.
#[derive(Debug, Default)]
pub(crate) struct IdentityRespawnGate {
    /// Cores with a request not yet handed out, in id order for a deterministic drain.
    pending: BTreeSet<CoreId>,
    /// When each core's last identity respawn was handed out.
    last: HashMap<CoreId, Instant>,
}

impl IdentityRespawnGate {
    /// Record that `core`'s identity is stale. Repeats before the drain collapse into one.
    pub(crate) fn request(&mut self, core: CoreId) {
        self.pending.insert(core);
    }

    /// Hand out every pending core whose last identity respawn is at least [`MIN_RESPAWN_GAP`]
    /// old, and stamp it with `now`. The rest stay pending for a later call.
    pub(crate) fn take_due(&mut self, now: Instant) -> Vec<CoreId> {
        let due: Vec<CoreId> = self
            .pending
            .iter()
            .copied()
            .filter(|core| {
                self.last
                    .get(core)
                    .is_none_or(|at| now.saturating_duration_since(*at) >= MIN_RESPAWN_GAP)
            })
            .collect();
        for core in &due {
            self.pending.remove(core);
            self.last.insert(*core, now);
        }
        due
    }

    /// Drop `core`'s pending request once some respawn served it, whoever asked for that one.
    pub(crate) fn fulfilled(&mut self, core: CoreId) {
        self.pending.remove(&core);
    }

    /// Forget a core that left the session, so a later id reuse starts clean.
    pub(crate) fn forget(&mut self, core: CoreId) {
        self.pending.remove(&core);
        self.last.remove(&core);
    }
}

impl SessionManager {
    /// Drain the cores whose exchange identity went stale and are due a respawn at `now`.
    ///
    /// The coordinator hands each to [`SessionManager::reconnect`] through the same
    /// `reconnect_request` drain the Reconnect button and the core-update queue use. A core
    /// still inside its [`MIN_RESPAWN_GAP`] stays queued and is returned by a later call.
    pub fn take_identity_respawn_requests(&mut self, now: Instant) -> Vec<CoreId> {
        self.identity_respawns.take_due(now)
    }
}
