//! Statistics of the 5-minute ring depth channel, against figures worked out by hand.

use super::*;

/// One hour in milliseconds, the unit the synthetic stamps are written in.
const H: i64 = 3_600_000;

#[test]
fn nearest_rank_spread_of_one_to_ten() {
    // Shuffled on purpose: the function must sort. Nearest rank over ten values: the median is the
    // 5th smallest (ceil(0.5 * 10) = 5), p90 the 9th (ceil(0.9 * 10) = 9).
    let mut values = [7.0, 3.0, 10.0, 1.0, 9.0, 2.0, 8.0, 5.0, 4.0, 6.0];
    let s = spread(&mut values).expect("non-empty");
    assert_eq!(
        s,
        Spread {
            min: 1.0,
            median: 5.0,
            p90: 9.0,
            max: 10.0
        }
    );
}

#[test]
fn nearest_rank_spread_of_an_odd_sample_and_a_single_value() {
    // Five values: median rank ceil(2.5) = 3 -> 30, p90 rank ceil(4.5) = 5 -> 500.
    let mut values = [500.0, 10.0, 30.0, 20.0, 40.0];
    let s = spread(&mut values).expect("non-empty");
    assert_eq!((s.min, s.median, s.p90, s.max), (10.0, 30.0, 500.0, 500.0));

    let mut one = [12.0];
    let s = spread(&mut one).expect("non-empty");
    assert_eq!((s.min, s.median, s.p90, s.max), (12.0, 12.0, 12.0, 12.0));

    assert!(spread(&mut []).is_none());
}

#[test]
fn ring_facts_count_span_and_shares() {
    let mut f = RingFacts::default();
    // Out of order on purpose: the span is newest minus oldest, not last row minus first row.
    f.add(3 * H, 1.0, 1.0, 0.5, 0.5, 0.0); // open==high, close==low, no volume -> neither
    f.add(H, 1.0, 2.0, 0.5, 1.5, 10.0); // honest, with volume
    f.add(2 * H, 2.0, 2.0, 1.0, 1.5, 3.0); // close != low -> honest, with volume
    f.add(4 * H, 1.0, 1.0, 1.0, 1.0, 0.0); // flat: open==high, close==low -> neither
    assert_eq!(f.count, 4);
    assert_eq!((f.first_ms, f.last_ms), (H, 4 * H));
    assert_eq!(f.span_hours(), 3.0);
    assert_eq!(f.honest, 2);
    assert_eq!(f.with_volume, 2);
    // Newest stamp 4h; at 4h + 90 s the newest candle is 1.5 minutes old.
    assert_eq!(f.age_minutes(4 * H + 90_000), 1.5);
}

#[test]
fn percent_of_an_empty_whole_is_zero() {
    assert_eq!(percent(0, 0), 0.0);
    assert_eq!(percent(1, 4), 25.0);
}

/// A ring of `count` candles one period apart ending at `last_ms`, every one honest when asked.
fn ring(count: usize, last_ms: i64, honest: bool) -> RingFacts {
    let mut f = RingFacts::default();
    for i in 0..count {
        let t = last_ms - (count - 1 - i) as i64 * 300_000;
        let open = if honest { 1.5 } else { 2.0 };
        f.add(t, open, 2.0, 1.0, 1.0, 1.0);
    }
    f
}

#[test]
fn provider_summary_against_hand_computed_figures() {
    let now = 100 * H;
    let mut p = ProviderDepth::default();
    // Three filled rings, one empty ring, one market without a ring at all.
    p.add_ring("DEEPUSDT", 500, ring(13, now - 120_000, true)); // span 12 * 5 min = 1.0 h, age 2 min
    p.add_ring("MIDUSDT", 500, ring(7, now - 600_000, false)); // span 0.5 h, age 10 min
    p.add_ring("THINUSDT", 500, ring(1, now - 60_000, false)); // span 0, age 1 min
    p.add_ring("EMPTYUSDT", 500, RingFacts::default());
    p.add_without_ring();

    let line = p.summary(now);
    // 21 candles in all, 13 honest (61.9 %), every one with volume.
    let expected_head = "markets=5 ring=4 nonempty=3 cap=500 candles=21 \
         count[min/med/p90/max]=1/7/13/13 span_h[min/med/p90/max]=0.0/0.5/1.0/1.0 \
         age_min[med/max]=2.0/10.0 honest_oc=61.9% vol>0=100.0%";
    assert!(
        line.starts_with(expected_head),
        "summary drifted from the hand-computed figures:\n{line}"
    );
    assert!(
        line.contains("deepest DEEPUSDT n=13 span=1.0h age=2.0m honest=100.0% vol=100.0%"),
        "{line}"
    );
    assert!(
        line.contains("median MIDUSDT n=7 span=0.5h age=10.0m honest=0.0%"),
        "{line}"
    );
    assert!(line.contains("shallowest THINUSDT n=1 span=0.0h"), "{line}");
}

#[test]
fn a_provider_with_only_empty_rings_prints_its_counts_alone() {
    let mut p = ProviderDepth::default();
    p.add_ring("AUSDT", 500, RingFacts::default());
    assert!(!p.is_empty());
    assert_eq!(p.summary(0), "markets=1 ring=1 nonempty=0 cap=500");
    assert!(ProviderDepth::default().is_empty());
}
