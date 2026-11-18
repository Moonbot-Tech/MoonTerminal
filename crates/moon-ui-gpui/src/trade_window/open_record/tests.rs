//! Fixture coverage for the period neighbours a trade window is opened with.

use super::period_history;
use moon_core::db::{OffsetSegment, ReportAxis, ReportFilter, ReportStrategyKey, SideFilter};
use rusqlite::Connection;

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

/// Time the real 50,000-row history read, neighbour selection, and chart-history frame sync.
#[test]
#[ignore = "bench: S-smooth"]
fn bench_s_trade_window_open_50k() {
    std::thread::Builder::new()
        .stack_size(16 * 1024 * 1024)
        .spawn(trade_window_open_50k)
        .expect("bench thread")
        .join()
        .expect("bench result");
}

/// Run chart preparation on the stack size required by the Windows chart tests.
fn trade_window_open_50k() {
    use std::rc::Rc;
    use std::time::Instant;

    use gpui::{Bounds, GpuFrameInfo, point, px, size};
    use moon_core::config::ChartTheme;
    use moon_core::market::{MarketDataSource, MarketStore};

    crate::test_locale::warm();
    let conn = fixture();
    conn.execute_batch(
        "DELETE FROM orders_rep;
        WITH RECURSIVE n(x) AS (SELECT 1 UNION ALL SELECT x + 1 FROM n WHERE x < 50000)
        INSERT INTO orders_rep
        SELECT 7, x, 'BTC', x, x + 60, 10, 12, 2, 0, 0, 0, 4 FROM n;",
    )
    .expect("seed 50,000 history rows");
    let filter = ReportFilter::default();
    crate::test_support::bench::measure(
        "bench_s_trade_window_open_50k stimulus=cold_query_neighbours_chart_sync rows=50000 partial_(no_Backend_wiring)",
        3,
        || {
            let history = period_history(&conn, 7, "BTC", &filter).expect("period history");
            assert_eq!(history.len(), 50_000);
            let focus = history[25_000].clone();
            let neighbours = super::super::traces::select_neighbours(&history, &focus);
            assert_eq!(neighbours.len(), super::super::traces::NEIGHBOUR_CAP);
            let history = Rc::new(history);
            let visible = super::super::visible_history(&history, &focus, true);
            assert_eq!(visible.len(), 50_000);

            let mut engine = crate::chartdx::ChartEngine::new(0.0, ChartTheme::default());
            engine.open(7, "BTCUSDT");
            engine.set_historical(true);
            engine.set_follow(false, 50_100_000.0);
            assert!(engine.set_trade_history(visible));
            engine.set_scene_visible(true);
            engine.set_market_source(Some(MarketDataSource::new(MarketStore::shared(0.0))));
            engine.show_time_range(1_000.0, 50_100_000.0, 0.0);
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
            assert_eq!(engine.trade_history().len(), 50_000);
            engine
        },
    );
}
