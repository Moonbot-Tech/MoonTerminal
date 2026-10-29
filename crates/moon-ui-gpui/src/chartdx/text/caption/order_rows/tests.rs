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

/// The placement scan this file replaced: each candidate walks the accepted prefix.
fn competes_linear(a: &OrderCaption, b: &OrderCaption) -> bool {
    !(a.uid == b.uid && a.book != b.book) && a.x[0] < b.x[1] && b.x[0] < a.x[1]
}

fn occupied_linear(caption: &OrderCaption, top: f32, accepted: &[OrderCaption]) -> bool {
    accepted.iter().any(|other| {
        other.top.is_some_and(|y| {
            competes_linear(caption, other) && top < y + other.height && y < top + caption.height
        })
    })
}

fn layout_linear(captions: &mut [OrderCaption], visible: [f32; 2]) {
    captions.sort_unstable_by(|a, b| {
        let rank = |c: &OrderCaption| {
            if c.pinned {
                0
            } else if c.interacting && c.secondary {
                1
            } else if c.interacting {
                2
            } else if !c.secondary {
                3
            } else {
                4
            }
        };
        rank(a)
            .cmp(&rank(b))
            .then_with(|| a.distance.total_cmp(&b.distance))
            .then_with(|| a.uid.cmp(&b.uid))
            .then_with(|| a.book.cmp(&b.book))
            .then_with(|| a.source.cmp(&b.source))
    });
    for i in 0..captions.len() {
        let (accepted, rest) = captions.split_at_mut(i);
        let caption = &mut rest[0];
        let Some(top) = caption.top else { continue };
        if caption.pinned || !occupied_linear(caption, top, accepted) {
            continue;
        }
        if caption.secondary {
            if !caption.interacting {
                caption.top = None;
            }
            continue;
        }
        let mut best = None;
        let mut distance = f32::INFINITY;
        let mut best_inside = None;
        let mut distance_inside = f32::INFINITY;
        for other in accepted
            .iter()
            .filter(|other| competes_linear(caption, other))
        {
            let Some(y) = other.top else { continue };
            for candidate in [y - caption.height, y + other.height] {
                let delta = (candidate - top).abs();
                if delta < distance && !occupied_linear(caption, candidate, accepted) {
                    best = Some(candidate);
                    distance = delta;
                }
                if candidate >= visible[0]
                    && candidate + caption.height <= visible[1]
                    && delta < distance_inside
                    && !occupied_linear(caption, candidate, accepted)
                {
                    best_inside = Some(candidate);
                    distance_inside = delta;
                }
            }
        }
        caption.top = best_inside.or(best).or(Some(top));
    }
}

fn caption_seed(count: usize, gap: f32) -> Vec<OrderCaption> {
    (0..count)
        .map(|i| {
            let mut row = caption(i as u64, 40.0 + i as f32 * gap, i % 4 == 0, i % 5 == 0);
            row.distance = (count - i) as f32;
            row.height = 16.0 + (i % 3) as f32 * 4.0;
            row.x = if i % 5 == 0 {
                [220.0, 280.0]
            } else {
                [0.0, 90.0]
            };
            if i % 11 == 0 {
                row.pinned = true;
            }
            if i % 13 == 0 {
                row.interacting = true;
            }
            if i % 17 == 0 {
                row.top = None;
            }
            row
        })
        .collect()
}

/// Replacing the prefix scan must not move, hide, or reorder a caption.
///
/// Breakage: a merged-band edge that is not the slot the old scan picked covers a price label
/// or leaves a hole the user used to see the number in.
#[test]
fn caption_placement_matches_the_prefix_scan() {
    let mut state = 0x1234_5678u32;
    let mut next = || {
        state = state.wrapping_mul(1664525).wrapping_add(1013904223);
        state
    };
    for count in [0, 1, 2, 8, 40] {
        let mut seed = caption_seed(count, 0.0);
        for row in &mut seed {
            let n = next();
            row.top = Some((n % 80) as f32);
            row.distance = (n % 50) as f32;
            row.height = 10.0 + (n % 7) as f32;
            row.secondary = n % 3 == 0;
            row.book = n % 5 == 0;
            row.pinned = n % 19 == 0;
            row.interacting = n % 11 == 0;
            row.x = [0.0, 40.0 + (n % 30) as f32];
            if n % 23 == 0 {
                row.top = None;
            }
        }
        let mut linear = seed.clone();
        let mut current = seed;
        let visible = [0.0, 120.0];
        layout_linear(&mut linear, visible);
        layout_order_captions(&mut current, visible);
        assert_eq!(current.len(), linear.len());
        for (got, want) in current.iter().zip(linear.iter()) {
            assert_eq!(got.source, want.source);
            assert_eq!(got.uid, want.uid);
            assert_eq!(got.book, want.book);
            assert_eq!(got.top, want.top);
        }
    }
}

/// Time caption placement for hundreds of order labels on one presented frame.
///
/// Spread labels do not overlap. Piled labels share one row, so each number is displaced along
/// that row. Both run on the UI thread for every frame that draws order captions. The larger
/// counts check that a right-edge pile stays linear.
#[test]
#[ignore = "manual heavy-user timing probe"]
fn bench_goal_e_order_captions() {
    use std::hint::black_box;
    use std::time::Instant;

    fn time(seed: &[OrderCaption], iters: u32, linear: bool) -> u128 {
        let start = Instant::now();
        for _ in 0..iters {
            let mut batch = seed.to_vec();
            if linear {
                layout_linear(black_box(&mut batch), [0.0, 20_000.0]);
            } else {
                layout_order_captions(black_box(&mut batch), [0.0, 20_000.0]);
            }
            black_box(batch.len());
        }
        start.elapsed().as_nanos() / u128::from(iters)
    }

    let spread = caption_seed(400, 40.0);
    let piled = caption_seed(400, 0.0);
    println!(
        "goal_e_captions_400_spread_linear: {} ns/iter",
        time(&spread, 5, true)
    );
    println!(
        "goal_e_captions_400_spread: {} ns/iter",
        time(&spread, 20, false)
    );
    println!(
        "goal_e_captions_400_piled_linear: {} ns/iter",
        time(&piled, 1, true)
    );
    println!(
        "goal_e_captions_400_piled: {} ns/iter",
        time(&piled, 20, false)
    );
    for count in [800, 1600] {
        let piled_n = caption_seed(count, 0.0);
        let spread_n = caption_seed(count, 40.0);
        println!(
            "goal_e_captions_{count}_piled: {} ns/iter",
            time(&piled_n, 10, false)
        );
        println!(
            "goal_e_captions_{count}_spread: {} ns/iter",
            time(&spread_n, 10, false)
        );
    }
}
