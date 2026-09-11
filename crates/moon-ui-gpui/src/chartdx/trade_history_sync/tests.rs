//! Regressions for trade-history drawing inputs and filtering.

use std::collections::HashMap;

use super::{trade_kind_visible, trade_mark};
use moon_core::config::ChartGraphicsCfg;
use moon_core::db::{ChartTradeRecord, OffsetSegment, ReportAxis};

/// Moving this filter back into the durable QUERY, or inverting either checkbox, must fail here.
///
/// The pair is the graphics popup's two trade-kind boxes, and the rule is one box per kind with no
/// interaction between them: unticking "real" must not touch emulator marks and vice versa, and
/// unticking both must leave nothing drawn rather than everything. The LOCATION matters as much as
/// the truth table - a display toggle must never decide which rows the history was read with,
/// because the 1000-row cap is applied after any SQL predicate and hiding one kind would then
/// surface older trades of the other that had been truncated away.
#[test]
fn each_trade_kind_checkbox_hides_only_its_own_kind() {
    let cfg = |real: bool, emulator: bool| ChartGraphicsCfg {
        show_real_trades: real,
        show_emulator_trades: emulator,
        ..ChartGraphicsCfg::default()
    };

    // Shipped default: everything visible.
    assert!(trade_kind_visible(&ChartGraphicsCfg::default(), false));
    assert!(trade_kind_visible(&ChartGraphicsCfg::default(), true));

    // One box each, no crosstalk.
    assert!(trade_kind_visible(&cfg(true, false), false));
    assert!(!trade_kind_visible(&cfg(true, false), true));
    assert!(!trade_kind_visible(&cfg(false, true), false));
    assert!(trade_kind_visible(&cfg(false, true), true));

    // Both off draws NOTHING - never everything, which is what a single tri-state predicate that
    // cannot express "neither" would have produced.
    assert!(!trade_kind_visible(&cfg(false, false), false));
    assert!(!trade_kind_visible(&cfg(false, false), true));
}

/// `chartdx/trade_history_sync.rs:trade_mark` must correct seconds before scaling milliseconds.
///
/// Replacing the two `axis.to_utc` calls with raw record dates would put entry and exit arrows on
/// the wrong candle whenever a core reports a non-zero clock offset.
#[test]
fn trade_mark_places_a_clock_skewed_trade_on_its_true_utc_candles() {
    let record = ChartTradeRecord {
        record_id: 9,
        core_uid: 42,
        coin: "BTCUSDT".into(),
        buy_date: 1_700_000_000,
        close_date: 1_700_000_900,
        buy_price: 63_000.0,
        sell_price: 63_100.0,
        quantity: 0.25,
        is_short: false,
        emulator: false,
        profit: Some(25.0),
        quote: None,
        profit_pct: Some(0.16),
    };
    let axis = ReportAxis::from_measured(
        HashMap::from([(
            record.core_uid,
            vec![OffsetSegment {
                from_utc: 0,
                offset_secs: 3_600,
            }],
        )]),
        chrono_tz::UTC,
    );

    let mark = trade_mark(&record, &axis);

    assert_eq!(mark.buy_ms, 1_699_996_400_000);
    assert_eq!(mark.close_ms, 1_699_997_300_000);
}

/// A late market-only match must replace both arrows and connector without a session refresh.
/// Prefix/suffix marker sentinels also guard the existing overlay paint order in both themes.
#[test]
fn market_only_match_republishes_geometry_and_preserves_other_overlays() {
    use crate::chartdx::{ChartEngine, PaneRender};
    use moon_chart::layers::MarkerInstance;
    use moon_chart::trade_marks::TapePrint;
    use moon_chart::view::ChartView;
    use moon_core::config::ChartThemeSet;
    use std::rc::Rc;

    for light in [false, true] {
        let theme = ChartThemeSet::default().get(light).clone();
        let engine = ChartEngine::new(1_700_000_000_000.0, theme);
        let mut data = engine.data.borrow_mut();
        data.trade_history = Rc::new(vec![ChartTradeRecord {
            record_id: 1,
            core_uid: 42,
            coin: "TEST".into(),
            buy_date: 1_700_000_031,
            close_date: 1_700_000_032,
            buy_price: 0.14275000000000002,
            sell_price: 0.14142000000000005,
            quantity: 10.0,
            is_short: true,
            emulator: false,
            profit: None,
            quote: None,
            profit_pct: None,
        }]);
        let mut pane = PaneRender::new();
        pane.core = Some(42);
        pane.market = "TEST".into();
        let mut view = ChartView::new(1_700_000_000_000.0);
        view.px_per_ms = 0.05;
        view.px_per_price = 100_000.0;
        let sentinel = |t| MarkerInstance::arrow(t, 1.0, 6.0, 4.5, 4.0, 1, [1.0; 4]);
        let mut markers = vec![sentinel(123.0)];
        let mut segs = Vec::new();
        pane.trade_geometry =
            data.append_trade_history_geometry(0, 42, &view, &mut pane, &mut markers, &mut segs);
        markers.push(sentinel(456.0));
        pane.trade_userdata
            .set(&mut pane.layers, Vec::new(), Vec::new(), segs, markers);
        assert!(pane.live_trade_snap.observe([
            TapePrint {
                t_ms: 1_700_000_031_766,
                price: f64::from(0.14275f32)
            },
            TapePrint {
                t_ms: 1_700_000_032_650,
                price: f64::from(0.14142f32)
            },
        ]));
        data.refresh_live_trade_geometry(0, 42, &view, &mut pane);
        assert_eq!(pane.trade_userdata.markers.first().unwrap().t_rel, 123.0);
        assert_eq!(pane.trade_userdata.markers.last().unwrap().t_rel, 456.0);
        let line = &pane.trade_userdata.segs[0];
        assert_eq!((line.t0_rel, line.t1_rel), (31_766.0, 32_650.0));
        assert_eq!((line.p0, line.p1), (0.14275f32, 0.14142f32));
        let mut times = pane
            .trade_geometry
            .clusters
            .iter()
            .map(|c| c.t_ms as i64)
            .collect::<Vec<_>>();
        times.sort();
        assert_eq!(times, vec![1_700_000_031_766, 1_700_000_032_650]);
        assert_eq!(pane.trade_geometry.sources, vec![0]);
        assert_eq!(pane.trade_userdata.markers.len(), 4);
        // Same generation on a DIFFERENT provider must not retain the prior tape's estimates.
        pane.live_trade_snap = Default::default();
        data.refresh_live_trade_geometry(0, 42, &view, &mut pane);
        assert_eq!(pane.trade_userdata.segs[0].t0_rel, 31_000.0);
    }
}
