//! The core's clock offset, sampled from its Ping.
//!
//! MoonProto refreshes `MoonClient::server_time_delta_ms` on every received Ping (every 0.3–1 s):
//! the core's local wall clock, timezone included, minus this machine's OS UTC clock. Reading it
//! is one atomic load, but nothing announces a change, so the feed loop polls it on its own
//! deadline, [`SAMPLE_EVERY`], corrects it by this machine's own clock error (`pc_clock`) so it
//! measures the core against true UTC rather than against this PC, and feeds each reading to the
//! core's [`OffsetEstimator`] — which owns adoption: whole seconds, snapping to a whole zone,
//! agreement, the deadband against the offset in force, the quarantine after Ready.
//!
//! The raw delta is never applied to a report row directly, though MoonProto offers that
//! (`ServerClock::report_millis_to_utc`): it carries network delay and clock error that move by
//! milliseconds from one Ping to the next, and it is today's value only. Report rows are read for
//! any period, and while the core is offline too; they go through the stored, dated segments the
//! estimator's adoptions open (`db::report_axis`).

use std::time::{Duration, Instant};

use crate::session::core_time_offset::OffsetEstimator;

/// How often the feed loop reads the Ping clock.
///
/// Fast enough that a fresh connection adopts within the estimator's rules — the quarantine, then
/// three samples spread over `MIN_SPREAD_MS` — about 35 s after Ready; slow enough that the extra
/// wake costs a quiet core nothing measurable. A clock or timezone change is adopted the same
/// way, within about half a minute (a step DOWN waits for the higher samples to leave the
/// estimator's window).
pub(super) const SAMPLE_EVERY: Duration = Duration::from_secs(5);

/// What one reading told the feed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum PingOffset {
    /// A new offset, at least `DEADBAND_SECS` from the one in force: store it as a segment.
    Adopted(i32),
    /// The offset in force, confirmed by this connection's Pings for the first time: show it as
    /// live — Core Status's "Observed", sample count and the Ping as its source — and store
    /// nothing, since nothing changed.
    Confirmed(i32),
}

/// One core connection's Ping-clock sampler.
///
/// The getter is not a Ping: it holds the last one received, and while the connection is down
/// MoonProto keeps returning the previous connection's last Ping. Fed every 5 s regardless, that one
/// Ping would pass for many — the estimator's agreement and spread rules count arrivals, so one
/// held value re-read past them could adopt an offset no second Ping ever confirmed. Only a reading
/// taken while the connection is Ready AND different from the last one fed is therefore a sample:
/// each new Ping moves the delta by its own delay, so a repeated value is the same Ping. A reading
/// that lands in the estimator's quarantine is consumed there all the same — the held Ping read
/// right after Ready never counts at all, which is the point.
#[derive(Debug)]
pub(super) struct PingClock {
    estimator: OffsetEstimator,
    /// When the next reading is due.
    next_due: Instant,
    /// The last raw delta handed to the estimator, kept across reconnects so the held value of
    /// the previous connection is recognised as already counted.
    last_fed: Option<i64>,
    /// Whether this connection has already reported the offset in force (adopted or confirmed).
    confirmed: bool,
}

impl PingClock {
    /// A sampler whose first reading is due at once.
    ///
    /// Args:
    ///     now: The feed loop's clock.
    ///
    /// Returns:
    ///     A sampler with an empty estimator.
    pub(super) fn new(now: Instant) -> Self {
        Self {
            estimator: OffsetEstimator::new(),
            next_due: now,
            last_fed: None,
            confirmed: false,
        }
    }

    /// Start from the offset already stored for this core, so a new connection adopts only a
    /// real change from it (`DEADBAND_SECS`) rather than re-opening a segment for jitter.
    ///
    /// Args:
    ///     stored: The newest stored segment's offset in seconds; `None` for a core never measured.
    pub(super) fn seed(&mut self, stored: Option<i32>) {
        self.estimator.seed(stored);
    }

    /// The connection entered Ready: start the estimator's quarantine, and let this connection
    /// confirm the offset in force once more.
    ///
    /// Args:
    ///     now_ms: This machine's UTC clock, in milliseconds.
    pub(super) fn note_ready(&mut self, now_ms: i64) {
        self.estimator.note_ready(now_ms);
        self.confirmed = false;
    }

    /// How long the feed loop may sleep before the next reading is due.
    ///
    /// Args:
    ///     now: The feed loop's clock.
    ///
    /// Returns:
    ///     Zero when a reading is already due.
    pub(super) fn wait(&self, now: Instant) -> Duration {
        self.next_due.saturating_duration_since(now)
    }

    /// Take one reading when it is due and feed it to the estimator if it is a new Ping.
    ///
    /// Args:
    ///     now: The feed loop's clock.
    ///     recv_ms: This machine's UTC clock, in milliseconds, read at the same moment.
    ///     delta_ms: `MoonClient::server_time_delta_ms()`; `None` before the first Ping.
    ///     pc_error_ms: This machine's clock error, `true UTC − local clock`
    ///         (`pc_clock::read`); `None` measures against the local clock as is.
    ///     ready: Whether the connection is Ready now. Before Ready no quarantine has started
    ///         and the value may be the previous connection's, so nothing is fed.
    ///
    /// Returns:
    ///     An adoption, the first confirmation of the offset in force on this connection, or
    ///     `None` — not due, not Ready, no Ping yet, the same Ping again, or nothing new to say.
    pub(super) fn poll(
        &mut self,
        now: Instant,
        recv_ms: i64,
        delta_ms: Option<i64>,
        pc_error_ms: Option<i64>,
        ready: bool,
    ) -> Option<PingOffset> {
        if now < self.next_due {
            return None;
        }
        self.next_due = now + SAMPLE_EVERY;
        let delta_ms = delta_ms.filter(|_| ready)?;
        if self.last_fed == Some(delta_ms) {
            return None;
        }
        self.last_fed = Some(delta_ms);
        // `core − true UTC = (core − local) − (true UTC − local)`. An overflowing sum is no clock
        // at all; the estimator's zone band rejects anything implausible that does fit.
        let delta_ms = delta_ms.checked_sub(pc_error_ms.unwrap_or(0))?;
        let core_time_ms = recv_ms.checked_add(delta_ms)?;
        if let Some(offset) = self.estimator.observe(core_time_ms, recv_ms) {
            self.confirmed = true;
            return Some(PingOffset::Adopted(offset));
        }
        if self.confirmed || self.estimator.best().is_none() {
            return None;
        }
        self.confirmed = true;
        self.estimator.adopted().map(PingOffset::Confirmed)
    }

    /// Samples standing behind the adopted value, for the status the UI shows.
    ///
    /// Returns:
    ///     Count of distinct Ping readings in the estimator's window.
    pub(super) fn samples(&self) -> u32 {
        self.estimator.samples()
    }
}

#[cfg(test)]
mod tests;
