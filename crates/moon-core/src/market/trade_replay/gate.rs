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
//! check-before-send against the host's refusal history. [`ReplayGate::refuse`],
//! [`ReplayGate::honour_retry_after`] and [`ReplayGate::note_used_weight`] write that history,
//! and each keeps the later deadline when a wait is already standing — a later event may extend
//! a host's wait and must not shorten it. [`ReplayGate::clear`] forgets a curve once the venue
//! answers a request sent after that refusal; a weight stop or a `Retry-After` stands until its
//! own deadline. The backoff shape matches the native backfill gate, so this codebase carries
//! ONE escalation curve rather than a second opinion about retry timing. The check records
//! nothing:
//! a host's lanes run side by side (a chart window beside the tuner's batch), and a permit that
//! stood for the length of a walk would have refused the neighbour for a refusal the venue
//! never gave.

use std::collections::{HashMap, VecDeque};
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

/// Share of a host's documented IP weight the planned pace may spend, in percent.
///
/// Binance futures publish 2 400 weight per minute; 75% of that is 1 800. The header stop
/// uses the same share against the weight the venue reports for the whole IP.
const WEIGHT_SHARE_PERCENT: u64 = 75;

/// How long a booked weight counts against the host's per-minute budget.
const WEIGHT_WINDOW: Duration = Duration::from_secs(60);

/// Documented `REQUEST_WEIGHT` per minute for the Binance hosts this replay calls.
///
/// Read from each host's `exchangeInfo` on 2026-09-26: `fapi` and `dapi` both publish 2 400,
/// and `data-api.binance.vision` publishes 6 000. Other venues are not weight-budgeted here.
///
/// Args:
///     host: Stable host key, from the route.
///
/// Returns:
///     The host's IP weight limit per minute, or `None` when this gate does not budget it.
pub(crate) fn binance_weight_limit(host: &str) -> Option<u32> {
    match host {
        "fapi.binance.com" | "dapi.binance.com" => Some(2_400),
        "data-api.binance.vision" => Some(6_000),
        _ => None,
    }
}

/// The planned pace's ceiling for a host: [`WEIGHT_SHARE_PERCENT`] of its documented limit.
///
/// Args:
///     limit: The host's documented weight per minute.
///
/// Returns:
///     Weight this process may book in any 60 s window.
pub(crate) fn weight_budget(limit: u32) -> u32 {
    (u64::from(limit) * WEIGHT_SHARE_PERCENT / 100) as u32
}

/// Whether `used` is strictly over [`WEIGHT_SHARE_PERCENT`] of `limit`.
///
/// Exactly 75% does not stop: 1 800 of 2 400 is the ceiling the pace is allowed to sit on.
///
/// Args:
///     used: `X-MBX-USED-WEIGHT-1M`, the IP's total for the minute.
///     limit: The host's documented weight per minute.
///
/// Returns:
///     `true` when the host must not be sent to until the next UTC minute.
pub(crate) fn weight_over_share(used: u32, limit: u32) -> bool {
    u64::from(used) * 100 > u64::from(limit) * WEIGHT_SHARE_PERCENT
}

/// Parse a Binance integer header (`X-MBX-USED-WEIGHT-1M`, `Retry-After` in seconds).
///
/// Missing, empty, and non-integer text — including an HTTP-date `Retry-After` — are `None`,
/// so the caller leaves the host alone or falls back to the backoff curve.
///
/// Args:
///     raw: The header value, or `None` when the response did not send it.
///
/// Returns:
///     The integer, or `None`.
pub(crate) fn parse_u32_header(raw: Option<&str>) -> Option<u32> {
    let text = raw?.trim();
    if text.is_empty() {
        return None;
    }
    text.parse().ok()
}

/// Seconds from `unix_secs` until the next UTC minute boundary.
///
/// Binance's weight window resets on that boundary. A timestamp already on the boundary
/// waits out the whole new minute: the header just read belongs to the minute that started.
///
/// Args:
///     unix_secs: Seconds since the Unix epoch, UTC.
///
/// Returns:
///     A delay from 1 to 60 seconds.
pub(crate) fn secs_until_next_utc_minute(unix_secs: u64) -> u32 {
    let into = (unix_secs % 60) as u32;
    match into {
        0 => 60,
        spent => 60 - spent,
    }
}

/// How many refusals it takes before a host's backoff sits at its ceiling.
///
/// Deliberately NOT a budget that runs out. The native backfill gate this is shaped after stops
/// asking after five attempts, but it is re-armed whenever a core reconnects; nothing here has an
/// equivalent event, so an exhausted counter would lock the host out for the entire session with
/// no way back — `clear` is only reachable through a request that can no longer be made. A
/// plateau at [`RETRY_MAX_S`] is the honest version of the same restraint: the user waits ten
/// minutes, not forever, and the number the window shows is one that actually elapses.
const MAX_ATTEMPTS: u32 = 5;

/// Why a host is waiting. The curve clamps; the other two honour the number they were given.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum WaitKind {
    /// Doubling backoff, clamped to [`RETRY_MIN_S`]..=[`RETRY_MAX_S`].
    Curve,
    /// A venue `Retry-After` in seconds. Not clamped, including a multi-hour 418 ban.
    RetryAfter,
    /// `X-MBX-USED-WEIGHT-1M` over 75%. `delay_s` runs to the next UTC minute.
    Weight,
}

/// A `Retry-After` or weight stop hidden under a longer curve.
///
/// The curve is the deadline a caller waits out. [`ReplayGate::clear`] may end that curve
/// and must still wait this stop out.
#[derive(Clone, Copy, Debug)]
struct CoveredStop {
    /// When this stop ends.
    until: Instant,
    /// [`WaitKind::RetryAfter`] or [`WaitKind::Weight`].
    kind: WaitKind,
}

/// One host's refusal history.
#[derive(Clone, Copy, Debug)]
struct Attempt {
    /// When the venue last refused, or when a weight stop was recorded.
    last: Instant,
    /// Delay this refusal imposed before the next request is due.
    delay_s: u32,
    /// How many refusals in a row since the last success. Weight stops do not increment it.
    attempts: u32,
    /// Which rule [`imposed_delay_s`] applies.
    kind: WaitKind,
    /// A shorter ban or weight stop covered by a longer curve, if there is one.
    covered: Option<CoveredStop>,
}

/// One host's booked send slots and the weights those sends spend.
struct HostPace {
    /// Last booked slot, which may still lie in the future.
    last: Instant,
    /// `(slot, weight)` oldest first. Empty for a host this gate does not budget.
    weights: VecDeque<(Instant, u32)>,
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
    match prev.kind {
        // A ban and a weight stop name their own wait. The curve's 600 s ceiling must not
        // shorten a 418, and its 30 s floor must not lengthen a short Retry-After.
        WaitKind::RetryAfter | WaitKind::Weight => prev.delay_s,
        WaitKind::Curve => match prev.attempts >= MAX_ATTEMPTS {
            true => RETRY_MAX_S,
            false => prev.delay_s.max(RETRY_MIN_S),
        },
    }
}

/// Earliest slot at or after `now` that keeps `floor` behind `last` and, when `weight` spends
/// a budget, keeps every 60 s window of booked weight at or under `budget`.
///
/// Bookings are monotonic: the caller holds the host's lock, so every entry already in
/// `weights` is at or before `last`. A request heavier than the whole budget is booked anyway
/// — one call cannot be split — which does not arise for the weights this crate sends.
///
/// Args:
///     last: The host's previous booked slot, or `None` when it has never been paced.
///     weights: Booked `(slot, weight)` pairs, oldest first. Updated with the new slot when
///         `weight` is non-zero.
///     now: Current instant.
///     floor: Least time since the previous slot.
///     weight: Weight this request spends. Zero skips the budget.
///     budget: Weight allowed in any [`WEIGHT_WINDOW`]. Ignored when `weight` is zero.
///
/// Returns:
///     The slot this request may start at.
fn book_slot(
    last: Option<Instant>,
    weights: &mut VecDeque<(Instant, u32)>,
    now: Instant,
    floor: Duration,
    weight: u32,
    budget: u32,
) -> Instant {
    let mut slot = match last {
        Some(last) => now.max(last + floor),
        None => now,
    };
    if weight == 0 || budget == 0 {
        return slot;
    }
    loop {
        let window_start = slot.checked_sub(WEIGHT_WINDOW).unwrap_or(slot);
        while weights.front().is_some_and(|(at, _)| *at <= window_start) {
            weights.pop_front();
        }
        let used: u32 = weights.iter().map(|(_, booked)| *booked).sum();
        if used.saturating_add(weight) <= budget || weights.is_empty() {
            weights.push_back((slot, weight));
            return slot;
        }
        let Some(&(oldest, _)) = weights.front() else {
            weights.push_back((slot, weight));
            return slot;
        };
        let opens_at = oldest + WEIGHT_WINDOW;
        if opens_at <= slot {
            weights.pop_front();
            continue;
        }
        slot = opens_at;
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

/// The instant before which the host must not be sent to.
///
/// Args:
///     attempt: The host's current wait.
///
/// Returns:
///     `last` plus the delay [`imposed_delay_s`] will honour.
fn not_before(attempt: &Attempt) -> Instant {
    attempt.last + Duration::from_secs(u64::from(imposed_delay_s(attempt)))
}

/// The mandatory stop `attempt` itself is, or the one a curve already covers.
///
/// Args:
///     attempt: A wait being merged.
///
/// Returns:
///     That stop, or `None` when `attempt` is a curve with nothing under it.
fn mandatory_cover(attempt: &Attempt) -> Option<CoveredStop> {
    if matches!(attempt.kind, WaitKind::RetryAfter | WaitKind::Weight) {
        Some(CoveredStop {
            until: not_before(attempt),
            kind: attempt.kind,
        })
    } else {
        attempt.covered
    }
}

/// The later of two covered stops.
///
/// Args:
///     left: One stop, if any.
///     right: The other stop, if any.
///
/// Returns:
///     Whichever ends later. An equal pair keeps `left`.
fn later_cover(left: Option<CoveredStop>, right: Option<CoveredStop>) -> Option<CoveredStop> {
    match (left, right) {
        (Some(a), Some(b)) => Some(if b.until > a.until { b } else { a }),
        (Some(a), None) => Some(a),
        (None, other) => other,
    }
}

/// A curve's covered stop, or nothing when the stored kind is already mandatory.
///
/// A winning ban or weight stop is itself the mandatory deadline, so a shorter stop under
/// it does not need a second record.
///
/// Args:
///     kind: The kind about to be stored.
///     covered: The later mandatory stop seen while merging.
///
/// Returns:
///     `covered` for a curve, otherwise `None`.
fn cover_under(kind: WaitKind, covered: Option<CoveredStop>) -> Option<CoveredStop> {
    match kind {
        WaitKind::Curve => covered,
        WaitKind::RetryAfter | WaitKind::Weight => None,
    }
}

/// The later of the standing wait and `proposed`.
///
/// The not-before instant never moves earlier. When `proposed` does not win and
/// `count_attempt` is set, the standing wait keeps its deadline and kind but takes
/// `proposed.attempts`, so a curve step still counts under a longer ban or weight stop.
/// A ban or weight stop that loses to a longer curve is kept as [`Attempt::covered`], so
/// [`ReplayGate::clear`] can end the curve without opening the host early.
///
/// Args:
///     standing: The host's current wait, if any.
///     proposed: The wait this writer wants to record.
///     count_attempt: Whether a losing write still advances the attempt counter.
///
/// Returns:
///     The attempt to store.
fn merge_wait(standing: Option<Attempt>, proposed: Attempt, count_attempt: bool) -> Attempt {
    let Some(prev) = standing else {
        return proposed;
    };
    let covered = later_cover(mandatory_cover(&prev), mandatory_cover(&proposed));
    if not_before(&prev) < not_before(&proposed) {
        let mut won = proposed;
        won.covered = cover_under(won.kind, covered);
        return won;
    }
    Attempt {
        last: prev.last,
        delay_s: prev.delay_s,
        attempts: if count_attempt {
            proposed.attempts
        } else {
            prev.attempts
        },
        kind: prev.kind,
        covered: cover_under(prev.kind, covered),
    }
}

/// The wait left after a success ends a curve that still covers a ban or a weight stop.
///
/// The delay rounds up to the next second so the stored deadline does not land before
/// `stop.until`.
///
/// Args:
///     stop: The covered stop that has not elapsed.
///     now: Current instant.
///     attempts: The curve's attempt count, carried onto the stop.
///
/// Returns:
///     An attempt of `stop.kind` that lasts until `stop.until`.
fn covered_remainder(stop: CoveredStop, now: Instant, attempts: u32) -> Attempt {
    let left = stop.until.saturating_duration_since(now);
    let mut secs = left.as_secs();
    if left.subsec_nanos() > 0 {
        secs = secs.saturating_add(1);
    }
    Attempt {
        last: now,
        delay_s: u32::try_from(secs).unwrap_or(u32::MAX).max(1),
        attempts,
        kind: stop.kind,
        covered: None,
    }
}

/// Per-host pacing and backoff for the public replay endpoints.
#[derive(Default)]
pub struct ReplayGate {
    /// Booked slots and, for a budgeted host, the weights those slots spend.
    paced: Mutex<HashMap<&'static str, HostPace>>,
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
    /// default floor allows (see `TradeRoute::page_interval`). The time floor is per CALL:
    /// candle pages keep the default gap, and only the heavy pages wait that long. The weight
    /// ledger in [`Self::pace_weighted`] is per host, so both still spend one Binance budget.
    ///
    /// Args:
    ///     host: Stable host key, from the route.
    ///     floor: Least time since the host's previous request.
    ///
    /// Returns:
    ///     The slot this call booked — the instant it returns at, give or take the wake-up.
    pub fn pace_at(&self, host: &'static str, floor: Duration) -> Instant {
        self.pace_weighted(host, floor, 0)
    }

    /// [`Self::pace_at`] that also spends `weight` against the host's 75% budget.
    ///
    /// A host with no [`binance_weight_limit`], or a request of weight zero, keeps the time
    /// floor only. Binance futures and spot share one ledger per host, so an aggTrades page
    /// and a kline page on `fapi.binance.com` draw from the same 1 800 weight per minute.
    /// The sleep is the gap until the booked slot, on this host's lane; other hosts are other
    /// lanes and are not held.
    ///
    /// Args:
    ///     host: Stable host key, from the route.
    ///     floor: Least time since the host's previous request.
    ///     weight: Documented weight of this request. Zero skips the budget.
    ///
    /// Returns:
    ///     The slot this call booked.
    pub fn pace_weighted(&self, host: &'static str, floor: Duration, weight: u32) -> Instant {
        let budget = binance_weight_limit(host)
            .filter(|_| weight > 0)
            .map(weight_budget)
            .unwrap_or(0);
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
            let slot = if let Some(pace) = paced.get_mut(host) {
                let slot = book_slot(
                    Some(pace.last),
                    &mut pace.weights,
                    now,
                    floor,
                    weight,
                    budget,
                );
                pace.last = slot;
                slot
            } else {
                let mut weights = VecDeque::new();
                let slot = book_slot(None, &mut weights, now, floor, weight, budget);
                paced.insert(
                    host,
                    HostPace {
                        last: slot,
                        weights,
                    },
                );
                slot
            };
            (slot, slot.saturating_duration_since(now))
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
    /// A standing wait that ends later is left in place, so a transient error cannot replace a
    /// 418 or a weight stop with the 30 s floor. The attempt counter still advances, so a host
    /// that keeps failing reaches the ceiling once that longer wait has elapsed and the curve
    /// is writing again.
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
        // A Retry-After or a weight stop is not a step of the curve. The next curve refusal
        // starts again at the floor; only the attempt count carries, so a host that keeps
        // failing still reaches the ceiling.
        let curve_delay = previous
            .filter(|prev| prev.kind == WaitKind::Curve)
            .map(|prev| prev.delay_s);
        let proposed = Attempt {
            last: now,
            delay_s: next_delay_s(curve_delay),
            attempts: previous.map_or(1, |prev| prev.attempts.saturating_add(1)),
            kind: WaitKind::Curve,
            covered: None,
        };
        claims.insert(host, merge_wait(previous, proposed, true));
    }

    /// Wait exactly `seconds` for this host, the venue's `Retry-After`.
    ///
    /// Not clamped to the 30–600 s curve: a 418 ban is the number of seconds the header
    /// names, and a short 429 is that number too rather than the 30 s floor. Zero means the
    /// venue said the host is due now. A deadline already further out is not pulled in, so a
    /// short 429 on the other lane leaves a standing 418 or weight stop where it is.
    ///
    /// Args:
    ///     host: Stable host key, from the route.
    ///     now: Current instant.
    ///     seconds: Parsed `Retry-After` value.
    pub fn honour_retry_after(&self, host: &'static str, now: Instant, seconds: u32) {
        let mut claims = self
            .claims
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let previous = claims.get(host).copied();
        let proposed = Attempt {
            last: now,
            delay_s: seconds,
            attempts: previous.map_or(1, |prev| prev.attempts.saturating_add(1)),
            kind: WaitKind::RetryAfter,
            covered: None,
        };
        // A losing header must not advance the curve: crossing the attempt ceiling would
        // jump a standing curve to 600 s because a short 429 arrived.
        claims.insert(host, merge_wait(previous, proposed, false));
    }

    /// Stop `host` until the next UTC minute when `used` is over 75% of its limit.
    ///
    /// Missing from the budget table, or a used weight that is not over the share, does
    /// nothing. A live wait that already ends at or after the minute boundary is left
    /// standing: a 418, and a curve longer than the seconds left in the UTC minute, are not
    /// shortened. A shorter wait is extended to that boundary, which is how a 429 whose
    /// `Retry-After` ends inside the hot minute still stays stopped until the window resets.
    /// A second reading that does not extend the deadline does not log again.
    ///
    /// Args:
    ///     host: Stable host key, from the route.
    ///     used: `X-MBX-USED-WEIGHT-1M` from this response.
    ///     now: Current instant.
    ///     unix_secs: UTC seconds, for the minute boundary.
    ///
    /// Returns:
    ///     `true` when this call extended the host's wait.
    pub fn note_used_weight(
        &self,
        host: &'static str,
        used: u32,
        now: Instant,
        unix_secs: u64,
    ) -> bool {
        let Some(limit) = binance_weight_limit(host) else {
            return false;
        };
        if !weight_over_share(used, limit) {
            return false;
        }
        let delay_s = secs_until_next_utc_minute(unix_secs);
        let mut claims = self
            .claims
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let standing = claims.get(host).copied();
        let proposed = Attempt {
            last: now,
            delay_s,
            attempts: standing.map(|prev| prev.attempts).unwrap_or(0),
            kind: WaitKind::Weight,
            covered: None,
        };
        let merged = merge_wait(standing, proposed, false);
        let extended = standing.is_none_or(|prev| not_before(&merged) > not_before(&prev));
        // A weight stop shorter than the standing curve does not move the deadline and must
        // not log, but the merge still has to be stored: it is the covered stop `clear`
        // leaves behind.
        if !extended {
            if standing.is_some_and(|prev| {
                prev.covered.map(|stop| (stop.until, stop.kind))
                    != merged.covered.map(|stop| (stop.until, stop.kind))
            }) {
                claims.insert(host, merged);
            }
            return false;
        }
        log::warn!(
            "[x] binance weight {used}/{limit} on {host}; pausing {delay_s}s until the next UTC minute"
        );
        claims.insert(host, merged);
        true
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

    /// Forget a host's curve after it answered a request sent at `asked_at`, AFTER the refusal.
    ///
    /// Without this a user who hit a transport failure once would carry an escalating backoff
    /// for the rest of the session even though the venue is answering again. Only an ANSWER
    /// clears: our own stops — a cancel, a deadline, a budget — say nothing about the venue.
    /// And only an answer to a request sent AFTER the refusal: a neighbour lane's request that
    /// was already in flight when the venue refused proves nothing about the refusal, and
    /// clearing on it would reset the escalation to the floor and hit a limited host every
    /// thirty seconds instead of backing off toward the cap.
    ///
    /// A weight stop and a `Retry-After` are not that curve. One answered request does not mean
    /// the IP weight window reset, and it does not lift a 418. Those stand until their own
    /// deadline has elapsed; a success before then leaves them. A shorter one of those stops
    /// folded under a longer curve is the same: the success ends the curve and leaves the
    /// covered stop in its place.
    ///
    /// Args:
    ///     host: Stable host key, from the route.
    ///     asked_at: When the answered request was sent.
    pub fn clear(&self, host: &'static str, asked_at: Instant) {
        let mut claims = self
            .claims
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(prev) = claims.get(host).copied() else {
            return;
        };
        if prev.last >= asked_at {
            return;
        }
        // `last` is a real instant in production. A synthetic last still in the future has
        // not elapsed either; `duration_since` would panic on it.
        let now = Instant::now();
        if matches!(prev.kind, WaitKind::RetryAfter | WaitKind::Weight)
            && (now < prev.last || refused_for(Some(&prev), now).is_some())
        {
            return;
        }
        if let Some(stop) = prev.covered {
            if now < stop.until {
                claims.insert(host, covered_remainder(stop, now, prev.attempts));
                return;
            }
        }
        claims.remove(host);
    }
}

#[cfg(test)]
mod tests;
