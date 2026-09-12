use super::*;

fn bucket(t_open_ms: i64, tf_ms: i64, buy: f32, sell: f32) -> SideVolumeBucket {
    SideVolumeBucket {
        t_open_ms,
        tf_ms,
        buy_quote: buy,
        sell_quote: sell,
    }
}

/// Zero is Auto and must survive; anything else lands on a listed width, so a hand-typed `20`
/// draws as the fifteen-second step the popup lights.
#[test]
fn snap_keeps_auto_and_lands_typed_widths_on_the_list() {
    assert_eq!(snap_tf_s(0), 0);
    assert_eq!(snap_tf_s(20), 15);
    assert_eq!(snap_tf_s(100), 60);
    assert_eq!(snap_tf_s(10_000), 300);
    for c in SIDE_TF_CHOICES_S {
        assert_eq!(snap_tf_s(c), c);
    }
}

/// Auto follows Moonbot's table row for row, on the visible span rounded to whole minutes.
#[test]
fn auto_follows_the_moonbot_table() {
    let minutes = |m: f64| m * 60_000.0;
    assert_eq!(auto_tf_ms(minutes(1.0)), 2_000);
    assert_eq!(auto_tf_ms(minutes(4.4)), 2_000);
    // 4.5 minutes rounds to 5: the second row.
    assert_eq!(auto_tf_ms(minutes(4.5)), 5_000);
    assert_eq!(auto_tf_ms(minutes(9.0)), 5_000);
    assert_eq!(auto_tf_ms(minutes(10.0)), 15_000);
    assert_eq!(auto_tf_ms(minutes(19.0)), 15_000);
    assert_eq!(auto_tf_ms(minutes(20.0)), 30_000);
    assert_eq!(auto_tf_ms(minutes(39.0)), 30_000);
    assert_eq!(auto_tf_ms(minutes(40.0)), 60_000);
    assert_eq!(auto_tf_ms(minutes(59.0)), 60_000);
    assert_eq!(auto_tf_ms(minutes(60.0)), 180_000);
    assert_eq!(auto_tf_ms(minutes(119.0)), 180_000);
    assert_eq!(auto_tf_ms(minutes(120.0)), 300_000);
    // Moonbot never goes past five minutes on Auto, however wide the span.
    assert_eq!(auto_tf_ms(minutes(60.0 * 24.0 * 7.0)), 300_000);
}

/// Moonbot's own zoom ladder (1 m, 1.4 m, 2.8 m ... 6 h) lands on the rows it documents.
#[test]
fn auto_on_the_moonbot_zoom_ladder() {
    let rows: [(f64, i64); 10] = [
        (1.0, 2_000),
        (1.4, 2_000),
        (2.8, 2_000),
        (5.6, 5_000),
        (11.0, 15_000),
        (22.0, 30_000),
        (45.0, 60_000),
        (90.0, 180_000),
        (180.0, 300_000),
        (360.0, 300_000),
    ];
    for (minutes, tf) in rows {
        assert_eq!(auto_tf_ms(minutes * 60_000.0), tf, "{minutes} min visible");
    }
}

/// The two-second row is served as stated on one-second slots; a hand-picked interval passes
/// through untouched.
#[test]
fn effective_interval_follows_auto_and_the_hand_pick() {
    assert_eq!(effective_tf_ms(0, 60_000.0), 2_000);
    assert_eq!(effective_tf_ms(0, 30.0 * 60_000.0), 30_000);
    assert_eq!(effective_tf_ms(180, 60_000.0), 180_000);
}

/// A broken span falls to the narrowest row instead of a panic.
#[test]
fn auto_saturates_on_a_broken_span() {
    assert_eq!(auto_tf_ms(f64::NAN), 2_000);
    assert_eq!(auto_tf_ms(0.0), 2_000);
    assert_eq!(auto_tf_ms(-5.0), 2_000);
}

/// The sums are sampled once per slot until a pixel outgrows a slot, then once per pixel.
#[test]
fn sampling_follows_the_pixel_once_it_outgrows_a_slot() {
    assert_eq!(sample_step_ms(1.0), 1_000);
    assert_eq!(sample_step_ms(0.001), 1_000);
    // 1 px per 12 s: twelve slots per pixel.
    assert_eq!(sample_step_ms(1.0 / 12_000.0), 12_000);
    assert_eq!(sample_step_ms(f32::NAN), 1_000);
    assert_eq!(sample_step_ms(0.0), 1_000);
}

/// Overlaid columns are as tall as their larger side; stacked ones as tall as both. The same
/// buckets therefore scale differently, which is what the scale label has to reflect.
#[test]
fn the_kind_decides_what_the_maximum_measures() {
    let buckets = [bucket(0, 5_000, 10.0, 4.0), bucket(5_000, 5_000, 3.0, 9.0)];
    assert_eq!(visible_side_max(&buckets, 0.0, 10_000.0, false), Some(10.0));
    assert_eq!(visible_side_max(&buckets, 0.0, 10_000.0, true), Some(14.0));
}

/// Only buckets on screen scale the band, and an empty screen scales nothing.
#[test]
fn only_visible_buckets_count_and_an_empty_window_is_none() {
    let buckets = [
        bucket(0, 5_000, 100.0, 0.0),
        bucket(50_000, 5_000, 1.0, 1.0),
    ];
    assert_eq!(
        visible_side_max(&buckets, 48_000.0, 60_000.0, false),
        Some(1.0)
    );
    assert_eq!(
        visible_side_max(&buckets, 100_000.0, 200_000.0, false),
        None
    );
    assert_eq!(
        visible_side_max(&[bucket(0, 5_000, 0.0, 0.0)], 0.0, 1.0, true),
        None
    );
}

/// A negative or non-finite side cannot make a column: it reads as nothing on that side.
#[test]
fn broken_sides_do_not_scale_the_band() {
    let buckets = [
        bucket(0, 5_000, f32::NAN, 2.0),
        bucket(5_000, 5_000, -5.0, 1.0),
    ];
    assert_eq!(visible_side_max(&buckets, 0.0, 10_000.0, true), Some(2.0));
}

/// The readout names the bucket under the pointer by its half-open span.
#[test]
fn bucket_at_uses_the_half_open_span() {
    let buckets = [bucket(0, 5_000, 1.0, 0.0), bucket(5_000, 5_000, 2.0, 0.0)];
    assert_eq!(bucket_at(&buckets, 4_999.0).map(|b| b.buy_quote), Some(1.0));
    assert_eq!(bucket_at(&buckets, 5_000.0).map(|b| b.buy_quote), Some(2.0));
    assert_eq!(bucket_at(&buckets, 10_000.0), None);
    assert_eq!(bucket_at(&buckets, f64::NAN), None);
}
