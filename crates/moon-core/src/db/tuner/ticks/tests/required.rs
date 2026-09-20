use super::super::{Deal, RUN_UP_MS, TAIL_MS, required_spans};
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
    let spans = window.focus_spans();
    let required = required_spans(&deal_at(buy, close), &spans);
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
    let required = required_spans(&deal_at(buy, close), &spans);
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
    let bare = replay_window_ms(buy, close, 0)
        .expect("window")
        .focus_spans();
    assert_eq!(
        required_spans(&deal_at(buy, close), &bare).spans(),
        &[(buy, close)]
    );
}
