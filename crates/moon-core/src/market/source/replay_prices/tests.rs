//! Regression coverage for bounded archive price lines and unavailable replay providers.

use super::prepare_replay_price_lines;
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
