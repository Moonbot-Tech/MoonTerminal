//! Upload-retention and candle-visibility regression checks for the drawing magnet.

use super::{FigureSnapData, candle_nodes};
use crate::chartdx::types::{CandleGpu, CandleStyleGpu};

/// A coarse candle whose plotted centre differs from the selected timeframe's centre.
fn candle() -> CandleGpu {
    CandleGpu {
        t_open_rel: 100.0,
        open: 20.0,
        high: 24.0,
        low: 18.0,
        close: 22.0,
        volume: 1.0,
        tf_rel: 60.0,
    }
}

/// Hidden candles and hidden wicks cannot attract nodes to prices the chart does not show.
#[test]
fn candle_candidates_follow_hidden_zone_wicks_and_own_timeframe() {
    let mut style = CandleStyleGpu {
        tf_rel_ms: 10.0,
        zone_start_rel: 100.0,
        hide_start_rel: f32::MAX,
        fill_alpha: 1.0,
        ..Default::default()
    };
    let row = candle();
    let nodes = candle_nodes(&row, 1_000.0, style).collect::<Vec<_>>();
    assert_eq!(
        nodes.iter().map(|node| node.price).collect::<Vec<_>>(),
        vec![20.0, 22.0]
    );
    assert!(nodes.iter().all(|node| node.time_ms == 1_130.0));
    style.wicks_in_zone = 1.0;
    assert_eq!(
        candle_nodes(&row, 1_000.0, style)
            .map(|node| node.price)
            .collect::<Vec<_>>(),
        vec![20.0, 22.0, 24.0, 18.0]
    );
    style.hide_start_rel = 100.0;
    assert_eq!(candle_nodes(&row, 1_000.0, style).count(), 0);
    style.hide_start_rel = f32::MAX;
    style.fill_alpha = 0.0;
    style.wicks_in_zone = 0.0;
    assert_eq!(candle_nodes(&row, 1_000.0, style).count(), 0);
    style.mode = 1.0;
    assert_eq!(candle_nodes(&row, 1_000.0, style).count(), 2);
}

/// Clearing a candle layer must not expose its previously retained OHLC after a mode switch.
#[test]
fn candle_replacement_and_clear_retire_previous_series() {
    let mut data = FigureSnapData::default();
    data.set_candles(&[candle()]);
    data.set_candles(&[]);
    assert!(data.candles.is_empty());
    data.set_candles(&[candle()]);
    data.clear_candles();
    assert!(data.candles.is_empty());
}

/// DX11 must not snap to old candles dropped by its fixed-size upload buffer.
#[test]
fn retained_candles_match_platform_upload_tail() {
    let rows = (0..4100)
        .map(|index| CandleGpu {
            t_open_rel: index as f32,
            ..candle()
        })
        .collect::<Vec<_>>();
    let mut data = FigureSnapData::default();
    data.set_candles(&rows);
    let (count, first) = if cfg!(windows) {
        (4096, 4.0)
    } else {
        (4100, 0.0)
    };
    assert_eq!(data.candles.len(), count);
    assert_eq!(data.candles.first().map(|row| row.t_open_rel), Some(first));
    assert_eq!(data.candles.last().map(|row| row.t_open_rel), Some(4099.0));
}
