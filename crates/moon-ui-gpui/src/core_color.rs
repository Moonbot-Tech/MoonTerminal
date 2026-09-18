//! Shared lookup of the configured core colour for detection cards and chart arrivals, and the
//! one rule that makes it readable wherever it is painted.

use moon_core::config::ServerConfig;
use moon_core::session::CoreId;

/// Contrast a painted core colour must reach against its ground: WCAG's 3:1 for non-text marks.
///
/// One floor for the arrival border AND the Detects card, on purpose. The two used to disagree —
/// the border swapped an unreadable colour for the theme accent while the card painted it as is —
/// and a user comparing a yellow card with a blue frame reads that as the frame having the wrong
/// colour, not as a readability guard. The same lift against each place's own ground keeps the two
/// the same hue and each readable where it sits.
const CORE_COLOR_FLOOR: f64 = 3.0;

/// Return the configured RGB colour for `core`, or `None` when its server is absent.
/// Callers retain their own fallback for a missing colour because cards and flashing borders have
/// different needs; a PRESENT colour they pass through [`readable`].
pub(crate) fn core_color(servers: &[ServerConfig], core: CoreId) -> Option<[u8; 3]> {
    servers
        .iter()
        .find(|server| server.id == core)
        .map(|server| server.color)
}

/// The core colour as it may be painted on `ground`: unchanged when it already reads, otherwise
/// the same hue pulled to [`CORE_COLOR_FLOOR`] by lightness alone.
///
/// A pick that reads is returned byte for byte, so every core whose colour worked before this rule
/// keeps exactly the colour it had. Only picks that used to be swapped for the accent — every
/// yellow on the light theme, every navy on the dark one — now come back recognisable instead.
///
/// Args:
///     color: The configured colour.
///     ground: The background it is about to be drawn on.
pub(crate) fn readable(color: [u8; 3], ground: [u8; 3]) -> [u8; 3] {
    crate::contrast::lift_to_contrast(color, ground, CORE_COLOR_FLOOR)
}

#[cfg(test)]
mod tests;
