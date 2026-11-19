use super::super::swing::Swing;
use super::super::test_bars::ace;
use super::super::{DetectCtx, Detector, FoundShape};
use super::*;
use moon_core::config::auto_figures::{DetectorParams, SwingParams};

fn level(price: f64, strength: f32) -> Found {
    Found {
        shape: FoundShape::Horizontal { price },
        touches: Vec::new(),
        strength,
        first_ms: 0.0,
        last_ms: 0.0,
        broken: None,
        touch_volume: 0.0,
        tolerance_frac: 0.0,
        tag: None,
    }
}

fn take(count: u8, side: TakeSide, order: TakeOrder) -> Take {
    Take {
        count,
        side,
        order,
        ..Take::default()
    }
}

/// Levels at 90, 95, 99 below and 101, 110, 120 above a price of 100.
fn ladder() -> Vec<Found> {
    vec![
        level(120.0, 5.0),
        level(90.0, 9.0),
        level(101.0, 1.0),
        level(99.0, 2.0),
        level(110.0, 7.0),
        level(95.0, 3.0),
    ]
}

/// Nearest per side: "two above and two below" is `count = 2, side = Both`.
///
/// Breakage: `count` applied to both sides together, or distance measured without `abs`.
#[test]
fn nearest_two_per_side() {
    let got = select(
        &ladder(),
        &take(2, TakeSide::Both, TakeOrder::Nearest),
        100.0,
        0.0,
    );
    assert_eq!(got, vec![2, 4, 3, 5], "101, 110 above; 99, 95 below");
}

/// Strongest ranks by the detector's strength, one side at a time.
///
/// Breakage: the sort direction reversed, or the side filter skipped.
#[test]
fn strongest_on_one_side() {
    let f = ladder();
    assert_eq!(
        select(
            &f,
            &take(1, TakeSide::Above, TakeOrder::Strongest),
            100.0,
            0.0
        ),
        vec![4]
    );
    assert_eq!(
        select(
            &f,
            &take(5, TakeSide::Below, TakeOrder::Strongest),
            100.0,
            0.0
        ),
        vec![1, 5, 3]
    );
}

/// No count, no reference price, or a figure with no price: nothing kept, never a panic.
///
/// Breakage: a NaN reference sorting every line onto one side, or a NaN strength ranked first.
#[test]
fn degenerate_inputs() {
    let f = ladder();
    let both = take(2, TakeSide::Both, TakeOrder::Strongest);
    assert!(select(&f, &take(0, TakeSide::Both, TakeOrder::Nearest), 100.0, 0.0).is_empty());
    assert!(select(&f, &both, f64::NAN, 0.0).is_empty());
    assert!(select(&f, &both, 0.0, 0.0).is_empty());
    assert!(select(&[], &both, 100.0, 0.0).is_empty());
    let odd = vec![
        level(f64::NAN, 1.0),
        level(105.0, f32::NAN),
        level(106.0, 1.0),
    ];
    assert_eq!(select(&odd, &both, 100.0, 0.0), vec![2, 1]);
}

/// "Strongest by volume" ranks by the money traded on the touching bars, not by the touch count:
/// the level touched most is NOT the one kept when another took more volume. Ties fall back to
/// nearest first.
///
/// Breakage: `strength` ignored (the touch rank wins), the volume read from the wrong field, or
/// the sort direction reversed.
#[test]
fn strongest_by_volume_ranks_by_touch_volume() {
    let with_volume = |price: f64, strength: f32, touch_volume: f64| Found {
        touch_volume,
        ..level(price, strength)
    };
    let found = vec![
        // Most touches, least money.
        with_volume(101.0, 9.0, 10.0),
        with_volume(110.0, 2.0, 5_000.0),
        with_volume(105.0, 3.0, 800.0),
        // Ties with the next on volume; nearer, so it goes first.
        with_volume(103.0, 1.0, 800.0),
    ];
    let by_touches = take(2, TakeSide::Above, TakeOrder::Strongest);
    assert_eq!(select(&found, &by_touches, 100.0, 0.0), vec![0, 2]);
    let by_volume = Take {
        strength: StrengthBasis::Volume,
        ..by_touches
    };
    assert_eq!(select(&found, &by_volume, 100.0, 0.0), vec![1, 3]);
    // Under `Nearest` the basis changes nothing.
    let nearest = Take {
        order: TakeOrder::Nearest,
        ..by_volume
    };
    assert_eq!(select(&found, &nearest, 100.0, 0.0), vec![0, 3]);
}

/// A sloped line is sided by its projection at the moment asked for, not by its endpoints.
///
/// Breakage: siding a line by `p0` or `p1`.
#[test]
fn sloped_lines_are_sided_at_the_moment() {
    let line = Found {
        shape: FoundShape::Sloped {
            t0_ms: 0.0,
            p0: 90.0,
            t1_ms: 10.0,
            p1: 95.0,
        },
        ..level(0.0, 1.0)
    };
    let above = take(1, TakeSide::Above, TakeOrder::Nearest);
    assert!(select(std::slice::from_ref(&line), &above, 100.0, 10.0).is_empty());
    assert_eq!(
        select(&[line], &above, 100.0, 30.0),
        vec![0],
        "105 at t = 30"
    );
}

/// On the real `chart-ace` levels: never more than `count` per side, every kept line on its side.
///
/// Breakage: a side filter that lets a line through on the wrong side, or a per-side cap applied
/// once for both.
#[test]
fn real_levels_respect_count_and_side() {
    let bars = ace(5);
    let found = Swing.detect(
        &bars,
        &DetectorParams::Swing(SwingParams {
            lookback_bars: 2_000,
            ..SwingParams::default()
        }),
        &DetectCtx {
            tf_ms: 300_000,
            now_ms: 0.0,
        },
    );
    assert!(found.len() > 4, "enough levels to cut: {}", found.len());
    let price = f64::from(bars[bars.len() - 1].close);
    for count in [1u8, 2, 3] {
        for order in [TakeOrder::Nearest, TakeOrder::Strongest] {
            let got = select(&found, &take(count, TakeSide::Both, order), price, 0.0);
            let above = got
                .iter()
                .filter(|&&i| found[i].shape.price_at(0.0).is_some_and(|p| p >= price))
                .count();
            let below = got.len() - above;
            assert!(above <= usize::from(count) && below <= usize::from(count));
            let only_below = select(&found, &take(count, TakeSide::Below, order), price, 0.0);
            assert!(
                only_below
                    .iter()
                    .all(|&i| found[i].shape.price_at(0.0).is_some_and(|p| p < price))
            );
        }
    }
}

/// A horizontal two-line formation from `lower` to `upper`.
fn pair(upper: f64, lower: f64) -> Found {
    let side = |p: f64| super::super::Seg {
        t0_ms: 0.0,
        p0: p,
        t1_ms: 1.0,
        p1: p,
    };
    Found {
        shape: FoundShape::Pair {
            upper: side(upper),
            lower: side(lower),
        },
        ..level(upper, 1.0)
    }
}

/// A formation is kept on the side of its NEARER line: a channel 99..101 is above a price of 100.6
/// (its top is nearer) and below one of 99.3 (its bottom is), and its distance is to that line — so
/// with the 101 level beside it, nearest first puts whichever is closer first.
///
/// Breakage: the side taken from the upper line alone (a price inside the channel would always see
/// it above), or the distance measured to the far side.
#[test]
fn a_formation_takes_the_side_of_its_nearer_line() {
    let both = take(1, TakeSide::Both, TakeOrder::Nearest);
    assert_eq!(select(&[pair(101.0, 99.0)], &both, 100.6, 0.0), [0]);
    let above = take(1, TakeSide::Above, TakeOrder::Nearest);
    assert_eq!(select(&[pair(101.0, 99.0)], &above, 100.6, 0.0), [0]);
    assert!(select(&[pair(101.0, 99.0)], &above, 99.3, 0.0).is_empty());
    let below = take(1, TakeSide::Below, TakeOrder::Nearest);
    assert_eq!(select(&[pair(101.0, 99.0)], &below, 99.3, 0.0), [0]);
    // At 99.3 the channel's bottom (0.3 away) is nearer than a lone level at 98.9 (0.4 away).
    let found = [level(98.9, 1.0), pair(101.0, 99.0)];
    assert_eq!(select(&found, &below, 99.3, 0.0), [1]);
}
