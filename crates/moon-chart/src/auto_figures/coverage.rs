//! Whether the bars of a UTC calendar period cover it: the one check the calendar detectors share
//! (`extremes`, `volume_profile`, `period_levels`).
//!
//! The 5-minute cache is written only while the terminal runs, and the core's ring fills only the
//! hours the core ran, so a period whose start, end or a stretch inside is missing is normal. A
//! level read from part of a period is a different level — a day's high from three of its 288 bars
//! was 1.1 % under the real one on a live cache (BTCUSDT, 19.11.2026) — and a wrong level drawn
//! with confidence is worse than an honest gap. So, with `tf` the bar length of the row:
//!
//! - a period's START is present when a valid bar opens within one bar length of it
//!   ([`opens_at`]) — the period's own first bar, not merely a bar of the input before it;
//! - a FINISHED period's END is present when its last bar's close, one bar length after its open,
//!   reaches it ([`closes_at`]);
//! - a period is COVERED ([`covered`]) when its start is present, its end too when it is finished,
//!   and its bars are at least [`MIN_COVERAGE`] of the bars the covered span should hold — the
//!   whole period when it is finished, up to its last closed bar when it is the current one (the
//!   rest of it has not happened yet).
//!
//! An open or a close (`period_levels`) needs only its edge bar; a high, a low or a profile needs
//! the whole span (`extremes`, `volume_profile`).

/// Share of a span's bars that must be present for it to count as complete.
///
/// The same 90 % the plan names for "enough history" (`docs-internal/AUTO_FIGURES.md` §13.3): a
/// few missing bars move a POC or a high by noise, a missing hour can move it to another price.
pub const MIN_COVERAGE: f64 = 0.9;

/// Whether a bar opening at `t_open_ms` is the first bar of a period starting at `start`: it opens
/// at or after the start and within one bar length (`tf`) of it.
pub(crate) fn opens_at(t_open_ms: f64, start: f64, tf: f64) -> bool {
    t_open_ms >= start && t_open_ms < start + tf
}

/// Whether a bar opening at `t_open_ms` is the last bar of a period ending at `end`: its close, one
/// bar length (`tf`) after its open, reaches the end.
pub(crate) fn closes_at(t_open_ms: f64, end: f64, tf: f64) -> bool {
    t_open_ms + tf >= end
}

/// Whether the valid bars of one period — `count` of them, the first opening at `first_ms` and the
/// last at `last_ms` — cover the period from `start`: up to `end` when the period is finished
/// (`Some`), up to the last bar's close when it is the current one (`None`). See the module doc.
///
/// `false` for no bars and for a bar length that is not a positive number.
pub(crate) fn covered(
    first_ms: f64,
    last_ms: f64,
    count: usize,
    start: f64,
    end: Option<f64>,
    tf: f64,
) -> bool {
    if !(tf > 0.0) || count == 0 || !opens_at(first_ms, start, tf) {
        return false;
    }
    let span_end = match end {
        Some(end) if !closes_at(last_ms, end, tf) => return false,
        Some(end) => end,
        None => last_ms + tf,
    };
    let expected = ((span_end - start) / tf).round().max(1.0);
    count as f64 >= MIN_COVERAGE * expected
}
