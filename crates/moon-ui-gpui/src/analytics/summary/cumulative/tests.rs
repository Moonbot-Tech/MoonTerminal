use super::{
    MAX_CORE_LINES, MAX_SWING_LABELS, drawn_core_order, hover_bucket_at, place_labels,
    swing_labels, swing_points,
};
use moon_core::db::analytics::CoreSeries;

/// Independent old layout oracle: last-painted half-open column owns the pointer.
fn old_hover_bucket(frac: f32, n: usize) -> Option<usize> {
    let step = 1.0 / n.saturating_sub(1).max(1) as f32;
    (0..n).rev().find(|&bi| {
        let centre = bi as f32 * step;
        let left = (centre - step / 2.0).max(0.0);
        let right = (centre + step / 2.0).min(1.0);
        left <= frac && frac < right
    })
}

/// Ideal-grid rounding loses f32 edges and the one-bucket gap, shifting popup identity.
#[test]
fn single_hover_matches_all_old_columns_and_shared_edges() {
    for n in 1usize..=500 {
        let step = 1.0 / n.saturating_sub(1).max(1) as f32;
        for bi in 0..n {
            let centre = bi as f32 * step;
            let left = (centre - step / 2.0).max(0.0);
            let right = (centre + step / 2.0).min(1.0);
            for sample in 0..=8 {
                let frac = left + (right - left) * sample as f32 / 8.0;
                assert_eq!(
                    hover_bucket_at(frac, n),
                    old_hover_bucket(frac, n),
                    "n={n} bi={bi} frac={frac:?}"
                );
            }
            assert_eq!(hover_bucket_at(left, n), old_hover_bucket(left, n));
            assert_eq!(hover_bucket_at(right, n), old_hover_bucket(right, n));
            if bi + 1 < n {
                let next_left = ((bi + 1) as f32 * step - step / 2.0).max(0.0);
                if next_left == right {
                    assert_eq!(hover_bucket_at(right, n), Some(bi + 1));
                }
            }
        }
    }
    assert_eq!(hover_bucket_at(0.5, 1), None);
    assert_eq!(hover_bucket_at(0.5001, 1), None);
    assert_eq!(hover_bucket_at(1.0, 1), None);
    assert_eq!(hover_bucket_at(0.0, 0), None);
}

/// Build only the fields the line-selection helper reads.
fn core(uid: u64, total: f64) -> CoreSeries {
    CoreSeries {
        uid,
        name: format!("core-{uid}"),
        per_bucket: Vec::new(),
        per_bucket_trades: Vec::new(),
        total,
        trades: 0,
    }
}

/// `cumulative.rs:place_labels` must retain labels separated vertically by one full label height.
/// Changing its clear-on-either-axis `||` to `&&` drops a readable swing label, hiding a period move.
#[test]
fn place_labels_keeps_labels_clear_on_one_axis() {
    let labels = place_labels(&[(0.0, 0.0, 100.0), (0.0, 10.0, 50.0)], 10.0, 10.0);

    assert_eq!(
        labels,
        vec![0, 1],
        "vertical clearance must keep both labels"
    );
}

/// `cumulative.rs:place_labels` must prefer the larger absolute move when label rectangles overlap.
/// Replacing its descending-magnitude ordering with natural order keeps the smaller swing and hides the period's largest move.
#[test]
fn place_labels_keeps_the_larger_colliding_move() {
    let labels = place_labels(&[(0.0, 0.0, 12.0), (2.0, 1.0, -30.0)], 10.0, 10.0);

    assert_eq!(
        labels,
        vec![1],
        "the larger absolute move must survive the collision"
    );
}

/// A saw that turns at EVERY bucket: the threshold must be backed off until the labels
/// fit, or the chart becomes a wall of overlapping numbers.
#[test]
fn swing_labels_are_bounded_on_a_saw() {
    let pts: Vec<f32> = (0..400)
        .map(|i| if i % 2 == 0 { 0.0 } else { 1000.0 })
        .collect();
    let s = swing_labels(&pts);
    assert!(
        s.len() <= MAX_SWING_LABELS,
        "saw produced {} labels, cap is {MAX_SWING_LABELS}",
        s.len()
    );
    assert!(s.windows(2).all(|w| w[0] < w[1]), "not ascending: {s:?}");
}

/// A real dip must still be labelled once the count is within the cap — the back-off
/// must not fire when there is nothing to back off from.
#[test]
fn swing_labels_keep_a_real_dip() {
    let mut pts: Vec<f32> = (0..=10).map(|i| i as f32 * 100.0).collect();
    pts.extend((1..=10).map(|i| 1000.0 - i as f32 * 60.0));
    pts.extend((1..=20).map(|i| 400.0 + i as f32 * 60.0));
    let s = swing_labels(&pts);
    assert!(s.contains(&20), "the trough must survive: {s:?}");
    assert!(s.len() <= MAX_SWING_LABELS, "{s:?}");
}

/// A monotone rise has no turns at all, so only its starting low is labelled — the end
/// is the period total and lives in the card header.
#[test]
fn swing_monotone_keeps_only_the_start() {
    let pts: Vec<f32> = (0..20).map(|i| i as f32).collect();
    assert_eq!(swing_points(&pts, 5.0), vec![0]);
}

/// Noise below the threshold must not become labels — that is the whole point of it.
#[test]
fn swing_ignores_jitter() {
    let pts: Vec<f32> = (0..40)
        .map(|i| i as f32 + if i % 2 == 0 { 0.4 } else { -0.4 })
        .collect();
    let s = swing_points(&pts, 10.0);
    assert_eq!(
        s,
        vec![0],
        "±0.4 jitter under a 10.0 threshold must produce no turns: {s:?}"
    );
}

/// A real dip and the recovery after it are both reported, in order.
#[test]
fn swing_reports_trough_and_peak() {
    // up to 100 (idx 10), down to 40 (idx 20), up to 160 (idx 40)
    let mut pts: Vec<f32> = (0..=10).map(|i| i as f32 * 10.0).collect();
    pts.extend((1..=10).map(|i| 100.0 - i as f32 * 6.0));
    pts.extend((1..=20).map(|i| 40.0 + i as f32 * 6.0));
    let s = swing_points(&pts, 20.0);
    assert!(s.contains(&10), "peak before the dip missing: {s:?}");
    assert!(s.contains(&20), "trough missing: {s:?}");
    assert!(
        !s.contains(&(pts.len() - 1)),
        "the last bucket must stay unlabelled — the header shows it: {s:?}"
    );
    assert!(s.windows(2).all(|w| w[0] < w[1]), "not ascending: {s:?}");
}

/// Degenerate inputs must not panic or index past the end.
#[test]
fn swing_short_inputs() {
    assert!(swing_points(&[], 1.0).is_empty());
    assert!(swing_points(&[5.0], 1.0).is_empty());
    assert_eq!(swing_points(&[5.0, 7.0], 1.0), vec![0]);
}

/// `summary/cumulative.rs:drawn_core_order` must cap lines by absolute contribution, not signed
/// profit. Sorting by signed total hides the largest loss, precisely the line a user needs when
/// the cumulative chart's total masks one badly losing core.
#[test]
fn drawn_core_order_keeps_the_largest_loss_inside_the_line_cap() {
    let mut cores: Vec<_> = (0..12)
        .map(|uid| core(uid as u64, 120.0 - uid as f64 * 10.0))
        .collect();
    cores.push(core(99, -1_000.0));

    let order = drawn_core_order(&cores);
    assert_eq!(order.len(), MAX_CORE_LINES);
    assert_eq!(
        order[0], 12,
        "the largest absolute contributor is the loss core"
    );
    assert_eq!(order, vec![12, 0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10]);
}
