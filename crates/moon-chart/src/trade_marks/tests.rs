//! Unit tests for closed-trade-history geometry.

use super::*;

/// `trade_marks.rs:fold_cluster` — replacing the shared `cluster_weight` clamp with the raw
/// quantity in the `total` accumulator lets a cluster holding a positive and a negative quantity
/// divide the (still clamped) numerators by an unclamped denominator, placing the aggregate
/// marker outside the range of its own members — a price at which nothing traded.
#[test]
fn fold_cluster_aggregate_stays_inside_member_range_with_mixed_sign_qty() {
    let higher = TradeAction {
        t_ms: 1_000,
        price: 10.0,
        qty: 2.0,
        buy: true,
        is_short: false,
        mark: 0,
    };
    let lower = TradeAction {
        t_ms: 500,
        price: 5.0,
        qty: -1.0,
        buy: true,
        is_short: false,
        mark: 1,
    };
    let group = [&higher, &lower];
    let cluster = fold_cluster(&group, (true, false));

    let (t_lo, t_hi) = (lower.t_ms as f64, higher.t_ms as f64);
    let (p_lo, p_hi) = (lower.price, higher.price);
    assert!(
        cluster.t_ms >= t_lo && cluster.t_ms <= t_hi,
        "aggregate t_ms {} escaped member range [{t_lo}, {t_hi}]",
        cluster.t_ms
    );
    assert!(
        cluster.price >= p_lo && cluster.price <= p_hi,
        "aggregate price {} escaped member range [{p_lo}, {p_hi}]",
        cluster.price
    );
}

/// `trade_marks.rs:hit_trade_marks` must apply the same arrow-size multiplier as
/// `build_trade_geometry`; omitting it leaves the enlarged arrow's lower body unclickable.
#[test]
fn trade_hit_area_grows_with_the_drawn_arrow_scale() {
    let mark = TradeMark {
        buy_ms: 1_000,
        close_ms: 2_000,
        buy_price: 100.0,
        sell_price: 101.0,
        qty: 1.0,
        is_short: false,
    };
    let base_ctx = TradeGeometryCtx {
        epoch_ms: 0.0,
        long_rgb: [1, 2, 3],
        short_rgb: [4, 5, 6],
        scale: 1.0,
        px_per_ms: 1.0,
        px_per_price: 1.0,
        arrow_scale: 1.0,
        connector_thickness: CONNECTOR_THICKNESS,
        hovered: None,
    };
    let mut base_markers = Vec::new();
    let mut base_segs = Vec::new();
    build_trade_geometry(&[mark], &base_ctx, &mut base_markers, &mut base_segs);

    let enlarged_ctx = TradeGeometryCtx {
        arrow_scale: 2.0,
        ..base_ctx
    };
    let mut enlarged_markers = Vec::new();
    let mut enlarged_segs = Vec::new();
    build_trade_geometry(
        &[mark],
        &enlarged_ctx,
        &mut enlarged_markers,
        &mut enlarged_segs,
    );
    let base_arrow = base_markers
        .iter()
        .find(|marker| marker.shape == MARKER_SHAPE_ARROW_UP)
        .expect("a long entry must draw an up arrow");
    let enlarged_arrow = enlarged_markers
        .iter()
        .find(|marker| marker.shape == MARKER_SHAPE_ARROW_UP)
        .expect("a long entry must draw an up arrow");
    assert_eq!(enlarged_arrow.size, base_arrow.size * 2.0);
    assert_eq!(enlarged_arrow.thickness, base_arrow.thickness * 2.0);

    let drawn_arrow = TradeMarkAt {
        x: 0.0,
        apex_y: 0.0,
        buy: true,
        count: 1,
    };
    let cursor_in_only_the_enlarged_body = (0.0, 20.0);
    assert_eq!(
        hit_trade_marks([drawn_arrow], cursor_in_only_the_enlarged_body, 1.0, 1.0,),
        None,
        "the cursor must be beyond the default-size arrow body"
    );
    assert_eq!(
        hit_trade_marks([drawn_arrow], cursor_in_only_the_enlarged_body, 1.0, 2.0,)
            .expect("the drawn two-times arrow must reach the same cursor")
            .stack,
        vec![0]
    );
}

/// The shipped graphics settings must survive their own normalizer.
///
/// `normalize_chart_graphics` exists because this value is COMPARED — a chart re-bakes its base
/// texture when its settings differ from the ones it drew with — so a value that the normalizer
/// still moves differs from itself on every notification, which is a re-bake per frame rather than
/// a wrong pixel. The defaults are handed straight to a chart by
/// `WindowLayout::reset_chart_graphics_default`, without passing the normalizer on the way, and
/// this is what keeps that shortcut honest: a `def_*` that ever drifts outside its own clamp fails
/// here rather than in the frame loop.
#[test]
fn the_shipped_graphics_survive_their_own_normalizer() {
    let shipped = ChartGraphicsCfg::default();
    assert_eq!(
        normalize_chart_graphics(shipped),
        shipped,
        "a shipped graphics default sits outside the range its own normalizer accepts"
    );
}

fn mark_at(buy_ms: i64, buy_price: f64, close_ms: i64, sell_price: f64) -> TradeMark {
    TradeMark {
        buy_ms,
        close_ms,
        buy_price,
        sell_price,
        qty: 1.0,
        is_short: false,
    }
}

fn print_at(t_ms: i64, price: f64) -> TapePrint {
    TapePrint { t_ms, price }
}

/// Empty tape, or every print outside the one-second window, must leave the report stamp alone.
///
/// Widening the window to "nearest print anywhere" would drag a scalp onto an unrelated later
/// trade; refusing to snap when nothing is in range is the only safe fallback.
#[test]
fn snap_mark_to_tape_keeps_the_report_stamp_when_no_print_is_in_range() {
    let mark = mark_at(10_000, 100.0, 20_000, 101.0);
    assert_eq!(snap_mark_to_tape(mark, &[]), mark);
    let too_far = [print_at(10_000 + TAPE_SNAP_WINDOW_MS + 1, 100.0)];
    assert_eq!(snap_mark_to_tape(mark, &too_far), mark);
}

/// A fill 766 ms into a second is stored as that second's `.000`; snapping must land on the print.
#[test]
fn snap_mark_to_tape_moves_a_truncated_second_onto_the_print_in_that_second() {
    let mark = mark_at(12_000, 0.22947, 44_000, 0.23368);
    let tape = [
        print_at(12_766, 0.22950),
        print_at(44_120, 0.23370),
        print_at(80_000, 0.24000),
    ];
    let snapped = snap_mark_to_tape(mark, &tape);
    assert_eq!(snapped.buy_ms, 12_766);
    assert_eq!(snapped.buy_price, 0.22950);
    assert_eq!(snapped.close_ms, 44_120);
    assert_eq!(snapped.sell_price, 0.23370);
}

/// Two prints in the same second: pick the closer PRICE, not the closer time.
///
/// The report price is the fill (or a few hundredths off it). The first print of the second is
/// often the spike top, which is exactly the "triangle in the air" the snap exists to leave.
#[test]
fn snap_mark_to_tape_prefers_the_closer_price_inside_the_window() {
    let mark = mark_at(1_000, 10.0, 2_000, 11.0);
    let tape = [
        print_at(1_100, 12.0),
        print_at(1_800, 10.05),
        print_at(2_050, 11.0),
    ];
    let snapped = snap_mark_to_tape(mark, &tape);
    assert_eq!(snapped.buy_ms, 1_800);
    assert_eq!(snapped.buy_price, 10.05);
    assert_eq!(snapped.close_ms, 2_050);
    assert_eq!(snapped.sell_price, 11.0);
}

/// Each end snaps on its own window. An entry candidate must not steal the exit, or a 32-second
/// scalp would collapse onto one print.
#[test]
fn snap_mark_to_tape_snaps_each_end_independently() {
    let mark = mark_at(1_000, 10.0, 33_000, 12.0);
    let tape = [print_at(1_400, 10.0), print_at(32_500, 12.0)];
    let snapped = snap_mark_to_tape(mark, &tape);
    assert_eq!(snapped.buy_ms, 1_400);
    assert_eq!(snapped.close_ms, 32_500);
}
