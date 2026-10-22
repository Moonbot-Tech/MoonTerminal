use super::*;

/// Builds a synthetic mini-candle without live market data.
fn mini(t: f64, low: f32, high: f32) -> PathMini {
    PathMini {
        t_open_ms: t,
        low,
        high,
    }
}

const DAY: f64 = 86_400_000.0;

#[test]
fn only_minis_inside_the_hidden_bucket_and_left_of_the_dots_are_placed() {
    let minis = [
        mini(DAY - 5_000.0, 1.0, 2.0), // closed bucket: its body is drawn
        mini(DAY, 2.0, 3.0),
        mini(DAY + 5_000.0, 3.0, 4.0),
        mini(DAY + 10_000.0, 4.0, 5.0), // ends past the first dot
    ];
    let mut out = Vec::new();
    let last = place_path(&minis, DAY, DAY + 12_000.0, DAY, None, &mut out);
    assert_eq!(out.len(), 2);
    assert_eq!(out[0].t_open_rel, 0.0);
    assert_eq!(out[1].t_open_rel, 5_000.0);
    assert_eq!(last, Some(3.5));
}

#[test]
fn no_bar_reaches_under_a_raw_dot() {
    let minis: Vec<_> = (0..100)
        .map(|i| mini(DAY + i as f64 * 5_000.0, 1.0, 2.0))
        .collect();
    let dots_left = DAY + 123_456.0;
    let mut out = Vec::new();
    place_path(&minis, DAY, dots_left, 0.0, None, &mut out);
    assert!(!out.is_empty());
    for bar in &out {
        assert!(f64::from(bar.t_open_rel) + PATH_MINI_MS <= dots_left);
    }
}

#[test]
fn nothing_is_placed_when_the_buffer_does_not_reach_the_hidden_bucket() {
    let minis = [
        mini(DAY - 10_000.0, 1.0, 2.0),
        mini(DAY - 5_000.0, 1.0, 2.0),
    ];
    let mut out = Vec::new();
    assert_eq!(
        place_path(&minis, DAY, DAY + 60_000.0, 0.0, None, &mut out),
        None
    );
    assert!(out.is_empty());
}

#[test]
fn nothing_is_placed_without_a_hide_boundary_or_dots() {
    let minis = [mini(DAY, 1.0, 2.0)];
    let mut out = Vec::new();
    place_path(&minis, f64::INFINITY, DAY + 60_000.0, 0.0, None, &mut out);
    place_path(&minis, f64::NAN, DAY + 60_000.0, 0.0, None, &mut out);
    place_path(&minis, DAY, f64::NAN, 0.0, None, &mut out);
    assert!(out.is_empty());
}

#[test]
fn left_of_the_oldest_mini_stays_blank() {
    let minis = [mini(DAY + 3_600_000.0, 1.0, 2.0)];
    let mut out = Vec::new();
    place_path(&minis, DAY, f64::INFINITY, DAY, None, &mut out);
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].t_open_rel, 3_600_000.0);
}

#[test]
fn an_append_continues_the_colour_of_the_previous_bar() {
    let mut out = Vec::new();
    let mid = place_path(
        &[mini(DAY, 2.0, 4.0)],
        DAY,
        f64::INFINITY,
        0.0,
        None,
        &mut out,
    );
    let mid = place_path(
        &[mini(DAY + 5_000.0, 1.0, 2.0)],
        DAY,
        f64::INFINITY,
        0.0,
        mid,
        &mut out,
    );
    place_path(
        &[mini(DAY + 10_000.0, 5.0, 6.0)],
        DAY,
        f64::INFINITY,
        0.0,
        mid,
        &mut out,
    );
    assert_eq!(
        out.iter().map(|b| b.rising).collect::<Vec<_>>(),
        [true, false, true]
    );
}

/// Provides cached prices independently of the path composition's range calculations.
fn cached(t: f64, low: f32, high: f32) -> ChartCandle {
    ChartCandle {
        t_open_ms: t,
        open: low,
        high,
        low,
        close: high,
        volume: 0.0,
        quote_volume: 0.0,
    }
}

/// Losing the minute prefix or using five-second widths for it leaves most of a daily candle blank.
#[test]
fn daily_open_path_spans_minutes_minis_and_the_dot_seam_without_hiding() {
    let minutes: Vec<_> = (0..720)
        .map(|i| cached(DAY + i as f64 * 60_000.0, 10.0, 12.0))
        .collect();
    let minis: Vec<_> = (0..24)
        .map(|i| mini(DAY + 43_200_000.0 + i as f64 * 5_000.0, 11.0, 13.0))
        .collect();
    let dots = DAY + 43_320_000.0;
    let mut out = Vec::new();
    let mid = place_prefix(
        Some(&minutes),
        &[],
        (DAY, minis[0].t_open_ms),
        DAY as i64,
        DAY,
        &mut out,
    );
    place_path(&minis, DAY, dots, DAY, mid, &mut out);
    assert_eq!(out.len(), 744);
    assert_eq!((out[0].t_open_rel, out[0].duration_ms), (0.0, 60_000.0));
    assert_eq!(out[720].duration_ms, 5_000.0);
    for pair in out.windows(2) {
        assert_eq!(pair[0].t_open_rel + pair[0].duration_ms, pair[1].t_open_rel);
    }
    assert_eq!(
        out.last().unwrap().t_open_rel + out.last().unwrap().duration_ms,
        43_320_000.0
    );
}

/// A sub-minute path must not draw a minute bar over its current bucket or raw dots.
#[test]
fn one_minute_timeframe_uses_only_minis_before_the_dots() {
    let mut out = Vec::new();
    place_prefix(
        Some(&[cached(DAY, 1.0, 2.0)]),
        &[],
        (DAY, DAY + 60_000.0),
        60_000,
        DAY,
        &mut out,
    );
    place_path(
        &[mini(DAY, 2.0, 3.0)],
        DAY,
        DAY + 5_000.0,
        DAY,
        None,
        &mut out,
    );
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].duration_ms, 5_000.0);
}

/// Sparse minute data must retain the uncovered five-minute portions, without double drawing.
#[test]
fn minute_rows_override_only_their_own_part_of_fallback_and_clip_the_mini_seam() {
    let mut out = Vec::new();
    place_prefix(
        Some(&[cached(DAY + 60_000.0, 2.0, 3.0)]),
        &[cached(DAY, 1.0, 4.0)],
        (DAY, DAY + 185_000.0),
        DAY as i64,
        DAY,
        &mut out,
    );
    assert_eq!(
        out.iter()
            .map(|b| (b.t_open_rel, b.duration_ms, b.low))
            .collect::<Vec<_>>(),
        [
            (0.0, 60_000.0, 1.0),
            (60_000.0, 60_000.0, 2.0),
            (120_000.0, 65_000.0, 1.0)
        ]
    );
}

/// Unavailable and empty minute reads both allow honest fallback, while no data invents no path.
#[test]
fn unavailable_and_authoritative_empty_minute_reads_use_fallback() {
    for minutes in [None, Some([].as_slice())] {
        let mut out = Vec::new();
        place_prefix(
            minutes,
            &[cached(DAY, 1.0, 4.0)],
            (DAY, DAY + 300_000.0),
            DAY as i64,
            DAY,
            &mut out,
        );
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].duration_ms, 300_000.0);
        out.clear();
        place_prefix(
            minutes,
            &[],
            (DAY, DAY + 300_000.0),
            DAY as i64,
            DAY,
            &mut out,
        );
        assert!(out.is_empty());
    }
}

/// MoonProto folds evicted prints in batches and every batch re-anchors its first mini-candle, so
/// on a busy market minis sit one or two seconds apart. Drawn five seconds wide each, they stacked
/// three to five deep when the chart was stretched.
#[test]
fn minis_closer_than_five_seconds_meet_edge_to_edge() {
    let minis = [
        mini(DAY, 1.0, 2.0),
        mini(DAY + 1_000.0, 2.0, 3.0),
        mini(DAY + 2_500.0, 1.5, 2.5),
        mini(DAY + 9_000.0, 3.0, 4.0),
    ];
    let mut out = Vec::new();
    place_path(&minis, DAY, DAY + 20_000.0, DAY, None, &mut out);
    assert_eq!(
        out.iter()
            .map(|b| (b.t_open_rel, b.duration_ms))
            .collect::<Vec<_>>(),
        // A gap wider than five seconds keeps the five-second width: a quiet stretch stays blank.
        [
            (0.0, 1_000.0),
            (1_000.0, 1_500.0),
            (2_500.0, 5_000.0),
            (9_000.0, 5_000.0)
        ]
    );
}

/// The pane can hold trades the client ring already folded into minis: those minis wait under
/// the crosses and settle one by one as the crosses leave, never drawn over them.
#[test]
fn minis_under_resident_crosses_wait_for_them() {
    let minis = [
        mini(DAY, 1.0, 2.0),
        mini(DAY + 5_000.0, 1.0, 2.0),
        mini(DAY + 10_000.0, 1.0, 2.0),
    ];
    // The oldest cross at +12 s: the third mini would reach it.
    assert_eq!(ready_minis(&minis, DAY + 12_000.0), 2);
    // That cross left the ring: the third settles.
    assert_eq!(ready_minis(&minis, DAY + 15_000.0), 3);
    assert_eq!(ready_minis(&minis, f64::INFINITY), 3);
    assert_eq!(ready_minis(&minis, f64::NAN), 0);
    // The placed bars match what was counted as settled.
    let mut out = Vec::new();
    place_path(&minis, DAY, DAY + 12_000.0, DAY, None, &mut out);
    assert_eq!(out.len(), 2);
}

/// A mini closed early by its successor settles as soon as the crosses clear its own bar, not five
/// seconds later: a nominal width left a blank strip at the path/cross seam.
#[test]
fn a_mini_settles_by_its_own_width() {
    let minis = [mini(DAY, 1.0, 2.0), mini(DAY + 1_000.0, 1.0, 2.0)];
    assert_eq!(ready_minis(&minis, DAY + 1_500.0), 1);
    let mut out = Vec::new();
    place_path(&minis, DAY, DAY + 1_500.0, DAY, None, &mut out);
    assert_eq!(
        out.iter()
            .map(|b| (b.t_open_rel, b.duration_ms))
            .collect::<Vec<_>>(),
        [(0.0, 1_000.0)]
    );
}

/// An append brings the successor of the last bar already drawn: that bar is cut at its open.
#[test]
fn an_appended_mini_trims_the_bar_before_it() {
    // The successor opens 1.2 s later: the drawn bar shrinks to that.
    assert_eq!(fit_before(10_000.0, 5_000.0, 11_200.0), 1_200.0);
    // A successor past the bar's end leaves it alone.
    assert_eq!(fit_before(10_000.0, 5_000.0, 17_000.0), 5_000.0);
    // Never down to zero, which the shader reads as a full-width candle.
    assert_eq!(fit_before(10_000.0, 5_000.0, 10_000.0), 1.0);
    assert_eq!(fit_before(10_000.0, 5_000.0, f32::NAN), 5_000.0);
}
