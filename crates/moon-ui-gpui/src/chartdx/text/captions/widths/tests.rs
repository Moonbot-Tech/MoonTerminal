//! The two rules that keep one zone's bands apart: what the figures may take, and what is left.
//!
//! Explicit imports: the chartdx parent re-exports `gpui::*`, whose own `test` shadows the built-in
//! attribute and makes `#[test]` expand recursively.

use moon_core::config::LabelAlign;

use super::{Taken, edge_cap, free_width, prose_owed};

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
