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

/// `figure_snap.rs:FigureSnapData::candles_near` dropping its `max_tf` widening (or its 1 ms
/// slack) cuts a candle that opened left of the magnet window but plots its node inside it, so the
/// drawing magnet misses a candle at the window edge. The windowed slice must yield exactly the
/// nodes the old unwindowed scan over every retained row yielded, for random windows.
#[test]
fn windowed_candidates_equal_the_unwindowed_scan() {
    let mut state = 0x2468_ACE0_u64;
    let mut next = move |n: u64| {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state % n
    };
    // Ascending opens, mixed own widths, some rows drawn at the series timeframe instead.
    let widths = [0.0, 60_000.0, 300_000.0, 3_600_000.0];
    let mut t = 0.0f32;
    let rows = (0..4_000)
        .map(|_| {
            t += (next(4) * 60_000) as f32;
            let lo = 10.0 + next(1_000) as f32;
            CandleGpu {
                t_open_rel: t,
                open: lo,
                high: lo + 5.0,
                low: lo - 5.0,
                close: lo + 1.0,
                volume: 1.0,
                tf_rel: widths[next(4) as usize],
            }
        })
        .collect::<Vec<_>>();
    let mut data = FigureSnapData::default();
    data.set_candles(&rows);
    let style = CandleStyleGpu {
        tf_rel_ms: 120_000.0,
        zone_start_rel: f32::MAX,
        hide_start_rel: f32::MAX,
        fill_alpha: 1.0,
        wicks_in_zone: 1.0,
        ..Default::default()
    };
    let epoch = 1.7e12;
    let key = |n: &moon_core::figures::FigNode| (n.time_ms.to_bits(), n.price.to_bits());
    let mut edge_hits = 0;
    for q in 0..3_000 {
        let left = next(t as u64) as f64 + if q % 2 == 0 { 0.0 } else { 0.25 };
        let right = left + next(20) as f64 * 30_000.0;
        let within = |n: &moon_core::figures::FigNode| {
            n.time_ms >= epoch + left && n.time_ms <= epoch + right
        };
        let mut want = data
            .candles
            .iter()
            .flat_map(|row| candle_nodes(row, epoch, style))
            .filter(within)
            .map(|n| key(&n))
            .collect::<Vec<_>>();
        let near = data.candles_near(left, right, style.tf_rel_ms);
        let mut got = near
            .iter()
            .flat_map(|row| candle_nodes(row, epoch, style))
            .filter(within)
            .map(|n| key(&n))
            .collect::<Vec<_>>();
        want.sort_unstable();
        got.sort_unstable();
        assert_eq!(got, want, "window [{left}, {right}]");
        edge_hits += usize::from(
            near.first()
                .is_some_and(|row| f64::from(row.t_open_rel) < left && !want.is_empty()),
        );
        if q % 1_000 == 0 {
            println!(
                "[figure_snap] before={} after={}",
                data.candles.len(),
                near.len()
            );
        }
    }
    assert!(edge_hits > 0);
}
