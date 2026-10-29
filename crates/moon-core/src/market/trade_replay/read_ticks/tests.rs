//! Frozen full-scan parity for the replay's ordered tick-range reader.

use super::*;

/// Skipping whole prepared prefixes must preserve fractional edges, bad-price filtering and Y fit.
#[test]
fn chart_market_prepared_slice_matches_frozen_filter_and_price_scan() {
    let ticks: Vec<_> = [-1.9, -1.0, -0.9, 0.0, 0.9, 1.0, 1.9, 2.0, 3.0]
        .into_iter()
        .enumerate()
        .map(|(index, time_ms)| Tick {
            time_ms,
            price: if index == 4 {
                f32::NAN
            } else if index == 5 {
                0.0
            } else {
                10.0 + index as f32
            },
            qty: index as f32,
            side: crate::feed::Side::Buy,
        })
        .collect();
    for from in -3..5 {
        for to in from..5 {
            for visible in [None, Some((from, to)), Some((0, 1))] {
                let expected: Vec<_> = ticks
                    .iter()
                    .filter(|tick| {
                        tick.time_ms.is_finite()
                            && tick.price.is_finite()
                            && tick.price > 0.0
                            && (tick.time_ms as i64) >= from
                            && (tick.time_ms as i64) <= to
                    })
                    .copied()
                    .collect();
                let expected_range = visible.and_then(|(from, to)| {
                    expected
                        .iter()
                        .filter(|tick| {
                            tick.price.is_finite()
                                && tick.price > 0.0
                                && tick.time_ms >= from as f64
                                && tick.time_ms <= to as f64
                        })
                        .fold(None, |range: Option<(f32, f32)>, tick| {
                            Some(match range {
                                None => (tick.price, tick.price),
                                Some((lo, hi)) => (lo.min(tick.price), hi.max(tick.price)),
                            })
                        })
                });
                let mut actual = Vec::new();
                assert_eq!(
                    copy_ticks(&ticks, (from, to), visible, &mut actual),
                    expected_range
                );
                let values = |ticks: &[Tick]| {
                    ticks
                        .iter()
                        .map(|t| (t.time_ms, t.price, t.qty, t.side))
                        .collect::<Vec<_>>()
                };
                assert_eq!(values(&actual), values(&expected));
            }
        }
    }
}
