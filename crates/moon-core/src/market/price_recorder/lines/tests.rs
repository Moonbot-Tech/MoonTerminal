//! The file's stretches as chart lines: what becomes a point, and what must not.

use super::lines_of;
use crate::market::price_recorder::store::{PriceSample, PriceSpan};

fn sample(t_ms: i64, bid: f64, ask: f64, mark: f64) -> PriceSample {
    PriceSample {
        t_ms,
        bid,
        ask,
        mark,
        funding: None,
    }
}

fn points(line: &[crate::feed::PricePoint]) -> Vec<(f64, f32)> {
    line.iter().map(|p| (p.time_ms, p.price)).collect()
}

/// Last is the midpoint the core draws, not either side; a sample that moved only the other line
/// adds no point to this one.
#[test]
fn last_is_the_book_midpoint_and_each_line_keeps_only_its_changes() {
    let lines = lines_of(&[PriceSpan {
        from_ms: 1_000,
        to_ms: 9_000,
        samples: vec![
            sample(1_000, 10.0, 12.0, 11.5),
            // The book moved, the mark did not.
            sample(3_000, 11.0, 13.0, 11.5),
            // The mark moved, the midpoint did not.
            sample(5_000, 10.5, 13.5, 12.25),
        ],
    }]);
    let span = &lines.spans[0];
    assert_eq!((span.from_ms, span.to_ms), (1_000, 9_000));
    assert_eq!(points(&span.last), vec![(1_000.0, 11.0), (3_000.0, 12.0)]);
    assert_eq!(points(&span.mark), vec![(1_000.0, 11.5), (5_000.0, 12.25)]);
}

/// A book with an empty side has no Last; drawing `(0 + ask) / 2` would put a line at half the
/// price. The mark of the same sample still stands.
#[test]
fn a_one_sided_book_draws_no_last_but_keeps_its_mark() {
    let lines = lines_of(&[PriceSpan {
        from_ms: 0,
        to_ms: 4_000,
        samples: vec![
            sample(0, 0.0, 20.0, 19.0),
            sample(1_000, f64::NAN, 20.0, 19.5),
            sample(2_000, 18.0, 20.0, 0.0),
        ],
    }]);
    let span = &lines.spans[0];
    assert_eq!(points(&span.last), vec![(2_000.0, 19.0)]);
    assert_eq!(points(&span.mark), vec![(0.0, 19.0), (1_000.0, 19.5)]);
}

/// Stretches stay apart: merging them would draw one stretch's last value across the hole.
#[test]
fn stretches_stay_separate() {
    let lines = lines_of(&[
        PriceSpan {
            from_ms: 0,
            to_ms: 1_000,
            samples: vec![sample(0, 1.0, 3.0, 2.0)],
        },
        PriceSpan {
            from_ms: 5_000,
            to_ms: 6_000,
            samples: vec![sample(5_000, 3.0, 5.0, 4.0)],
        },
    ]);
    assert_eq!(lines.spans.len(), 2);
    assert_eq!(points(&lines.spans[1].last), vec![(5_000.0, 4.0)]);
}
