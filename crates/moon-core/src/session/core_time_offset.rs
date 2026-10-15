//! Where a core's stored clock offset came from.
//!
//! The offset itself is the core's timezone, read from its Ping by `feed::live::ping_clock` and
//! stored as dated segments in `core_time_offset` (`db::rep::core_offset`), which the report axis
//! applies (`db::report_axis`). Sign convention matches
//! [`crate::db::report_axis::OffsetSegment::offset_secs`]: seconds EAST of UTC on the core's
//! clock.
//!
//! Until 2026-10 an estimator here measured the core's clock to the second from the same Ping —
//! the raw delta, this PC's SNTP-measured error taken out, an agreement window over distinct
//! Pings. It never adopted an offset on a stable link whose Pings rounded to one millisecond
//! (#931) and was replaced by MoonProto's rounded timezone, which needs none of it. Segments it
//! and the log-era estimator stored stay in the table and keep their labels below.

/// Where an adopted offset came from.
///
/// [`Self::None`] is the DEFAULT because a core nothing has been measured on is the state every
/// core starts in, and it must stay distinguishable from a measured zero: a diagnosis surface that
/// cannot tell "never measured" from "runs on UTC" turns a silent estimator failure into a
/// confident wrong claim.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum OffsetSource {
    /// The core's Ping clock: the live source.
    Ping,
    /// The core's log lines: the live source until 2026-10, still the label of the segments it
    /// stored, which a restart seeds from.
    Log,
    Replica,
    Skew,
    #[default]
    None,
}

impl OffsetSource {
    /// The label stored in `core_time_offset.source`.
    ///
    /// Returns:
    ///     The stable lowercase name [`Self::from_label`] reads back.
    pub fn label(self) -> &'static str {
        match self {
            OffsetSource::Ping => "ping",
            OffsetSource::Log => "log",
            OffsetSource::Replica => "replica",
            OffsetSource::Skew => "skew",
            OffsetSource::None => "none",
        }
    }

    /// Read a stored label back.
    ///
    /// Args:
    ///     label: A `core_time_offset.source` value.
    ///
    /// Returns:
    ///     The source it names, or [`Self::None`] for a label this build does not know — the
    ///     segment's offset still applies, only its provenance is unknown.
    pub fn from_label(label: &str) -> Self {
        match label {
            "ping" => OffsetSource::Ping,
            "log" => OffsetSource::Log,
            "replica" => OffsetSource::Replica,
            "skew" => OffsetSource::Skew,
            _ => OffsetSource::None,
        }
    }
}
