//! Regression coverage for the shared corner-caption zone and its degenerate-layout guards.

use crate::chartdx::text::caption::{CaptionBox, caption_geom};

/// Pane 1000 wide, plot 760 wide, a 220-wide book occupying the right side.
fn with_book() -> Option<crate::chartdx::text::caption::CaptionGeom> {
    caption_geom(0.0, 1000.0, 0.0, 780.0, 50.0, true, 780.0, 30.0, 4.0)
}

/// Catches replacing the live order-book edge with a second fixed-width calculation, which would
/// let caption text escape the book on layouts whose book width differs from the default.
#[test]
fn the_caption_zone_is_the_books_own_rectangle() {
    let g = with_book().expect("a laid-out pane has a caption zone");
    // Left edge comes from the book, not from a second copy of the width rule.
    assert_eq!(g.zone_left, 780.0);
    assert_eq!(g.right_x, 970.0);
    assert_eq!(g.max_w, 190.0);
}

/// Catches ignoring the narrowed live book rectangle on cramped panes, which would let the core
/// name spill left across the candles.
#[test]
fn a_narrowed_book_narrows_the_caption_with_it() {
    // The engine shrinks the book on a cramped pane; the caption must follow it rather than keep
    // assuming GLASS_ZONE_PX, which is what let the core name spill past the book's left edge.
    let wide = with_book().expect("wide");
    let narrow =
        caption_geom(0.0, 1000.0, 0.0, 850.0, 50.0, true, 850.0, 30.0, 4.0).expect("narrow");
    assert!(narrow.max_w < wide.max_w);
    assert_eq!(narrow.zone_left, 850.0);
}

/// Catches anchoring a no-book caption to the pane edge, where it would occupy the reserved outer
/// zone instead of remaining inside the plot.
#[test]
fn without_a_book_the_zone_is_carved_off_the_plot_not_the_pane() {
    let g = caption_geom(0.0, 1000.0, 0.0, 800.0, 50.0, false, f32::NAN, 30.0, 4.0)
        .expect("a plot-only pane still captions");
    // Anchored at the PLOT's right edge, not the pane's.
    assert_eq!(g.right_x, 770.0);
    // And bounded by a book-sized budget rather than running across the whole chart.
    assert!(g.max_w <= moon_chart::GLASS_ZONE_PX);
    assert!(g.zone_left >= 0.0);
}

/// Catches allowing a caption to consume all of a narrow plot, obscuring the chart it labels.
#[test]
fn a_very_narrow_plot_gets_half_of_it_at_most() {
    // Half the plot, so a caption can never occupy the entire width of a thin pane.
    let g = caption_geom(0.0, 200.0, 0.0, 180.0, 10.0, false, f32::NAN, 30.0, 4.0)
        .expect("narrow but usable");
    assert!(g.max_w <= 90.0, "max_w was {}", g.max_w);
}

/// Catches returning invalid or negative caption geometry before a pane has a usable layout.
#[test]
fn a_degenerate_pane_draws_no_caption() {
    // Zero-width, inverted, and non-finite panes must answer "nothing", never a negative budget
    // that a truncation routine would then have to defend against.
    assert!(caption_geom(0.0, 0.0, 0.0, 0.0, 0.0, false, f32::NAN, 30.0, 4.0).is_none());
    assert!(caption_geom(500.0, 100.0, 0.0, 80.0, 0.0, false, f32::NAN, 30.0, 4.0).is_none());
    assert!(caption_geom(f32::NAN, 1000.0, 0.0, 800.0, 0.0, true, 780.0, 30.0, 4.0).is_none());
    // A pane narrower than the close-button inset leaves no room at all.
    assert!(caption_geom(0.0, 20.0, 0.0, 20.0, 0.0, true, 0.0, 30.0, 4.0).is_none());
}

/// `caption.rs:caption_geom` must bail as soon as the right anchor lands left of the pane's own
/// left edge, before the zone/`max_w` arithmetic below it runs at all.
///
/// Breakage this pins: dropping the `right_x <= pane_left` early return, reasoning a pane is
/// never that narrow. A plot area whose own left edge sits outside its pane (a stale layout from
/// the previous frame) then lets the no-book budget fall back to `plot_left` instead of the
/// (already invalid) right anchor, producing a POSITIVE `max_w` and a caption plate anchored left
/// of the pane it is supposed to be drawn inside — instead of correctly drawing nothing.
#[test]
fn an_anchor_left_of_the_pane_draws_no_caption() {
    assert!(
        caption_geom(100.0, 1000.0, 0.0, 200.0, 0.0, false, f32::NAN, 150.0, 4.0).is_none(),
        "right_x (200-150=50) sits left of pane_left (100); this must bail rather than \
         reach the zone/max_w arithmetic"
    );
}

/// Catches trusting a previous frame's out-of-bounds book rectangle, which would create a caption
/// plate wider than the current pane.
#[test]
fn a_stale_book_rectangle_is_clamped_into_the_pane() {
    // A book rect left over from a previous layout can sit outside the pane for one frame; the
    // zone must stay inside it rather than produce a caption budget wider than the pane.
    let g = caption_geom(0.0, 1000.0, 0.0, 780.0, 50.0, true, -400.0, 30.0, 4.0).expect("clamped");
    assert_eq!(g.zone_left, 0.0);
    assert!(g.max_w <= 1000.0);
}

/// `caption.rs:CaptionBox` must measure the plate from the runs actually drawn.
///
/// Breakage this pins: replacing it with a formula over an assumed set of rows. The scale badge,
/// broom delta, coin, and core name are each optional, so any fixed roster is wrong for some
/// combinations: the plate then covers part of the text and leaves the rest unreadable.
#[test]
fn a_plate_is_measured_from_the_runs_that_were_drawn() {
    let mut empty = CaptionBox::default();
    assert_eq!(
        empty.plate(1.0),
        [0.0; 4],
        "a column with no runs has no plate"
    );
    // One row at x=100 width 60, a second below it starting further left and running wider.
    empty.add(100.0, 60.0, 10.0, 16.0);
    empty.add(80.0, 100.0, 26.0, 14.0);
    let [x, y, w, h] = empty.plate(1.0);
    assert_eq!(
        x,
        80.0 - 5.0,
        "the plate starts at the leftmost run, less its inset"
    );
    assert_eq!(y, 10.0 - 2.0, "and at the topmost run, less its inset");
    assert_eq!(w, 100.0 + 5.0 + 3.0, "spanning to the rightmost run's end");
    assert_eq!(h, 30.0 + 4.0, "and down to the lowest run's bottom");

    // The device scale factor multiplies the finished rectangle, it does not enter the padding.
    let scaled = empty.plate(2.0);
    assert_eq!(scaled, [x * 2.0, y * 2.0, w * 2.0, h * 2.0]);
}
