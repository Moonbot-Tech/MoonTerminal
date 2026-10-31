use super::*;
use crate::db::tuner::ticks::tests::deal;
use crate::market::price_recorder::{PriceSample, PriceSpan};

/// A sample arriving at `t_ms` (the terminal's clock).
fn sample(t_ms: i64, bid: f64, ask: f64, mark: f64) -> PriceSample {
    PriceSample {
        t_ms,
        bid,
        ask,
        mark,
        funding: None,
    }
}

fn span(from_ms: i64, to_ms: i64, samples: Vec<PriceSample>) -> PriceSpan {
    PriceSpan {
        from_ms,
        to_ms,
        samples,
    }
}

/// A MoonShot deal bought at 10 s whose report stamped `dmark` there.
fn moonshot(dmark: f64) -> Deal {
    let mut d = deal();
    d.deltas.dmark = dmark;
    d
}

/// The list's mark delta at a point: `(mid − mark) / mid · 100`.
fn listed(bid: f64, ask: f64, mark: f64) -> f64 {
    let mid = (bid + ask) / 2.0;
    (mid - mark) / mid * 100.0
}

/// A point stamped on arrival speaks for the core a second earlier: reading it at its arrival
/// would hand the stops a book a second late.
#[test]
fn a_point_holds_from_its_arrival_less_the_list_delay() {
    let spans = [span(
        0,
        30_000,
        vec![sample(0, 99.0, 99.2, 0.0), sample(12_000, 98.0, 98.2, 0.0)],
    )];
    let track = QuoteTrack::new(&spans, &moonshot(0.0)).expect("track");
    assert_eq!(track.at(11_000 - 1).map(|p| p.bid), Some(99.0));
    assert_eq!(track.at(11_000).map(|p| p.bid), Some(98.0));
    assert_eq!(track.next_point(5_000, 20_000), Some(11_000));
}

/// Nothing is known outside a recorded stretch: a hole must fall back, never carry the last
/// price across it.
#[test]
fn a_hole_answers_nothing() {
    let spans = [
        span(0, 5_000, vec![sample(0, 99.0, 99.2, 0.0)]),
        span(20_000, 30_000, vec![sample(20_000, 97.0, 97.2, 0.0)]),
    ];
    let track = QuoteTrack::new(&spans, &moonshot(0.0)).expect("track");
    assert!(track.at(10_000).is_none());
    assert_eq!(track.at(19_500).map(|p| p.bid), Some(97.0));
    assert_eq!(track.next_point(3_000, 10_000), None);
}

/// A list that carried no usable book ends the stretch: the price before it must not stand for
/// a book nobody knew.
#[test]
fn an_unusable_sample_ends_the_stretch() {
    let spans = [span(
        0,
        30_000,
        vec![
            sample(1_000, 99.0, 99.2, 0.0),
            sample(5_000, 0.0, 0.0, 0.0),
            sample(9_000, 98.0, 98.2, 0.0),
        ],
    )];
    let track = QuoteTrack::new(&spans, &moonshot(0.0)).expect("track");
    assert_eq!(track.at(2_000).map(|p| p.bid), Some(99.0));
    assert!(track.at(6_000).is_none());
    assert_eq!(track.at(8_000).map(|p| p.bid), Some(98.0));
}

/// At the report's stamp the report's figure stands — it is the core's own; elsewhere the list's.
#[test]
fn the_mark_delta_is_the_reports_at_its_stamp_and_the_lists_elsewhere() {
    let first = listed(100.0, 100.2, 99.9);
    let report = first + 0.03;
    let spans = [span(
        0,
        30_000,
        vec![
            sample(9_000, 100.0, 100.2, 99.9),
            sample(15_000, 101.0, 101.2, 100.5),
        ],
    )];
    let track = QuoteTrack::new(&spans, &moonshot(report)).expect("track");
    assert!(track.mark_live());
    assert_eq!(track.dmark_at(10_000), Some(report));
    let later = track.dmark_at(14_500).expect("listed");
    assert!((later - listed(101.0, 101.2, 100.5)).abs() < 1e-12);
    // The deal reads it through its deltas.
    let mut d = moonshot(report);
    d.quotes = Some(Arc::new(track));
    assert!((d.deltas_at(14_500).dmark - later).abs() < 1e-12);
    assert_eq!(d.deltas_at(40_000).dmark, report);
}

/// A list that disagrees with the report at its stamp is another market's, and a report without
/// a mark delta (spot, a core without mark) has nothing to follow: both keep the snapshot.
#[test]
fn the_mark_delta_stays_the_snapshot_without_agreement() {
    let spans = [span(0, 30_000, vec![sample(9_000, 100.0, 100.2, 99.0)])];
    let off = listed(100.0, 100.2, 99.0) + MARK_STAMP_TOLERANCE_PP + 0.1;
    let track = QuoteTrack::new(&spans, &moonshot(off)).expect("track");
    assert!(!track.mark_live());
    assert_eq!(track.dmark_at(10_000), None);
    let track = QuoteTrack::new(&spans, &moonshot(0.0)).expect("track");
    assert!(!track.mark_live());
}

/// Hook and Spread stamp at the order's creation: the sell's modifiers, read at the fill, take the
/// list's value there and not the creation's.
#[test]
fn a_kind_stamped_at_the_order_reads_the_list_at_the_fill() {
    let mut d = moonshot(listed(100.0, 100.2, 99.9));
    d.kind = "MoonHook".into();
    d.buy_set_ms = Some(4_000);
    let spans = [span(
        0,
        30_000,
        vec![
            sample(4_500, 100.0, 100.2, 99.9),
            sample(10_500, 102.0, 102.2, 103.0),
        ],
    )];
    d.quotes = QuoteTrack::new(&spans, &d).map(Arc::new);
    assert!(d.quotes.as_ref().is_some_and(|q| q.mark_live()));
    let at_fill = d.deltas_at(d.buy_ms).dmark;
    assert!((at_fill - listed(102.0, 102.2, 103.0)).abs() < 1e-12);
}

/// A taker who traded after the list's point traded at the book as it then stood.
#[test]
fn the_newer_of_the_list_and_the_print_stands() {
    assert_eq!(newest(Some((1_000, 99.0)), Some((1_500, 98.5))), Some(98.5));
    assert_eq!(newest(Some((1_000, 99.0)), Some((900, 98.5))), Some(99.0));
    assert_eq!(newest(None, Some((900, 98.5))), Some(98.5));
}
