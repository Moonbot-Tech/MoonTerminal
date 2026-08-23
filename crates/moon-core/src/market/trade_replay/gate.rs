//! Send permits for the public replay endpoints, so a click burst cannot get the user IP-limited.
//!
//! Every route this module guards is a PUBLIC, unauthenticated endpoint, and exchanges budget
//! those per source ADDRESS. The realistic failure is not one expensive request — it is a user
//! walking down a report and opening eight rows in five seconds, each fanning into several pages.
//!
//! # Why the key is the HOST and not the venue
//!
//! An IP budget belongs to the host that enforces it, and one host serves several venues:
//! `api.bybit.com` answers Bybit spot AND Bybit futures. Keying permits by venue would hand those
//! two independent budgets for one real one, which is exactly the burst the gate exists to
//! prevent. Binance is the mirror case — spot, USD-M and COIN-M live on three different hosts and
//! genuinely deserve three budgets — and a host key gets both right without a special case.
//!
//! # Two layers, and both are needed
//!
//! [`ReplayGate::pace`] is the NORMAL path: a floor between consecutive requests to one host,
//! which only works because a single worker thread makes every call, so the floor is process-wide
//! rather than per-caller wishful thinking. [`ReplayGate::claim`] is the FAILURE path: a
//! claim-before-send permit with the same backoff shape the native backfill gate already uses, so
//! this codebase carries ONE escalation curve rather than a second opinion about retry timing.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Floor between two consecutive requests to one host.
///
/// The same value the valuation provider paces its public calls with; a shared number is worth
/// more here than a per-venue table nobody can keep current with the vendors.
const MIN_INTERVAL: Duration = Duration::from_millis(100);

/// Shortest backoff after a host refuses, in seconds.
const RETRY_MIN_S: u32 = 30;

/// Longest backoff, in seconds.
const RETRY_MAX_S: u32 = 600;

/// How many refusals it takes before a host's backoff sits at its ceiling.
///
/// Deliberately NOT a budget that runs out. The native backfill gate this is shaped after stops
/// asking after five attempts, but it is re-armed whenever a core reconnects; nothing here has an
/// equivalent event, so an exhausted counter would lock the host out for the entire session with
/// no way back — `clear` is only reachable through a request that can no longer be made. A
/// plateau at [`RETRY_MAX_S`] is the honest version of the same restraint: the user waits ten
/// minutes, not forever, and the number the window shows is one that actually elapses.
const MAX_ATTEMPTS: u32 = 5;

/// One host's refusal history.
#[derive(Clone, Copy, Debug)]
struct Attempt {
    /// When the permit was last taken.
    last: Instant,
    /// Delay this attempt imposed before the next one is due.
    delay_s: u32,
    /// How many permits have been taken since the last success.
    attempts: u32,
}

/// Next backoff after `previous`, doubling from the floor and saturating at the cap.
///
/// Args:
///     previous: The delay the last attempt imposed, or `None` for a first attempt.
///
/// Returns:
///     Delay in seconds.
const fn next_delay_s(previous: Option<u32>) -> u32 {
    match previous {
        None => RETRY_MIN_S,
        Some(prev) => match prev.saturating_mul(2) {
            n if n > RETRY_MAX_S => RETRY_MAX_S,
            n if n < RETRY_MIN_S => RETRY_MIN_S,
            n => n,
        },
    }
}

/// Whether a host may be asked again.
///
/// Args:
///     previous: The host's last attempt, or `None` when it has never been asked.
///     now: Current instant.
///
/// Returns:
///     `true` when a permit may be taken.
fn is_due(previous: Option<&Attempt>, now: Instant) -> bool {
    let Some(prev) = previous else {
        return true;
    };
    // Past the attempt ceiling the wait stops growing but never becomes infinite; see
    // [`MAX_ATTEMPTS`] for why a spent budget would be a session-long lockout.
    let delay_s = match prev.attempts >= MAX_ATTEMPTS {
        true => RETRY_MAX_S,
        false => prev.delay_s.max(RETRY_MIN_S),
    };
    now.duration_since(prev.last) >= Duration::from_secs(u64::from(delay_s))
}

/// Per-host pacing and backoff for the public replay endpoints.
#[derive(Default)]
pub struct ReplayGate {
    /// Last request start per host.
    paced: Mutex<HashMap<&'static str, Instant>>,
    /// Refusal history per host.
    claims: Mutex<HashMap<&'static str, Attempt>>,
}

impl ReplayGate {
    /// Build an empty gate.
    ///
    /// Returns:
    ///     A gate that has never paced or refused anything.
    pub fn new() -> Self {
        Self::default()
    }

    /// Sleep until this host may be called again.
    ///
    /// Called on the worker thread immediately before a request. It BLOCKS, which is correct here
    /// and only here: one worker owns every replay request, so the sleep serialises the process
    /// rather than stalling a caller who could have proceeded.
    ///
    /// Args:
    ///     host: Stable host key, from the route.
    pub fn pace(&self, host: &'static str) {
        let wait = {
            let mut paced = self
                .paced
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let wait = paced
                .get(host)
                .map(|last| MIN_INTERVAL.saturating_sub(last.elapsed()))
                .unwrap_or_default();
            paced.insert(host, Instant::now() + wait);
            wait
        };
        if !wait.is_zero() {
            std::thread::sleep(wait);
        }
    }

    /// Take the send permit for a host, or learn how long it is refused for.
    ///
    /// The permit is taken BEFORE the request goes out, exactly as the native backfill gate does:
    /// a permit taken afterwards would let a burst through while the first request was still in
    /// flight, which is the only moment that matters.
    ///
    /// Args:
    ///     host: Stable host key, from the route.
    ///     now: Current instant.
    ///
    /// Returns:
    ///     `Ok(())` when the request may be sent, or `Err(seconds)` with the remaining wait.
    pub fn claim(&self, host: &'static str, now: Instant) -> Result<(), u32> {
        let mut claims = self
            .claims
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let previous = claims.get(host).copied();
        if !is_due(previous.as_ref(), now) {
            let prev = previous.expect("not due implies a recorded attempt");
            let delay_s = match prev.attempts >= MAX_ATTEMPTS {
                true => RETRY_MAX_S,
                false => prev.delay_s.max(RETRY_MIN_S),
            };
            let waited = now.duration_since(prev.last).as_secs();
            // The number the window counts down IS the number this gate will honour: it is
            // derived from the same delay `is_due` just refused on, so the retry the user waits
            // out actually goes through.
            let remaining = u64::from(delay_s).saturating_sub(waited) as u32;
            return Err(remaining.max(1));
        }
        claims.insert(
            host,
            Attempt {
                last: now,
                delay_s: next_delay_s(previous.map(|p| p.delay_s)),
                attempts: previous.map_or(1, |p| p.attempts + 1),
            },
        );
        Ok(())
    }

    /// Forget a host's refusal history after it answered successfully.
    ///
    /// Without this a user who hit a limit once would carry an escalating backoff for the rest of
    /// the session even though the venue is answering again.
    ///
    /// Args:
    ///     host: Stable host key, from the route.
    pub fn clear(&self, host: &'static str) {
        self.claims
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(host);
    }
}
