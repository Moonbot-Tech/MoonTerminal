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
