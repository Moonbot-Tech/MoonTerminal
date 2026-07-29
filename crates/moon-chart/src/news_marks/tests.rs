use super::*;

/// Where the geometry actually puts a mark's center, read back from the instance the shader
/// consumes (`price` is its distance above the plot bottom). Deriving the oracle from the PRODUCTION
/// geometry is the point: the hit row and the drawn gem must agree, and a test that recomputed the
/// offset itself would pass while they drifted apart.
fn center_y(bottom: f32, scale: f32) -> f32 {
    let mut out = Vec::new();
    build_news_geometry(
        &[NewsMark::default()],
        0.0,
        None,
        [255, 255, 255],
        scale,
        SHAPE_GEM,
        &mut out,
    );
    bottom - out[0].price
}

#[test]
fn the_row_test_covers_the_gem_and_nothing_else() {
    let bottom = 400.0;
    let y = center_y(bottom, 1.0);
    assert!(in_mark_row(y, bottom, 1.0));
    assert!(in_mark_row(y + MARK_HALF_H, bottom, 1.0), "lower tip hits");
    assert!(in_mark_row(y - MARK_HALF_H, bottom, 1.0), "upper tip hits");
    // Mid-chart: the marks must not steal hover from the plot.
    assert!(!in_mark_row(y - 40.0, bottom, 1.0));
    // At 2× DPI the row sits twice as far from the bottom and is twice as tall, so the top of the
    // 2× gem is out of the 1× row's reach.
    let y2 = center_y(bottom, 2.0);
    let top_of_2x = y2 - MARK_HALF_H * 2.0;
    assert!(in_mark_row(top_of_2x, bottom, 2.0));
    assert!(!in_mark_row(top_of_2x, bottom, 1.0), "1× row is shorter");
}

#[test]
fn a_stack_comes_back_whole_with_the_nearest_named() {
    let xs = [100.0f32, 103.0, 200.0];
    let hit = hit_marks(xs, 102.0, 1.0).expect("two marks are within reach");
    assert_eq!(hit.nearest, 1, "103 is closer to 102 than 100 is");
    assert_eq!(
        hit.stack,
        vec![0, 1],
        "the stack keeps the marks' own order"
    );
    // A pixel of drift flips which is nearest but must NOT reorder the stack, or the hover card
    // would look like it changed on every raw pointer event.
    let drifted = hit_marks(xs, 101.0, 1.0).expect("still two");
    assert_eq!(drifted.nearest, 0);
    assert_eq!(drifted.stack, hit.stack);
}

#[test]
fn nothing_under_the_cursor_is_no_hit() {
    assert!(hit_marks([100.0f32], 140.0, 1.0).is_none());
}

#[test]
fn hit_reach_follows_the_device_scale() {
    // At 2× DPI the reach doubles, so the same pixel offset hits there and misses at 1×.
    assert_eq!(
        hit_marks([100.0f32], 111.0, 2.0).map(|h| h.nearest),
        Some(0)
    );
    assert!(hit_marks([100.0f32], 111.0, 1.0).is_none());
}

#[test]
fn geometry_is_bottom_anchored_and_scales_with_dpi() {
    let mut out = Vec::new();
    let marks = [NewsMark::new(0, []), NewsMark::new(60_000, [])];
    build_news_geometry(&marks, 0.0, None, [0, 255, 0], 2.0, SHAPE_GEM, &mut out);
    assert_eq!(out.len(), 2, "one instance per uncoloured mark");
    for m in &out {
        assert_eq!(m.anchor, MARKER_ANCHOR_BOTTOM);
        assert_eq!(m.sectors, 1.0, "a single colour is not sliced");
        assert_eq!(
            m.price,
            mark_center_offset(false) * 2.0,
            "offset scales with DPI"
        );
        assert_eq!(m.size, MARK_HALF_H * 2.0);
        assert_eq!(m.thickness, MARK_HALF_W * 2.0, "gem is taller than wide");
        // Uncoloured news takes the caller's neutral colour.
        assert_eq!(m.color[1], 1.0);
        assert_eq!(m.color[0], 0.0);
    }
    assert_eq!(out[1].t_rel, 60_000.0, "time is rebased on the epoch");
}

#[test]
fn one_wedge_per_distinct_tag_colour() {
    let mut out = Vec::new();
    // Two tags share a colour and a third adds one: two wedges, not three.
    let mark = NewsMark::new(0, [0xFF0000, 0x00FF00, 0xFF0000]);
    assert_eq!(mark.colors(), [0xFF0000, 0x00FF00]);
    build_news_geometry(&[mark], 0.0, None, [255, 255, 255], 1.0, SHAPE_GEM, &mut out);
    assert_eq!(out.len(), 2);
    assert_eq!(out[0].sector, 0.0);
    assert_eq!(out[1].sector, 1.0);
    assert!(out.iter().all(|m| m.sectors == 2.0));
    assert_eq!(out[0].color[0], 1.0, "first wedge is the first tag colour");
    assert_eq!(out[1].color[1], 1.0);
}

#[test]
fn the_wedge_count_is_capped_at_the_palette_size() {
    let mark = NewsMark::new(0, [1, 2, 3, 4, 5, 6]);
    assert_eq!(mark.colors().len(), MAX_MARK_COLORS);
    assert_eq!(mark.colors(), [1, 2, 3, 4]);
}

#[test]
fn a_hovered_mark_grows_upward_from_the_axis() {
    let mut rest = Vec::new();
    let mut hot = Vec::new();
    let marks = [NewsMark::new(0, [])];
    build_news_geometry(&marks, 0.0, None, [255, 255, 255], 1.0, SHAPE_GEM, &mut rest);
    build_news_geometry(&marks, 0.0, Some(0), [255, 255, 255], 1.0, SHAPE_GEM, &mut hot);
    assert!(hot[0].size > rest[0].size, "the hovered gem is bigger");
    assert!(hot[0].thickness > rest[0].thickness);
    // The instance's `price` is its center's distance above the plot bottom, and the gem's lower tip
    // sits `size` below the center: keeping the two equal is what pins the tip to the axis at BOTH
    // sizes, so growing cannot push the mark off it.
    assert_eq!(rest[0].price, rest[0].size);
    assert_eq!(hot[0].price, hot[0].size);
    assert!(hot[0].color[3] > rest[0].color[3], "and fully opaque");
}
