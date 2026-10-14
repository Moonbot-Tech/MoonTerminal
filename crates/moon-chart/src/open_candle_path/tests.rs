use super::*;

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
