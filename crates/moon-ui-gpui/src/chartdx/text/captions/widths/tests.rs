//! The two rules that keep one zone's bands apart: what the figures may take, and what is left.
//!
//! Explicit imports: the chartdx parent re-exports `gpui::*`, whose own `test` shadows the built-in
//! attribute and makes `#[test]` expand recursively.

use moon_core::config::LabelAlign;

use super::{
    CAPTION_GAP, Taken, band_max_w, band_rank, edge_cap, free_width, line_budget, prose_owed,
};

/// A whole zone, in the round numbers the arithmetic is easiest to read at.
const ZONE: f32 = 1000.0;

/// What the bands drawn before this one took, written the way the drawing pass writes it.
fn taken(pairs: &[(LabelAlign, f32)]) -> Taken {
    let mut taken = Taken::default();
    for (align, w) in pairs {
        taken.set(*align, *w);
    }
    taken
}

/// A long detect line: the figures beside it are held to their share so the sentence stays legible.
#[test]
fn long_prose_holds_the_figures_to_a_share() {
    assert_eq!(edge_cap(ZONE, 5000.0), Some(292.0));
}

/// A two-word detect message asks for little, so it takes little: the modules beside it keep
/// printing at the width they always did.
#[test]
fn short_prose_costs_the_figures_almost_nothing() {
    assert_eq!(edge_cap(ZONE, 100.0), Some(442.0));
}

/// A zone too small to divide keeps its old behaviour — captions that touch — rather than losing a
/// band under `MIN_LEGIBLE_W`, which is dropped rather than truncated.
#[test]
fn a_cramped_zone_is_not_divided_at_all() {
    assert_eq!(edge_cap(40.0, ZONE), None);
    assert_eq!(edge_cap(0.0, 100.0), None);
    assert_eq!(edge_cap(-5.0, 100.0), None);
}

/// Nothing to divide FOR: a zone whose elastic band measured nothing — or whose measurement came
/// back NaN, a shaper answering for a face it could not load — leaves every band the whole width
/// rather than dividing against a width nobody knows.
#[test]
fn nothing_to_divide_for_divides_nothing() {
    assert_eq!(edge_cap(ZONE, 0.0), None);
    assert_eq!(edge_cap(ZONE, f32::NAN), None);
}

/// The everyday shape: two modest edge modules, prose between them. It loses the WIDER edge on both
/// sides, because it is anchored to the middle and has to stay there.
#[test]
fn a_centred_band_loses_its_wider_neighbour_twice() {
    let taken = taken(&[(LabelAlign::Left, 200.0), (LabelAlign::Right, 100.0)]);
    assert_eq!(free_width(ZONE, LabelAlign::Center, taken), 584.0);
}

/// Nothing beside it: the whole zone, gap included. This is the shape of most panes, and a band
/// that printed nothing must not narrow the one that did.
#[test]
fn a_band_with_empty_neighbours_keeps_the_zone() {
    assert_eq!(free_width(ZONE, LabelAlign::Center, Taken::default()), ZONE);
    assert_eq!(free_width(ZONE, LabelAlign::Left, Taken::default()), ZONE);
}

/// An edge runs until the centre's near edge — the centre is anchored to the middle, so it costs
/// each edge half of what it took.
#[test]
fn an_edge_stops_where_the_centre_starts() {
    let taken = taken(&[(LabelAlign::Center, 400.0)]);
    assert_eq!(free_width(ZONE, LabelAlign::Left, taken), 292.0);
    assert_eq!(free_width(ZONE, LabelAlign::Right, taken), 292.0);
}

/// With the middle empty an edge runs until the OTHER edge starts, and keeps everything before it.
#[test]
fn an_edge_stops_where_the_far_band_starts() {
    assert_eq!(
        free_width(ZONE, LabelAlign::Left, taken(&[(LabelAlign::Right, 300.0)])),
        692.0
    );
    assert_eq!(
        free_width(ZONE, LabelAlign::Right, taken(&[(LabelAlign::Left, 300.0)])),
        692.0
    );
}

/// Both at once: whichever neighbour is met first is the one that bounds the band.
#[test]
fn an_edge_takes_the_nearer_of_its_two_bounds() {
    let taken = taken(&[(LabelAlign::Center, 100.0), (LabelAlign::Right, 700.0)]);
    assert_eq!(free_width(ZONE, LabelAlign::Left, taken), 292.0);
}

/// A zone that ran out of room answers zero rather than a negative width: the drawing pass compares
/// the budget against `MIN_LEGIBLE_W` and drops the caption, which is what should happen here.
#[test]
fn a_band_with_no_room_left_answers_zero() {
    let squeezed = taken(&[(LabelAlign::Left, 200.0)]);
    assert_eq!(free_width(100.0, LabelAlign::Center, squeezed), 0.0);
    let squeezed = taken(&[(LabelAlign::Right, 200.0)]);
    assert_eq!(free_width(100.0, LabelAlign::Left, squeezed), 0.0);
}

/// A width that came back negative or NaN is read as zero on the way in, so a band that measured
/// nonsense bounds its neighbours by nothing instead of by a number nothing can compare against.
#[test]
fn a_nonsense_width_is_read_as_nothing() {
    let nonsense = taken(&[(LabelAlign::Left, f32::NAN), (LabelAlign::Right, -30.0)]);
    assert_eq!(free_width(ZONE, LabelAlign::Center, nonsense), ZONE);
}

/// A short centred caption blocks only the lines that share its Y. Past that the edge spends the
/// whole zone — otherwise a skip-reason list wraps at half the plot while the candles beside it
/// are empty.
#[test]
fn past_a_short_centre_the_edge_gets_the_zone() {
    let mut neighbours = Taken::default();
    neighbours.set_extent(LabelAlign::Center, 200.0, 20.0);
    assert_eq!(
        line_budget(ZONE, LabelAlign::Left, neighbours, 0.0, 0.0),
        392.0
    );
    assert_eq!(
        line_budget(ZONE, LabelAlign::Left, neighbours, 20.0, 0.0),
        ZONE,
        "the core name has ended; the skip list may use the rest of the plot"
    );
}

/// A wrapping skip list that would open beside the core is moved past the core's own height, then
/// spends the whole zone. Squeezing it into the leftover next to the name is what printed through
/// it and wrapped the first sentence in half.
#[test]
fn a_wrapping_column_clears_a_short_centre_then_takes_the_zone() {
    let mut neighbours = Taken::default();
    neighbours.set_extent(LabelAlign::Center, 200.0, 20.0);
    assert_eq!(neighbours.blocking_height(LabelAlign::Left), 20.0);
    assert_eq!(
        line_budget(ZONE, LabelAlign::Left, neighbours, 20.0 + CAPTION_GAP, 0.0,),
        ZONE
    );
}

/// A tall far edge still bounds a line that has cleared the centre: full width would print through
/// a right-hand module that runs the height of the band.
#[test]
fn a_clear_line_still_yields_to_a_tall_far_edge() {
    let mut neighbours = Taken::default();
    neighbours.set_extent(LabelAlign::Center, 200.0, 20.0);
    neighbours.set_extent(LabelAlign::Right, 100.0, 400.0);
    assert_eq!(
        line_budget(ZONE, LabelAlign::Left, neighbours, 50.0, 0.0),
        free_width(ZONE, LabelAlign::Left, taken(&[(LabelAlign::Right, 100.0)])),
    );
}

/// A column with no wrapping neighbour still yields to figures already drawn — otherwise a
/// strategy-filter list spends the whole plot and prints through the core name in the centre.
#[test]
fn a_hungry_band_takes_only_what_the_figures_left() {
    let taken = taken(&[(LabelAlign::Center, 200.0)]);
    assert_eq!(
        band_max_w(ZONE, None, false, true, LabelAlign::Left, taken),
        free_width(ZONE, LabelAlign::Left, taken),
    );
}

/// A column alone on the zone keeps the whole width: there is nothing to yield to.
#[test]
fn a_hungry_band_with_empty_neighbours_keeps_the_zone() {
    assert_eq!(
        band_max_w(ZONE, None, false, true, LabelAlign::Left, Taken::default()),
        ZONE,
    );
}

/// A wrapping detect line still caps the column the same way it caps figures. Treating the
/// column as a second elastic band would skip the split entirely and print the skip list through
/// the sentence.
#[test]
fn a_hungry_band_uses_the_figure_cap_when_prose_is_dividing_the_zone() {
    let cap = edge_cap(ZONE, 5000.0);
    assert_eq!(
        band_max_w(ZONE, cap, false, true, LabelAlign::Left, Taken::default()),
        cap.unwrap(),
    );
}

/// Figures still spend the whole zone when nothing wraps and nothing is a column. Narrowing them
/// against each other was never the defect, and would truncate the coin and the core name.
#[test]
fn a_figure_band_is_not_narrowed_against_another_figure() {
    let taken = taken(&[(LabelAlign::Center, 200.0)]);
    assert_eq!(
        band_max_w(ZONE, None, false, false, LabelAlign::Left, taken),
        ZONE,
    );
}

/// Draw order: figures, then columns, then wrapping prose. Inverting it either prints the skip
/// list through the core name or hands the wrapping detect line the whole zone before the column
/// has reported what it took.
#[test]
fn columns_draw_after_figures_and_before_prose() {
    assert!(band_rank(false, false) < band_rank(false, true));
    assert!(band_rank(false, true) < band_rank(true, false));
}

/// The two rules have to agree: whatever the figures are capped at, the elastic band must still be
/// left exactly what the cap was computed to leave it. This is the guarantee that keeps a detect
/// line from vanishing on a busy chart, so it is checked across a spread of zones and sentence
/// lengths rather than at one comfortable point.
#[test]
fn the_cap_leaves_the_prose_what_it_was_promised() {
    for total in [80.0, 200.0, ZONE, 4000.0_f32] {
        for prose in [10.0, 300.0, 5000.0_f32] {
            let Some(cap) = edge_cap(total, prose) else {
                continue;
            };
            let owed = prose_owed(total, prose);
            let both = taken(&[(LabelAlign::Left, cap), (LabelAlign::Right, cap)]);
            let left = free_width(total, LabelAlign::Center, both);
            assert!(
                left >= owed - 0.01,
                "zone {total}, prose {prose}: prose got {left}, was promised {owed}"
            );
        }
    }
}
