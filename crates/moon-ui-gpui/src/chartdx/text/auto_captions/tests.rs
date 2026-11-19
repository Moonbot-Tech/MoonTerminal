//! Placement of auto-line captions: on the drawn side, and clear of the order captions.
//!
//! Explicit imports, never `use super::*`: the text module re-exports `gpui::*`, whose own `test`
//! would shadow the built-in `#[test]` attribute (CONTRIBUTING.md).

use moon_chart::auto_figures::draw::drawn_side;
use moon_chart::auto_figures::{Found, FoundShape, LineTag, PairSide, Seg};
use moon_core::config::auto_figures::{CaptionPlace, Extend};

use super::{
    CaptionCand, CaptionMap, CaptionSpot, ClearBox, SAME_PRICE_PX, caption_spot, clear_of,
    group_same_price, line_y_at,
};
use crate::chartdx::text::OrderCaption;
use crate::chartdx::text::{READOUT_PAD_X, READOUT_PAD_Y};

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
            line_y: 180.0,
            drawn: spot.drawn,
        }
    );
}

/// A caption that slid left along a SLOPED side is read at its new place, not carried over at the
/// old height, and never left of the side's drawn start.
///
/// By hand, under a view from 90: the triangle's top falls 110 → 106 over 1..3 s, 2 units per
/// second; at x 500 (5 s) it is at 102, y 500 − (102 − 90) · 20 = 260; at its drawn end, x 800
/// (8 s, the apex), 96 → 380. A slide to x 50 (0.5 s) is left of the drawn start (x 100, 1 s) and
/// reads the start: 110 → y 100.
///
/// Breakage: the slid caption kept at its first height (review finding: it drifted off a sloped
/// line by slope · dx), or read left of where its line is drawn.
#[test]
fn a_slid_caption_follows_its_sloped_line() {
    let triangle = formation(
        LineTag::Triangle,
        (1_000.0, 110.0, 3_000.0, 106.0),
        (2_000.0, 90.0, 4_000.0, 92.0),
    );
    let drawn = [100.0, 800.0];
    let y = |x| line_y_at(&map(90.0), &triangle.shape, Some(PairSide::Upper), x, drawn);
    assert_eq!(y(800.0), Some(380.0));
    assert_eq!(y(500.0), Some(260.0));
    assert_eq!(y(50.0), Some(100.0));
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

/// An order caption in the price column, its padded box ending at `right` and `w` wide, its text
/// at `top`, 12 px tall.
fn order(right: f32, w: f32, top: Option<f32>) -> OrderCaption {
    OrderCaption {
        source: 0,
        book: false,
        uid: 1,
        secondary: false,
        interacting: false,
        pinned: false,
        distance: 0.0,
        top: top.map(|t| t - READOUT_PAD_Y),
        x: [right - w - 2.0 * READOUT_PAD_X, right],
        height: 12.0 + 2.0 * READOUT_PAD_Y,
    }
}

/// [`clear_of`] against order captions, through the conversion the text pass makes of them.
fn clear_of_orders(
    x: f32,
    w: f32,
    top: f32,
    h: f32,
    orders: &[OrderCaption],
    min_left: f32,
) -> Option<f32> {
    let boxes: Vec<ClearBox> = orders.iter().filter_map(ClearBox::of_order).collect();
    clear_of(x, w, top, h, &boxes, min_left)
}

/// A right-anchored caption keeps its place with nothing beside it, and slides LEFT along its line
/// past every order caption it would cover — one after another, as each slide meets the next —
/// never up or down; an order caption the layout hid, or one in another row, is not in the way;
/// with no room left of the column it is dropped rather than drawn over the orders.
///
/// By hand: an 80-wide caption ending at 990 at top 100 (12 tall), its padded box 905..995. An
/// order 60 wide ends its box at 995, so starts it at 925: the caption must end at 925 − 5 = 920.
/// A second order in the same row, its box 840..900, is clear of the caption's first place but
/// not of its second (835..925): it pushes the caption on to 840 − 5 = 835 — whichever order the
/// layout left the two in. An order at top 130 does not share the row; a hidden one is ignored.
/// With the plot starting at 800, an 80-wide caption cannot end at 835 (its text would start at
/// 755): dropped.
///
/// Breakage: the auto caption drawn over the order numbers (13.3), moved off its line, a hidden
/// order caption still pushing it, or a loop that stops after the first order it meets.
#[test]
fn a_right_end_caption_slides_left_of_the_order_captions() {
    let (w, h, top) = (80.0, 12.0, 100.0);
    assert_eq!(clear_of_orders(990.0, w, top, h, &[], 0.0), Some(990.0));
    let first = order(995.0, 60.0, Some(100.0));
    assert_eq!(
        clear_of_orders(990.0, w, top, h, std::slice::from_ref(&first), 0.0),
        Some(925.0 - READOUT_PAD_X)
    );
    let beside = order(900.0, 50.0, Some(104.0));
    assert_eq!(beside.x[0], 840.0);
    let both = [first.clone(), beside.clone()];
    assert_eq!(
        clear_of_orders(990.0, w, top, h, &both, 0.0),
        Some(840.0 - READOUT_PAD_X)
    );
    let swapped = [beside, first.clone()];
    assert_eq!(
        clear_of_orders(990.0, w, top, h, &swapped, 0.0),
        Some(840.0 - READOUT_PAD_X)
    );
    let elsewhere = [order(995.0, 60.0, Some(130.0)), order(995.0, 60.0, None)];
    assert_eq!(
        clear_of_orders(990.0, w, top, h, &elsewhere, 0.0),
        Some(990.0)
    );
    assert_eq!(clear_of_orders(990.0, w, top, h, &both, 800.0), None);
    // An order caption far to the right in the same row does not touch a caption that needed no
    // slide, however close its text sits to `min_left`: it stays where it was drawn before.
    let far = [order(1_200.0, 60.0, Some(100.0))];
    assert_eq!(clear_of_orders(990.0, w, top, h, &far, 950.0), Some(990.0));
}

/// A right-end caption candidate of `row` whose line sits at `y`.
fn cand(row: usize, y: f32, place: CaptionPlace) -> CaptionCand {
    CaptionCand {
        li: 0,
        row,
        place,
        spot: CaptionSpot {
            x: 995.0,
            ax: 1.0,
            line_y: y,
            drawn: [0.0, 1_000.0],
        },
        shown: true,
        lead: usize::MAX,
    }
}

/// Lines of different rows at one price — within a pixel and a half on screen — share ONE caption,
/// led by the first of them; a line farther off, a line of a row the caption already holds, and a
/// line captioned at another place each keep a caption of their own.
///
/// By hand: rows 0 and 1 at 100 and 101 (1 px) share row 0's caption; row 2 at 101.5 (1.5 px from
/// the lead) joins it too; row 0's second line at 100 does not (its row is in already) and leads
/// its own; row 3 at 102 (2 px) leads its own; row 4 at 100 captioned at the left end leads its
/// own.
///
/// Breakage: two captions printed over each other at one price (the screenshot: a double top and
/// the day's high at 5.3000), a tolerance that merges neighbouring levels, or one row's two lines
/// folded into one caption.
#[test]
fn lines_of_different_rows_at_one_price_share_one_caption() {
    let mut cands = [
        cand(0, 100.0, CaptionPlace::RightEnd),
        cand(1, 101.0, CaptionPlace::RightEnd),
        cand(2, 100.0 + SAME_PRICE_PX, CaptionPlace::RightEnd),
        cand(0, 100.0, CaptionPlace::RightEnd),
        cand(3, 102.0, CaptionPlace::RightEnd),
        cand(4, 100.0, CaptionPlace::LeftEnd),
    ];
    group_same_price(&mut cands);
    let leads: Vec<usize> = cands.iter().map(|c| c.lead).collect();
    assert_eq!(leads, [0, 0, 0, 3, 4, 5]);
}

/// The shared caption prints every name in the rows' order, then the first line's other parts;
/// with no name among them it is not merged, and the first line's own caption stands.
///
/// Breakage: the price printed once per row, the names out of row order, or an empty merged string
/// drawn in place of the lead's caption.
#[test]
fn a_shared_caption_joins_the_names_then_the_rest() {
    use crate::chartdx::auto_figures_sync::caption::merged_caption;
    let mut out = String::from("stale");
    assert!(merged_caption(
        &mut out,
        ["Double top", "Day high"].into_iter(),
        Some("5.3000"),
    ));
    assert_eq!(out, "Double top · Day high · 5.3000");
    assert!(merged_caption(
        &mut out,
        ["Double top", "Day high"].into_iter(),
        None
    ));
    assert_eq!(out, "Double top · Day high");
    assert!(!merged_caption(
        &mut out,
        std::iter::empty(),
        Some("5.3000")
    ));
    assert!(out.is_empty());
}

/// A caption whose box meets an auto caption already drawn at another price slides LEFT along its
/// line past it, exactly as it does past an order caption; one in another row stays.
///
/// By hand: the first caption, 80 wide, right-anchored at 990 with its text top at 100, has the
/// box 905..995 (padded by `READOUT_PAD_X`). A 60-wide caption ending at 990 with its top at 106
/// shares those rows: it must end at 905 − 5 = 900. At top 130 it is in another row and keeps 990.
///
/// Breakage: auto captions of neighbouring prices drawn over each other, or the drawn box built
/// from the wrong side of its anchor.
#[test]
fn a_caption_slides_left_of_an_auto_caption_already_drawn() {
    let (w, h) = (60.0, 12.0);
    let drawn = [ClearBox::drawn(990.0, 1.0, 80.0, 100.0, h)];
    assert_eq!(drawn[0].x, [905.0, 995.0]);
    assert_eq!(
        clear_of(990.0, w, 106.0, h, &drawn, 0.0),
        Some(905.0 - READOUT_PAD_X)
    );
    assert_eq!(clear_of(990.0, w, 130.0, h, &drawn, 0.0), Some(990.0));
    // A left-anchored caption's box lies to the right of its anchor.
    assert_eq!(
        ClearBox::drawn(10.0, 0.0, 80.0, 100.0, h).x,
        [10.0 - READOUT_PAD_X, 90.0 + READOUT_PAD_X]
    );
}
