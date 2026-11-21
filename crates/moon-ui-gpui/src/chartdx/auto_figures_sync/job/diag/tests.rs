use moon_chart::auto_figures::tf::BarSource;
use moon_chart::auto_figures::{Found, FoundShape, Touch};
use moon_core::config::auto_figures::{DetectorParams, SwingParams, Take, TfId};
use moon_core::market::ChartCandle;

use super::{JobDiag, RowSeen, row_lines};
use crate::chartdx::auto_figures_sync::job::RowKey;

/// One hour bar at `hour` (since the epoch) with `close`, high and low a cent around it.
fn bar(hour: f64, close: f32) -> ChartCandle {
    ChartCandle {
        t_open_ms: hour * 3_600_000.0,
        open: close,
        high: close + 0.01,
        low: close - 0.01,
        close,
        volume: 1.0,
        quote_volume: 1.0,
    }
}

/// A row's block names where its bars came from and how many it read against its need, prints
/// every input bar — a ring bar's unknown close as `-` — and lists what the detector found and
/// which of it the row keeps.
///
/// The oracle: two bars, one of them without a close, one level found at 1.0, and the default
/// take keeping it (index 0) against the last price.
///
/// Breakage: a block that omits the source or the bars (the very thing the channel exists to
/// show), a ring bar printed with a made-up close, or the kept list taken from another row.
#[test]
fn a_row_block_names_its_source_bars_and_kept_lines() {
    let mut ring = bar(2.0, 1.0);
    ring.open = f32::NAN;
    ring.close = f32::NAN;
    let bars = [bar(1.0, 1.02), ring];
    let key = RowKey {
        detector: DetectorParams::Swing(SwingParams::default()),
        tf: TfId::H1,
        last_closed_ms: 7_200_000,
        chart_ready: true,
        chart_deep_history: false,
    };
    let found = [Found {
        shape: FoundShape::Horizontal { price: 1.0 },
        touches: vec![Touch {
            t_ms: 3_600_000.0,
            price: 1.0,
        }],
        strength: 1.0,
        first_ms: 3_600_000.0,
        last_ms: 3_600_000.0,
        broken: None,
        touch_volume: 0.0,
        tolerance_frac: 0.0025,
        tag: None,
        origin: None,
    }];
    let diag = JobDiag {
        label: "core/AKEUSDT".to_string(),
        takes: vec![(3, Take::default())],
    };
    let mut out = Vec::new();
    row_lines(
        &diag,
        &RowSeen {
            ix: 3,
            key: &key,
            source: BarSource::ChartResampled,
            needed: 40,
            bars: &bars,
            found: &found,
            read_failed: false,
            lagging: false,
        },
        7_200_000.0,
        &mut out,
    );
    let text = out.join("\n");
    assert!(text.contains("row 3 "), "{text}");
    assert!(text.contains("source=ChartResampled bars=2/40"), "{text}");
    assert!(text.contains("no_close=1"), "{text}");
    assert!(text.contains("found[0] level 1 touches=1"), "{text}");
    assert!(text.contains("kept against the last close"), "{text}");
    assert!(text.contains(": [0]"), "{text}");
    assert!(text.contains("1970-01-01 02:00 1.01/0.99/-"), "{text}");
}
