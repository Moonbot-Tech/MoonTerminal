use super::*;
use moon_core::feed::{OrderRow, OrderTrace, OrderTracePoint};

fn test_order_with_buy_trace() -> OrderRow {
    OrderRow {
        market: "BTCUSDT".into(),
        market_display: "BTCUSDT".into(),
        is_short: false,
        size: 0.01,
        remaining_size: 0.01,
        sl_on: false,
        ts_on: false,
        sl_strat: false,
        ts_strat: false,
        vstop_strat: false,
        vstop_on: false,
        sl_fixed: false,
        ts_fixed: false,
        vstop_fixed: false,
        vstop_level: 0.0,
        vstop_vol: 0.0,
        buy_price: 60_000.0,
        sell_price: 0.0,
        create_time_ms: 1_000.0,
        price: 61_000.0,
        fill_pct: 0.0,
        strat: "test".into(),
        strat_name: String::new(),
        strat_id: 0,
        status: String::new(),
        uid: 42,
        emulator: false,
        job_is_done: false,
        pending: false,
        filled: false,
        stop_loss: None,
        trailing: None,
        take_profit: None,
        vstop: None,
        pending_cond: None,
        liq: None,
        panic_sell: false,
        is_moon_shot: false,
        corridor_price_down: 0.0,
        corridor_price_up: 0.0,
        buy_trace: Some(OrderTrace {
            points: vec![
                OrderTracePoint {
                    time_ms: 1_000.0,
                    price: 60_000.0,
                },
                OrderTracePoint {
                    time_ms: 2_000.0,
                    price: 60_000.0,
                },
                OrderTracePoint {
                    time_ms: 0.0,
                    price: 0.0,
                },
                OrderTracePoint {
                    time_ms: 2_000.0,
                    price: 61_000.0,
                },
            ],
            tmp_point: Some(OrderTracePoint {
                time_ms: 2_500.0,
                price: 61_500.0,
            }),
            stop_price: Some(59_500.0),
            stop_time_ms: Some(2_000.0),
        }),
        sell_trace: None,
    }
}

fn near(a: f32, b: f32) -> bool {
    (a - b).abs() < 0.001
}

#[test]
fn moonshot_zone_keeps_moonbot_fixed_opacity() {
    let mut row = test_order_with_buy_trace();
    row.is_moon_shot = true;
    row.corridor_price_down = 59_000.0;
    row.corridor_price_up = 61_000.0;
    row.fill_pct = 0.0;

    let mut store = OrderLineStore::default();
    assert!(store.update(&[row]));

    let mut zones = Vec::new();
    let mut hlines = Vec::new();
    let mut segs = Vec::new();
    let mut markers = Vec::new();
    build_order_geometry(
        &store,
        "BTCUSDT",
        &OrdersStyle::default(),
        None,
        None,
        0.0,
        3_000.0,
        0.0,
        10_000.0,
        10_000.0,
        &mut zones,
        &mut hlines,
        &mut segs,
        &mut markers,
    );

    assert_eq!(zones.len(), 1);
    assert!(near(zones[0].price0, 59_000.0));
    assert!(near(zones[0].price1, 61_000.0));
    assert!(
        near(zones[0].color[3], MB_MOONSHOT_ZONE_ALPHA),
        "MoonShot area opacity must not inherit pending order line alpha"
    );
}

#[test]
fn server_trace_is_separate_from_active_order_line() {
    let mut store = OrderLineStore::default();
    assert!(store.update(&[test_order_with_buy_trace()]));

    let mut zones = Vec::new();
    let mut hlines = Vec::new();
    let mut segs = Vec::new();
    let mut markers = Vec::new();
    build_order_geometry(
        &store,
        "BTCUSDT",
        &OrdersStyle::default(),
        None,
        None,
        0.0,
        3_000.0,
        0.0,
        10_000.0,
        10_000.0,
        &mut zones,
        &mut hlines,
        &mut segs,
        &mut markers,
    );

    assert!(
        segs.iter().any(|s| {
            near(s.extend, 1.0)
                && near(s.p0, 60_000.0)
                && near(s.p1, 60_000.0)
                && near(s.t0_rel, 1_000.0)
        }),
        "active order line must stay a straight live-price segment"
    );
    assert!(
        segs.iter().any(|s| {
            near(s.extend, 0.0)
                && near(s.t0_rel, 1_000.0)
                && near(s.t1_rel, 2_000.0)
                && near(s.p0, 60_000.0)
                && near(s.p1, 60_000.0)
                && near(s.pattern, SEG_PATTERN_DASH_DOT_DOT)
                && near(s.thickness, 1.0)
        }),
        "server trace must keep its own horizontal history segment"
    );
    assert!(
        segs.iter().any(|s| {
            near(s.extend, 0.0)
                && near(s.t0_rel, 2_000.0)
                && near(s.t1_rel, 2_000.0)
                && near(s.p0, 60_000.0)
                && near(s.p1, 61_000.0)
                && near(s.pattern, SEG_PATTERN_DASH_DOT_DOT)
                && near(s.thickness, 1.0)
        }),
        "server trace must keep its own vertical price-change segment"
    );
    assert!(
        segs.iter().any(|s| {
            near(s.extend, 0.0)
                && near(s.t0_rel, 2_500.0)
                && near(s.t1_rel, 2_500.0)
                && near(s.p0, 61_000.0)
                && near(s.p1, 61_500.0)
                && near(s.pattern, SEG_PATTERN_DOT)
                && near(s.thickness, 1.0)
        }),
        "server trace temp point must be drawn as Moonbot dotted vertical preview"
    );
    assert!(
        segs.iter().any(|s| {
            near(s.extend, 0.0)
                && near(s.t0_rel, 1_000.0)
                && near(s.t1_rel, 2_000.0)
                && near(s.p0, 59_500.0)
                && near(s.p1, 59_500.0)
                && near(s.pattern, SEG_PATTERN_DOT)
                && near(s.thickness, 2.0)
        }),
        "server trace stop-line must be drawn like Moonbot SetStopPrice"
    );
}

#[test]
fn dragging_order_keeps_server_trace_visible() {
    let mut store = OrderLineStore::default();
    assert!(store.update(&[test_order_with_buy_trace()]));

    let mut zones = Vec::new();
    let mut hlines = Vec::new();
    let mut segs = Vec::new();
    let mut markers = Vec::new();
    build_order_geometry(
        &store,
        "BTCUSDT",
        &OrdersStyle::default(),
        None,
        Some((42, LineKind::Buy, 62_000.0)),
        0.0,
        3_000.0,
        0.0,
        10_000.0,
        10_000.0,
        &mut zones,
        &mut hlines,
        &mut segs,
        &mut markers,
    );

    assert!(
        segs.iter().any(|s| {
            near(s.extend, 1.0)
                && near(s.p0, 62_000.0)
                && near(s.p1, 62_000.0)
                && near(s.t0_rel, 1_000.0)
        }),
        "drag preview must move only the active order line"
    );
    assert!(
        segs.iter().any(|s| {
            near(s.extend, 0.0)
                && near(s.t0_rel, 1_000.0)
                && near(s.t1_rel, 2_000.0)
                && near(s.p0, 60_000.0)
                && near(s.p1, 60_000.0)
                && near(s.pattern, SEG_PATTERN_DASH_DOT_DOT)
        }),
        "drag preview must not hide the server trace object"
    );
}
