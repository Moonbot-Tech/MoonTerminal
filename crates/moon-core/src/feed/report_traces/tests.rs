use std::sync::Arc;

use moonproto::{MoonTime, OrderType, ReportTrace, ReportTracePoint};

use super::*;

fn point(time_ms: i64, price: f64) -> ReportTracePoint {
    ReportTracePoint {
        time: MoonTime::from_unix_millis(time_ms),
        price,
    }
}

fn trace(own: bool, order_type: OrderType, points: Vec<ReportTracePoint>) -> ReportTrace {
    ReportTrace {
        own,
        order_type,
        stop_price: 0.0,
        stop_time: MoonTime::ZERO,
        points: Arc::from(points),
    }
}

#[test]
fn maps_every_buy_flavour_to_the_entry_line_and_sell_to_the_exit() {
    let out = archived_traces_from_proto(&[
        trace(true, OrderType::Buy, vec![point(1_000, 1.0)]),
        trace(true, OrderType::BuyStop, vec![point(1_000, 1.0)]),
        trace(true, OrderType::BuyLimit, vec![point(1_000, 1.0)]),
        trace(false, OrderType::Sell, vec![point(1_000, 1.0)]),
    ]);
    let kinds: Vec<_> = out.iter().map(|t| (t.own, t.kind)).collect();
    assert_eq!(
        kinds,
        vec![
            (true, ArchivedLineKind::Entry),
            (true, ArchivedLineKind::Entry),
            (true, ArchivedLineKind::Entry),
            (false, ArchivedLineKind::Exit),
        ]
    );
}

#[test]
fn drops_unset_coordinates_and_a_line_left_with_none() {
    let out = archived_traces_from_proto(&[
        trace(
            true,
            OrderType::Buy,
            vec![point(0, 5.0), point(2_000, 0.0), point(3_000, 7.5)],
        ),
        trace(true, OrderType::Sell, vec![point(0, 5.0)]),
    ]);
    assert_eq!(out.len(), 1, "a line with no usable point is not a line");
    assert_eq!(out[0].points, vec![(3_000.0, 7.5)]);
}

#[test]
fn stop_marker_needs_both_a_price_and_a_time() {
    let mut with_stop = trace(true, OrderType::Sell, vec![point(1_000, 2.0)]);
    with_stop.stop_price = 1.5;
    with_stop.stop_time = MoonTime::from_unix_millis(4_000);
    let mut price_only = with_stop.clone();
    price_only.stop_time = MoonTime::ZERO;
    let out = archived_traces_from_proto(&[with_stop, price_only]);
    assert_eq!(out[0].stop_price, Some(1.5));
    assert_eq!(out[0].stop_time_ms, Some(4_000.0));
    assert_eq!(out[1].stop_price, None);
    assert_eq!(out[1].stop_time_ms, None);
}
