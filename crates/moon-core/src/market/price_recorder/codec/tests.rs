use super::super::ring::{Funding, Point, Segment};
use super::*;

fn point(t_ms: i64, bid: f64, ask: f64, mark: f64) -> Point {
    Point {
        t_ms,
        bid,
        ask,
        mark,
    }
}

fn funding(t_ms: i64, rate: f64, next_ms: i64) -> Funding {
    Funding {
        t_ms,
        rate,
        next_ms,
    }
}

#[test]
fn a_segment_reads_back_as_the_state_after_every_change() {
    let seg = Segment {
        from_ms: 10_000,
        to_ms: 20_000,
        points: vec![
            point(10_000, 1.0e-8, 1.1e-8, 1.05e-8),
            point(12_000, 1.0e-8, 1.2e-8, 1.05e-8),
            point(14_000, 0.9e-8, 1.2e-8, 1.0e-8),
        ],
        funding: vec![
            funding(10_000, 0.0001, 1_800_000),
            funding(13_000, -0.0002, 1_800_000),
        ],
    };
    let decoded = decode(seg.from_ms, &encode(&seg)).unwrap();
    let want = vec![
        Decoded {
            t_ms: 10_000,
            bid: 1.0e-8,
            ask: 1.1e-8,
            mark: 1.05e-8,
            funding: Some((0.0001, 1_800_000)),
        },
        Decoded {
            t_ms: 12_000,
            bid: 1.0e-8,
            ask: 1.2e-8,
            mark: 1.05e-8,
            funding: Some((0.0001, 1_800_000)),
        },
        Decoded {
            t_ms: 13_000,
            bid: 1.0e-8,
            ask: 1.2e-8,
            mark: 1.05e-8,
            funding: Some((-0.0002, 1_800_000)),
        },
        Decoded {
            t_ms: 14_000,
            bid: 0.9e-8,
            ask: 1.2e-8,
            mark: 1.0e-8,
            funding: Some((-0.0002, 1_800_000)),
        },
    ];
    assert_eq!(decoded, want);
}

#[test]
fn an_unchanged_value_is_not_written_again() {
    let one = Segment {
        from_ms: 0,
        to_ms: 10,
        points: vec![point(0, 1.0, 2.0, 1.5)],
        funding: Vec::new(),
    };
    let two = Segment {
        points: vec![point(0, 1.0, 2.0, 1.5), point(5, 1.1, 2.0, 1.5)],
        ..one.clone()
    };
    // Version, then one record: delta (1) + flags (1) + three prices (24); the second record
    // carries only its bid: delta (1) + flags (1) + 8.
    assert_eq!(encode(&one).len(), 1 + 26);
    assert_eq!(encode(&two).len(), 1 + 26 + 10);
}

#[test]
fn funding_ahead_of_the_first_price_rides_on_it() {
    let seg = Segment {
        from_ms: 100,
        to_ms: 200,
        points: vec![point(150, 1.0, 2.0, 1.5)],
        funding: vec![funding(100, 0.0003, 7)],
    };
    let decoded = decode(seg.from_ms, &encode(&seg)).unwrap();
    assert_eq!(decoded.len(), 1);
    assert_eq!(decoded[0].t_ms, 150);
    assert_eq!(decoded[0].funding, Some((0.0003, 7)));
}

#[test]
fn a_broken_blob_is_refused() {
    let seg = Segment {
        from_ms: 0,
        to_ms: 10,
        points: vec![point(0, 1.0, 2.0, 1.5)],
        funding: Vec::new(),
    };
    let data = encode(&seg);
    assert_eq!(
        decode(0, &data[..data.len() - 1]),
        Err(DecodeError::Truncated)
    );
    let mut other = data.clone();
    other[0] = 99;
    assert_eq!(decode(0, &other), Err(DecodeError::Version(99)));
    // A first record that does not carry every price has no state to start from.
    assert_eq!(
        decode(0, &[VERSION, 0, BID, 0, 0, 0, 0, 0, 0, 0, 0]),
        Err(DecodeError::NoState)
    );
}

#[test]
fn zigzag_round_trips_negative_times() {
    for v in [0, 1, -1, i64::MAX, i64::MIN, -1_700_000_000_000] {
        assert_eq!(unzigzag(zigzag(v)), v);
    }
}

#[test]
fn a_cut_opens_with_the_state_in_force_and_keeps_only_its_stretch() {
    let seg = Segment {
        from_ms: 10_000,
        to_ms: 20_000,
        points: vec![
            point(10_000, 1.0, 2.0, 1.5),
            point(12_000, 1.1, 2.0, 1.5),
            point(15_000, 1.2, 2.1, 1.6),
            point(18_000, 1.3, 2.2, 1.7),
        ],
        funding: vec![funding(10_000, 0.0001, 99_000)],
    };
    let data = encode(&seg);
    let (start, cut_data) = cut(10_000, &data, 13_000, 15_000).unwrap().unwrap();
    assert_eq!(start, 13_000);
    let back = decode(13_000, &cut_data).unwrap();
    let shape: Vec<_> = back.iter().map(|d| (d.t_ms, d.bid, d.mark)).collect();
    // 13 000 carries what held since 12 000; 18 000 is past the stretch.
    assert_eq!(shape, vec![(13_000, 1.1, 1.5), (15_000, 1.2, 1.6)]);
    assert!(
        back.iter().all(|d| d.funding == Some((0.0001, 99_000))),
        "funding in force carries into the cut"
    );
    // A stretch on a change starts with that change; one before the segment starts at it.
    let (start, _) = cut(10_000, &data, 15_000, 16_000).unwrap().unwrap();
    assert_eq!(start, 15_000);
    let (start, early) = cut(10_000, &data, 5_000, 11_000).unwrap().unwrap();
    assert_eq!(start, 10_000);
    assert_eq!(decode(10_000, &early).unwrap().len(), 1);
    assert_eq!(cut(10_000, &[VERSION], 0, 30_000).unwrap(), None);
}
