use super::*;
use crate::feed::Side;

const T0: i64 = 1_800_000_000_000;

fn tick(at: i64, price: f32) -> Tick {
    Tick {
        time_ms: at as f64,
        price,
        qty: 1.0,
        side: Side::Buy,
    }
}

/// A price that climbs 0.1 % a second for two minutes.
fn climb() -> Vec<Tick> {
    (0..=120)
        .map(|s| tick(T0 + s * 1_000, 100.0 * (1.0 + 0.001 * s as f32)))
        .collect()
}

#[test]
fn identical_tapes_agree_on_every_touch() {
    let needed = Coverage::one((T0, T0 + 120_000));
    let ticks = climb();
    let c = compare(
        &needed,
        Tape {
            covered: &needed,
            ticks: &ticks,
        },
        Tape {
            covered: &needed,
            ticks: &ticks,
        },
    );
    assert_eq!(c.common_ms, needed.width_ms());
    assert_eq!(c.recorded_prints, 121);
    assert_eq!(c.recorded_volume, c.captured_volume);
    assert!(c.touches.both > 0);
    assert_eq!(c.touches.only_captured + c.touches.only_recorded, 0);
    assert_eq!(c.touches.same_ms, c.touches.both);
}

#[test]
fn a_print_the_recorded_tape_lacks_is_a_fill_it_misses() {
    let needed = Coverage::one((T0, T0 + 120_000));
    let captured: Vec<Tick> = (0..=120)
        .map(|s| tick(T0 + s * 1_000, 100.0))
        .chain([tick(T0 + 30_500, 101.5)])
        .collect::<Vec<_>>();
    let mut captured = captured;
    captured.sort_by(|a, b| a.time_ms.total_cmp(&b.time_ms));
    // The same flat tape without the spike: the coarsened archive folded it away.
    let recorded: Vec<Tick> = (0..=120).map(|s| tick(T0 + s * 1_000, 100.0)).collect();
    let c = compare(
        &needed,
        Tape {
            covered: &needed,
            ticks: &recorded,
        },
        Tape {
            covered: &needed,
            ticks: &captured,
        },
    );
    assert!(c.touches.only_captured > 0, "{:?}", c.touches);
    assert_eq!(
        c.touches.only_recorded, 0,
        "a coarser tape never invents a fill"
    );
    assert_eq!(c.captured_prints, c.recorded_prints + 1);
}

#[test]
fn only_the_stretch_both_cover_is_compared() {
    let needed = Coverage::one((T0, T0 + 120_000));
    let ticks = climb();
    // The recorder lost the first minute.
    let recorded_cov = Coverage::one((T0 + 60_000, T0 + 120_000));
    let recorded: Vec<Tick> = ticks
        .iter()
        .filter(|t| t.time_ms as i64 >= T0 + 60_000)
        .copied()
        .collect();
    let c = compare(
        &needed,
        Tape {
            covered: &recorded_cov,
            ticks: &recorded,
        },
        Tape {
            covered: &needed,
            ticks: &ticks,
        },
    );
    assert_eq!(c.recorded_ms, recorded_cov.width_ms());
    assert_eq!(c.common_ms, recorded_cov.width_ms());
    assert_eq!(c.captured_prints, c.recorded_prints);
    assert_eq!(c.touches.only_captured + c.touches.only_recorded, 0);
}
