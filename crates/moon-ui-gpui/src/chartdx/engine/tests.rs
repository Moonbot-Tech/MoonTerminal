//! Regression coverage for the toolbar's Live/Pause camera transition.

use super::ChartEngine;
use moon_core::config::ChartTheme;

/// Resuming Live must preserve a manually chosen time scale at both narrow and wide plot widths.
#[test]
fn pause_and_live_preserve_time_zoom_through_the_next_prepare() {
    for width in [320.0, 1400.0] {
        let now = 1_700_000_000_000.0;
        let mut engine = ChartEngine::new(now, ChartTheme::default());
        engine.open(42, "TESTUSDT");
        let ppm = {
            let mut container = engine.container.borrow_mut();
            let view = &mut container.panes_mut().first_mut().expect("main pane").view;
            view.ensure_default_window(width, 60.0, None);
            view.zoom_x_at(
                128.0,
                width,
                width * 0.5,
                now,
                moon_chart::view::XZoom::Ctrl,
            );
            view.px_per_ms
        };
        for cycle in 1..=3 {
            assert!(engine.set_follow(false, now + cycle as f64 * 1000.0));
            assert!(engine.set_follow(true, now + cycle as f64 * 1000.0 + 500.0));
            let mut container = engine.container.borrow_mut();
            let view = &mut container.panes_mut().first_mut().expect("main pane").view;
            view.ensure_default_window(width, 60.0, None);
            assert_eq!(view.px_per_ms, ppm, "Live reset the selected time zoom");
            assert_eq!(view.right_time_ms, now + cycle as f64 * 1000.0 + 500.0);
        }
    }
}

/// Catches `data_state/state.rs:apply_slot_geometry` collapsing the frame's two factors back into
/// one (the chart would zoom with the interface, or draw into a wrong-sized target), and
/// `engine.rs:chart_local_from_window_pos` crossing with the chart-design factor instead of the
/// window's (under UI zoom the pointer would land at `1 / zoom` of where it is).
#[test]
fn slot_geometry_keeps_device_density_while_the_pointer_crosses_with_the_windows_factor() {
    use gpui::{Bounds, GpuFrameInfo, point, px, size};
    use std::time::Instant;

    let now = 1_700_000_000_000.0;
    let mut engine = ChartEngine::new(now, ChartTheme::default());
    engine.open(42, "TESTUSDT");
    // The scene starts hidden. A frame before this returns Skip and installs no slot.
    engine.set_scene_visible(true);
    engine.data.borrow_mut().frame(GpuFrameInfo {
        now: Instant::now(),
        bounds: Bounds {
            origin: point(px(10.0), px(20.0)),
            size: size(px(200.0), px(100.0)),
        },
        scale_factor: 3.0,
        content_zoom: 1.5,
        presentable: true,
    });

    let (bounds, ppp, (w, h)) = engine.slot_geometry().expect("the frame installed a slot");
    assert_eq!(
        f32::from(bounds.origin.x),
        10.0,
        "slot bounds stay content space"
    );
    assert_eq!(
        (w, h),
        (600, 300),
        "the target is the slot in device pixels: bounds × 3.0"
    );
    assert_eq!(
        ppp, 2.0,
        "the chart's own factor excludes the zoom: 3.0 / 1.5"
    );
    assert_eq!(engine.slot_content_zoom(), 1.5);
    assert_eq!(engine.slot_scale_factor(), 3.0);

    let ((x, y), within) = engine
        .chart_local_from_window_pos(point(px(60.0), px(70.0)))
        .expect("the frame installed a slot");
    assert_eq!((x, y), (150.0, 150.0), "(60 − 10) × 3 and (70 − 20) × 3");
    assert!(within);
}

/// Whole-pixel live edge, the same rounding `ChartView::quantize_edge_ms` uses.
///
/// Args:
///     epoch_ms: Chart time origin.
///     px_per_ms: Scale read back from the pane after the frame.
///     edge_ms: Wall-clock instant the sync may have anchored.
///
/// Returns:
///     `edge_ms` snapped onto the pane's pixel grid.
fn quantized_edge(epoch_ms: f64, px_per_ms: f32, edge_ms: f64) -> f64 {
    let ppm = f64::from(px_per_ms.max(moon_chart::view::MIN_PX_PER_MS));
    let rel = edge_ms - epoch_ms;
    (rel * ppm).round() / ppm + epoch_ms
}

/// One-level book. Bids are descending and asks ascending, which `OrderBookModel::update` requires.
///
/// Args:
///     bid: Best bid.
///     ask: Best ask.
///
/// Returns:
///     A snapshot whose midpoint is `(bid + ask) / 2`.
fn book(bid: f32, ask: f32) -> moon_core::feed::OrderBook {
    moon_core::feed::OrderBook {
        bids: vec![moon_core::feed::Level {
            price: bid,
            qty: 1.0,
        }],
        asks: vec![moon_core::feed::Level {
            price: ask,
            qty: 1.0,
        }],
    }
}

/// One presentable slot. Same bounds every call so a later frame is not a resize.
///
/// Args:
///     engine: Chart whose data state receives the frame.
fn present_frame(engine: &mut ChartEngine) {
    use gpui::{Bounds, GpuFrameInfo, point, px, size};
    use std::time::Instant;

    engine.data.borrow_mut().frame(GpuFrameInfo {
        now: Instant::now(),
        bounds: Bounds {
            origin: point(px(0.0), px(0.0)),
            size: size(px(800.0), px(400.0)),
        },
        scale_factor: 1.0,
        content_zoom: 1.0,
        presentable: true,
    });
}

/// Read the pane camera the next pull will publish.
///
/// Args:
///     engine: Chart whose main pane is open.
///
/// Returns:
///     `(right_time_ms, center_price, px_per_ms)`.
fn camera(engine: &ChartEngine) -> (f64, f32, f32) {
    let container = engine.container.borrow();
    let view = &container.panes().first().expect("main pane").view;
    (view.right_time_ms, view.center_price, view.px_per_ms)
}

/// Park the live pane on a known camera without marking the view dirty.
///
/// Args:
///     engine: Chart whose main pane is open.
///     right_time_ms: Time anchor to store.
///     center_price: Y center to store.
///     price_range: Y range to store. Kept wide enough that a later book mid clears `CENTER_BUFFER`.
fn park_camera(engine: &mut ChartEngine, right_time_ms: f64, center_price: f32, price_range: f32) {
    let mut container = engine.container.borrow_mut();
    let view = &mut container.panes_mut().first_mut().expect("main pane").view;
    view.follow = true;
    view.right_time_ms = right_time_ms;
    view.center_price = center_price;
    view.price_range = price_range;
}

/// `engine.rs:ChartEngine::set_scene_visible` dropping `if visible { data.mark_view_dirty(); }`
/// leaves the next shown frame on the camera parked while the chart was hidden, so a covered
/// chart comes back on the old book center after the book has moved.
#[test]
fn reveal_after_a_hidden_book_update_refreshes_center_and_live_edge() {
    use std::collections::HashMap;

    use moon_core::market::{MarketDataSource, MarketStore};

    const CORE: u64 = 42;
    const MARKET: &str = "TESTUSDT";
    let epoch = 1_700_000_000_000.0;
    let store = MarketStore::shared(epoch);
    {
        let mut guard = store.write().expect("market store");
        guard.reset(CORE, MARKET);
        guard.apply_book(CORE, MARKET, &book(100.0, 101.0));
    }
    let source = MarketDataSource::new(store.clone());
    let mut providers = HashMap::new();
    providers.insert(CORE, CORE);
    source.set_provider_map(&providers);

    let mut engine = ChartEngine::new(epoch, ChartTheme::default());
    engine.open(CORE, MARKET);
    engine.set_scene_visible(true);
    engine.set_market_source(Some(source.clone()));
    present_frame(&mut engine);
    assert!(
        !engine.data.borrow().view_dirty,
        "the first visible frame must finish its pull"
    );

    // Far from the live edge, and above the first book's mid, so a spurious pull cannot
    // leave both numbers sitting on these sentinels.
    let sentinel_edge = epoch + 50_000.0;
    let sentinel_center = 180.0;
    park_camera(&mut engine, sentinel_edge, sentinel_center, 20.0);
    engine.set_scene_visible(true);
    present_frame(&mut engine);
    let (right, center, _) = camera(&engine);
    assert_eq!(right, sentinel_edge, "repeated visibility must not pull");
    assert_eq!(
        center, sentinel_center,
        "repeated visibility must not refit"
    );

    engine.set_scene_visible(false);
    {
        let mut guard = store.write().expect("market store");
        guard.apply_book(CORE, MARKET, &book(249.0, 251.0));
    }
    let (bid, ask) = source
        .with_orderbook_view(CORE, MARKET, |view| {
            view.and_then(|(book, _)| book.best_bid_ask())
        })
        .expect("the hidden book update is in the store");
    let mid = (bid + ask) * 0.5;
    assert!(
        mid > sentinel_center,
        "fixture mid {mid} must sit above the parked center"
    );

    engine.set_scene_visible(true);
    let now_before = moon_core::util::time::now_unix_ms();
    present_frame(&mut engine);
    let now_after = moon_core::util::time::now_unix_ms();
    let (right, center, ppm) = camera(&engine);
    let q_before = quantized_edge(epoch, ppm, now_before);
    let q_after = quantized_edge(epoch, ppm, now_after);
    let pixel_ms = 1.0 / f64::from(ppm.max(moon_chart::view::MIN_PX_PER_MS));
    let edge_ok = right == q_before
        || right == q_after
        || ((right - q_before).abs() <= pixel_ms && (right - now_after).abs() <= 2_000.0);
    assert!(
        center > sentinel_center && center < mid && edge_ok,
        "reveal frame kept hidden center {center} (book mid {mid}) and edge {right} \
         (quantized {q_before}..{q_after})"
    );
}
