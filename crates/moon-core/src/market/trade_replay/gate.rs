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
//! kept under one lock per host across every thread that calls it (the host's lanes, see
//! `worker`): each caller books the instant its own request may go and waits for it, so the
//! floor is process-wide for the host rather than per-caller wishful thinking.
//!
//! [`ReplayGate::claim`] is the FAILURE path: a
//! check-before-send against the host's refusal history, which [`ReplayGate::refuse`] writes
//! when the venue actually refuses and [`ReplayGate::clear`] erases when it answers again — the
//! same backoff shape the native backfill gate already uses, so this codebase carries ONE
//! escalation curve rather than a second opinion about retry timing. The check records nothing:
//! a host's lanes run side by side (a chart window beside the tuner's batch), and a permit that
//! stood for the length of a walk would have refused the neighbour for a refusal the venue
//! never gave.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Floor between two consecutive requests to one host, when the route names no higher one.
///
/// The same value the valuation provider paces its public calls with; a shared number is worth
/// more here than a per-venue table nobody can keep current with the vendors. A route whose
/// documented weight makes this floor a refusal — Binance futures' aggTrades, see
/// `TradeRoute::page_interval` — hands its own floor to [`ReplayGate::pace_at`].
pub const MIN_INTERVAL: Duration = Duration::from_millis(100);

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
    /// When the venue last refused.
    last: Instant,
    /// Delay this refusal imposed before the next request is due.
    delay_s: u32,
    /// How many refusals in a row since the last success.
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

/// The delay an attempt imposes on the next one.
///
/// Past the attempt ceiling the wait stops growing but never becomes infinite; see
/// [`MAX_ATTEMPTS`] for why a spent budget would be a session-long lockout.
fn imposed_delay_s(prev: &Attempt) -> u32 {
    match prev.attempts >= MAX_ATTEMPTS {
        true => RETRY_MAX_S,
        false => prev.delay_s.max(RETRY_MIN_S),
    }
}

/// How long the host is still refused for, or `None` when it is due and a permit may be taken.
///
/// The number the window counts down IS the number the gate will honour: it is derived from the
/// same delay the refusal rests on, so the retry the user waits out actually goes through.
///
/// Args:
///     previous: The host's last attempt, or `None` when it has never been asked.
///     now: Current instant.
///
/// Returns:
///     Remaining seconds, at least one, or `None`.
fn refused_for(previous: Option<&Attempt>, now: Instant) -> Option<u32> {
    let prev = previous?;
    let delay_s = imposed_delay_s(prev);
    let waited = now.duration_since(prev.last);
    if waited >= Duration::from_secs(u64::from(delay_s)) {
        return None;
    }
    let remaining = u64::from(delay_s).saturating_sub(waited.as_secs()) as u32;
    Some(remaining.max(1))
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
    /// Called on a lane thread immediately before a request. It BLOCKS, which is correct here
    /// and only here: the caller has booked the next slot of its host under the lock, so the
    /// sleep serialises that host's calls across its lanes rather than stalling a caller who
    /// could have proceeded.
    ///
    /// Args:
    ///     host: Stable host key, from the route.
    pub fn pace(&self, host: &'static str) {
        self.pace_at(host, MIN_INTERVAL);
    }

    /// [`Self::pace`] with the caller's own floor — a route whose pages weigh more than the
    /// default floor allows (see `TradeRoute::page_interval`). The floor is per CALL, not per
    /// host: the same host's candle pages keep the default, and only the heavy pages wait.
    ///
    /// Args:
    ///     host: Stable host key, from the route.
    ///     floor: Least time since the host's previous request.
    ///
    /// Returns:
    ///     The slot this call booked — the instant it returns at, give or take the wake-up.
    pub fn pace_at(&self, host: &'static str, floor: Duration) -> Instant {
        let (slot, wait) = {
            let mut paced = self
                .paced
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let now = Instant::now();
            // Booked against the host's LAST BOOKED slot, which may still lie in the future
            // when another lane of the host is waiting for it — `elapsed` on a future instant
            // reads zero and would let this caller book a slot before that one's plus the
            // floor.
            let wait = paced
                .get(host)
                .map(|last| (*last + floor).saturating_duration_since(now))
                .unwrap_or_default();
            let slot = now + wait;
            paced.insert(host, slot);
            (slot, wait)
        };
        if !wait.is_zero() {
            std::thread::sleep(wait);
        }
        slot
    }

    /// Whether a host may be sent to, or how long it is still refused for.
    ///
    /// Checked BEFORE the request goes out, exactly as the native backfill gate does; it records
    /// nothing — the record is the venue's refusal ([`Self::refuse`]), not our asking.
    ///
    /// Args:
    ///     host: Stable host key, from the route.
    ///     now: Current instant.
    ///
    /// Returns:
    ///     `Ok(())` when the request may be sent, or `Err(seconds)` with the remaining wait.
    pub fn claim(&self, host: &'static str, now: Instant) -> Result<(), u32> {
        match self.refused_for(host, now) {
            Some(remaining) => Err(remaining),
            None => Ok(()),
        }
    }

    /// Record that the venue refused a request to this host — a 429, a transport or service
    /// failure — and escalate its backoff: the floor the first time, doubling on each refusal in
    /// a row, saturating at the cap ([`next_delay_s`]).
    ///
    /// Args:
    ///     host: Stable host key, from the route.
    ///     now: Current instant.
    pub fn refuse(&self, host: &'static str, now: Instant) {
        let mut claims = self
            .claims
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let previous = claims.get(host).copied();
        claims.insert(
            host,
            Attempt {
                last: now,
                delay_s: next_delay_s(previous.map(|p| p.delay_s)),
                attempts: previous.map_or(1, |p| p.attempts + 1),
            },
        );
    }

    /// How long a host is still refused for, without taking or recording anything.
    ///
    /// The number a requester waits out before asking again — the same one [`Self::claim`]
    /// would refuse with right now — read after a walk stopped on the gate, where a second
    /// `claim` would take the permit the moment the wait ended.
    ///
    /// Args:
    ///     host: Stable host key, from the route.
    ///     now: Current instant.
    ///
    /// Returns:
    ///     Remaining seconds, or `None` when a permit may be taken now.
    pub fn refused_for(&self, host: &'static str, now: Instant) -> Option<u32> {
        let claims = self
            .claims
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        refused_for(claims.get(host), now)
    }

    /// Forget a host's refusal history after it answered a request sent at `asked_at`, AFTER
    /// the refusal.
    ///
    /// Without this a user who hit a limit once would carry an escalating backoff for the rest of
    /// the session even though the venue is answering again. Only an ANSWER clears: our own
    /// stops — a cancel, a deadline, a budget — say nothing about the venue. And only an answer
    /// to a request sent AFTER the refusal: a neighbour lane's request that was already in
    /// flight when the venue refused proves nothing about the refusal, and clearing on it would
    /// reset the escalation to the floor and hit a limited host every thirty seconds instead of
    /// backing off toward the cap.
    ///
    /// Args:
    ///     host: Stable host key, from the route.
    ///     asked_at: When the answered request was sent.
    pub fn clear(&self, host: &'static str, asked_at: Instant) {
        let mut claims = self
            .claims
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if claims.get(host).is_some_and(|prev| prev.last < asked_at) {
            claims.remove(host);
        }
    }
}

#[cfg(test)]
mod tests;
