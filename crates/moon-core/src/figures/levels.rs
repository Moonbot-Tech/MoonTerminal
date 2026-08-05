//! Ratio scales shared by the Fibonacci family.
//!
//! A scale is a list of [`Level`]s — a ratio plus how loudly it is drawn. The retracement, the
//! trend-based extension, the Fibonacci channel and the time zones all read the SAME list, so a
//! set edited once is edited for all of them.

/// How loudly a level is drawn, relative to the figure's own colour.
///
/// The scale is monochrome by design: the figure's colour is chosen by the user in the pencil
/// popup, and a per-level palette belongs to the per-level style editor, not to the tool. Until
/// that editor exists, emphasis is what separates the levels a trader actually watches from the
/// ones that are there for completeness.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Emphasis {
    /// The anchors of the move (0 and 1): present, but not what the eye should land on.
    Anchor,
    /// The retracements a trader watches — the golden group.
    Key,
    /// Everything else on the scale: present and readable, but not shouted.
    Minor,
}

impl Emphasis {
    /// Alpha multiplier applied to the figure's colour for this level's LINE.
    pub const fn line_alpha(self) -> f32 {
        match self {
            Emphasis::Anchor => 0.75,
            Emphasis::Key => 1.0,
            Emphasis::Minor => 0.85,
        }
    }
}

/// One level of a ratio scale.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Level {
    /// Position on the scale: 0 is the end of the move, 1 its start, beyond 1 an extension.
    pub ratio: f64,
    pub emphasis: Emphasis,
}

/// Alpha of the band filling the gap between two neighbouring levels.
///
/// Deliberately faint: eleven stacked bands at a readable alpha would bury the candles they are
/// drawn over, and the fill is there to group the levels, not to be read itself. Neighbouring
/// bands alternate between this and [`BAND_ALPHA_ALT`] — one alpha for all of them reads as a
/// single flat wash and groups nothing.
pub const BAND_ALPHA: f32 = 0.10;
/// Alpha of every other band, so two neighbours can be told apart.
pub const BAND_ALPHA_ALT: f32 = 0.04;

// A fill must stay behind the line that bounds it, and the two band alphas must differ or the
// bands merge into one wash. Checked here, at compile time, rather than by a test comparing two
// constants to each other.
const _: () = assert!(BAND_ALPHA_ALT > 0.0);
const _: () = assert!(BAND_ALPHA_ALT < BAND_ALPHA);
// And no level may be drawn invisible or brighter than the figure it belongs to.
const _: () = assert!(Emphasis::Anchor.line_alpha() >= 0.4 && Emphasis::Anchor.line_alpha() <= 1.0);
const _: () = assert!(Emphasis::Key.line_alpha() >= 0.4 && Emphasis::Key.line_alpha() <= 1.0);
const _: () = assert!(Emphasis::Minor.line_alpha() >= 0.4 && Emphasis::Minor.line_alpha() <= 1.0);

/// The default Fibonacci scale, in ascending ratio order — the set every charting package ships
/// with, and the one the reference terminal draws.
///
/// Order matters twice: bands fill the gap between neighbours, and the hit test walks the list.
pub const FIB_LEVELS: &[Level] = &[
    Level {
        ratio: 0.0,
        emphasis: Emphasis::Anchor,
    },
    Level {
        ratio: 0.236,
        emphasis: Emphasis::Minor,
    },
    Level {
        ratio: 0.382,
        emphasis: Emphasis::Key,
    },
    Level {
        ratio: 0.5,
        emphasis: Emphasis::Key,
    },
    Level {
        ratio: 0.618,
        emphasis: Emphasis::Key,
    },
    Level {
        ratio: 0.786,
        emphasis: Emphasis::Minor,
    },
    Level {
        ratio: 1.0,
        emphasis: Emphasis::Anchor,
    },
    Level {
        ratio: 1.618,
        emphasis: Emphasis::Key,
    },
    Level {
        ratio: 2.618,
        emphasis: Emphasis::Minor,
    },
    Level {
        ratio: 3.618,
        emphasis: Emphasis::Minor,
    },
    Level {
        ratio: 4.236,
        emphasis: Emphasis::Minor,
    },
];

/// Price of `ratio` on the scale spanned by a move from `start` to `end`.
///
/// The convention is the charting one and it is NOT symmetric: **0 sits at the END of the move
/// and 1 at its START**, so a retracement of a fall is read from the bottom up. Ratios above 1
/// continue past the start, away from the end — that is what makes 1.618 an extension target
/// rather than a point inside the move.
pub fn price_at(start: f64, end: f64, ratio: f64) -> f64 {
    end + (start - end) * ratio
}

#[cfg(test)]
mod tests;
