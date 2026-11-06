//! Read back the native target to catch debt that never reaches the cached chart image.

use crate::chartdx::{ChartEngine, PaneRender, gpu::test_support::TestTarget, types::BookStyle};
use gpui::GpuFrameDecision;
use moon_core::{config::ChartTheme, data::LevelInstance};
use std::time::Duration;

/// A quiet, visible chart with one fully coloured book stripe and no feed to hide missed work.
fn book_chart() -> ChartEngine {
    let mut engine = ChartEngine::new(0.0, ChartTheme::default());
    engine.set_scene_visible(true);
    engine.state.borrow_mut().chart_labels = std::rc::Rc::new(moon_core::config::ChartLabelsCfg {
        rows: Default::default(),
    });
    engine.data.borrow_mut().frame(super::frame_info());
    let mut pane = PaneRender::new();
    pane.active = true;
    pane.follow = false;
    pane.gpu_prepare_dirty = true;
    pane.pane_bounds = [0.0, 0.0, 320.0, 200.0];
    pane.view.bounds = pane.pane_bounds;
    pane.view.resolution = [320.0, 200.0];
    pane.view.price_to_px = 1.0;
    pane.view.view_price0 = 0.0;
    pane.orderbook_view = pane.view;
    pane.book_style = BookStyle {
        book_bg: [0., 0., 0., 1.],
        bid: [0., 1., 0., 1.],
        ask: [1., 0., 0., 1.],
        ..BookStyle::default()
    };
    pane.layers.set_orderbook(&[level(0.0)], true);
    engine.state.borrow_mut().panes.push(pane);
    engine
}

/// The sampled pixel at y=128 lies inside this price stripe (y=120..136).
fn level(kind: f32) -> LevelInstance {
    LevelInstance {
        price: 64.0,
        span: 16.0,
        len_norm: 1.0,
        kind,
    }
}

/// Run the same prepare/draw boundary as the canvas driver, then read the final target.
fn draw(engine: &ChartEngine, target: &TestTarget, generation: u64) -> Vec<u8> {
    let mut state = engine.state.borrow_mut();
    state.prepare_gpu(&target.raw(generation)).unwrap();
    state.draw_gpu(&target.raw(generation)).unwrap();
    target.pixels()
}

/// Inspect the stripe rather than the layer's dirty flag: a fresh book texture hidden behind an
/// unchanged outer BaseCache is still the old picture to the user.
fn stripe(pixels: &[u8]) -> &[u8] {
    &pixels[(128 * 320 + 160) * 4..(128 * 320 + 160) * 4 + 4]
}

/// A final book update inside the throttle must become visible without another data revision.
#[test]
fn quiet_book_debt_reaches_the_outer_base_cache() {
    // The full debug chart frame shares the large stack requirement of the market-sync tests.
    std::thread::Builder::new()
        .stack_size(16 * 1024 * 1024)
        .spawn(book_debt)
        .unwrap()
        .join()
        .unwrap();
}

/// Exercise the final throttled update through the real quiet-chart frame and both GPU caches.
fn book_debt() {
    let engine = book_chart();
    let target = TestTarget::new(320, 200);
    assert_eq!(stripe(&draw(&engine, &target, 1)), &[0, 255, 0, 255]);
    // All shaders are warm before starting the real throttle window.
    {
        let mut state = engine.state.borrow_mut();
        state.panes[0].layers.set_orderbook(&[level(0.0)], true);
        state.panes[0].gpu_prepare_dirty = true;
        state.base_dirty = true;
    }
    draw(&engine, &target, 1);
    {
        let mut state = engine.state.borrow_mut();
        state.panes[0].layers.set_orderbook(&[level(1.0)], false);
        state.panes[0].gpu_prepare_dirty = true;
        state.base_dirty = true;
    }
    assert_eq!(
        stripe(&draw(&engine, &target, 1)),
        &[0, 255, 0, 255],
        "data throttle must hold the second update"
    );
    // Consume unrelated initial presentation debt before waiting for this book's deadline.
    assert!(
        matches!(
            engine.data.borrow_mut().frame(super::frame_info()),
            GpuFrameDecision::Skip
        ),
        "no unrelated clock may mask the book deadline"
    );
    std::thread::sleep(Duration::from_millis(210));
    assert!(
        matches!(
            engine.data.borrow_mut().frame(super::frame_info()),
            GpuFrameDecision::RequestPresent
        ),
        "the last book revision must own its wake-up"
    );
    assert_eq!(
        stripe(&draw(&engine, &target, 1)),
        &[0, 0, 255, 255],
        "rebaked book must reach the presented base"
    );
    assert!(
        matches!(
            engine.data.borrow_mut().frame(super::frame_info()),
            GpuFrameDecision::Skip
        ),
        "paid debt must settle"
    );
}
