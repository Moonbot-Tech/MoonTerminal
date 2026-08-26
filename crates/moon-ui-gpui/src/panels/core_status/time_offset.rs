//! Core-clock-offset presentation for both Core Status modes.
//!
//! Two surfaces over one measured status: the single-cell summary the column shows, and the
//! structured hover behind it. The hover is BUILT FROM the same assembled facts the cell reads,
//! never written a second time by hand, so the two cannot drift apart — the idiom
//! `panels/core_status/startup.rs::{startup_facts, startup_tooltip}` established.
//!
//! Everything here is pure and GPUI-free so the decisions can be tested: `moon-ui-gpui` is a binary
//! crate with no `[lib]`, and a panel decision that needs a real test has to be a free function
//! first.

use moon_core::feed::CoreTimeOffsetStatus;
use moon_core::session::core_time_offset::OffsetSource;
use rust_i18n::t;

#[cfg(test)]
mod tests;

/// What the tz-offset column shows for one row.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum TzOffsetCell {
    /// An offset has been adopted for this core.
    Measured { offset_secs: i32 },
    /// Nothing has ever been adopted for this core.
    ///
    /// Distinct from a measured zero on purpose: a core that runs on UTC and a core nobody has
    /// measured must not read the same.
    Unknown,
}

/// Decide what the tz-offset column shows, from the retained measurement.
///
/// Args:
///     s: The core's retained clock-offset status.
///
/// Returns:
///     The cell to render.
pub(super) fn tz_offset_cell(s: &CoreTimeOffsetStatus) -> TzOffsetCell {
    match s.offset_secs {
        Some(offset_secs) => TzOffsetCell::Measured { offset_secs },
        None => TzOffsetCell::Unknown,
    }
}

/// Render the tz-offset cell as text.
///
/// A measured value renders as `UTC+02:00` / `UTC+00:00` / `UTC-04:00`: ASCII sign, two-digit
/// hours, two-digit minutes, always both, so a quarter-hour zone (the estimator buckets at 900 s)
/// renders correctly rather than losing its minutes.
///
/// Args:
///     cell: The decision from [`tz_offset_cell`].
///
/// Returns:
///     Localized cell text, or the never-measured marker.
pub(super) fn tz_offset_cell_text(cell: TzOffsetCell) -> String {
    match cell {
        TzOffsetCell::Measured { offset_secs } => {
            let sign = if offset_secs < 0 { '-' } else { '+' };
            let total_minutes = offset_secs.unsigned_abs() / 60;
            let hours = total_minutes / 60;
            let minutes = total_minutes % 60;
            format!("UTC{sign}{hours:02}:{minutes:02}")
        }
        TzOffsetCell::Unknown => t!("core_status.tz_off.unknown").to_string(),
    }
}

/// Assembled facts behind the tz-offset hover, read once from the retained status.
pub(super) struct TzOffsetFacts {
    /// Adopted offset, or `None` when nothing was ever adopted.
    pub(super) offset_secs: Option<i32>,
    /// Samples standing behind the adopted value (or behind the still-unmeasured state).
    pub(super) samples: u32,
    /// True-UTC instant of the LATEST observation carrying the current value, in milliseconds —
    /// see `CoreTimeOffsetStatus::observed_at_utc`. Rendered under «Замерено» / "Observed", never
    /// "adopted": a re-measurement that confirms an unchanged offset advances this while the
    /// durable adoption instant stays put.
    pub(super) observed_at_utc: i64,
    /// Which measurement produced the adopted value.
    pub(super) source: OffsetSource,
}

/// Assemble the hover's facts from the retained status.
///
/// Args:
///     s: The core's retained clock-offset status.
///
/// Returns:
///     The facts [`tz_offset_tooltip`] renders.
pub(super) fn tz_offset_facts(s: &CoreTimeOffsetStatus) -> TzOffsetFacts {
    TzOffsetFacts {
        offset_secs: s.offset_secs,
        samples: s.samples,
        observed_at_utc: s.observed_at_utc,
        source: s.source,
    }
}

/// Localized name of one offset source.
fn source_label(source: OffsetSource) -> String {
    match source {
        OffsetSource::Log => t!("core_status.tz_off.source.log"),
        OffsetSource::Replica => t!("core_status.tz_off.source.replica"),
        OffsetSource::Skew => t!("core_status.tz_off.source.skew"),
        OffsetSource::None => t!("core_status.tz_off.source.none"),
    }
    .to_string()
}

/// Render the assembled facts as the hover text.
///
/// Derived FROM [`tz_offset_facts`] rather than composed independently, so the cell and the hover
/// can never disagree about what a row reports. The observed instant renders in UTC rather than the
/// panel's selected display zone: this hover exists to explain a core's offset FROM UTC, and this
/// module takes no display-zone argument to keep it pure and independently testable.
///
/// Args:
///     f: The facts from [`tz_offset_facts`].
///
/// Returns:
///     The hover body, one `label: value` line per fact, or a single never-measured line plus the
///     sample count standing behind it.
pub(super) fn tz_offset_tooltip(f: &TzOffsetFacts) -> String {
    let Some(offset_secs) = f.offset_secs else {
        return format!(
            "{}\n{}: {}",
            t!("core_status.tz_off.f.unmeasured"),
            t!("core_status.tz_off.f.samples"),
            f.samples
        );
    };
    [
        format!(
            "{}: {}",
            t!("core_status.tz_off.f.offset"),
            tz_offset_cell_text(TzOffsetCell::Measured { offset_secs })
        ),
        format!("{}: {}", t!("core_status.tz_off.f.samples"), f.samples),
        format!(
            "{}: {}",
            t!("core_status.tz_off.f.observed"),
            moon_core::util::display_time::format_minute(f.observed_at_utc / 1000, chrono_tz::UTC)
        ),
        format!(
            "{}: {}",
            t!("core_status.tz_off.f.source"),
            source_label(f.source)
        ),
    ]
    .join("\n")
}

/// Sort rank; measured values order by offset, unknown trails every measured one.
pub(super) fn tz_offset_rank(cell: TzOffsetCell) -> (u8, i32) {
    match cell {
        TzOffsetCell::Measured { offset_secs } => (0, offset_secs),
        TzOffsetCell::Unknown => (1, 0),
    }
}
