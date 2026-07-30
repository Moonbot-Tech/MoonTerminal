use super::{MAX_SWING_LABELS, swing_labels, swing_points};

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
