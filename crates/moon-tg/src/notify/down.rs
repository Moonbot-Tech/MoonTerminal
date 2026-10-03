//! Core down and back notices, debounced and announced once per outage.

use std::collections::{BTreeMap, BTreeSet};

use moon_core::feed::ConnStatus;
use moon_core::telegram::notify::{DownRule, NotifyLedger};

/// Link state the down machine understands.
///
/// `ConnStatus::Ready` is up. Every other variant is a lost link. The enum has
/// no inactive or unconfigured variant, so [`link_of`] does not return `None`
/// for any status that exists today.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Link {
    /// The core is connected.
    Up,
    /// The core is not connected.
    Lost,
}

/// Map a feed connection status onto [`Link`].
///
/// Args:
///     status: Current `ConnStatus` for one core.
///
/// Returns:
///     `Some(Up)` for `Ready`. `Some(Lost)` for `Connecting`, `Stage`,
///     `Failed`, and `Disconnected`. `None` is reserved for an inactive status
///     this enum does not have.
pub(crate) fn link_of(status: &ConnStatus) -> Option<Link> {
    match status {
        ConnStatus::Ready => Some(Link::Up),
        ConnStatus::Connecting
        | ConnStatus::Stage(_)
        | ConnStatus::Failed(_)
        | ConnStatus::Disconnected => Some(Link::Lost),
    }
}

/// One notice emitted by [`DownTracker::step`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DownEvent {
    /// The core has been lost for at least the configured delay.
    Down {
        /// Core id.
        core: u64,
        /// UTC Unix seconds when this outage was first observed.
        since_utc: i64,
    },
    /// The core is back after a down notice.
    Back {
        /// Core id.
        core: u64,
        /// Seconds from the recorded loss to now, or 0 when the loss time was not kept.
        down_for_secs: i64,
    },
}

/// In-memory loss timers. Not written to the ledger.
///
/// After a restart this map is empty. A core that is still lost starts its
/// timer at the restart observation, and a core already in `down_announced`
/// does not emit a second down notice.
#[derive(Clone, Debug, Default)]
pub(crate) struct DownTracker {
    lost_since: BTreeMap<u64, i64>,
}

impl DownTracker {
    /// Advance the machine for one observation.
    ///
    /// Args:
    ///     rule: Down/back rule. `after_minutes` is the debounce.
    ///     ledger: Cores whose down decision was recorded; the caller saves it with the outbox.
    ///     links: Core id and link state observed now. A core absent from this
    ///         slice loses its timer and its `down_announced` entry. No back
    ///         notice is sent for it.
    ///     now_utc: Current UTC Unix seconds.
    ///
    /// Returns:
    ///     Down and back notices in `links` order. Empty when the rule is off;
    ///     that path also clears `down_announced` and the in-memory timers.
    pub(crate) fn step(
        &mut self,
        rule: &DownRule,
        ledger: &mut NotifyLedger,
        links: &[(u64, Link)],
        now_utc: i64,
    ) -> Vec<DownEvent> {
        if !rule.on {
            self.lost_since.clear();
            ledger.down_announced.clear();
            return Vec::new();
        }
        self.forget_absent(ledger, links);
        let delay_secs = i64::from(rule.after_minutes) * 60;
        let mut events = Vec::new();
        for &(core, link) in links {
            match link {
                Link::Lost => self.on_lost(ledger, &mut events, core, now_utc, delay_secs),
                Link::Up => self.on_up(ledger, &mut events, core, now_utc),
            }
        }
        events
    }

    /// Drop timers and announced bits for cores missing from this observation.
    ///
    /// No [`DownEvent::Back`] is emitted. A core that later reappears starts a
    /// new timer instead of continuing the outage it left with.
    ///
    /// Args:
    ///     ledger: Cores whose down decision was recorded; no delivery ack is read here.
    ///     links: Cores observed now. Any other core id is forgotten.
    fn forget_absent(&mut self, ledger: &mut NotifyLedger, links: &[(u64, Link)]) {
        let present: BTreeSet<u64> = links.iter().map(|(core, _)| *core).collect();
        self.lost_since.retain(|core, _| present.contains(core));
        ledger.down_announced.retain(|core| present.contains(core));
    }

    /// Record the first loss time and emit `Down` once the delay has elapsed.
    fn on_lost(
        &mut self,
        ledger: &mut NotifyLedger,
        events: &mut Vec<DownEvent>,
        core: u64,
        now_utc: i64,
        delay_secs: i64,
    ) {
        let since = *self.lost_since.entry(core).or_insert(now_utc);
        let elapsed = now_utc.saturating_sub(since);
        if elapsed >= delay_secs && ledger.down_announced.insert(core) {
            events.push(DownEvent::Down {
                core,
                since_utc: since,
            });
        }
    }

    /// Clear the timer and emit `Back` only after a down notice.
    fn on_up(
        &mut self,
        ledger: &mut NotifyLedger,
        events: &mut Vec<DownEvent>,
        core: u64,
        now_utc: i64,
    ) {
        let since = self.lost_since.remove(&core);
        if ledger.down_announced.remove(&core) {
            let down_for_secs = since.map_or(0, |since| now_utc.saturating_sub(since));
            events.push(DownEvent::Back {
                core,
                down_for_secs,
            });
        }
    }
}

#[cfg(test)]
mod tests;
