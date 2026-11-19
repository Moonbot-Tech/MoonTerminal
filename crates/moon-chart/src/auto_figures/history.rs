//! How much of its history a row must have read for its answer to count as complete — ONE rule,
//! read by the chart's "not enough history" note (`moon-ui-gpui`,
//! `chartdx::auto_figures_sync::job`) and by the screener's scan ([`super::scan`]), so a row the
//! chart calls complete is scanned, and a row it calls short is not, on the same bars.
//!
//! The share depends on what the row's answer is made of (`docs-internal/AUTO_FIGURES.md` §13.3):
//! a row that SAMPLES its bars ([`SAMPLE_HISTORY_PCT`]) or a CALENDAR row whose answer is a fact
//! about every bar of its period ([`CALENDAR_HISTORY_PCT`]).

use moon_core::config::auto_figures::DetectorParams;

/// Share of `DetectorParams::bars_needed` a row that SAMPLES its bars must have read, in %, before
/// it counts as having its full history.
///
/// Not 100: the local 5-minute cache is written only while the terminal runs, so holes are
/// normal, and a pivot row whose resampled bars come out a few short has its answer all the same —
/// its levels are the turns of whatever bars it has. On the scenes' recorded set the 4h levels row
/// reads 68 of its 72 bars — 94 % — and calling that "not enough history" told the reader
/// something false about a row that found its levels. Below 90 % a missing day starts to be a
/// missing swing, and the note is worth showing. Calendar rows ask for all of it instead.
pub const SAMPLE_HISTORY_PCT: u64 = 90;

/// Share of `DetectorParams::bars_needed` a CALENDAR row (period extreme, period level, volume
/// profile) must have read, in %: all of it.
///
/// Such a row's answer is a fact about every bar of its period, not a sample of them: the period's
/// open IS its first bar, its high or low may be in any one bar, and every bar's volume moves the
/// profile's POC. One bar missing can therefore change or erase the line itself — and a line that
/// is silently missing or wrong is exactly what the note must not hide. The alternative ("short
/// only when the detector found nothing") was rejected: a period-level row with the day open found
/// and the week open lost finds something, and a week high taken from an incomplete week is found
/// too — wrong, and unmarked.
pub const CALENDAR_HISTORY_PCT: u64 = 100;

/// The share of `bars_needed` a row of `params` must read to count as complete, in % — by what its
/// answer is made of. Exhaustive on purpose: a new detector kind has to say which it is.
///
/// A formation samples its bars like the pivot rows it is built from: its sides run through
/// whatever swings the bars hold.
pub fn full_history_pct(params: &DetectorParams) -> u64 {
    match params {
        DetectorParams::Swing(_)
        | DetectorParams::Trend(_)
        | DetectorParams::Squeeze(_)
        | DetectorParams::Formation(_)
        // One bar needed: 90 % and 100 % of it are the same test.
        | DetectorParams::RoundLevels(_) => SAMPLE_HISTORY_PCT,
        DetectorParams::PeriodExtreme(_)
        | DetectorParams::PeriodLevel(_)
        | DetectorParams::VolumeProfile(_) => CALENDAR_HISTORY_PCT,
    }
}

/// Whether `have` closed bars are a full history for a row of `params` that needs `needed` — at
/// least [`full_history_pct`] % of them. Integer arithmetic, so the boundary is exact: 9 of 10
/// bars is 90 %, not 89.99…
pub fn history_full(have: usize, needed: u32, params: &DetectorParams) -> bool {
    (have as u64).saturating_mul(100) >= u64::from(needed) * full_history_pct(params)
}

#[cfg(test)]
mod tests;
