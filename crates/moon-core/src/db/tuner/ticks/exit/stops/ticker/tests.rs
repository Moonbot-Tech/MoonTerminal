use super::*;
use crate::db::tuner::ticks::tests::deal;
use crate::market::price_recorder::{PriceSample, PriceSpan};

/// A ticker over one stretch whose samples arrive at `arrivals` with `bid`, the ask 0.2 above.
fn recorded(arrivals: &[(i64, f64)], to_ms: i64) -> Ticker {
    let samples = arrivals
        .iter()
        .map(|&(t_ms, bid)| PriceSample {
            t_ms,
            bid,
            ask: bid + 0.2,
            mark: 0.0,
            funding: None,
        })
        .collect();
    let spans = [PriceSpan {
        from_ms: arrivals[0].0,
        to_ms,
        samples,
    }];
    let track = QuoteTrack::new(&spans, &deal()).expect("track");
    Ticker::new(Some(Arc::new(track)), 2_150)
}

/// Inside the record the arrivals fall on its points; past them the period steps, so an arrival
/// that changed nothing is still sampled.
#[test]
fn arrivals_follow_the_record_and_step_where_it_is_silent() {
    // Points on the core's clock at 2 000 and 4 000; nothing changes after.
    let ticker = recorded(&[(3_000, 99.0), (5_000, 98.0)], 30_000);
    assert_eq!(ticker.first(0), 2_000);
    assert_eq!(ticker.next(2_000), 4_000);
    assert_eq!(ticker.next(4_000), 6_150);
    // Off the record the walk keeps its own clock.
    let bare = Ticker::new(None, 2_150);
    assert_eq!((bare.first(500), bare.next(500)), (500, 2_650));
}

/// The book is the record's unless a taker traded past it after its point.
#[test]
fn the_bid_is_the_records_until_a_later_print() {
    let ticker = recorded(&[(3_000, 99.0)], 30_000);
    assert_eq!(ticker.bid(2_500, Some((1_000, 98.0))), Some(99.0));
    assert_eq!(ticker.bid(2_500, Some((2_400, 98.0))), Some(98.0));
    assert!((ticker.ask(2_500, None).unwrap() - 99.2).abs() < 1e-12);
    // A short's stop watches the ask.
    assert!((ticker.stop_side(false, 2_500, None).unwrap() - 99.2).abs() < 1e-12);
    // In a hole only the print is known.
    assert_eq!(ticker.bid(40_000, Some((39_000, 97.0))), Some(97.0));
    assert_eq!(ticker.bid(40_000, None), None);
}
