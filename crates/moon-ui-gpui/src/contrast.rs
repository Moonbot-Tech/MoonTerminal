//! WCAG contrast arithmetic, shared by everything that paints a colour on a ground it did not pick.
//!
//! Three places used to carry their own copy of this: the screenshot header strip
//! (`panels/chart/shot/ink.rs`), the chart arrival border and MoonUI's own token tests. A user
//! picks a core's colour once, in Settings, and the terminal then draws it on grounds the user
//! ALSO configures — `ChartTheme.bg` ships white and near-black and accepts anything in between —
//! so no stored colour can promise to read everywhere. What can be promised is a derivation: keep
//! the colour where it reads, and where it does not, move it just far enough that it does.
//!
//! # Lightness, not a blend
//!
//! [`lift_to_contrast`] walks the colour's HSL lightness toward the contrasting pole and leaves hue
//! and saturation alone. Blending toward white in RGB (what [`mix`] does) would ALSO reach the
//! floor, but it desaturates on the way: a dark navy lifted for a dark theme comes out grey-lilac,
//! and the whole point of painting a core's colour is that the reader recognises it. Darkening
//! is the same either way — a straight line to black keeps saturation — so the one rule covers
//! both themes without a special case.
//!
//! Deliberately free of GPUI drawing types: only `Hsla`'s arithmetic is borrowed, so every test
//! here runs on every platform.

use crate::design::{hsla_to_rgb8, rgb_bytes_to_hsla};
use gpui::Hsla;

const WHITE: [u8; 3] = [255, 255, 255];
const BLACK: [u8; 3] = [0, 0, 0];

/// Lightness steps [`lift_to_contrast`] may take before it settles on the pole itself.
///
/// A bounded walk rather than a solve, for the reason `ink.rs` gives: it is total on every input,
/// and each step it can produce is a value a test can name. Fifty steps of two percent keep the
/// first readable candidate within a hair of the floor, so a colour that barely fails stays as
/// close to what the user picked as the floor allows.
const LIFT_STEPS: u32 = 50;

/// Return `color` when it clears `floor` against `ground`, otherwise the nearest lightness that does.
///
/// A colour that already reads is returned byte for byte — the promise every consumer relies on is
/// that a readable pick is never touched. One that does not read keeps its hue and saturation and
/// moves toward [`pole`] of `ground`: darker on a light ground, lighter on a dark one, stopping at
/// the first lightness that reaches `floor`. The walk ends at pure black or white, which by the
/// identity documented on [`pole`] reaches at least 4.58:1 on ANY ground, so a floor at or below
/// that is always met and a higher one degrades to the pole rather than to nothing.
///
/// Args:
///     color: The colour as picked.
///     ground: What it will be drawn on.
///     floor: The WCAG contrast ratio to reach.
///
/// Returns:
///     A colour at or above `floor` against `ground`, sharing `color`'s hue.
pub(crate) fn lift_to_contrast(color: [u8; 3], ground: [u8; 3], floor: f64) -> [u8; 3] {
    if contrast_ratio(color, ground) >= floor {
        return color;
    }
    let toward = pole(ground);
    let target_l = if toward == WHITE { 1.0 } else { 0.0 };
    let base = rgb_bytes_to_hsla(color);
    for step in 1..=LIFT_STEPS {
        let t = step as f32 / LIFT_STEPS as f32;
        let l = base.l + (target_l - base.l) * t;
        let candidate = hsla_to_rgb8(Hsla { l, ..base });
        if contrast_ratio(candidate, ground) >= floor {
            return candidate;
        }
    }
    toward
}

/// The pole that actually contrasts MOST with a ground: whichever of white and black wins.
///
/// **Asked rather than assumed, and that is not pedantry — the obvious threshold is wrong.**
/// "Light ground, go black; dark ground, go white" invites a `relative_luminance(bg) < 0.5` test,
/// and the WCAG crossover is not at `0.5`: contrast against white is `1.05 / (L + 0.05)` and
/// against black is `(L + 0.05) / 0.05`, which meet at `L ~ 0.179`. Everything between there and
/// `0.5` is a ground where the threshold picks the pole that contrasts LESS.
///
/// A plain mid-grey `[128,128,128]` is exactly such a ground, and it is worth following all the
/// way through because the margin is thin. Its luminance is `0.216`, so it reaches `3.95:1`
/// against white and `5.32:1` against black — black wins, and the band `ink.rs` then lifts
/// toward black is `[120,120,120]`, on which text can reach `4.76:1`: over the floor. Under the
/// `< 0.5` threshold the pole would have been WHITE, the band `[136,136,136]`, and text pushed
/// toward white reaches `3.54:1` — under the floor, on a theme a user is perfectly entitled to
/// configure. Comparing the two ratios has no threshold to get wrong.
///
/// (Every such pair multiplies to exactly `21`, since `1.05/(L+0.05) * (L+0.05)/0.05` cancels.
/// That identity is the cheapest way to check any number quoted above, and it is why the two
/// ratios for one ground can never both be small.)
///
/// A CHANNEL MEAN is wrong for a second, independent reason: it disagrees with luminance on
/// saturated grounds — a strong blue has a low mean and a lower luminance still, while a strong
/// yellow's mean says "middling" where the eye says "bright". The eye is what this picture is for.
///
/// The consequence that makes every floor built on this honest: the WORST ground possible is the
/// one at the crossover, and even there the winning pole still reaches about **4.58:1** — above
/// the readable floor. So a walk that ends at this pole is never illegible, whatever theme the
/// user configured.
///
/// Args:
///     against: The ground.
///
/// Returns:
///     Pure white or pure black, whichever contrasts more. Ties go to white, arbitrarily but
///     consistently — at a tie the two are equally readable by construction.
pub(crate) fn pole(against: [u8; 3]) -> [u8; 3] {
    if contrast_ratio(WHITE, against) >= contrast_ratio(BLACK, against) {
        WHITE
    } else {
        BLACK
    }
}

/// Blend `a` toward `b`.
///
/// Args:
///     a: The colour at `t == 0.0`.
///     b: The colour at `t == 1.0`.
///     t: How far along, clamped to `0.0..=1.0`.
///
/// Returns:
///     The blended colour, rounded per channel. Exact at both endpoints.
pub(crate) fn mix(a: [u8; 3], b: [u8; 3], t: f64) -> [u8; 3] {
    let t = t.clamp(0.0, 1.0);
    let channel = |from: u8, to: u8| -> u8 {
        let from = f64::from(from);
        (from + (f64::from(to) - from) * t)
            .round()
            .clamp(0.0, 255.0) as u8
    };
    [
        channel(a[0], b[0]),
        channel(a[1], b[1]),
        channel(a[2], b[2]),
    ]
}

/// WCAG relative luminance of an sRGB colour.
///
/// The gamma expansion is not optional decoration: averaging the raw 0-255 channels calls
/// `[0,255,0]` and `[128,128,128]` similarly bright, and they are nothing alike. This is the curve
/// every contrast floor in the crate is DEFINED against, so it has to be this one and not an
/// approximation of it. The knee is WCAG 2.x's `0.03928`, the same one MoonUI's token tests use,
/// rather than the sRGB spec's `0.04045`; the two agree to well within a rounding step for any
/// 8-bit colour, and one number in one place is the point of this module.
///
/// Args:
///     rgb: The colour.
///
/// Returns:
///     Luminance in `0.0..=1.0`.
pub(crate) fn relative_luminance(rgb: [u8; 3]) -> f64 {
    let expand = |channel: u8| -> f64 {
        let c = f64::from(channel) / 255.0;
        if c <= 0.03928 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * expand(rgb[0]) + 0.7152 * expand(rgb[1]) + 0.0722 * expand(rgb[2])
}

/// WCAG contrast ratio between two colours.
///
/// Symmetric by construction — it orders the two luminances itself rather than trusting the caller
/// to pass foreground first — because every caller compares a colour against a ground and none of
/// them should have to remember which way round that goes.
///
/// Args:
///     a: One colour.
///     b: The other.
///
/// Returns:
///     A ratio in `1.0..=21.0`: `1.0` for two identical colours, `21.0` for black against white.
pub(crate) fn contrast_ratio(a: [u8; 3], b: [u8; 3]) -> f64 {
    let first = relative_luminance(a);
    let second = relative_luminance(b);
    let (lighter, darker) = if first >= second {
        (first, second)
    } else {
        (second, first)
    };
    (lighter + 0.05) / (darker + 0.05)
}

#[cfg(test)]
mod tests;
