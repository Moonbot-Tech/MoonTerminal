//! Placement of auto-line captions on the drawn side.
//!
//! Explicit imports, never `use super::*`: the text module re-exports `gpui::*`, whose own `test`
//! would shadow the built-in `#[test]` attribute (CONTRIBUTING.md).

use moon_chart::auto_figures::draw::drawn_side;
use moon_chart::auto_figures::{Found, FoundShape, LineTag, PairSide, Seg};
use moon_core::config::auto_figures::{CaptionPlace, Extend};

use super::{CaptionMap, CaptionSpot, caption_spot};
use crate::chartdx::text::READOUT_PAD_X;

/// A 1000 × 500 plot at the origin, one physical pixel per logical one: 0.1 px per ms from the
/// epoch (the plot shows 0..10 s), 20 px per unit of price from `price0` at the bottom edge.
fn map(price0: f32) -> CaptionMap {
    CaptionMap {
        plot: [0.0, 0.0, 1000.0, 500.0],
        sf: 1.0,
        epoch_ms: 0.0,
        view_time0: 0.0,
        time_to_px: 0.1,
        view_price0: price0,
        price_to_px: 20.0,
    }
}

/// A two-line formation of `tag` with the sides `upper` and `lower`, `(t0, p0, t1, p1)` each.
fn formation(tag: LineTag, upper: (f64, f64, f64, f64), lower: (f64, f64, f64, f64)) -> Found {
    let seg = |(t0_ms, p0, t1_ms, p1)| Seg {
        t0_ms,
        p0,
        t1_ms,
        p1,
    };
    Found {
        shape: FoundShape::Pair {
            upper: seg(upper),
            lower: seg(lower),
        },
        touches: Vec::new(),
        strength: 1.0,
        first_ms: upper.0.min(lower.0),
        last_ms: upper.2.max(lower.2),
        broken: None,
        touch_volume: 0.0,
        tolerance_frac: 0.0,
        tag: Some(tag),
    }
}

/// A triangle's right-end caption sits at the END of its drawn side, at the price there.
///
/// By hand: the top falls 110 → 106 over 1..3 s, the bottom rises 90 → 92 over 2..4 s; they meet
/// at 8 s at 96 — x 800, y 500 − (96 − 80) · 20 = 180. Drawn against a reference of 20 s, the
/// side's span ends at 8 s, so the caption is right-aligned 5 px left of x 800 at y 180. With the
/// row's `RightEdge` span instead, it would sit at the plot edge (x 995) at the top side's
/// extrapolated 92 (y 240).
///
/// Breakage: a capped side's caption placed by the row's `extend` alone (review finding F1).
#[test]
fn a_right_end_caption_sits_at_the_end_of_the_drawn_side() {
    let triangle = formation(
        LineTag::Triangle,
        (1_000.0, 110.0, 3_000.0, 106.0),
        (2_000.0, 90.0, 4_000.0, 92.0),
    );
    let span = drawn_side(&triangle, PairSide::Upper, Extend::RightEdge, 20_000.0)
        .unwrap()
        .span();
    let spot = caption_spot(
        &map(80.0),
        &triangle.shape,
        Some(PairSide::Upper),
        span,
        CaptionPlace::RightEnd,
        990.0,
    )
    .expect("the end is on screen");
    assert!((spot.x - (800.0 - READOUT_PAD_X)).abs() < 0.01, "{spot:?}");
    assert_eq!((spot.ax, spot.line_y), (1.0, 180.0));
    // At the price axis: the label column, the height still read at the drawn end.
    let spot = caption_spot(
        &map(80.0),
        &triangle.shape,
        Some(PairSide::Upper),
        span,
        CaptionPlace::AtPriceAxis,
        990.0,
    )
    .unwrap();
    assert_eq!(
        spot,
        CaptionSpot {
            x: 990.0,
            ax: 1.0,
            line_y: 180.0
        }
    );
}

/// A formation's caption is placed on the side it was kept by.
///
/// By hand: a 100..110 range under a view from 95 — the lower side at y 500 − 5 · 20 = 400, the
/// upper at 200. Kept by its lower side, the caption sits at 400.
///
/// Breakage: the caption pass reading the pair through `price_at` (its upper side) instead of
/// `side_price_at(.., side)` — the caption lands on the far side of the range.
#[test]
fn a_formation_caption_sits_on_its_kept_side() {
    let range = formation(
        LineTag::Channel,
        (1_000.0, 110.0, 3_000.0, 110.0),
        (1_000.0, 100.0, 3_000.0, 100.0),
    );
    let at = |side| {
        caption_spot(
            &map(95.0),
            &range.shape,
            side,
            (1_000.0, f64::INFINITY),
            CaptionPlace::LeftEnd,
            990.0,
        )
        .map(|s| s.line_y)
    };
    assert_eq!(at(Some(PairSide::Lower)), Some(400.0));
    assert_eq!(at(Some(PairSide::Upper)), Some(200.0));
}
