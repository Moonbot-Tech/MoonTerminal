//! Regression tests for market fitting, replay transforms and history reads.

use moon_core::market::CandleViewCfg;
use moon_core::market::candles::{
    CANDLE_MODE_FILLED, CANDLE_MODE_OFF, CANDLE_TF_CHOICES_MIN, CANDLE_ZONE_MAX,
};

use super::window::{chart_history_floor_ms, hide_start_rel};

/// A volume reader completion must wake both quiet-source guards exactly once without a tick.
#[test]
fn chart_market_quiet_volume_completion_wakes_the_source_pull() {
    std::thread::Builder::new()
        .stack_size(16 * 1024 * 1024)
        .spawn(quiet_volume_completion)
        .unwrap()
        .join()
        .unwrap();
}

/// Drive the real chart pull after only the consumed background-completion revision goes stale.
fn quiet_volume_completion() {
    crate::test_locale::warm();
    let _window = crate::test_support::SharedWindowGuard::take();
    use gpui::{Bounds, GpuFrameInfo, point, px, size};
    use moon_core::config::ChartTheme;
    use moon_core::market::{MarketDataSource, MarketStore};
    let source = MarketDataSource::new(MarketStore::shared(0.0));
    let mut engine = crate::chartdx::ChartEngine::new(0.0, ChartTheme::default());
    engine.open(42, "XALPHAUSDT");
    engine.set_historical(true);
    engine.set_follow(false, 1_000.0);
    engine.set_scene_visible(true);
    engine.set_market_source(Some(source.clone()));
    let mut data = engine.data.borrow_mut();
    data.frame(GpuFrameInfo {
        now: std::time::Instant::now(),
        bounds: Bounds {
            origin: point(px(0.0), px(0.0)),
            size: size(px(1000.0), px(600.0)),
        },
        scale_factor: 1.0,
        content_zoom: 1.0,
        presentable: true,
    });
    assert!(
        !data.pull_market_source_if_visible(),
        "an unchanged source must stay idle"
    );
    data.last_volume_prepared_rev = source.volume_prepared_revision().wrapping_sub(1);
    assert!(
        data.pull_market_source_if_visible(),
        "completed volume work must not wait for a market event"
    );
    assert!(
        !data.pull_market_source_if_visible(),
        "consuming completion must restore the quiet gate"
    );
}

/// Dropping the replay's settlement band hides report-priced arrows when exchange tape differs.
#[test]
fn trade_window_settlement_prices_match_axis_after_zoom() {
    std::thread::Builder::new()
        .stack_size(16 * 1024 * 1024)
        .spawn(replay_price_transform)
        .unwrap()
        .join()
        .unwrap();
}

/// Exercise actual replay frame preparation, wheel navigation and fixed-percent scaling.
fn replay_price_transform() {
    let _window = crate::test_support::SharedWindowGuard::take();
    use gpui::{Bounds, GpuFrameInfo, point, px, size};
    use moon_core::config::ChartTheme;
    use moon_core::market::trade_replay::{
        Coverage, TickStatus, TickThinning, TradeReplaySeries, TradeReplaySource, replay_window_ms,
    };
    use moon_core::market::{MarketDataSource, MarketStore};
    use std::rc::Rc;
    use std::time::Instant;

    let window = replay_window_ms(100_000, 110_000, 30_000).unwrap();
    for zoom in [0.75, 1.0, 1.5] {
        let mut engine = crate::chartdx::ChartEngine::new(0.0, ChartTheme::default());
        engine.open(42, "XALPHAUSDT");
        engine.set_historical(true);
        engine.set_frozen_orders(None, Some((2.2334, 2.276)));
        engine.set_trade_history(Rc::new(vec![moon_core::db::ChartTradeRecord {
            record_id: 1,
            core_uid: 42,
            coin: "XALPHAUSDT".into(),
            buy_date: 100,
            close_date: 110,
            buy_ms: Some(100_000),
            close_ms: Some(110_000),
            sell_set_date: 100,
            sell_set_ms: None,
            buy_set_ms: None,
            corridor: None,
            buy_price: 2.276,
            sell_price: 2.2334,
            quantity: 130.0,
            is_short: true,
            emulator: false,
            profit: Some(5.47),
            quote: None,
            profit_pct: Some(1.85),
            report_uid: None,
        }]));
        engine.set_trade_replay(Some(Rc::new(TradeReplaySeries {
            source: TradeReplaySource::CoreTicks,
            venue: None,
            window,
            tf_ms: 60_000,
            candles: Vec::new(),
            ticks: vec![
                moon_core::feed::Tick {
                    time_ms: 100_000.0,
                    price: 2.1,
                    qty: 130.0,
                    side: moon_core::feed::Side::Sell,
                },
                moon_core::feed::Tick {
                    time_ms: 110_000.0,
                    price: 2.12,
                    qty: 130.0,
                    side: moon_core::feed::Side::Buy,
                },
            ],
            identity: 1,
            tick_status: TickStatus::Served,
            thinning: TickThinning::Raw,
            partial: false,
            side_slots: Vec::new(),
            covered: Coverage::one((window.from_ms, window.to_ms)),
        })));
        engine.show_time_range(60_000.0, 180_000.0, 0.0);
        engine.set_scene_visible(true);
        let source = MarketDataSource::new(MarketStore::shared(0.0));
        engine.set_market_source(Some(source.clone()));
        for step in 0..5 {
            if step == 1 {
                engine.set_scale(Some(0.10));
            }
            if step == 2 {
                let before = engine.pane_x_ppm(Some(0)).unwrap();
                let mut input = crate::chartdx::input::ChartInput::default();
                input.hovered_pane = Some(0);
                engine.with_container_mut(|container| {
                    assert!(input.wheel(
                        100.0,
                        false,
                        crate::chartdx::input::WheelMode::Zoom,
                        true,
                        container,
                        1000.0,
                        1.0,
                    ));
                });
                assert_eq!(engine.pane_x_ppm(Some(0)).unwrap(), before * 2.0);
            }
            if step >= 3 {
                if step == 4 {
                    // Repeating Show trade must undo both X navigation and a parked manual Y.
                    engine.with_container_mut(|container| {
                        let v = &mut container.panes_mut()[0].view;
                        v.pan_x_px(800.0, 0.0, 1000.0);
                        v.rmb_zoom(v.render_center, v.render_range, v.price_zoom(), 80.0, 0.0);
                    });
                }
                // These are the two operations Show trade applies to its own chart panel.
                engine.reapply_scale(None);
                engine.show_time_range(100_000.0, 110_000.0, 0.25);
            }
            let mut data = engine.data.borrow_mut();
            data.mark_view_dirty();
            data.frame(GpuFrameInfo {
                now: Instant::now(),
                bounds: Bounds {
                    origin: point(px(20.0), px(80.0)),
                    size: size(px(1000.0), px(600.0)),
                },
                scale_factor: 1.5 * zoom,
                content_zoom: zoom,
                presentable: true,
            });
            let render = data.render.borrow();
            let pane = render.panes.iter().find(|pane| pane.active).unwrap();
            let v = pane.view;
            let logical = engine.container.borrow();
            let view = &logical.panes()[0].view;
            let mut markers = Vec::new();
            let mut segs = Vec::new();
            data.append_trade_history_geometry(
                0,
                42,
                view,
                &mut markers,
                &mut segs,
                None,
                &mut None,
            );
            assert_eq!(markers.len(), 2);
            let mut read = Vec::new();
            for (marker, price) in markers.iter().zip([2.276_f32, 2.2334]) {
                assert_eq!(
                    marker.price, price,
                    "markers must use report prices, not tape"
                );
                if step >= 3 {
                    let x = v.bounds[0] + (marker.t_rel - v.view_time0) * v.time_to_px;
                    assert!(x > v.bounds[0] && x < v.bounds[0] + v.bounds[2]);
                }
                // The marker shaders place the apex with this physical-pixel mapping.
                let y = v.bounds[1] + v.bounds[3] - (marker.price - v.view_price0) * v.price_to_px;
                assert!(
                    y >= v.bounds[1] && y <= v.bounds[1] + v.bounds[3],
                    "settled price clipped: zoom={zoom}, step={step}"
                );
                // Axis labels and cursor readouts invert in chart-design logical pixels.
                let sf = 1.5;
                let axis = v.view_price0
                    + ((v.bounds[1] + v.bounds[3]) / sf - y / sf) / (v.price_to_px / sf);
                assert!((axis - price).abs() < 1e-6, "zoom={zoom}, step={step}");
                let expected_y = v.bounds[1] + v.bounds[3] * 0.5
                    - (price - view.render_center) / view.render_range * v.bounds[3];
                assert!((y - expected_y).abs() < 0.01, "view and GPU disagree");
                read.push(axis);
            }
            let short_pct = (read[0] - read[1]) / read[0] * 100.0;
            assert!((short_pct - 1.8717047).abs() < 0.0001);
            if step == 1 || step == 2 {
                assert_eq!(pane.scale_badge, Some(10));
                let span = v.bounds[3] / v.price_to_px;
                let center = v.view_price0 + span * 0.5;
                assert!((span / center * 100.0 - 10.0).abs() < 0.001);
            }
            drop(logical);
            drop(render);
            drop(data);
            engine.set_ruler(
                Some(crate::chartdx::ruler::RulerSpan {
                    pane: 0,
                    t0_ms: 100_000.0,
                    p0: read[0] as f64,
                    t1_ms: 110_000.0,
                    p1: read[1] as f64,
                }),
                &source,
            );
            assert!(
                engine
                    .state
                    .borrow()
                    .ruler
                    .as_ref()
                    .unwrap()
                    .move_line
                    .starts_with("-1.87%")
            );
        }
    }
}

/// Applying replay's full union to Main would squash candles around distant live targets.
#[test]
fn main_price_fit_still_bounds_distant_orders() {
    let ticks = Some((200.0, 210.0));
    let orders = Some((400.0, 500.0));
    assert_eq!(
        moon_chart::view::admit_order_band(ticks, orders),
        Some((200.0, 215.0))
    );
    assert_eq!(
        moon_chart::view::fit_band(ticks, orders, true),
        Some((200.0, 500.0))
    );
}

/// Local readiness needs timed retries at unchanged archive revisions, then freezes on acceptance.
#[test]
fn replay_retry_progresses_without_following_the_live_tick_clock() {
    use moon_core::market::{MarketRevisions, ReplayPriceLinesRead};
    use std::time::{Duration, Instant};
    let start = Instant::now();
    let mut revisions = MarketRevisions {
        archive: 7,
        generation: 1,
        ..Default::default()
    };
    let cached = super::history_signature(Some(revisions), true);
    let mut retry = super::next_replay_price_retry(None, start);
    let mut reads = 1;
    for ms in (0..10_000).step_by(50) {
        let now = start + Duration::from_millis(ms);
        revisions.history += 1;
        let source_changed = super::history_signature(Some(revisions), true) != cached;
        if source_changed || super::replay_price_retry_due(retry, now) {
            reads += 1;
            let demand = ReplayPriceLinesRead {
                capacity: 1,
                retry_at: (reads == 2).then_some(now + Duration::from_secs(3)),
            };
            retry = super::next_replay_price_retry(Some(demand), now);
        }
    }
    assert_eq!(
        reads, 3,
        "one unready read, one local refusal, then acceptance"
    );
    assert!(retry.is_none());
}

/// Consuming the provider-generation edge must not strand a quiet pane with empty price lines.
#[test]
fn a_quiet_replay_retries_missing_readers_at_the_same_archive_revision() {
    // The debug chart sync stack exceeds the Windows test harness's default thread stack.
    std::thread::Builder::new()
        .stack_size(16 * 1024 * 1024)
        .spawn(quiet_replay_retry)
        .unwrap()
        .join()
        .unwrap();
}

/// Drive both pull guards and the actual replay read on a quiet, provider-less chart.
fn quiet_replay_retry() {
    let _window = crate::test_support::SharedWindowGuard::take();
    use gpui::{Bounds, GpuFrameInfo, point, px, size};
    use moon_core::config::ChartTheme;
    use moon_core::market::trade_replay::{
        Coverage, TickStatus, TickThinning, TradeReplaySeries, TradeReplaySource, replay_window_ms,
    };
    use moon_core::market::{MarketDataSource, MarketStore};
    use std::rc::Rc;
    use std::time::{Duration, Instant};
    let window = replay_window_ms(100_000, 110_000, 30_000).unwrap();
    let mut engine = crate::chartdx::ChartEngine::new(0.0, ChartTheme::default());
    engine.open(42, "XALPHAUSDT");
    engine.set_follow(false, 140_000.0);
    engine.set_trade_replay(Some(Rc::new(TradeReplaySeries {
        source: TradeReplaySource::CoreTicks,
        venue: None,
        window,
        tf_ms: 60_000,
        candles: Vec::new(),
        ticks: vec![moon_core::feed::Tick {
            time_ms: 105_000.0,
            price: 1.0,
            qty: 1.0,
            side: moon_core::feed::Side::Buy,
        }],
        identity: 1,
        tick_status: TickStatus::Served,
        thinning: TickThinning::Raw,
        partial: false,
        side_slots: Vec::new(),
        covered: Coverage::one((window.from_ms, window.to_ms)),
    })));
    engine.set_scene_visible(true);
    let mut data = engine.data.borrow_mut();
    data.set_market_source(Some(MarketDataSource::new(MarketStore::shared(0.0))));
    data.frame(GpuFrameInfo {
        now: Instant::now(),
        bounds: Bounds {
            origin: point(px(0.0), px(0.0)),
            size: size(px(1000.0), px(600.0)),
        },
        scale_factor: 1.0,
        content_zoom: 1.0,
        presentable: true,
    });
    {
        let mut render = data.render.borrow_mut();
        let pane = render.panes.iter_mut().find(|pane| pane.active).unwrap();
        assert_eq!(pane.source_archive, 0);
        assert!(pane.replay_prices_retry_at.is_some());
        pane.replay_prices_retry_at = Some(Instant::now() + Duration::from_secs(60));
    }
    assert!(
        !data.pull_market_source_if_visible(),
        "pending does not mean every frame"
    );
    let overdue = Instant::now() - Duration::from_secs(1);
    {
        let mut render = data.render.borrow_mut();
        let pane = render.panes.iter_mut().find(|pane| pane.active).unwrap();
        pane.replay_prices_retry_at = Some(overdue);
    }
    assert!(
        data.pull_market_source_if_visible(),
        "no new market revision is required"
    );
    let render = data.render.borrow();
    let pane = render.panes.iter().find(|pane| pane.active).unwrap();
    assert_eq!(pane.source_archive, 0);
    assert!(
        pane.replay_prices_retry_at.is_some_and(|at| at > overdue),
        "the due read reschedules unavailable readers"
    );
}

/// A live tick must not re-copy frozen replay price lines; a late archive merge must do so.
#[test]
fn replay_history_ignores_live_ticks_but_reads_archive_merges() {
    let mut revisions = moon_core::market::MarketRevisions {
        history: 100,
        archive: 7,
        ..Default::default()
    };
    let initial = super::history_signature(Some(revisions), true);
    let mut reads = 0;
    for tick in 101..201 {
        revisions.history = tick;
        revisions.meta = tick;
        if super::history_signature(Some(revisions), true) != initial {
            reads += 1;
        }
    }
    assert_eq!(reads, 0);
    let live = super::history_signature(Some(revisions), false);
    revisions.history += 1;
    assert_ne!(super::history_signature(Some(revisions), false), live);
    revisions.archive += 1;
    assert_ne!(super::history_signature(Some(revisions), true), initial);
}

/// Losing the composed bar's own duration makes the backend cull a visible minute range as
/// though it were a five-second mini-candle, recreating the daily chart's narrow sliver.
#[test]
fn open_candle_path_gpu_width_reaches_the_visible_slice() {
    use crate::chartdx::candle_window::CandleWindow;
    use moon_core::market::ChartCandle;

    let candle = ChartCandle {
        t_open_ms: 86_400_000.0,
        low: 10.0,
        high: 12.0,
        ..Default::default()
    };
    let mut bars = Vec::new();
    moon_chart::open_candle_path::place_prefix(
        Some(&[candle]),
        &[],
        (86_400_000.0, 86_460_000.0),
        86_400_000,
        86_400_000.0,
        &mut bars,
    );
    let rows: Vec<_> = bars.iter().map(super::path_bar_gpu).collect();
    let mut window = CandleWindow::new(32);
    window.set_style_tf(5_000.0);
    window.set(&rows);
    assert_eq!(window.draw_slice(59_000.0, 59_500.0).candles, 1);
    assert_eq!(rows[0].tf_rel, 60_000.0);
}

/// `market.rs:chart_history_floor_ms` must keep every supported candle timeframe within the
/// 120-to-1500-bar request band; swapping the clamps or lowering the floor would silently request
/// too much history or make a chart appear empty near the current time.
#[test]
fn chart_history_floor_stays_finite_and_inside_the_bar_band() {
    let mut five_minute_bars = None;
    let mut thirty_minute_bars = None;

    for tf_min in CANDLE_TF_CHOICES_MIN {
        let cfg = CandleViewCfg {
            tf_min,
            mode: CANDLE_MODE_FILLED,
            ..CandleViewCfg::default()
        };
        let floor_ms = chart_history_floor_ms(cfg) as f64;
        let bars = floor_ms / cfg.tf_ms() as f64;

        assert!(floor_ms.is_finite());
        assert!(floor_ms >= 0.0);
        assert!((120.0..=1500.0).contains(&bars));

        if tf_min == 5 {
            five_minute_bars = Some(bars);
        }
        if tf_min == 30 {
            thirty_minute_bars = Some(bars);
        }
    }

    assert!(
        five_minute_bars.expect("the supported set includes five minutes")
            > thirty_minute_bars.expect("the supported set includes thirty minutes")
    );
}

/// `market.rs:chart_history_floor_ms` must return zero for CANDLE_MODE_OFF; dropping that early
/// return would rebuild and upload candle history even though the user selected a pure tick chart.
#[test]
fn chart_history_floor_is_zero_when_candles_are_off() {
    let cfg = CandleViewCfg {
        mode: CANDLE_MODE_OFF,
        ..CandleViewCfg::default()
    };

    assert_eq!(chart_history_floor_ms(cfg), 0.0);
}

/// A caption measuring around the pointer orders NO read while the pointer is off the pane.
///
/// Breakage: falling back to the live edge would answer a different question under a heading that
/// says "cursor", and the reader would have no way to tell which one they are looking at.
#[test]
fn a_measuring_period_is_dropped_without_a_pointer() {
    use moon_core::config::{LabelSpan, LabelWindow, SpanAnchor, VolumeSpanKey};
    use moon_core::market::{VolumeAt, VolumeSpan};

    let live = VolumeSpanKey {
        span: LabelSpan::Window,
        window: LabelWindow::M1,
        anchor: SpanAnchor::Now,
        liquidations: false,
    };
    let measured = VolumeSpanKey {
        anchor: SpanAnchor::Cursor,
        ..live
    };

    let without = super::spans::resolve_span_keys(&[live, measured], None);
    assert_eq!(
        without,
        vec![(VolumeSpan::Millis(60_000), VolumeAt::Now)],
        "only the live-edge period survives with no pointer"
    );

    let with = super::spans::resolve_span_keys(&[live, measured], Some(1_700_000_000_000));
    assert_eq!(
        with.len(),
        2,
        "both periods are read once the pointer lands"
    );
    assert!(with.contains(&(VolumeSpan::Millis(60_000), VolumeAt::Now)));
    assert!(with.contains(&(
        VolumeSpan::Millis(60_000),
        VolumeAt::Around(1_700_000_000_000)
    )));
}

/// Two captions over one period are ONE read, however many figures they print.
#[test]
fn one_period_is_resolved_once() {
    use moon_core::config::{LabelSpan, LabelWindow, SpanAnchor, VolumeSpanKey};

    let key = VolumeSpanKey {
        span: LabelSpan::Window,
        window: LabelWindow::M5,
        anchor: SpanAnchor::Now,
        liquidations: true,
    };
    assert_eq!(super::spans::resolve_span_keys(&[key, key], None).len(), 1);
}

/// The pointer's refresh may replace only what the pointer owns.
///
/// Breakage: the two anchors are refreshed on different clocks — the market's and the mouse's — so
/// replacing the set wholesale on a mouse move would blank every live-edge figure until the next
/// market revision, which on a quiet coin is a visible hole.
#[test]
fn a_pointer_refresh_leaves_the_live_edge_entries_alone() {
    use moon_core::market::{VolumeAt, VolumeSpan};

    let live = (VolumeSpan::Millis(60_000), VolumeAt::Now);
    let old_point = (VolumeSpan::Millis(60_000), VolumeAt::Around(1_000));
    let new_point = (VolumeSpan::Millis(60_000), VolumeAt::Around(2_000));

    let mut held = vec![(live, 1u32), (old_point, 2u32)];
    super::spans::merge_readouts(&mut held, vec![(new_point, 3u32)]);

    assert!(held.contains(&(live, 1)), "the live-edge reading survives");
    assert!(
        !held.iter().any(|(key, _)| *key == old_point),
        "the stale point is gone"
    );
    assert!(held.contains(&(new_point, 3)), "the fresh point is in");
}

/// The pointer leaving drops what it was measuring, rather than freezing it.
#[test]
fn a_pointer_leaving_clears_only_its_own_entries() {
    use moon_core::market::{VolumeAt, VolumeSpan};

    let live = (VolumeSpan::Millis(60_000), VolumeAt::Now);
    let point = (VolumeSpan::Millis(60_000), VolumeAt::Around(1_000));
    let mut held = vec![(live, 1u32), (point, 2u32)];

    super::spans::merge_readouts(&mut held, Vec::new());

    assert_eq!(held, vec![(live, 1)]);
}

/// Width and zoom invalidate the fit even with no pan; subpixel follow motion stays cheap.
#[test]
fn price_fit_cache_tracks_viewport_and_accumulates_subpixel_motion() {
    let cached = Some((1000.0, 60000.0, 0.01));
    assert!(!super::window::price_fit_window_changed(
        cached,
        (1050.0, 60000.0, 0.01)
    ));
    assert!(super::window::price_fit_window_changed(
        cached,
        (1100.0, 60000.0, 0.01)
    ));
    assert!(super::window::price_fit_window_changed(
        cached,
        (1000.0, 30000.0, 0.01)
    ));
    assert!(super::window::price_fit_window_changed(
        cached,
        (1000.0, 60000.0, 0.02)
    ));
    assert!(super::window::price_fit_window_changed(
        None,
        (1000.0, 60000.0, 0.01)
    ));
}

/// Measured on 2026-09-13 (Main pane, 5m, bucket open at 524454 rel): with `hide_candles = 3` the
/// boundary was `-52298` — the first trade's timestamp, 23 s past the open of the third-from-last
/// bucket at `-75546` — so that bucket kept its candle and two were hidden instead of three.
#[test]
fn hide_zone_boundary_snaps_the_first_trade_to_its_bucket_open() {
    let tf_ms = 300_000;
    let epoch_ms = 1_700_000_000_000.0;
    let now_ms = epoch_ms + 637_474.0;
    let bucket_open_rel =
        (moon_core::market::candles::bucket_open_ms(now_ms, tf_ms) - epoch_ms) as f32;
    let hide_open_rel = bucket_open_rel - 2.0 * tf_ms as f32;
    // First resident trade 23 s into the third-from-last bucket.
    let first_trade_rel = hide_open_rel + 23_000.0;
    let boundary = hide_start_rel(3, now_ms, tf_ms, epoch_ms, first_trade_rel);
    assert_eq!(
        boundary, hide_open_rel,
        "the bucket holding the first trade must be hidden too"
    );
}

/// The zone never reaches left of the first resident trade: with trades starting in the last
/// bucket only, `hide_candles = 3` hides that one bucket, not three.
#[test]
fn hide_zone_boundary_is_clamped_at_the_first_trade_bucket() {
    let tf_ms = 300_000;
    let epoch_ms = 1_700_000_000_000.0;
    let now_ms = epoch_ms + 637_474.0;
    let bucket_open_rel =
        (moon_core::market::candles::bucket_open_ms(now_ms, tf_ms) - epoch_ms) as f32;
    let first_trade_rel = bucket_open_rel + 90_000.0;
    let boundary = hide_start_rel(3, now_ms, tf_ms, epoch_ms, first_trade_rel);
    assert_eq!(boundary, bucket_open_rel);
}

#[test]
fn hide_zone_is_off_without_a_count_or_without_resident_trades() {
    let tf_ms = 300_000;
    let epoch_ms = 1_700_000_000_000.0;
    let now_ms = epoch_ms + 637_474.0;
    assert_eq!(hide_start_rel(0, now_ms, tf_ms, epoch_ms, 0.0), f32::MAX);
    assert_eq!(
        hide_start_rel(3, now_ms, tf_ms, epoch_ms, f32::NAN),
        f32::MAX
    );
}

/// Hide Max on the chart path must use the oldest trade's bucket, not a candle count back
/// from now. Treating the sentinel as a huge `hide_candles` would blank the whole series.
#[test]
fn hide_max_follows_the_oldest_trade_bucket_and_stays_off_without_one() {
    let tf_ms = 300_000_i64;
    let epoch_ms = 1_700_000_000_000.0;
    let now_ms = epoch_ms + 637_474.0;
    let oldest_rel = 410_000.0;
    let abs = epoch_ms + oldest_rel as f64;
    let expected = ((abs / tf_ms as f64).floor() * tf_ms as f64 - epoch_ms) as f32;
    assert_eq!(
        hide_start_rel(CANDLE_ZONE_MAX, now_ms, tf_ms, epoch_ms, oldest_rel),
        expected
    );
    assert_eq!(
        hide_start_rel(CANDLE_ZONE_MAX, now_ms, tf_ms, epoch_ms, f32::NAN),
        f32::MAX
    );
}

/// `data_state/market.rs:book_instances_stale` + `orderbook.rs:plan_book_bake`: keying staleness
/// on the exact window edges (or its derived width) rebuilds the book instances and rebakes the
/// glass on every frame of a live follow; leaving the emitted window must still rebuild at once,
/// and a backend without a margin keeps the exact old `(rev, lo, hi)` rule.
#[cfg(windows)]
#[test]
fn live_follow_inside_the_book_margin_keeps_instances_and_bitmap() {
    use crate::chartdx::orderbook::{BookBakeKey, book_v_margin_px, plan_book_bake};

    use super::window::book_instances_stale;

    let epoch = 1.7e12;
    let mut view = moon_chart::view::ChartView::new(epoch);
    let area = moon_chart::view::Rect {
        x: 0.0,
        y: 0.0,
        w: 1000.0,
        h: 600.0,
    };
    let mut now = epoch + 16.0;
    view.update_y(now, area.h, Some((50.0, 150.0)), Some(100.0));
    let gpu = |v: &moon_chart::view::ChartView| {
        crate::chartdx::view::view_gpu(
            v,
            area,
            [area.w, area.h],
            1.0,
            crate::chartdx::view::ViewStyle::default(),
        )
    };
    let window = |v: &moon_chart::view::ChartView| {
        let half = v.render_range.max(1e-9) * 0.5;
        (v.render_center - half, v.render_center + half)
    };
    let v_margin = book_v_margin_px(area.h);
    let g0 = gpu(&view);
    let margin_price = v_margin / g0.price_to_px;
    let rev = 7u64;
    let (lo0, hi0) = window(&view);
    let mut emit = (lo0 - margin_price, hi0 + margin_price);
    let mut last_range = view.render_range;
    let mut last_lo_hi = (lo0, hi0);
    let key = BookBakeKey {
        tex_h_total: area.h as u32 + 2 * v_margin as u32,
        v_margin,
        bake_p0: g0.view_price0 - v_margin / g0.price_to_px,
        price_to_px: g0.price_to_px,
        baked: true,
    };

    let (mut old_rebuilds, mut new_rebuilds, mut bakes) = (0, 0, 0);
    for i in 1..=120 {
        now += 16.0;
        let last = 100.0 + 25.0 * i as f32 / 120.0;
        view.update_y(now, area.h, Some((last - 50.0, last + 50.0)), Some(last));
        let (lo, hi) = window(&view);
        if (lo, hi) != last_lo_hi {
            old_rebuilds += 1;
        }
        if book_instances_stale(
            rev,
            emit,
            last_range,
            last_lo_hi,
            rev,
            view.render_range,
            lo,
            hi,
            margin_price,
        ) {
            new_rebuilds += 1;
            emit = (lo - margin_price, hi + margin_price);
            last_range = view.render_range;
        }
        last_lo_hi = (lo, hi);
        if plan_book_bake(&key, &gpu(&view), false, false, false).bake {
            bakes += 1;
        }
    }
    println!("[G1] book rebuilds per 120 follow syncs: old={old_rebuilds} new={new_rebuilds}");
    assert!(
        old_rebuilds > 0,
        "the drive must actually move the book window"
    );
    assert_eq!(
        new_rebuilds, 0,
        "book instances rebuilt inside the emitted window"
    );
    assert_eq!(bakes, 0, "book glass rebaked inside its margin");

    // Leaving the emitted window rebuilds, and that rebuild bakes immediately.
    let (lo, hi) = (emit.0 - 1.0, emit.0 - 1.0 + view.render_range);
    assert!(book_instances_stale(
        rev,
        emit,
        last_range,
        last_lo_hi,
        rev,
        view.render_range,
        lo,
        hi,
        margin_price
    ));
    let window_rebuilt = !(lo >= emit.0) || !(hi <= emit.1);
    let plan = plan_book_bake(&key, &gpu(&view), false, false, window_rebuilt);
    assert!(
        plan.bake && plan.immediate,
        "leaving the window must bake at once"
    );

    // No margin (Metal/wgpu): exactly the old (rev, lo, hi) inequality.
    let table = [
        (7u64, (1.0f32, 2.0f32)),
        (8, (1.0, 2.0)),
        (7, (1.0, 2.5)),
        (7, (0.5, 2.0)),
        (7, (1.5, 1.8)),
    ];
    for (r, (l, h)) in table {
        let stale = book_instances_stale(7, (1.0, 2.0), 1.0, (1.0, 2.0), r, 1.0, l, h, 0.0);
        assert_eq!(
            stale,
            r != 7 || (l, h) != (1.0, 2.0),
            "rev {r} lo {l} hi {h}"
        );
    }
}

/// `market.rs:volume_sample_timeframe_bound` seeding the patch fold at `0.0` drops a wide row
/// the patch does not touch.
///
/// The user-visible consequence is the volume band clipping a coarse candle that is still
/// retained. A wider tail is the other direction and must raise the bound.
#[test]
fn patch_bound_keeps_wide_row_and_wider_tail() {
    use super::candle_apply::CandleApply;
    use super::spans::volume_sample_timeframe_bound;

    let mut samples = Vec::with_capacity(8);
    for i in 0..8 {
        samples.push(moon_chart::VolumeSample {
            t_open_ms: i as f64,
            tf_ms: if i == 1 { 3_600_000.0 } else { 60_000.0 },
            quote_volume: 1.0,
        });
    }
    let previous = 3_600_000.0;
    let suffix_max = samples[4..]
        .iter()
        .map(|sample| sample.tf_ms)
        .fold(0.0, f64::max);
    assert!(
        suffix_max < previous,
        "fixture suffix is narrower than row 1"
    );
    assert_eq!(
        volume_sample_timeframe_bound(&samples, &CandleApply::Patch(4), previous),
        previous.max(suffix_max),
        "a patch must keep the older wide row"
    );

    samples[7].tf_ms = 7_200_000.0;
    let raised_suffix = samples[4..]
        .iter()
        .map(|sample| sample.tf_ms)
        .fold(0.0, f64::max);
    assert!(raised_suffix > previous, "fixture tail is a new maximum");
    assert_eq!(
        volume_sample_timeframe_bound(&samples, &CandleApply::Patch(4), previous),
        previous.max(raised_suffix),
        "a wider tail must raise the bound"
    );
}
