use super::super::ring::{Point, Segment};
use super::*;

fn segment(from_ms: i64, to_ms: i64, bids: &[(i64, f64)]) -> Filed {
    Filed {
        exchange: "ex:00000000".to_owned(),
        market: "XUSDT".to_owned(),
        segment: Segment {
            from_ms,
            to_ms,
            points: bids
                .iter()
                .map(|&(t_ms, bid)| Point {
                    t_ms,
                    bid,
                    ask: 2.0,
                    mark: 1.5,
                })
                .collect(),
            funding: Vec::new(),
        },
    }
}

#[test]
fn abutting_segments_read_as_one_stretch_and_a_gap_splits_it() {
    let mut store = Store::in_memory().unwrap();
    store
        .write(&[
            segment(1_000, 1_999, &[(1_000, 1.0), (1_500, 1.1)]),
            // Opens with the state it starts from, which the reader does not repeat.
            segment(2_000, 2_999, &[(2_000, 1.1), (2_500, 1.2)]),
            segment(5_000, 5_999, &[(5_000, 1.3)]),
        ])
        .unwrap();
    let spans = read_conn(store.conn(), "ex:00000000", "XUSDT", 0, 10_000).unwrap();
    let shape: Vec<_> = spans
        .iter()
        .map(|s| {
            (
                s.from_ms,
                s.to_ms,
                s.samples
                    .iter()
                    .map(|p| (p.t_ms, p.bid))
                    .collect::<Vec<_>>(),
            )
        })
        .collect();
    assert_eq!(
        shape,
        vec![
            (1_000, 2_999, vec![(1_000, 1.0), (1_500, 1.1), (2_500, 1.2)]),
            (5_000, 5_999, vec![(5_000, 1.3)]),
        ]
    );
    // Bounds pick whole segments that touch them, and another market reads nothing.
    let spans = read_conn(store.conn(), "ex:00000000", "XUSDT", 2_500, 2_600).unwrap();
    assert_eq!(spans.len(), 1);
    assert_eq!((spans[0].from_ms, spans[0].to_ms), (2_000, 2_999));
    assert!(
        read_conn(store.conn(), "ex:00000000", "YUSDT", 0, 10_000)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn an_unreadable_segment_breaks_the_stretch() {
    let mut store = Store::in_memory().unwrap();
    store
        .write(&[
            segment(1_000, 1_999, &[(1_000, 1.0)]),
            segment(2_000, 2_999, &[(2_000, 1.1)]),
            segment(3_000, 3_999, &[(3_000, 1.2)]),
        ])
        .unwrap();
    store
        .conn()
        .execute("UPDATE segments SET data = x'63' WHERE from_ms = 2000", [])
        .unwrap();
    let spans = read_conn(store.conn(), "ex:00000000", "XUSDT", 0, 10_000).unwrap();
    let bounds: Vec<_> = spans.iter().map(|s| (s.from_ms, s.to_ms)).collect();
    assert_eq!(bounds, vec![(1_000, 1_999), (3_000, 3_999)]);
}

#[test]
fn retention_drops_what_is_past_the_edge() {
    let mut store = Store::in_memory().unwrap();
    store
        .write(&[
            segment(1_000, 1_999, &[(1_000, 1.0)]),
            segment(5_000, 5_999, &[(5_000, 1.0)]),
        ])
        .unwrap();
    assert_eq!(store.trim(Some(2_000), u64::MAX).unwrap(), 1);
    let spans = read_conn(store.conn(), "ex:00000000", "XUSDT", 0, 10_000).unwrap();
    assert_eq!(spans.len(), 1);
    assert_eq!(spans[0].from_ms, 5_000);
}

#[test]
fn retention_drops_the_oldest_past_the_ceiling() {
    let mut store = Store::in_memory().unwrap();
    // Twenty markets filed side by side, as open trades are: the oldest rows of the file are
    // spread over every market.
    let markets: Vec<String> = (0..20).map(|m| format!("M{m}USDT")).collect();
    for i in 0..200 {
        let from = i * 1_000;
        let bids: Vec<_> = (0..20).map(|k| (from + k * 10, k as f64)).collect();
        let batch: Vec<_> = markets
            .iter()
            .map(|market| Filed {
                market: market.clone(),
                ..segment(from, from + 999, &bids)
            })
            .collect();
        store.write(&batch).unwrap();
    }
    let total = 200 * markets.len();
    let ceiling = store.used_bytes().unwrap() / 2;
    let deleted = store.trim(None, ceiling).unwrap();
    // Free pages stay in the file; what is in use fits the ceiling.
    assert!(store.used_bytes().unwrap() <= ceiling);
    // About half goes, not nearly everything.
    assert!(
        deleted > total / 3 && deleted < total * 2 / 3,
        "{deleted} of {total} rows deleted"
    );
    for market in &markets {
        let spans = read_conn(store.conn(), "ex:00000000", market, 0, i64::MAX).unwrap();
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].to_ms, 199_999, "the newest segment stays");
        assert!(spans[0].from_ms > 0, "the oldest went first");
    }
}

#[test]
fn a_missing_file_reads_as_nothing() {
    let path = std::env::temp_dir().join(format!(
        "moon-price-recorder-missing-{}.sqlite",
        std::process::id()
    ));
    assert!(
        read_at(&path, "ex:00000000", "XUSDT", 0, 1)
            .unwrap()
            .is_empty()
    );
    assert!(!path.exists(), "a read never creates the file");
}
