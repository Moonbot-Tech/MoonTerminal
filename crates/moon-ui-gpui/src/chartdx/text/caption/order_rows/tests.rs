//! Synthetic order-caption regression cases, independent of GPU and market infrastructure.

use super::{OrderCaption, layout_order_captions};

/// Build a synthetic 20-pixel band at the requested top, with identity independent of sorting.
fn caption(uid: u64, top: f32, secondary: bool, book: bool) -> OrderCaption {
    OrderCaption {
        source: uid as usize,
        book,
        uid,
        secondary,
        interacting: false,
        pinned: false,
        distance: uid as f32,
        top: Some(top),
        x: [0.0, 100.0],
        height: 20.0,
    }
}

/// Read a caption by identity so a regression in priority sorting cannot fool the oracle.
fn top(captions: &[OrderCaption], uid: u64, book: bool) -> Option<f32> {
    captions
        .iter()
        .find(|c| c.uid == uid && c.book == book)
        .expect("caption")
        .top
}

/// Dropping nearest-price arbitration would leave several close long or short sell sizes visible.
#[test]
fn close_sells_keep_nearest_size_and_its_depth_for_both_sides() {
    for direction in [-1.0, 1.0] {
        let mut captions = vec![
            caption(1, 100.0 + direction * 2.0, true, false),
            caption(2, 100.0 + direction, true, false),
            caption(3, 100.0, true, false),
            caption(3, 102.0, true, true),
            caption(2, 103.0, true, true),
        ];
        for c in &mut captions {
            c.distance = (4 - c.uid) as f32;
        }
        layout_order_captions(&mut captions, [0.0, 500.0]);
        assert_eq!(top(&captions, 3, false), Some(100.0));
        assert_eq!(top(&captions, 3, true), Some(102.0));
        assert_eq!(top(&captions, 2, false), None);
        assert_eq!(top(&captions, 1, false), None);
        assert_eq!(top(&captions, 2, true), None);
    }
}

/// Ignoring interaction priority would substitute the nearest stranger's size for the hovered one.
#[test]
fn hovered_order_keeps_its_pair_even_when_farther_from_price() {
    let mut captions = vec![
        caption(1, 100.0, true, false),
        caption(4, 102.0, true, false),
        caption(4, 104.0, true, true),
    ];
    captions[1].interacting = true;
    captions[2].interacting = true;
    layout_order_captions(&mut captions, [0.0, 500.0]);
    assert_eq!(top(&captions, 1, false), None);
    assert_eq!(top(&captions, 4, false), Some(102.0));
    assert_eq!(top(&captions, 4, true), Some(104.0));
}

/// Thinning a dragged caption would put a stranger's number beside the line the user is moving.
#[test]
fn dragged_order_keeps_all_captions_and_displaces_a_number() {
    let mut captions = vec![
        caption(1, 100.0, true, false),
        caption(2, 101.0, true, false),
        caption(3, 100.0, false, false),
    ];
    captions[1].interacting = true;
    layout_order_captions(&mut captions, [0.0, 500.0]);
    assert_eq!(top(&captions, 1, false), None);
    assert_eq!(top(&captions, 2, false), Some(101.0));
    assert_eq!(top(&captions, 3, false), Some(81.0));
}

/// Letting an ordinary size reserve first would displace both numbers instead of hiding the size.
#[test]
fn every_number_moves_to_the_nearest_free_row() {
    let mut captions = vec![
        caption(1, 100.0, true, false),
        caption(2, 102.0, false, false),
        caption(3, 99.0, false, false),
    ];
    layout_order_captions(&mut captions, [0.0, 500.0]);
    assert_eq!(top(&captions, 1, false), None);
    assert_eq!(top(&captions, 2, false), Some(102.0));
    assert_eq!(top(&captions, 3, false), Some(82.0));
}

/// Reversing ordinary priority would move B's number and retain the stranger's expendable size.
#[test]
fn stranger_size_is_hidden_instead_of_displacing_a_number() {
    let mut captions = vec![
        caption(1, 100.0, true, false),
        caption(2, 100.0, false, false),
    ];
    layout_order_captions(&mut captions, [0.0, 500.0]);
    assert_eq!(top(&captions, 1, false), None);
    assert_eq!(top(&captions, 2, false), Some(100.0));
}

/// Comparing only Y would hide a separate book depth or move a number beside a hovered depth.
#[test]
fn separate_book_depth_and_chart_number_keep_the_same_row() {
    for interacting in [false, true] {
        let mut captions = vec![
            caption(1, 100.0, true, true),
            caption(2, 100.0, false, false),
        ];
        captions[0].x = [100.0, 160.0];
        captions[0].interacting = interacting;
        layout_order_captions(&mut captions, [0.0, 500.0]);
        assert_eq!(top(&captions, 1, true), Some(100.0));
        assert_eq!(top(&captions, 2, false), Some(100.0));
    }
}

/// Treating book/chart as separate columns would retain a stranger's depth over a size at the edge.
#[test]
fn overlapping_size_and_depth_rectangles_still_compete() {
    let mut captions = vec![
        caption(1, 100.0, true, false),
        caption(2, 100.0, true, true),
    ];
    captions[1].x = [99.0, 160.0];
    layout_order_captions(&mut captions, [0.0, 500.0]);
    assert_eq!(top(&captions, 1, false), Some(100.0));
    assert_eq!(top(&captions, 2, true), None);
}

/// Removing number displacement would draw two order identities on top of each other.
#[test]
fn two_numbered_captions_on_one_row_still_displace() {
    let mut captions = vec![
        caption(1, 100.0, false, false),
        caption(2, 100.0, false, false),
    ];
    layout_order_captions(&mut captions, [0.0, 500.0]);
    assert_eq!(top(&captions, 1, false), Some(100.0));
    assert_eq!(top(&captions, 2, false), Some(80.0));
}

/// Reserving quantized rows or changing the size/depth offset would move an uncrowded order.
#[test]
fn no_collision_and_same_order_pair_keep_their_positions() {
    let mut captions = vec![
        caption(1, 100.0, true, false),
        caption(1, 102.0, true, true),
        caption(2, 150.0, false, false),
    ];
    layout_order_captions(&mut captions, [0.0, 500.0]);
    assert_eq!(top(&captions, 1, false), Some(100.0));
    assert_eq!(top(&captions, 1, true), Some(102.0));
    assert_eq!(top(&captions, 2, false), Some(150.0));
}

/// Applying on-screen displacement to pinned labels would break the established inward edge stack.
#[test]
fn pinned_edge_stack_is_unchanged() {
    let mut captions = vec![
        caption(1, 10.0, true, false),
        caption(1, 30.0, false, false),
    ];
    captions.iter_mut().for_each(|c| c.pinned = true);
    layout_order_captions(&mut captions, [0.0, 500.0]);
    assert_eq!(
        captions.iter().map(|c| c.top).collect::<Vec<_>>(),
        vec![Some(10.0), Some(30.0)]
    );
}

/// Unbounded nearest-row placement would push [N] out of view beside a crowded top edge.
#[test]
fn displaced_number_prefers_a_visible_row_near_the_edge() {
    let mut captions = vec![caption(1, 2.0, false, false), caption(2, 1.0, false, false)];
    layout_order_captions(&mut captions, [0.0, 100.0]);
    assert_eq!(top(&captions, 2, false), Some(22.0));
}

/// Sorting interacting captions only by price would let a later protected size cover [N].
#[test]
fn protected_sizes_reserve_rows_before_protected_numbers() {
    let mut captions = vec![
        caption(1, 100.0, false, false),
        caption(2, 102.0, true, false),
    ];
    captions.iter_mut().for_each(|c| c.interacting = true);
    layout_order_captions(&mut captions, [0.0, 500.0]);
    assert_eq!(top(&captions, 2, false), Some(102.0));
    assert_eq!(top(&captions, 1, false), Some(82.0));
}
