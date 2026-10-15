//! The core's timezone, read from its Ping.
//!
//! MoonProto refreshes `MoonClient::server_utc_offset_minutes` on every received Ping (every
//! 0.3–1 s): the core's local wall clock minus this machine's OS UTC clock, rounded to the nearest
//! 15 minutes — `Some(180)` is UTC+03:00, `Some(0)` is UTC, `None` means no Ping yet. Reading it is
//! one atomic load, but nothing announces a change, so the feed loop polls it on its own deadline,
//! [`SAMPLE_EVERY`], and compares it with the offset in force.
//!
//! The value is the timezone and nothing else. Until 2026-10 the terminal estimated the core's
//! clock to the second from the raw delta, corrected by this PC's SNTP-measured error, through an
//! agreement window over distinct Pings — and a stable link whose Pings rounded to the same
//! millisecond never counted as more than one Ping, so the offset was never adopted (#931). The
//! rounded value needs none of that: equal readings are as usable as changing ones, and a clock
//! that runs seconds fast stays uncorrected by design.
//!
//! A core whose stored segment came from that estimator and carries seconds (`+03:00:40`) differs
//! from its own zone, so its first connection on this build adopts the zone once — and, like any
//! adoption, makes the report writer re-value that core's history (`stage_rescan_core`). That
//! one-time cost is the price of the axis applying the zone the spec asks for.
//!
//! Report rows still never use the live value directly: they are read for any period, and while
//! the core is offline too, so they go through the stored, dated segments an adoption opens
//! (`db::report_axis`).

use std::time::{Duration, Instant};

use crate::db::report_axis::{MAX_OFFSET_SECS, MIN_OFFSET_SECS};

/// How often the feed loop reads the Ping timezone.
///
/// A timezone change on the core is stored within this long of its first Ping; the extra wake
/// costs a quiet core nothing measurable.
pub(super) const SAMPLE_EVERY: Duration = Duration::from_secs(5);

/// What one reading told the feed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum PingOffset {
    /// A timezone other than the one in force: store it as a segment.
    Adopted(i32),
    /// The offset in force, confirmed by this connection's Ping for the first time: show it as
    /// live — Core Status's "Observed" and the Ping as its source — and store nothing, since
    /// nothing changed.
    Confirmed(i32),
}

/// One core connection's Ping-timezone reader.
///
/// Read only while the connection is Ready: before it the getter may still hold the previous
/// connection's last Ping, and a confirmation must describe this connection. A held value read
/// right after Ready is the same core's own zone, so at worst it confirms the zone in force a few
/// seconds before the next reading adopts a change made while the link was down.
#[derive(Debug)]
pub(super) struct PingClock {
    /// The offset in force, in seconds east of UTC: the last one adopted, or the stored one
    /// [`Self::seed`] handed in; `None` when neither exists.
    in_force: Option<i32>,
    /// When the next reading is due.
    next_due: Instant,
    /// Whether this connection has already reported the offset in force (adopted or confirmed).
    confirmed: bool,
}

impl PingClock {
    /// A reader whose first reading is due at once.
    ///
    /// Args:
    ///     now: The feed loop's clock.
    ///
    /// Returns:
    ///     A reader with no offset in force.
    pub(super) fn new(now: Instant) -> Self {
        Self {
            in_force: None,
            next_due: now,
            confirmed: false,
        }
    }

    /// Start from the offset already stored for this core, so a new connection adopts only a
    /// real change from it rather than re-opening a segment for the same zone.
    ///
    /// Args:
    ///     stored: The newest stored segment's offset in seconds; `None` for a core never measured.
    pub(super) fn seed(&mut self, stored: Option<i32>) {
        self.in_force = stored;
    }

    /// The connection entered Ready: let this connection confirm the offset in force once more.
    pub(super) fn note_ready(&mut self) {
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

    /// Take one reading when it is due and compare it with the offset in force.
    ///
    /// Args:
    ///     now: The feed loop's clock.
    ///     offset_minutes: `MoonClient::server_utc_offset_minutes()`; `None` before the first
    ///         Ping, which is not a zero offset.
    ///     ready: Whether the connection is Ready now.
    ///
    /// Returns:
    ///     An adoption, the first confirmation of the offset in force on this connection, or
    ///     `None` — not due, not Ready, no Ping yet, a zone outside the report axis's band, or
    ///     nothing new to say.
    pub(super) fn poll(
        &mut self,
        now: Instant,
        offset_minutes: Option<i64>,
        ready: bool,
    ) -> Option<PingOffset> {
        if now < self.next_due {
            return None;
        }
        self.next_due = now + SAMPLE_EVERY;
        let minutes = offset_minutes.filter(|_| ready)?;
        // The axis drops a segment outside this band on load, so storing one would only open a
        // segment nothing applies.
        let offset_secs = minutes
            .checked_mul(60)
            .and_then(|secs| i32::try_from(secs).ok())
            .filter(|secs| (MIN_OFFSET_SECS..=MAX_OFFSET_SECS).contains(secs))?;
        if self.in_force != Some(offset_secs) {
            self.in_force = Some(offset_secs);
            self.confirmed = true;
            return Some(PingOffset::Adopted(offset_secs));
        }
        if std::mem::replace(&mut self.confirmed, true) {
            return None;
        }
        Some(PingOffset::Confirmed(offset_secs))
    }
}

#[cfg(test)]
mod tests;
