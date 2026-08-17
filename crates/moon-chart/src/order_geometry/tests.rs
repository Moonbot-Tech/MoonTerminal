use super::*;
use moon_core::config::ChartGraphicsCfg;
use moon_core::feed::{OrderRow, OrderTrace, OrderTracePoint};
use moon_core::util::time::now_unix_ms;

fn test_order_with_buy_trace() -> OrderRow {
    OrderRow {
        market: "BTCUSDT".into(),
        market_display: "BTCUSDT".into(),
        coin: "BTC".into(),
        quote: "USDT".into(),
        is_short: false,
        size: 0.01,
        remaining_size: 0.01,
        sl_on: false,
        ts_on: false,
        vstop_on: false,
        sl_fixed: false,
        ts_fixed: false,
        vstop_fixed: false,
        vstop_level: 0.0,
        vstop_vol: 0.0,
        buy_price: 60_000.0,
        sell_price: 0.0,
        create_time_ms: 1_000.0,
        sell_create_time_ms: 0.0,
        entry_fill_time_ms: 0.0,
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

/// Whether a segment is the still-holding continuation drawn past a filled entry's arrow.
///
/// Identified by COLOUR, because that is the only thing separating it from the entry line's own
/// objects: it sits at the same price, on the same line kind, and it deliberately runs past the fill
/// that every other entry object stops at. Assertions about the entry line therefore have to exclude
/// it explicitly, or they read the continuation as the very overrun they exist to forbid.
fn is_position_hold(seg: &SegInstance) -> bool {
    let hold = crate::layers::rgb_with_alpha(POSITION_HOLD_RGB, 1.0);
    near(seg.color[0], hold[0]) && near(seg.color[1], hold[1]) && near(seg.color[2], hold[2])
}

/// Builds the primary order-line segments for the shared BTCUSDT fixture market.
fn draw_order_segments(
    store: &OrderLineStore,
    style: &OrdersStyle,
    graphics: &ChartGraphicsCfg,
) -> Vec<SegInstance> {
    let mut zones = Vec::new();
    let mut hlines = Vec::new();
    let mut segs = Vec::new();
    let mut markers = Vec::new();
    build_order_geometry(
        store,
        "BTCUSDT",
        style,
        graphics,
        1.0,
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
    segs
}

/// Counts straight segments at one deliberately unique fixture price.
fn segment_count_at(segs: &[SegInstance], price: f32) -> usize {
    segs.iter()
        .filter(|seg| near(seg.p0, price) && near(seg.p1, price))
        .count()
}

/// `order_geometry.rs:build_order_geometry` must suppress only a closed order's sell line when
/// `hide_closed_sell_line` is enabled; deleting that guard leaves a stale exit stripe after closure.
#[test]
fn closed_sell_line_visibility_keeps_live_exit_lines() {
    let mut closed = test_order_with_buy_trace();
    closed.uid = 101;
    closed.buy_trace = None;
    closed.buy_price = 60_101.0;
    closed.filled = true;
    closed.fill_pct = 100.0;
    closed.sell_create_time_ms = 2_000.0;
    closed.sell_price = 61_101.0;
    closed.job_is_done = true;

    let mut live = test_order_with_buy_trace();
    live.uid = 202;
    live.buy_trace = None;
    live.buy_price = 60_202.0;
    live.filled = true;
    live.fill_pct = 100.0;
    live.sell_create_time_ms = 2_000.0;
    live.sell_price = 61_202.0;

    let mut store = OrderLineStore::default();
    assert!(store.update(&[closed, live]));

    let style = OrdersStyle::default();
    let hidden = draw_order_segments(&store, &style, &ChartGraphicsCfg::default());
    assert_eq!(
        segment_count_at(&hidden, 61_101.0),
        0,
        "the closed order's unique sell price must not leave a segment by default"
    );
    assert_eq!(
        segment_count_at(&hidden, 61_202.0),
        1,
        "a live order must keep its unique sell segment while closed exits are hidden"
    );

    let visible = draw_order_segments(
        &store,
        &style,
        &ChartGraphicsCfg {
            hide_closed_sell_line: false,
            ..ChartGraphicsCfg::default()
        },
    );
    assert_eq!(segment_count_at(&visible, 61_101.0), 1);
    assert_eq!(segment_count_at(&visible, 61_202.0), 1);
}

/// An order the core dates not at all keeps the local clock as its own creation while its exit line
/// still carries a real wire time from before that. Culling on the creation alone would drop the
/// whole order out of a window its line demonstrably occupies.
#[test]
fn an_order_dated_only_by_its_exit_survives_a_window_before_its_creation() {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock must be after the epoch")
        .as_millis() as f64;
    let mut row = test_order_with_buy_trace();
    row.buy_trace = None;
    row.create_time_ms = 0.0;
    row.sell_create_time_ms = now - 600_000.0;
    row.filled = true;
    row.fill_pct = 100.0;
    row.sell_price = 61_000.0;

    let mut store = OrderLineStore::default();
    assert!(store.update(&[row]));

    let mut zones = Vec::new();
    let mut hlines = Vec::new();
    let mut segs = Vec::new();
    let mut markers = Vec::new();
    // A window that ends before the fallback creation but holds the exit line's start.
    build_order_geometry(
        &store,
        "BTCUSDT",
        &OrdersStyle::default(),
        &ChartGraphicsCfg::default(),
        1.0,
        None,
        None,
        now - 900_000.0,
        now,
        0.0,
        400_000.0,
        400_000.0,
        &mut zones,
        &mut hlines,
        &mut segs,
        &mut markers,
    );

    assert!(
        !segs.is_empty(),
        "an order whose exit line reaches into the window must not be culled by its creation"
    );
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
        &ChartGraphicsCfg::default(),
        1.0,
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
        &ChartGraphicsCfg::default(),
        1.0,
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
        &ChartGraphicsCfg::default(),
        1.0,
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

/// A filled order fixture with one distinct price per drawable line kind, so geometry assertions
/// identify the user-visible line rather than relying on the flat GPU-vector order.
fn filled_order_without_trace(now: f64) -> OrderRow {
    let mut row = test_order_with_buy_trace();
    row.buy_trace = None;
    row.create_time_ms = now - 600_000.0;
    row.entry_fill_time_ms = now - 400_000.0;
    row.sell_create_time_ms = now - 300_000.0;
    row.buy_price = 60_000.0;
    row.sell_price = 61_000.0;
    row.stop_loss = Some(59_000.0);
    row.trailing = Some(59_500.0);
    row.take_profit = Some(62_000.0);
    row.vstop = Some(58_000.0);
    row.pending_cond = Some(57_000.0);
    row.filled = true;
    row.fill_pct = 100.0;
    row.remaining_size = 0.0;
    row
}

/// Builds one order's chart geometry in a window whose relative coordinates are stable around now.
fn geometry_for(row: OrderRow, now: f64) -> (Vec<SegInstance>, Vec<MarkerInstance>) {
    geometry_for_scaled(row, now, 1.0, &ChartGraphicsCfg::default())
}

/// As [`geometry_for`], with the device scale and graphics settings the caller wants to vary.
fn geometry_for_scaled(
    row: OrderRow,
    now: f64,
    scale: f32,
    graphics: &ChartGraphicsCfg,
) -> (Vec<SegInstance>, Vec<MarkerInstance>) {
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
        graphics,
        scale,
        None,
        None,
        now - 700_000.0,
        now + 100_000.0,
        0.0,
        800_000.0,
        800_000.0,
        &mut zones,
        &mut hlines,
        &mut segs,
        &mut markers,
    );
    (segs, markers)
}

/// `order_geometry.rs:build_order_geometry` must keep `fill_ms` limited to Buy; applying it to
/// every kind would truncate every live TP, SL, and trailing line at entry fill and hide working
/// order state from every filled order.
#[test]
fn filled_long_ends_only_its_entry_at_the_fill_arrow() {
    let now = now_unix_ms();
    let fill_rel = 300_000.0;
    let (segs, markers) = geometry_for(filled_order_without_trace(now), now);

    assert!(segs.iter().any(|seg| {
        near(seg.p0, 60_000.0)
            && near(seg.p1, 60_000.0)
            && near(seg.t1_rel, fill_rel)
            && near(seg.extend, 0.0)
    }));
    let endpoint_markers: Vec<_> = markers
        .iter()
        .filter(|marker| near(marker.t_rel, fill_rel) && near(marker.price, 60_000.0))
        .collect();
    assert_eq!(
        endpoint_markers
            .iter()
            .filter(|marker| near(marker.shape, MARKER_SHAPE_ARROW_UP))
            .count(),
        1,
        "a long entry fill must have exactly one buy arrow"
    );
    assert!(
        endpoint_markers
            .iter()
            .all(|marker| !near(marker.shape, MARKER_SHAPE_CROSS)),
        "the fill arrow must replace the entry endpoint cross"
    );

    for price in [61_000.0, 59_000.0, 59_500.0, 62_000.0, 58_000.0] {
        assert!(
            segs.iter().any(|seg| {
                near(seg.p0, price)
                    && near(seg.p1, price)
                    && near(seg.t1_rel, 800_000.0)
                    && near(seg.extend, 1.0)
            }),
            "the live line at {price} must remain extended to the pane edge"
        );
    }
}

/// `order_geometry.rs:build_order_geometry` must select a short entry's sell arrow; inverting
/// `if ord.is_short` would make the chart describe the trade direction as the opposite action.
#[test]
fn filled_short_ends_its_entry_with_only_a_down_arrow() {
    let now = now_unix_ms();
    let mut row = filled_order_without_trace(now);
    row.is_short = true;
    let (_, markers) = geometry_for(row, now);

    let endpoint_markers: Vec<_> = markers
        .iter()
        .filter(|marker| near(marker.t_rel, 300_000.0) && near(marker.price, 60_000.0))
        .collect();
    assert_eq!(
        endpoint_markers
            .iter()
            .filter(|marker| near(marker.shape, MARKER_SHAPE_ARROW_DOWN))
            .count(),
        1,
        "a short entry fill must have exactly one sell arrow"
    );
    assert!(
        endpoint_markers
            .iter()
            .all(|marker| !near(marker.shape, MARKER_SHAPE_ARROW_UP)),
        "a short entry must not show a buy arrow"
    );
}

/// `order_lines.rs:OrderLineStore::update` must retain the `filled` gate; dropping it would mark
/// a cancelled, unfilled entry at its wire close and falsely claim that the user made a trade.
#[test]
fn unfilled_order_with_a_dated_close_stays_live_and_unmarked() {
    let now = now_unix_ms();
    let mut row = filled_order_without_trace(now);
    row.filled = false;
    row.fill_pct = 0.0;
    row.remaining_size = row.size;
    let (segs, markers) = geometry_for(row, now);

    assert!(segs.iter().any(|seg| {
        near(seg.p0, 60_000.0)
            && near(seg.p1, 60_000.0)
            && near(seg.t1_rel, 800_000.0)
            && near(seg.extend, 1.0)
    }));
    assert!(
        markers
            .iter()
            .all(|marker| !near(marker.shape, MARKER_SHAPE_ARROW_UP)
                && !near(marker.shape, MARKER_SHAPE_ARROW_DOWN)),
        "an unfilled order must not emit a fill arrow"
    );
}

/// `order_geometry.rs:build_order_geometry` must preserve the old live entry when the wire never
/// dated a fill; treating zero as a fill would truncate the order line and invent a trade marker.
#[test]
fn filled_order_without_a_dated_fill_keeps_the_old_live_entry() {
    let now = now_unix_ms();
    let mut row = filled_order_without_trace(now);
    row.entry_fill_time_ms = 0.0;
    let (segs, markers) = geometry_for(row, now);

    assert!(segs.iter().any(|seg| {
        near(seg.p0, 60_000.0)
            && near(seg.p1, 60_000.0)
            && near(seg.t1_rel, 800_000.0)
            && near(seg.extend, 1.0)
    }));
    assert!(
        markers
            .iter()
            .all(|marker| !near(marker.shape, MARKER_SHAPE_ARROW_UP)
                && !near(marker.shape, MARKER_SHAPE_ARROW_DOWN)),
        "an undated fill must not emit an arrow"
    );
}

/// `order_geometry.rs:build_order_geometry` must keep the `max(start_t)` side of its fill clamp;
/// removing it lets an old wire fill precede a locally-fallback-created order and draws an inverted
/// entry segment that smears the chart.
#[test]
fn entry_fill_before_a_fallback_creation_is_clamped_to_the_line_start() {
    let now = now_unix_ms();
    let mut row = filled_order_without_trace(now);
    row.create_time_ms = 0.0;
    row.entry_fill_time_ms = now - 600_000.0;
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
        &ChartGraphicsCfg::default(),
        1.0,
        None,
        None,
        now - 700_000.0,
        now + 100_000.0,
        0.0,
        800_000.0,
        800_000.0,
        &mut zones,
        &mut hlines,
        &mut segs,
        &mut markers,
    );
    assert!(segs.iter().any(|seg| {
        near(seg.p0, 60_000.0)
            && near(seg.p1, 60_000.0)
            && near(seg.t0_rel, seg.t1_rel)
            && near(seg.extend, 0.0)
    }));
    assert!(markers.iter().any(|marker| {
        near(marker.price, 60_000.0) && near(marker.shape, MARKER_SHAPE_ARROW_UP)
    }));
}

/// `order_geometry.rs:build_order_geometry` must leave the old cross branch behind `else if`;
/// splitting it into independent branches would draw both a fill arrow and a close cross together.
#[test]
fn closed_filled_entry_marks_the_fill_not_the_later_close() {
    let now = now_unix_ms();
    let mut row = filled_order_without_trace(now);
    row.job_is_done = true;
    let (segs, markers) = geometry_for(row, now);
    let fill_rel = 300_000.0;

    assert!(segs.iter().any(|seg| {
        near(seg.p0, 60_000.0)
            && near(seg.p1, 60_000.0)
            && near(seg.t1_rel, fill_rel)
            && near(seg.extend, 0.0)
    }));
    let endpoint_markers: Vec<_> = markers
        .iter()
        .filter(|marker| near(marker.t_rel, fill_rel) && near(marker.price, 60_000.0))
        .collect();
    assert_eq!(
        endpoint_markers
            .iter()
            .filter(|marker| near(marker.shape, MARKER_SHAPE_ARROW_UP))
            .count(),
        1
    );
    assert!(
        endpoint_markers
            .iter()
            .all(|marker| !near(marker.shape, MARKER_SHAPE_CROSS)),
        "a filled entry must not draw a second close cross at its fill"
    );
}

/// The retained fill date of the shared BTCUSDT fixture order.
///
/// Assertions about adoption have to read THIS rather than `update`'s boolean: "nothing changed" is
/// also what a store that never adopted anything reports, so the flag alone cannot tell a fill held
/// steady from a fill that was never taken.
fn retained_fill(store: &OrderLineStore) -> Option<f64> {
    store
        .iter_market("BTCUSDT")
        .find(|order| order.uid == 42)
        .expect("the retained order must remain observable")
        .entry_fill_ms
}

/// `order_lines.rs:OrderLineStore::update` must ADOPT a folded fill once and then hold it steady
/// against its own moving local-clock stand-in.
///
/// Two edits must fail here, and the test used to catch only one of them. Replacing the current
/// match with bare equality re-adopts the clamp on every refresh, rebuilding and re-uploading all
/// order geometry for as long as the skew lasts — that is the `!store.update(..)` half. But DELETING
/// the adoption block outright also left this green, because a store that never adopts anything
/// reports "nothing changed" just as loudly as one holding a fill steady. So the retained timestamp
/// itself is now observed: it must be present, it must be the FOLDED stand-in rather than the raw
/// future wire date, and it must be bit-identical across the second update.
#[test]
fn future_fill_timestamp_is_adopted_once_without_rebuilding_each_update() {
    let now = now_unix_ms();
    let mut row = filled_order_without_trace(now);
    let wire_fill_ms = now + 600_000.0;
    row.entry_fill_time_ms = wire_fill_ms;
    let mut store = OrderLineStore::default();

    assert!(store.update(&[row.clone()]));
    let adopted = retained_fill(&store).expect(
        "a filled entry must adopt a fill date on the first update, folded or not — deleting the \
         adoption block leaves this None while every `changed` flag still looks correct",
    );
    assert!(
        adopted < wire_fill_ms,
        "a fill the core dates in the FUTURE must be folded back to the local clock, not taken \
         raw: adopted {adopted}, wire {wire_fill_ms}"
    );

    std::thread::sleep(std::time::Duration::from_millis(5));
    assert!(
        !store.update(&[row]),
        "the changing local clamp must not make an already-adopted fill look different"
    );
    assert_eq!(
        retained_fill(&store),
        Some(adopted),
        "the folded stand-in must be held EXACTLY, not re-folded to the later local clock"
    );
}

/// `order_lines.rs:OrderLineStore::update` must replace its first folded fill date when the wire
/// clock catches up; collapsing the current match to a presence-only comparison would leave the
/// entry arrow at the local time it was first observed instead of the trade's wire-dated fill.
#[test]
fn future_fill_timestamp_settles_to_wire_time_after_clock_catches_up() {
    let now = now_unix_ms();
    let wire_fill_ms = now + 100.0;
    let mut row = filled_order_without_trace(now);
    row.entry_fill_time_ms = wire_fill_ms;
    let mut store = OrderLineStore::default();

    assert!(store.update(&[row.clone()]));
    std::thread::sleep(std::time::Duration::from_millis(150));
    assert!(
        now_unix_ms() > wire_fill_ms,
        "the local clock must have passed the wire fill before the settling update"
    );
    assert!(
        store.update(&[row]),
        "settling a folded fill to the dated wire instant must update the retained order"
    );
    assert_eq!(
        retained_fill(&store),
        Some(wire_fill_ms),
        "the retained fill date must settle to the wire timestamp once the clocks agree"
    );
}

/// `order_geometry.rs:build_order_geometry` must bound a filled entry's repricing staircase at
/// `line_end_eff` and reject raw reprices after that bound; restoring `line_end` would draw the
/// path beyond the fill arrow, while testing clamped endpoints would stack false risers at it.
#[test]
fn filled_repriced_entry_path_stops_at_fill_without_post_fill_risers() {
    let now = now_unix_ms();
    let fill_rel = 300_000.0;
    let mut row = filled_order_without_trace(now);
    row.buy_price = 60_000.0;
    let mut store = OrderLineStore::default();
    assert!(store.update(&[row.clone()]));

    row.buy_price = 60_100.0;
    assert!(store.update(&[row.clone()]));
    row.buy_price = 60_200.0;
    assert!(store.update(&[row]));

    let mut zones = Vec::new();
    let mut hlines = Vec::new();
    let mut segs = Vec::new();
    let mut markers = Vec::new();
    build_order_geometry(
        &store,
        "BTCUSDT",
        &OrdersStyle::default(),
        &ChartGraphicsCfg::default(),
        1.0,
        None,
        None,
        now - 700_000.0,
        now + 100_000.0,
        0.0,
        800_000.0,
        800_000.0,
        &mut zones,
        &mut hlines,
        &mut segs,
        &mut markers,
    );

    assert!(
        !segs.iter().any(|seg| {
            // The still-holding continuation is the ONE object that legitimately runs past the fill at
            // the entry price, so it is excluded by colour rather than by price or time. Every other
            // segment at one of these prices reaching past the fill is the overrun this forbids.
            !is_position_hold(seg)
                && near(seg.p0, seg.p1)
                && [60_000.0, 60_100.0, 60_200.0]
                    .iter()
                    .any(|price| near(seg.p0, *price))
                && seg.t1_rel > fill_rel + 1.0
        }),
        "the repricing path must not extend beyond the entry fill"
    );
    assert!(
        !segs.iter().any(|seg| {
            near(seg.t0_rel, fill_rel) && near(seg.t1_rel, fill_rel) && !near(seg.p0, seg.p1)
        }),
        "a reprice after the fill must not add a vertical riser at the fill"
    );
    assert!(
        segs.iter().any(|seg| {
            near(seg.p0, 60_000.0) && near(seg.p1, 60_000.0) && near(seg.t1_rel, fill_rel)
        }),
        "the first repricing path step must terminate at the entry fill"
    );
}

/// Dropping the still-holding continuation must fail here.
///
/// The entry line stops at the fill arrow, which is right — the entry LEG ended there — but on its own
/// it deletes an OPEN position from the chart: the user sees an arrow and then nothing, while the money
/// is still on the table. One orange segment carries the entry price on to the pane edge for as long as
/// the order is open, in `SEG_EXTEND_EDGE` so it follows the pane's own edge uniform rather than a
/// relative time computed here. It must END when the order closes, or a closed order would advertise a
/// position nobody holds.
#[test]
fn a_filled_open_order_continues_from_its_fill_arrow_to_the_edge() {
    let now = now_unix_ms();
    let fill_rel = 300_000.0;
    let (segs, _) = geometry_for(filled_order_without_trace(now), now);
    let held: Vec<&SegInstance> = segs.iter().filter(|seg| is_position_hold(seg)).collect();
    assert_eq!(
        held.len(),
        1,
        "a filled open order must emit exactly one continuation segment"
    );
    let held = held[0];
    assert!(
        near(held.t0_rel, fill_rel),
        "the continuation must start at the fill, got {}",
        held.t0_rel
    );
    assert!(
        near(held.p0, 60_000.0) && near(held.p1, 60_000.0),
        "the continuation must hold the entry price"
    );
    assert!(
        near(held.extend, SEG_EXTEND_EDGE),
        "the continuation must run to the pane edge through the edge uniform"
    );
    assert!(
        near(held.pattern, SEG_PATTERN_SOLID),
        "the continuation must be SOLID: the entry's dash means 'placed, not yet filled', and this \
         segment only exists after the fill, so inheriting it would fly the pending flag over a \
         position that is actually held"
    );

    // Sell/Stop/Trailing/TakeProfit/VStop each have their own distinct fixture price; none of them may
    // have picked up a continuation of its own.
    for price in [61_000.0, 59_000.0, 59_500.0, 62_000.0, 58_000.0] {
        assert!(
            !segs
                .iter()
                .any(|seg| is_position_hold(seg) && near(seg.p0, price)),
            "only the entry may continue; price {price} must not"
        );
    }

    // Closing the order ends the hold. `job_is_done` is the terminal status the store closes on.
    let mut closed = filled_order_without_trace(now);
    closed.job_is_done = true;
    let (closed_segs, _) = geometry_for(closed, now);
    assert!(
        !closed_segs.iter().any(is_position_hold),
        "a closed order must not advertise a held position"
    );
}

/// The fill arrow must be the SAME SIZE as a closed-trade-history arrow at the same device scale and
/// the same user setting.
///
/// `MarkerInstance::size`/`thickness` are PHYSICAL px while `ARROW_HALF_*` are LOGICAL, so passing the
/// bare constants left the fill arrow at its 1x size on every HiDPI monitor — visibly smaller than the
/// trade-history arrow it deliberately reuses the glyph of — and deaf to the triangle-size slider. The
/// oracle here is `trade_marks` itself rather than a copied number, so the two cannot drift apart
/// again.
#[test]
fn the_fill_arrow_matches_a_trade_history_arrow_at_every_scale_and_setting() {
    let now = now_unix_ms();
    for (scale, setting) in [(1.0_f32, 1.0_f32), (2.0, 1.0), (1.5, 2.5), (2.0, 0.1)] {
        let graphics = ChartGraphicsCfg {
            trade_arrow_scale: setting,
            ..ChartGraphicsCfg::default()
        };
        let (_, markers) =
            geometry_for_scaled(filled_order_without_trace(now), now, scale, &graphics);
        let arrow = markers
            .iter()
            .find(|m| near(m.shape, crate::layers::MARKER_SHAPE_ARROW_UP))
            .unwrap_or_else(|| panic!("a filled long entry must mark its fill (scale {scale})"));

        // What `build_trade_geometry` would emit for the same inputs. `clamp_arrow_scale` is shared on
        // purpose: a setting of 0.1 clamps, and clamping it differently on the two paths is exactly how
        // one arrow ends up a different size from the other.
        let expected = crate::trade_marks::clamp_arrow_scale(setting) * scale;
        assert!(
            near(arrow.size, ARROW_HALF_H * expected),
            "half height at scale {scale} setting {setting}: got {}, want {}",
            arrow.size,
            ARROW_HALF_H * expected
        );
        assert!(
            near(arrow.thickness, ARROW_HALF_W * expected),
            "half width at scale {scale} setting {setting}: got {}, want {}",
            arrow.thickness,
            ARROW_HALF_W * expected
        );
    }
}
