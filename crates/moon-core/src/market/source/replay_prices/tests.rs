//! Regression coverage for bounded archive price lines and unavailable replay providers.

use super::{overlay_stored_price_lines, prepare_replay_price_lines};
use crate::feed::PricePoint;
use crate::market::source::ChartHistoryBuffers;

/// Dropping archive last points or inventing marks would hide the line or fabricate spot data.
#[test]
fn replay_price_lines_use_archive_last_without_fabricating_marks() {
    let mut out = ChartHistoryBuffers {
        last_points: [
            (3000.0, 30.0),
            (1000.0, 10.0),
            (2000.0, 20.0),
            (4000.0, 40.0),
        ]
        .map(|(time_ms, price)| PricePoint { time_ms, price })
        .to_vec(),
        ..Default::default()
    };
    assert_eq!(prepare_replay_price_lines((1000, 3000), &mut out), 3);
    assert_eq!(
        out.last_points
            .iter()
            .map(|p| (p.time_ms, p.price))
            .collect::<Vec<_>>(),
        vec![(1000.0, 10.0), (2000.0, 20.0), (3000.0, 30.0)]
    );
    assert!(out.mark_points.is_empty());
}

/// Mark points must use their own prices and bounds; malformed rows cannot poison GPU geometry.
#[test]
fn replay_price_lines_clip_marks_and_discard_invalid_prices() {
    let mut out = ChartHistoryBuffers {
        mark_points: [
            (1000.0, 12.0),
            (2000.0, 22.0),
            (3000.0, f32::NAN),
            (2500.0, 0.0),
            (f64::NAN, 50.0),
        ]
        .map(|(time_ms, price)| PricePoint { time_ms, price })
        .to_vec(),
        ..Default::default()
    };
    assert_eq!(prepare_replay_price_lines((1500, 3000), &mut out), 1);
    assert_eq!(out.mark_points.len(), 1);
    assert_eq!(
        (out.mark_points[0].time_ms, out.mark_points[0].price),
        (2000.0, 22.0)
    );
    assert!(out.last_points.is_empty());
}

fn pts(line: &[PricePoint]) -> Vec<(f64, f32)> {
    line.iter().map(|p| (p.time_ms, p.price)).collect()
}

fn line(points: &[(f64, f32)]) -> Vec<PricePoint> {
    points
        .iter()
        .map(|&(time_ms, price)| PricePoint { time_ms, price })
        .collect()
}

/// Inside a recorded stretch the file replaces the ring; outside it the ring stays. The stretch
/// is held from its clipped start to its clipped end, so a trade older than the run gets a line
/// across the whole stretch instead of from its first change.
#[test]
fn stored_lines_replace_the_ring_inside_their_stretch_only() {
    use crate::market::price_recorder::{StoredPriceLines, StoredPriceSpan};
    let mut out = ChartHistoryBuffers {
        mark_points: line(&[(1_000.0, 1.0), (5_000.0, 5.0), (9_000.0, 9.0)]),
        ..Default::default()
    };
    let stored = StoredPriceLines {
        spans: vec![StoredPriceSpan {
            from_ms: 2_000,
            to_ms: 20_000,
            last: line(&[(2_000.0, 50.0)]),
            mark: line(&[(2_000.0, 40.0), (3_000.0, 41.0), (6_000.0, 42.0)]),
        }],
    };
    let capacity = overlay_stored_price_lines((0, 8_000), &stored, &mut out);
    assert_eq!(
        pts(&out.mark_points),
        vec![
            (1_000.0, 1.0),
            (2_000.0, 40.0),
            (3_000.0, 41.0),
            (6_000.0, 42.0),
            (8_000.0, 42.0)
        ]
    );
    assert_eq!(
        pts(&out.last_points),
        vec![(2_000.0, 50.0), (8_000.0, 50.0)]
    );
    assert_eq!(capacity, 5);

    // A window that starts inside the stretch opens on the value in force there: the mark set at
    // 3_000 still holds at 4_000.
    let mut out = ChartHistoryBuffers::default();
    overlay_stored_price_lines((4_000, 7_000), &stored, &mut out);
    assert_eq!(
        pts(&out.mark_points),
        vec![(4_000.0, 41.0), (6_000.0, 42.0), (7_000.0, 42.0)]
    );
}

/// A stretch outside the window leaves the ring untouched, and an empty store changes nothing.
#[test]
fn stored_lines_outside_the_window_leave_the_ring_alone() {
    use crate::market::price_recorder::{StoredPriceLines, StoredPriceSpan};
    let ring = line(&[(1_000.0, 1.0), (2_000.0, 2.0)]);
    let mut out = ChartHistoryBuffers {
        last_points: ring.clone(),
        ..Default::default()
    };
    let stored = StoredPriceLines {
        spans: vec![StoredPriceSpan {
            from_ms: 10_000,
            to_ms: 20_000,
            last: line(&[(10_000.0, 7.0)]),
            mark: Vec::new(),
        }],
    };
    overlay_stored_price_lines((0, 3_000), &stored, &mut out);
    assert_eq!(pts(&out.last_points), pts(&ring));
    overlay_stored_price_lines((0, 3_000), &StoredPriceLines::default(), &mut out);
    assert_eq!(pts(&out.last_points), pts(&ring));
}

/// An unavailable provider clears stale lines and stays retryable without replacing other data.
#[test]
fn replay_price_lines_clear_stale_buffers_when_provider_is_unavailable() {
    let source =
        crate::market::source::MarketDataSource::new(crate::market::MarketStore::shared(0.0));
    let mut out = ChartHistoryBuffers::default();
    out.last_points.push(PricePoint {
        time_ms: 2000.0,
        price: 10.0,
    });
    out.mark_points.clone_from(&out.last_points);
    out.candles.push(crate::market::ChartCandle {
        t_open_ms: 1000.0,
        ..Default::default()
    });
    assert_eq!(
        source.read_replay_price_lines(
            1,
            "SYNTHETIC",
            (1000, 3000),
            &mut out,
            &mut crate::market::ChartHistoryCursor::default(),
        ),
        None
    );
    assert!(out.last_points.is_empty() && out.mark_points.is_empty());
    assert_eq!(out.candles.len(), 1);
    assert_eq!(out.candles[0].t_open_ms, 1000.0);
}
