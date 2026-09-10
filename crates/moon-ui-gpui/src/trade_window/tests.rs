use super::remembered_geometry;
use moon_core::config::layout::GeomRect;

/// New tick groups must replace a streaming snapshot without reframing or reverting to candles.
#[test]
fn progressive_trade_ticks_preserve_view_and_accept_core_completion() {
    use super::{TradeWindowState, fold_outcome};
    use moon_core::market::trade_replay::{
        TickStatus, TradeReplayOutcome, TradeReplaySeries, TradeReplaySource, replay_window,
    };
    let state = TradeWindowState::Ready {
        source: TradeReplaySource::Ticks,
        tf_min: 1,
        tick_status: TickStatus::Streaming,
        bucket_ms: 0,
        partial: true,
        brand: moon_core::venue::Brand::Binance,
    };
    let mut series = TradeReplaySeries {
        source: TradeReplaySource::Ticks,
        venue: moon_core::venue::venue(3).expect("spot venue"),
        window: replay_window(100_000, 100_060).expect("valid trade"),
        tf_ms: 60_000,
        candles: Vec::new(),
        ticks: vec![moon_core::feed::Tick {
            time_ms: 100_000_000.0,
            price: 10.0,
            qty: 1.0,
            side: moon_core::feed::Side::Buy,
        }],
        identity: 42,
        tick_status: TickStatus::Streaming,
        bucket_ms: 0,
        partial: true,
        covered: Some((99_700_000, 100_000_000)),
    };
    let next = fold_outcome(&state, true, &TradeReplayOutcome::Ready(series.clone()));
    assert!(next.accept);
    assert!(
        !next.frame,
        "a streamed group must preserve the user's pan and zoom"
    );
    series.source = TradeReplaySource::CoreTicks;
    series.tick_status = TickStatus::Served;
    let final_core = fold_outcome(&state, true, &TradeReplayOutcome::Ready(series.clone()));
    assert!(final_core.accept && final_core.restore_candle_mode);
    let awaiting = TradeWindowState::Ready {
        source: TradeReplaySource::Klines1m,
        tf_min: 1,
        tick_status: TickStatus::AwaitingCore,
        bucket_ms: 0,
        partial: false,
        brand: moon_core::venue::Brand::Bybit,
    };
    let delayed_core = fold_outcome(&awaiting, true, &TradeReplayOutcome::Ready(series.clone()));
    assert!(delayed_core.accept && delayed_core.restore_candle_mode);
    assert!(
        !delayed_core.frame,
        "a delayed archive must preserve the candle view's pan and zoom"
    );
    series.source = TradeReplaySource::Klines1m;
    series.ticks.clear();
    series.candles.push(moon_core::market::ChartCandle {
        t_open_ms: 100_000_000.0,
        open: 10.0,
        high: 11.0,
        low: 9.0,
        close: 10.0,
        volume: 1.0,
        quote_volume: 10.0,
    });
    series.tick_status = TickStatus::NoRoute;
    assert!(fold_outcome(&awaiting, true, &TradeReplayOutcome::Ready(series.clone())).accept);
    assert!(!fold_outcome(&state, true, &TradeReplayOutcome::Ready(series)).accept);
}

fn rect(x: i32, y: i32, w: u32, h: u32, display_uuid: Option<uuid::Uuid>) -> GeomRect {
    GeomRect {
        x,
        y,
        w,
        h,
        maximized: false,
        fullscreen: false,
        display_uuid,
    }
}

/// `trade_window::remembered_geometry` must undo a cascade rather than persist it. Deleting the
/// nonzero-cascade branch makes each reopened trade window remember its offset and walk off-screen
/// over time, while losing the previous display identity can restore it on the wrong monitor.
#[test]
fn remembered_trade_window_geometry_never_persists_a_cascade_offset() {
    let identity = uuid::Uuid::from_u128(0xfeed_cafe_dead_beef_0123_4567_89ab_cdef);
    let observed = rect(134, 234, 900, 600, Some(identity));

    let uncascaded = remembered_geometry(None, observed, 0.0);
    assert_eq!(
        (
            uncascaded.x,
            uncascaded.y,
            uncascaded.w,
            uncascaded.h,
            uncascaded.display_uuid
        ),
        (
            observed.x,
            observed.y,
            observed.w,
            observed.h,
            observed.display_uuid
        ),
        "an uncascaded observation must be remembered whole"
    );
    let first_cascade = remembered_geometry(None, observed, 34.0);
    assert_eq!(
        (
            first_cascade.x,
            first_cascade.y,
            first_cascade.w,
            first_cascade.h,
            first_cascade.display_uuid
        ),
        (100, 200, 900, 600, Some(identity)),
        "the first cascaded window must subtract its opening offset before saving"
    );

    let previous = rect(100, 200, 640, 480, Some(identity));
    let subsequent_cascade = remembered_geometry(Some(previous), observed, 34.0);
    assert_eq!(
        (
            subsequent_cascade.x,
            subsequent_cascade.y,
            subsequent_cascade.w,
            subsequent_cascade.h,
            subsequent_cascade.display_uuid
        ),
        (100, 200, 900, 600, Some(identity)),
        "a cascaded window keeps the remembered origin and display while retaining its new size"
    );

    let mut saved = previous;
    for _ in 0..5 {
        let observed = rect(saved.x + 34, saved.y + 34, saved.w, saved.h, Some(identity));
        saved = remembered_geometry(Some(saved), observed, 34.0);
        assert_eq!(
            (saved.x, saved.y, saved.display_uuid),
            (100, 200, Some(identity)),
            "reopening a cascaded window repeatedly must not drift its remembered origin"
        );
    }
}
