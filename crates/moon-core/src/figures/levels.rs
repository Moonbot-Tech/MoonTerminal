//! Ratio scales shared by the Fibonacci family.
//!
//! A scale is a list of [`Level`]s — a ratio, its own colour, and how loudly it is drawn. The
//! retracement, the trend-based extension, the Fibonacci channel and the time zones all read the
//! SAME list, so a set edited once is edited for all of them.

/// How loudly a level is drawn, on top of its own hue.
///
/// Hue says WHICH level this is; emphasis says how loudly to say it, so the golden group still
/// reads as the group a trader watches even though every level now carries a colour.
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
    /// Alpha multiplier applied to this level's own colour for its LINE.
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
    /// The level's own hue, `RGB`, as every charting package colours a ratio scale: the reader
    /// recognises 0.618 by its teal and 1.618 by its blue without looking at the number.
    ///
    /// Deliberately NOT from the theme palette. A theme colour means "this is a warning", "this is
    /// the accent"; these mean "this is the 0.618", and they must stay the same in the light theme,
    /// in the dark theme and beside a screenshot from any other terminal. They are also the reason
    /// this crate needs no palette dependency: a figure's colours have always been raw RGBA here.
    pub color: [u8; 3],
}

// Emphasis only DIMS a level's own hue; it may never make one invisible, nor raise it past full.
const _: () = assert!(Emphasis::Anchor.line_alpha() >= 0.4 && Emphasis::Anchor.line_alpha() <= 1.0);
const _: () = assert!(Emphasis::Key.line_alpha() >= 0.4 && Emphasis::Key.line_alpha() <= 1.0);
const _: () = assert!(Emphasis::Minor.line_alpha() >= 0.4 && Emphasis::Minor.line_alpha() <= 1.0);

/// The default Fibonacci scale, in ascending ratio order — the set every charting package ships
/// with, and the one the reference terminal draws.
///
/// Order matters twice: bands fill the gap between neighbours, and the hit test walks the list.
///
/// The hues are the ones a trader already reads elsewhere: grey for the move's own two ends, then
/// warm-to-cool ACROSS the retracement — red at 0.236 through orange, green and teal to cyan at
/// 0.786 — and past the move red again, purple and pink for the far extensions, with blue marking
/// the first one at 1.618. Anyone who has looked at a retracement before can name a level by its
/// colour without re-learning the scale.
pub const FIB_LEVELS: &[Level] = &[
    Level {
        ratio: 0.0,
        emphasis: Emphasis::Anchor,
        color: GREY,
    },
    Level {
        ratio: 0.236,
        emphasis: Emphasis::Minor,
        color: RED,
    },
    Level {
        ratio: 0.382,
        emphasis: Emphasis::Key,
        color: ORANGE,
    },
    Level {
        ratio: 0.5,
        emphasis: Emphasis::Key,
        color: GREEN,
    },
    Level {
        ratio: 0.618,
        emphasis: Emphasis::Key,
        color: TEAL,
    },
    Level {
        ratio: 0.786,
        emphasis: Emphasis::Minor,
        color: CYAN,
    },
    Level {
        ratio: 1.0,
        emphasis: Emphasis::Anchor,
        color: GREY,
    },
    Level {
        ratio: 1.618,
        emphasis: Emphasis::Key,
        color: BLUE,
    },
    Level {
        ratio: 2.618,
        emphasis: Emphasis::Minor,
        color: RED,
    },
    Level {
        ratio: 3.618,
        emphasis: Emphasis::Minor,
        color: PURPLE,
    },
    Level {
        ratio: 4.236,
        emphasis: Emphasis::Minor,
        color: PINK,
    },
];

// Hues of the scale. Named rather than inlined: 0 and 1 must be the SAME grey, and 0.236 and 2.618
// the same red, or the pairing that makes the scale readable is lost to a typo.
const GREY: [u8; 3] = [0x78, 0x7B, 0x86];
const RED: [u8; 3] = [0xF2, 0x36, 0x45];
const ORANGE: [u8; 3] = [0xFF, 0x98, 0x00];
const GREEN: [u8; 3] = [0x4C, 0xAF, 0x50];
const TEAL: [u8; 3] = [0x08, 0x99, 0x81];
const CYAN: [u8; 3] = [0x00, 0xBC, 0xD4];
const BLUE: [u8; 3] = [0x29, 0x62, 0xFF];
const PURPLE: [u8; 3] = [0x9C, 0x27, 0xB0];
const PINK: [u8; 3] = [0xE9, 0x1E, 0x63];

/// One hue that STANDS FOR the whole scale, for a picker cell that cannot show eleven.
///
/// The golden ratio's, because that is the level the scale is drawn for. Returned rather than
/// re-typed in the UI so the swatch cannot drift from the levels it promises.
pub fn scale_swatch() -> [u8; 3] {
    FIB_LEVELS
        .iter()
        .find(|l| l.ratio == 0.618)
        .map_or(TEAL, |l| l.color)
}

/// A level's ratio as it is READ: `0.618`, `1`, `4.236` — three decimals with the trailing zeros
/// dropped, which is how every charting package writes them and how a trader names them out loud.
pub fn fmt_ratio(ratio: f64) -> String {
    // Anything the third decimal cannot see is zero, sign included. `+ 0.0` alone folds only a
    // literal -0.0, and a ratio DERIVED from prices — Moonbot's fibo recovers its ratios by
    // division — lands on values like -2.2e-6, which would print as "-0".
    let ratio = if ratio.abs() < 5e-4 { 0.0 } else { ratio };
    let s = format!("{:.3}", ratio);
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

/// The ratios a Fibonacci level is NAMED by, across the sets charting packages offer.
///
/// Used only to name a ratio that was RECOVERED by division — see [`snap_ratio`]. Our own scale
/// does not consult it: there the ratio is the input, not a measurement.
/// The widest gap that still counts as "the same level, measured imprecisely".
///
/// Bounds [`snap_ratio`]'s error bar. Chosen against the values a user can deliberately place: a
/// quarter retracement sits 0.014 from 0.236, so anything at or above that would rename it.
const MAX_SNAP: f64 = 0.01;

const NAMED_RATIOS: &[f64] = &[
    0.0, 0.236, 0.382, 0.5, 0.618, 0.786, 1.0, 1.236, 1.272, 1.414, 1.618, 2.0, 2.618, 3.618, 4.236,
];

/// The hue a ratio is drawn in, for a scale whose levels were RECOVERED rather than chosen.
///
/// Looks the ratio up in [`FIB_LEVELS`], so a level named 0.618 is the same teal whether our own
/// tool placed it or Moonbot's object arrived carrying its price. `None` for a ratio our scale has
/// no entry for — Moonbot's set is a user setting on that side and need not match ours.
pub fn color_of_ratio(ratio: f64) -> Option<[u8; 3]> {
    FIB_LEVELS
        .iter()
        .find(|l| (l.ratio - ratio).abs() < 1e-6)
        .map(|l| l.color)
}

/// Names a recovered ratio, snapping it to a canonical one when the measurement cannot tell them
/// apart.
///
/// Moonbot stores a level as a PRICE computed in single precision, so recovering its ratio divides
/// away an `f32` rounding error whose size is `ulp(price) / |span|`. On a wide move that is
/// invisible; on a narrow one it is not — a level Moonbot calls 0.236 recovers as 0.234375 when the
/// span is a dollar on a $60 000 price, and a label reading "0.234" names a level that does not
/// exist. Snapping inside the error bar reports the value the format cannot distinguish from;
/// outside it, the measurement stands and is shown as measured.
pub fn snap_ratio(ratio: f64, price: f64, span: f64) -> f64 {
    let span = span.abs();
    if !ratio.is_finite() || !price.is_finite() || span == 0.0 || !span.is_finite() {
        return ratio;
    }
    // The error the recovery itself can carry: four `f32` ulps of the price, spread over the span.
    // Floored so a wide move, where it underflows to nothing, still absorbs plain f64 noise; and
    // CAPPED, because past a point the bar grows wide enough to swallow a level the user really did
    // place. The cap is the largest error worth calling "the format cannot tell these apart", not
    // the largest that keeps two names apart — half the table's smallest gap (0.018) would still
    // rename a deliberate 0.25 to 0.236.
    let bar = (4.0 * f32::EPSILON as f64 * price.abs() / span).clamp(1e-9, MAX_SNAP);
    // NEAREST, not the first inside the bar: the table is ordered by value, and 1.236 precedes
    // 1.272, so "first" renamed an exactly drawn 1.272 to its neighbour.
    NAMED_RATIOS
        .iter()
        .copied()
        .filter(|named| (ratio - named).abs() <= bar)
        .min_by(|a, b| {
            (ratio - a)
                .abs()
                .partial_cmp(&(ratio - b).abs())
                .unwrap_or(std::cmp::Ordering::Equal)
        })
        .unwrap_or(ratio)
}

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
