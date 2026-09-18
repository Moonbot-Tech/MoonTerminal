//! The header strip's colours, derived from the chart's own theme.
//!
//! The strip is burnt into a picture people paste into a chat, so its legibility is a requirement
//! rather than a taste call — and it has to hold in EVERY theme, because `ChartTheme.bg` is
//! user-configurable and ships both dark and light. Hard-coding a grey satisfies exactly one of
//! those and is invisible in the other. So nothing here is a chosen colour: every value is DERIVED
//! from the two the panel already hands the shot (`bg` and the chart's own supporting-text colour),
//! against a stated contrast floor that the derivation is not allowed to miss.
//!
//! # Why the contrast floors are numbers and not adjectives
//!
//! "Muted grey for the secondary text" is a sound instruction and an unfalsifiable one — muted by
//! how much, against what? Stated as `contrast_ratio(secondary, band) >= 4.5` it becomes a
//! property a test can hold, in every theme, including one a user invents. The two floors are the
//! WCAG AA/AAA body-text ratios: 7.0 for what a reader is meant to scan and 4.5 for what they are
//! meant to be able to read when they look. A Telegram thumbnail is the hostile case this exists
//! for.
//!
//! # Why here, and why not `palette`
//!
//! Here rather than beside the `TextOutW` that consumes it, for the reason every pure module under
//! `super` gives: `super::paint_win` is Windows-only, so arithmetic placed there is arithmetic no
//! test on another platform ever executes. And **not** named `palette`: the crate's own theme
//! contract (`tests/theme_contract/theme.rs`) bans that word followed by a path separator on every
//! source line under `src/`, comments included, so a module of that name would fail the contract
//! merely by being CALLED — with an error message about MoonUI that reads as nonsense here. The
//! name `ink` is deliberate; do not "fix" it. (This paragraph cannot spell the banned token, for
//! the same reason. That is not an oversight either — the contract really does scan prose.)
//!
//! Deliberately free of GPUI, of GDI and of the `windows` crate. The contrast arithmetic itself —
//! luminance, ratio, the pole and the blend — lives in `crate::contrast`, shared with the core
//! colour the arrival border and the Detects card paint; this module keeps only the strip's own
//! derivation.

use crate::contrast::{contrast_ratio, mix, pole};

/// The strip's five colours, all derived, none chosen.
pub(super) struct Palette {
    /// The strip's own ground: the chart's background, lifted just enough to read as a band.
    pub(super) band: [u8; 3],
    /// The one-pixel rule along the strip's BOTTOM edge.
    pub(super) hairline: [u8; 3],
    /// The coin.
    pub(super) lead: [u8; 3],
    /// The figures a reader actually scans for.
    pub(super) primary: [u8; 3],
    /// Context and view metadata — venue, stamp, timeframe, scale, and the window tokens.
    pub(super) secondary: [u8; 3],
}

/// How far the band is lifted off the chart's background, as a fraction toward the contrast pole.
///
/// Small on purpose. The band's job is to say "this row is a caption", which a barely perceptible
/// step already does; a visible slab would read as a second window pasted above the chart.
const BAND_LIFT: f64 = 0.06;

/// How far the hairline is lifted, as a fraction toward the contrast pole.
const HAIRLINE_LIFT: f64 = 0.22;

/// Contrast the scanned text AIMS for against the band, and the floor the muted text may not go
/// below. The WCAG AAA and AA body-text ratios.
///
/// **The first is a TARGET, the second is a GUARANTEE, and the difference is deliberate.** `bg` is
/// user-configurable, so a ground can exist on which nothing reaches 7.0 at all — a mid-grey band
/// tops out well below it (see [`pole`]). Promising a ratio the arithmetic cannot deliver would
/// make the floor a lie in exactly the themes nobody tests. So the walk aims at 7.0, then falls
/// back to the best-contrasting pole, and what is GUARANTEED, on every ground including the worst
/// possible, is `4.58:1` for both registers — above 4.5, which is why 4.5 is the number the tests
/// hold and the design promises.
///
/// In the two SHIPPED themes there is plenty of room and the target is met outright: the primary
/// reaches ~9.3:1 on the dark theme and ~18.4:1 on the light one.
const PRIMARY_CONTRAST: f64 = 7.0;
const SECONDARY_CONTRAST: f64 = 4.5;

/// The search that finds a colour meeting a contrast floor: fixed count, fixed step.
///
/// Deliberately a bounded walk rather than a solve. It is total — it terminates on every input,
/// including `bg == text` and a theme with no contrast at all — and every step it can produce is a
/// value a test can name, which a closed-form inversion of the luminance curve would not be.
const STEPS: u32 = 20;
const STEP: f64 = 0.05;

/// Every colour the strip draws, for one chart theme.
///
/// The chart's own supporting-text colour is the STARTING POINT rather than the answer: it is
/// guaranteed to read against `bg`, but the strip does not draw on `bg` — it draws on [`band`],
/// and it needs two distinct registers on it. So `primary` walks that colour AWAY from the band
/// until it is unambiguously scannable, and `secondary` walks it TOWARD the band until one more
/// step would take it under the readable floor.
///
/// `lead == primary`, and that is the design rather than an oversight: the coin is the subject and
/// is distinguished by SIZE and WEIGHT. One accent, not five — a header that gives every field its
/// own colour is the pile this redesign exists to remove.
///
/// On a ground too flat to hold two readable registers, `secondary` collapses onto `primary`
/// rather than going quieter than the floor. The hierarchy then rests on size and weight alone,
/// which is a weaker header and a legible one; the alternative is a caption nobody can read in a
/// thumbnail, which is the failure this module exists to make impossible.
///
/// Args:
///     bg: The chart's background colour, which the strip sits above.
///     text: The chart's supporting-text colour — the one it already writes axis labels in, and so
///         the one guaranteed to read against `bg` in every theme.
///
/// Returns:
///     The strip's five colours.
pub(super) fn palette(bg: [u8; 3], text: [u8; 3]) -> Palette {
    let away = pole(bg);
    let band = mix(bg, away, BAND_LIFT);
    let hairline = mix(bg, away, HAIRLINE_LIFT);
    let primary = toward_pole(text, band, PRIMARY_CONTRAST);
    Palette {
        band,
        hairline,
        lead: primary,
        primary,
        secondary: toward_band(text, band, SECONDARY_CONTRAST),
    }
}

/// Push `text` away from `band` until it reaches `target` contrast, or as far as it can go.
///
/// Args:
///     text: The theme's own text colour, the starting point.
///     band: The ground the result will be drawn on.
///     target: The contrast ratio to reach.
///
/// Returns:
///     The first stepped colour meeting `target`, or the pole itself when none does — a theme
///     whose own text cannot reach the floor even at pure white or pure black is degenerate, and
///     the pole is the best answer that exists rather than a failure.
fn toward_pole(text: [u8; 3], band: [u8; 3], target: f64) -> [u8; 3] {
    let away = pole(band);
    for step in 0..=STEPS {
        let candidate = mix(text, away, f64::from(step) * STEP);
        if contrast_ratio(candidate, band) >= target {
            return candidate;
        }
    }
    away
}

/// Mute `text` toward `band` for as long as it stays at or above `floor`.
///
/// Args:
///     text: The theme's own text colour, the starting point.
///     band: The ground the result will be drawn on, and what it is muted toward.
///     floor: The contrast ratio the result may not go below.
///
/// Returns:
///     The most muted stepped colour still at or above `floor`. When the theme's own text does not
///     clear the floor to begin with there is nothing to mute, so it is pushed away instead — a
///     secondary that is illegible is worse than one that is not visibly quieter.
fn toward_band(text: [u8; 3], band: [u8; 3], floor: f64) -> [u8; 3] {
    if contrast_ratio(text, band) < floor {
        return toward_pole(text, band, floor);
    }
    let mut best = text;
    for step in 1..=STEPS {
        let candidate = mix(text, band, f64::from(step) * STEP);
        if contrast_ratio(candidate, band) < floor {
            break;
        }
        best = candidate;
    }
    best
}

#[cfg(test)]
mod tests;
