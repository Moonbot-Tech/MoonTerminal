use super::super::{Deal, ORDER_WAIT_CAP_MS, RUN_UP_MS, TAIL_MS, model_window, required_spans};
use super::deal;
use crate::market::trade_replay::{Coverage, replay_window_ms};

const MINUTE_MS: i64 = 60_000;

fn deal_at(buy_ms: i64, close_ms: i64) -> Deal {
    let mut d = deal();
    d.buy_ms = buy_ms;
    d.close_ms = close_ms;
    d
}

/// A short deal asks for the run-up through the tail and nothing more: a tape that stops short
/// of the fifteen-minute trail still counts, one that starts inside the run-up or ends inside
/// the tail does not.
#[test]
fn short_deal_requires_the_run_up_through_the_tail_only() {
    let buy = 100 * MINUTE_MS;
    let close = buy + 20_000;
    let window = replay_window_ms(buy, close, 15 * MINUTE_MS).expect("window");
    let required = required_spans(&window);
    assert_eq!(required.spans(), &[(buy - RUN_UP_MS, close + TAIL_MS)]);
    let trail_cut = Coverage::one((buy - RUN_UP_MS, close + TAIL_MS));
    assert!(
        trail_cut.covers(&required),
        "the trail past the tail is optional"
    );
    let late_start = Coverage::one((buy - RUN_UP_MS + 1, close + 15 * MINUTE_MS));
    assert!(!late_start.covers(&required), "the run-up is not");
    let ends_at_close = Coverage::one((buy - 15 * MINUTE_MS, close));
    assert!(!ends_at_close.covers(&required), "neither is the tail");
}

/// A long position's window asks only around its two ends, so the requirement is clipped to
/// them: the unwalked hours in the middle are not owed. A zero margin asks for the position
/// alone, and the requirement shrinks to it.
#[test]
fn long_position_and_zero_margin_require_only_what_the_window_asks() {
    let buy = 100 * MINUTE_MS;
    let close = buy + 8 * 60 * MINUTE_MS;
    let margin = 15 * MINUTE_MS;
    let window = replay_window_ms(buy, close, margin).expect("window");
    let spans = window.focus_spans();
    assert!(spans.is_split());
    let required = required_spans(&window);
    let half = margin / 2;
    assert_eq!(
        required.spans(),
        &[
            (buy - RUN_UP_MS, buy + half),
            (close - half, close + TAIL_MS)
        ]
    );
    assert!(spans.covers(&required));
    let close = buy + 20_000;
    let bare = replay_window_ms(buy, close, 0).expect("window");
    assert_eq!(required_spans(&bare).spans(), &[(buy, close)]);
}

/// An order created two minutes before its fill is replayed from its creation: the window opens
/// there, and the run-up is owed before it, not before the fill.
#[test]
fn the_model_window_opens_at_the_orders_creation() {
    let buy = 100 * MINUTE_MS;
    let close = buy + 20_000;
    let mut d = deal_at(buy, close);
    d.buy_set_ms = Some(buy - 2 * MINUTE_MS);
    let window = model_window(&d, MINUTE_MS, 60 * MINUTE_MS).expect("window");
    assert_eq!(
        (window.open_ms, window.close_ms),
        (buy - 2 * MINUTE_MS, close)
    );
    assert_eq!(
        required_spans(&window).spans(),
        &[(buy - 2 * MINUTE_MS - RUN_UP_MS, close + TAIL_MS)]
    );
}

/// Where the order's life cannot be walked as one stretch — it waited past the cap, or with the
/// position it outruns the long-position threshold — the window opens at the fill as before, and
/// nothing before it is owed.
#[test]
fn the_model_window_opens_at_the_fill_when_the_orders_life_is_not_one_stretch() {
    let buy = 100 * MINUTE_MS;
    let close = buy + 20_000;
    let mut waited = deal_at(buy, close);
    waited.buy_set_ms = Some(buy - ORDER_WAIT_CAP_MS - 1);
    let window = model_window(&waited, MINUTE_MS, 60 * MINUTE_MS).expect("window");
    assert_eq!(window.open_ms, buy, "past the cap");

    let mut long = deal_at(buy, buy + 59 * MINUTE_MS);
    long.buy_set_ms = Some(buy - 2 * MINUTE_MS);
    let window = model_window(&long, MINUTE_MS, 60 * MINUTE_MS).expect("window");
    assert_eq!(
        window.open_ms, buy,
        "creation to close outruns the threshold"
    );
    assert_eq!(
        window.long_position_ms,
        60 * MINUTE_MS,
        "the caller's threshold"
    );
    assert_eq!(required_spans(&window).spans()[0].0, buy - RUN_UP_MS);

    // A creation stamp after the fill is no order's, and is not taken.
    let mut odd = deal_at(buy, close);
    odd.buy_set_ms = Some(buy + 1);
    assert_eq!(odd.order_open_ms(), None);
    assert_eq!(
        model_window(&odd, MINUTE_MS, 60 * MINUTE_MS)
            .expect("window")
            .open_ms,
        buy
    );
}
