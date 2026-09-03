//! Unit tests for the pane layout every chart surface shares.

use super::*;
use crate::persistence::chart_persist::PriceAxisPos;

const PANE: Rect = Rect {
    x: 40.0,
    y: 12.0,
    w: 900.0,
    h: 400.0,
};

fn areas(pane: Rect, broom: bool, book: bool, axis: PriceAxisPos) -> PaneAreas {
    pane_layout(pane, broom, book, true, axis, 1.0)
}

/// Both areas stay inside the pane and neither overlaps the other, whatever the flags — the
/// property every hit test depends on, checked across the combinations rather than per case.
#[test]
fn the_two_areas_tile_the_pane_without_overlapping() {
    for broom in [false, true] {
        for book in [false, true] {
            for axis in [PriceAxisPos::Left, PriceAxisPos::Right, PriceAxisPos::Hide] {
                let a = areas(PANE, broom, book, axis);
                let case = format!("broom={broom} book={book} axis={axis:?}");
                assert!(a.plot.x >= PANE.x, "plot starts left of the pane ({case})");
                assert!(
                    a.plot.x + a.plot.w <= PANE.x + PANE.w,
                    "plot runs past the pane ({case})"
                );
                assert!(a.glass.x >= PANE.x, "book starts left of the pane ({case})");
                assert!(
                    a.glass.x + a.glass.w <= PANE.x + PANE.w,
                    "book runs past the pane ({case})"
                );
                // A plot floored at one pixel is the collapsed one broom mode leaves behind; it
                // sits inside the book by construction and has nothing to overlap.
                if a.glass.w > 0.0 && a.plot.w > 1.0 {
                    assert!(
                        a.plot.x + a.plot.w <= a.glass.x || a.glass.x + a.glass.w <= a.plot.x,
                        "plot and book overlap ({case})"
                    );
                }
            }
        }
    }
}

/// The ordinary pane: an axis gutter on the left, the book flush against the right edge, and the
/// plot filling everything between them.
#[test]
fn a_left_axis_leaves_the_plot_between_its_gutter_and_the_book() {
    let a = areas(PANE, false, true, PriceAxisPos::Left);
    assert!(matches!(a.axis_pos, PriceAxisPos::Left));
    assert_eq!(a.glass.w, moon_chart::GLASS_ZONE_PX);
    assert_eq!(a.glass.x + a.glass.w, PANE.x + PANE.w);
    assert_eq!(a.plot.x, PANE.x + moon_chart::PRICE_AXIS_W);
    assert_eq!(a.plot.x + a.plot.w, a.glass.x);
}

/// A right-side axis puts its gutter OUTBOARD of the book, so the book is not flush right. Measuring
/// the book back from the pane's right edge instead — which two hit tests used to do — left its
/// left part answering as chart.
#[test]
fn a_right_axis_sits_outboard_of_the_book() {
    let a = areas(PANE, false, true, PriceAxisPos::Right);
    assert_eq!(a.plot.x, PANE.x);
    assert_eq!(a.glass.x, a.plot.x + a.plot.w);
    assert_eq!(
        a.glass.x + a.glass.w + moon_chart::PRICE_AXIS_W,
        PANE.x + PANE.w
    );
}

/// A pane too narrow to seat a full book beside a usable plot gets a narrower book rather than no
/// plot at all.
#[test]
fn a_cramped_pane_narrows_the_book_and_keeps_a_plot() {
    let narrow = Rect {
        w: moon_chart::PRICE_AXIS_W + moon_chart::GLASS_ZONE_PX * 2.5,
        ..PANE
    };
    let a = areas(narrow, false, true, PriceAxisPos::Left);
    assert!(a.glass.w < moon_chart::GLASS_ZONE_PX && a.glass.w > 0.0);
    assert!(a.plot.w > a.glass.w);
}

#[test]
fn a_disabled_book_gives_its_width_back_to_the_plot() {
    let a = areas(PANE, false, false, PriceAxisPos::Left);
    assert_eq!(a.glass.w, 0.0);
    assert_eq!(a.plot.x + a.plot.w, PANE.x + PANE.w);
}

/// The case the panel's hit testing exists to agree with: in broom mode the book IS the pane, edge
/// to edge, so a click anywhere on it is a book click and there is no plot left to pan.
#[test]
fn broom_mode_hands_the_whole_pane_to_the_book() {
    let a = areas(PANE, true, true, PriceAxisPos::Left);
    assert!(matches!(a.axis_pos, PriceAxisPos::Hide));
    assert_eq!(a.glass.x, PANE.x);
    assert_eq!(a.glass.w, PANE.w);
    assert_eq!(a.plot.w, 1.0);
}

/// Broom mode draws the book even with the window's own Order Book toggle cleared — `ChartDataState`
/// sets `orderbook_on = orderbook_enabled || orderbook_only` — so the layout must not take the
/// disabled branch and hand the pane to a plot nobody draws.
#[test]
fn broom_mode_outranks_a_cleared_order_book_toggle() {
    let a = areas(PANE, true, false, PriceAxisPos::Left);
    assert_eq!(a.glass.x, PANE.x);
    assert_eq!(a.glass.w, PANE.w);
}

/// A right-side axis is hidden by broom mode like any other, so the book reaches both edges instead
/// of leaving a gutter nothing draws into.
#[test]
fn broom_mode_hides_a_right_side_axis_too() {
    let a = areas(PANE, true, true, PriceAxisPos::Right);
    assert!(matches!(a.axis_pos, PriceAxisPos::Hide));
    assert_eq!(a.glass.w, PANE.w);
}

/// The time axis reserves its gutter under BOTH areas, and hiding it gives that height back —
/// the vertical half of the same answer, so a caller cannot take one half from here and derive the
/// other itself.
#[test]
fn the_time_axis_gutter_shortens_both_areas() {
    let with = pane_layout(PANE, false, true, true, PriceAxisPos::Left, 1.0);
    let without = pane_layout(PANE, false, true, false, PriceAxisPos::Left, 1.0);
    assert_eq!(with.plot.h, with.glass.h);
    assert_eq!(without.plot.h, PANE.h);
    assert_eq!(PANE.h - with.plot.h, moon_chart::TIME_AXIS_H);
}

/// Both areas scale with the display: at 2x device pixels the reserved gutters double, which is what
/// keeps a hit test in device pixels agreeing with what was drawn on a HiDPI screen.
#[test]
fn the_reserved_gutters_follow_the_pixel_scale() {
    let one = pane_layout(PANE, false, true, true, PriceAxisPos::Left, 1.0);
    let two = pane_layout(PANE, false, true, true, PriceAxisPos::Left, 2.0);
    assert_eq!(two.plot.x - PANE.x, (one.plot.x - PANE.x) * 2.0);
    assert_eq!(PANE.h - two.plot.h, (PANE.h - one.plot.h) * 2.0);
}

/// An unpresented slot reports a width of zero. Every number still has to come back finite, because
/// hit tests run against this layout before the first frame is drawn — and a broom pane's book has
/// to start at the pane rather than beside a gutter that mode does not reserve.
///
/// The plot is deliberately NOT asserted to be inside such a pane: a left-side axis reserves its
/// gutter regardless, which puts the plot past the right edge of a zero-width pane. Harmless,
/// because a pane of no width holds no pointer, and the first real frame replaces these numbers.
#[test]
fn an_unpresented_slot_stays_finite() {
    for broom in [false, true] {
        let a = areas(Rect { w: 0.0, ..PANE }, broom, true, PriceAxisPos::Left);
        for v in [
            a.plot.x, a.plot.w, a.plot.h, a.glass.x, a.glass.w, a.glass.h,
        ] {
            assert!(v.is_finite(), "non-finite geometry for broom={broom}");
        }
        assert_eq!(
            a.glass.x, PANE.x,
            "book starts off the pane for broom={broom}"
        );
    }
}
