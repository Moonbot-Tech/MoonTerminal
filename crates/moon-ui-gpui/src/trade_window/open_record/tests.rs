//! Fixture coverage for the period neighbours a trade window is opened with.

use super::{RecordKey, RecordOpenGeneration, period_history, prepare_history};
use crate::chartdx::{ChartEngine, trade_history_sync::PreparedTradeHistory};
use gpui::{Bounds, GpuFrameInfo, point, px, size};
use moon_core::config::{ChartGraphicsCfg, ChartTheme, TradeHistoryStyle};
use moon_core::db::{
    ChartTradeRecord, OffsetSegment, ReportAxis, ReportFilter, ReportStrategyKey, SideFilter,
};
use moon_core::market::{MarketDataSource, MarketStore};
use rusqlite::Connection;
use std::rc::Rc;
use std::time::Instant;

/// Create closed-trade fixtures with real Report column names and no live application state.
fn fixture() -> Connection {
    let conn = Connection::open_in_memory().expect("fixture DB");
    conn.execute_batch(
        "CREATE TABLE orders_rep (
            core_uid INTEGER, newrecid INTEGER, coin TEXT, buydate INTEGER, closedate INTEGER,
            buyprice REAL, sellprice REAL, quantity REAL, isshort INTEGER,
            emulator INTEGER, deleted INTEGER, strategyid INTEGER
        );
        INSERT INTO orders_rep VALUES
            (7, 1, 'BTC', 50, 100, 10, 12, 2, 0, 0, 0, 4),
            (7, 2, 'BTC', 200, 250, 10, 12, 2, 0, 0, 0, 4),
            (7, 3, 'BTC', 50, 250, 10, 12, 2, 0, 0, 0, 4),
            (7, 4, 'BTC', 201, 250, 10, 12, 2, 0, 0, 0, 4),
            (7, 5, 'BTC', 50, 99, 10, 12, 2, 0, 0, 0, 4),
            (8, 6, 'BTC', 110, 120, 10, 12, 2, 0, 0, 0, 4),
            (7, 7, 'BTC-PERP', 110, 120, 10, 12, 2, 0, 0, 0, 4),
            (7, 8, 'BTC', 110, 120, 10, 12, 2, 1, 0, 0, 4),
            (7, 9, 'BTC', 110, 120, 10, 12, 2, 0, 1, 0, 4),
            (7, 10, 'BTC', 110, 120, 10, 12, 2, 0, 0, 1, 4),
            (7, 11, 'BTC', 110, 120, 10, 12, 2, 0, 0, 0, 5),
            (7, 12, 'BTC', 110, 0, 10, 12, 2, 0, 0, 0, 4);",
    )
    .expect("seed neighbours");
    conn
}

/// Close-only selection loses id 2; widening scope admits independently seeded decoys.
#[test]
fn neighbours_respect_either_endpoint_and_report_predicates() {
    let conn = fixture();
    let filter = ReportFilter {
        date_from: Some(100),
        date_to: Some(200),
        side: SideFilter::Long,
        emulator: Some(false),
        strategies: Some(vec![ReportStrategyKey {
            core_uid: 7,
            strategy_id: 4,
        }]),
        ..ReportFilter::default()
    };
    let rows = period_history(&conn, 7, "btc", &filter).expect("period history");
    assert_eq!(
        rows.iter().map(|r| r.record_id).collect::<Vec<_>>(),
        vec![2, 1]
    );
}

/// Restoring the old 1,000-record cap drops the oldest clicked trade and period neighbours.
#[test]
fn neighbours_include_more_than_the_old_history_cap() {
    let conn = fixture();
    conn.execute_batch(
        "DELETE FROM orders_rep;
        WITH RECURSIVE n(x) AS (SELECT 1 UNION ALL SELECT x + 1 FROM n WHERE x < 1001)
        INSERT INTO orders_rep SELECT 7, x, 'BTC', 100, 200 + x, 10, 12, 2, 0, 0, 0, 4 FROM n;",
    )
    .expect("seed more than old cap");
    let filter = ReportFilter {
        date_from: Some(100),
        date_to: Some(100),
        ..ReportFilter::default()
    };
    let rows = period_history(&conn, 7, "BTC", &filter).expect("all neighbours");
    assert_eq!(rows.len(), 1001);
    assert_eq!(rows.last().map(|r| r.record_id), Some(1));
}

/// Comparing raw stamps against UTC bounds loses boundary trades on an offset core.
#[test]
fn neighbours_use_the_report_axis_for_inclusive_bounds() {
    let conn = fixture();
    let axis = ReportAxis::from_measured(
        std::collections::HashMap::from([(
            7,
            vec![OffsetSegment {
                from_utc: 0,
                offset_secs: 3600,
            }],
        )]),
        chrono_tz::UTC,
    );
    let filter = ReportFilter {
        date_from: Some(-3500),
        date_to: Some(-3400),
        axis,
        side: SideFilter::Long,
        emulator: Some(false),
        strategies: Some(vec![ReportStrategyKey {
            core_uid: 7,
            strategy_id: 4,
        }]),
        ..ReportFilter::default()
    };
    let rows = period_history(&conn, 7, "BTC", &filter).expect("offset period");
    assert_eq!(
        rows.iter().map(|r| r.record_id).collect::<Vec<_>>(),
        vec![2, 1]
    );
}

/// Run a headless chart check on the stack size required by the Windows chart tests.
fn chart_thread(check: fn()) {
    std::thread::Builder::new()
        .stack_size(16 * 1024 * 1024)
        .spawn(check)
        .expect("bench thread")
        .join()
        .expect("bench result");
}

/// Seed the same period and record order used by the large-history benchmark.
fn large_fixture() -> Connection {
    let conn = fixture();
    conn.execute_batch(
        "DELETE FROM orders_rep;
        WITH RECURSIVE n(x) AS (SELECT 1 UNION ALL SELECT x + 1 FROM n WHERE x < 50000)
        INSERT INTO orders_rep
        SELECT 7, x, 'BTC', x, x + 60, 10, 12, 2, 0, 0, 0, 4 FROM n;",
    )
    .expect("seed 50,000 history rows");
    conn
}

/// Build and frame a frozen viewer without Backend or GPU wiring.
fn chart(
    history: Rc<Vec<ChartTradeRecord>>,
    prepared: Option<&Rc<PreparedTradeHistory>>,
    axis: ReportAxis,
    (start, end): (f64, f64),
    graphics: ChartGraphicsCfg,
) -> ChartEngine {
    let mut engine = ChartEngine::new(0.0, ChartTheme::default());
    engine.open(7, "BTCUSDT");
    engine.set_historical(true);
    engine.set_follow(false, end);
    engine.set_report_axis(axis);
    engine.set_chart_graphics(graphics);
    if let Some(prepared) = prepared {
        engine
            .data_handle()
            .prepare_frozen_trade_history(&history, prepared);
    }
    assert!(engine.set_trade_history(history));
    engine.set_scene_visible(true);
    engine.set_market_source(Some(MarketDataSource::new(MarketStore::shared(0.0))));
    engine.show_time_range(start, end, 0.0);
    engine.prepare_frame_for_tests(GpuFrameInfo {
        now: Instant::now(),
        bounds: Bounds {
            origin: point(px(0.0), px(0.0)),
            size: size(px(1_000.0), px(600.0)),
        },
        scale_factor: 1.0,
        content_zoom: 1.0,
        presentable: true,
    });
    engine
}

/// Prepared and synchronous history must draw the same marks for mixed real and emulator trades.
#[test]
fn prepared_trade_window_matches_synchronous_history() {
    chart_thread(|| {
        crate::test_locale::warm();
        let axis = ReportAxis::from_measured(
            std::collections::HashMap::from([(
                7,
                vec![OffsetSegment {
                    from_utc: 0,
                    offset_secs: 1,
                }],
            )]),
            chrono_tz::UTC,
        );
        for (conn, count) in [(fixture(), None), (large_fixture(), Some(50_000))] {
            let rows =
                period_history(&conn, 7, "BTC", &ReportFilter::default()).expect("period history");
            if let Some(count) = count {
                assert_eq!(rows.len(), count);
            } else {
                assert!(rows.iter().any(|record| record.emulator));
                assert!(rows.iter().any(|record| !record.emulator));
            }
            let history = Rc::new(rows);
            let focus = history
                .iter()
                .skip(history.len() / 2)
                .find(|record| !record.emulator)
                .unwrap();
            let (neighbours, prepared) = prepare_history(&history, focus, axis.clone());
            let mut expected_neighbours = history
                .iter()
                .enumerate()
                .filter(|(_, row)| row.record_id != focus.record_id)
                .collect::<Vec<_>>();
            expected_neighbours.sort_unstable_by_key(|(ordinal, row)| {
                (row.close_date.abs_diff(focus.close_date), *ordinal)
            });
            assert_eq!(
                neighbours.iter().collect::<Vec<_>>(),
                expected_neighbours
                    .into_iter()
                    .take(super::super::traces::NEIGHBOUR_CAP)
                    .map(|(_, row)| row)
                    .collect::<Vec<_>>()
            );
            let prepared = Rc::new(prepared);
            for show_others in [true, false] {
                let visible = super::super::visible_history(&history, focus, show_others);
                let expected = if show_others {
                    history.as_slice()
                } else {
                    std::slice::from_ref(focus)
                };
                assert_eq!(visible.as_slice(), expected);
                let span = (0.0, 50_100_000.0);
                for (graphics, lines) in [
                    (ChartGraphicsCfg::default(), None),
                    (
                        ChartGraphicsCfg {
                            trade_history_style: TradeHistoryStyle::MoonbotLines,
                            show_emulator_trades: false,
                            ..ChartGraphicsCfg::default()
                        },
                        Some([axis.stamp_to_utc_ms(focus.close_stamp(), 7) as f64]),
                    ),
                ] {
                    let legacy = chart(visible.clone(), None, axis.clone(), span, graphics);
                    let ready = chart(
                        visible.clone(),
                        show_others.then_some(&prepared),
                        axis.clone(),
                        span,
                        graphics,
                    );
                    let (before, before_marks, before_segs) = legacy.trade_geometry_for_tests(
                        7,
                        lines.as_ref().map(|line| line.as_slice()),
                        span,
                    );
                    let (after, after_marks, after_segs) = ready.trade_geometry_for_tests(
                        7,
                        lines.as_ref().map(|line| line.as_slice()),
                        span,
                    );
                    assert_eq!(
                        ready.trade_history().as_slice(),
                        legacy.trade_history().as_slice()
                    );
                    assert!(
                        !before_marks.is_empty(),
                        "fixture must exercise the marker pass: rows={}, show_others={show_others}, lines={lines:?}",
                        history.len()
                    );
                    assert_eq!(after.clusters, before.clusters);
                    assert_eq!(after.sources, before.sources);
                    assert_eq!(after.order_by_t, before.order_by_t);
                    assert_eq!(
                        bytemuck::cast_slice::<_, u8>(&after_marks),
                        bytemuck::cast_slice::<_, u8>(&before_marks)
                    );
                    assert_eq!(
                        bytemuck::cast_slice::<_, u8>(&after_segs),
                        bytemuck::cast_slice::<_, u8>(&before_segs)
                    );
                }
            }
        }
    });
}

/// Two distinct pending records can both open even if their reads finish out of order.
#[test]
fn distinct_pending_records_both_open() {
    let mut pending = RecordOpenGeneration::default();
    let a = (7, RecordKey::RecordId(1));
    let b = (7, RecordKey::RecordId(2));
    let first = pending.next(a);
    let second = pending.next(b);

    assert!(pending.complete(b, second));
    assert!(pending.complete(a, first));
    assert!(pending.pending.is_empty());
}

/// An older completion cannot open or consume a newer click for the same record.
#[test]
fn repeated_record_open_supersedes_its_previous_request() {
    let mut pending = RecordOpenGeneration::default();
    let key = (7, RecordKey::RecordId(1));
    let older = pending.next(key);
    let latest = pending.next(key);

    assert!(!pending.complete(key, older));
    assert!(pending.complete(key, latest));
    assert!(!pending.complete(key, latest));
    assert!(pending.pending.is_empty());
}

/// Time the independent background stages and the retained publication plus first frame.
#[test]
#[ignore = "bench: S-smooth"]
fn bench_s_trade_window_open_50k() {
    chart_thread(trade_window_open_50k);
}

/// Measure a focused one-minute chart over a full 50,000-row Report period.
fn trade_window_open_50k() {
    crate::test_locale::warm();
    let conn = large_fixture();
    let filter = ReportFilter::default();
    use crate::test_support::bench::measure;

    measure(
        "bench_s_trade_window_open_50k_period_history rows=50000",
        12,
        || period_history(&conn, 7, "BTC", &filter).expect("period history"),
    );
    let history = Rc::new(period_history(&conn, 7, "BTC", &filter).expect("period history"));
    assert_eq!(history.len(), 50_000);
    let focus = &history[25_000];
    measure(
        "bench_s_trade_window_open_50k_select_neighbours rows=50000",
        12,
        || super::super::traces::select_neighbours(&history, focus),
    );
    assert_eq!(
        super::super::traces::select_neighbours(&history, focus).len(),
        super::super::traces::NEIGHBOUR_CAP
    );
    measure(
        "bench_s_trade_window_open_50k_visible_history rows=50000",
        12,
        || super::super::visible_history(&history, focus, true),
    );
    let visible = super::super::visible_history(&history, focus, true);
    assert_eq!(visible.len(), 50_000);
    let axis = ReportAxis::default();
    measure(
        "bench_s_trade_window_open_50k_precompute rows=50000",
        12,
        || PreparedTradeHistory::new(&history, 7, axis.clone()),
    );
    let prepared = Rc::new(PreparedTradeHistory::new(&history, 7, axis.clone()));
    let buy = axis.stamp_to_utc_ms(focus.buy_stamp(), 7);
    let close = axis.stamp_to_utc_ms(focus.close_stamp(), 7);
    let (start, end) = super::super::frame::trade_frame(buy, close, 60_000).unwrap();
    let span = (start as f64, end as f64);
    measure(
        "bench_s_trade_window_open_50k_ui_thread stimulus=attach_history_first_CPU_frame rows=50000 partial_(no_Backend_wiring)",
        12,
        || {
            let engine = chart(
                visible.clone(),
                Some(&prepared),
                axis.clone(),
                span,
                ChartGraphicsCfg::default(),
            );
            let (geom, markers, segs) = engine.trade_geometry_for_tests(7, None, span);
            assert_eq!(engine.trade_history().len(), 50_000);
            assert!(!markers.is_empty());
            (engine, geom, markers, segs)
        },
    );
}
