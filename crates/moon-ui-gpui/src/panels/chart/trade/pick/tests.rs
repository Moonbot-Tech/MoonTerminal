// Do not use `super::*` for gpui: this crate's modules re-export GPUI's `test` attribute macro,
// which would shadow the built-in `#[test]`. Only the ranking is under test here.
use super::OrderCandidate;
use moon_core::session::order_lines::LineKind;

fn candidate(dist: f32, overshoot: f32, size: f32, seq: u64) -> OrderCandidate {
    OrderCandidate {
        uid: seq,
        kind: LineKind::Sell,
        price: 100.0,
        short: false,
        dist,
        start_x: f32::NEG_INFINITY,
        fill_pct: 0.0,
        overshoot,
        size,
        seq,
        pinned: overshoot > 0.0,
    }
}

/// `trade.rs:OrderCandidate::beats` decides which line a drag grabs, and pixel distance decides it
/// EXACTLY. Rounding the distance was tried and quietly changed the ordinary grab: a stop half a
/// pixel from the pointer would lose to an exit a pixel and a half away, and those two legs route to
/// different commands on the core.
#[test]
fn the_nearest_line_wins_the_grab() {
    let near_line = candidate(0.5, 0.0, 1.0, 1);
    let far_line = candidate(1.4, 0.0, 900.0, 900);
    assert!(near_line.beats(&far_line));
    assert!(!far_line.beats(&near_line));
}

/// Two exits pinned to the same edge are clamped to the very same Y, so their distances tie on
/// their own and the rest of the rule decides: nearest to the price, then the larger position, then
/// the later order — which is what puts the last one on top.
#[test]
fn pinned_exits_rank_by_price_then_position_then_arrival() {
    let pinned_a = candidate(2.0, 300.0, 5.0, 1);
    let pinned_b = candidate(2.0, 40.0, 1.0, 2);
    assert!(pinned_b.beats(&pinned_a), "nearest to the price wins first");

    let small = candidate(2.0, 40.0, 1.0, 7);
    let big = candidate(2.0, 40.0, 9.0, 3);
    assert!(big.beats(&small), "the larger position breaks a price tie");

    let older = candidate(2.0, 40.0, 9.0, 3);
    let newer = candidate(2.0, 40.0, 9.0, 8);
    assert!(newer.beats(&older), "the last one is on top");
    assert!(!older.beats(&newer));
}

/// An on-screen line still wins over a pinned one on plain proximity: the pin widens what is
/// reachable at the plot's edge, it does not outrank a line the pointer is actually on.
#[test]
fn an_on_screen_line_outranks_a_pinned_one_by_distance() {
    let on_screen = candidate(1.0, 0.0, 1.0, 1);
    let pinned = candidate(3.0, 500.0, 1_000.0, 99);
    assert!(on_screen.beats(&pinned));
    assert!(!pinned.beats(&on_screen));
}
